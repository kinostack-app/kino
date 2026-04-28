use sqlx::SqlitePool;
use uuid::Uuid;

use crate::download::vpn::{VpnManager, health};
use crate::settings::quality_profile::default_quality_items;
use crate::tmdb::TmdbClient;

/// Initialize config and default quality profile on first run.
/// Reads environment variables for preconfiguration. Returns `true`
/// if this was the first-run insert (no existing config row), so the
/// caller can act on that signal — currently used by `main.rs` to
/// decide whether to auto-open the browser to the setup wizard.
pub async fn ensure_defaults(pool: &SqlitePool, data_path: &str) -> anyhow::Result<bool> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM config WHERE id = 1")
        .fetch_one(pool)
        .await?;
    let was_first_run = exists == 0;

    if exists == 0 {
        // Always generate a random API key. Previously we honoured a
        // `KINO_API_KEY` env var for "deterministic dev key" convenience
        // — but that meant the same UUID lived in docker-compose.yml +
        // .devcontainer/post-create.sh + frontend SetupWizard.tsx and
        // would have been a publicly-searchable admin secret on the
        // public repo. The wizard now relies on a narrowed
        // empty-config auth pass-through (see `auth.rs::is_setup_wizard_path`)
        // so dev and prod use the same flow.
        let api_key = Uuid::new_v4().to_string();
        tracing::info!(api_key = %api_key, "generated API key for first run");

        // Read optional env vars for preconfiguration
        let tmdb_key = std::env::var("KINO_TMDB_API_KEY").unwrap_or_default();
        let media_path = std::env::var("KINO_MEDIA_PATH").unwrap_or_default();
        let download_path = std::env::var("KINO_DOWNLOAD_PATH").unwrap_or_default();
        // Custom Cast Receiver application ID (Cast Console). NULL =
        // use the default media receiver (`CC1AD845`) for non-custom
        // playback. Setting via env var keeps the dev devcontainer
        // pointing at the kino-registered receiver across `just
        // reset` cycles without needing a settings-page round-trip.
        let cast_app_id = std::env::var("KINO_CAST_RECEIVER_APP_ID")
            .ok()
            .filter(|s| !s.is_empty());

        // Probe for hardware acceleration on first run so the user
        // gets the fastest backend that actually works on their box
        // without having to visit Settings → Playback. Any subsequent
        // reconfiguration is respected: we only auto-pick on the
        // fresh-insert path.
        let caps = crate::playback::hw_probe::run_probe("ffmpeg").await;
        let initial_hw = caps
            .suggested()
            .map_or_else(|| "none".to_string(), |b| b.as_config_value().to_string());
        if initial_hw == "none" {
            tracing::info!("first-run: no hardware acceleration detected, using software");
        } else {
            tracing::info!(
                hw_acceleration = %initial_hw,
                "first-run: auto-selected hardware acceleration",
            );
        }
        // Seed the process-wide cache so the status banner can act on
        // the probe result immediately without waiting for the
        // background refresh on the next boot.
        crate::playback::hw_probe_cache::set_cached(caps);

        // Default backup location lives under the data path; users
        // who want a NAS / external-drive target override via
        // Settings → Backup. Empty until populated here so the
        // schema doesn't need to know about `data_path`.
        let default_backup_location = format!("{}/backups", data_path.trim_end_matches('/'));

        // Seed `listen_port` from the KINO_PORT env var if set, so
        // the .deb's `Environment=KINO_PORT=80` ends up reflected in
        // the config row that Settings → General → Port displays
        // and edits. After this first-run insert, Settings is the
        // authoritative source — KINO_PORT is only consulted at
        // schema-bootstrap time. The schema default (80) covers
        // installs without the env (cargo install, AppImage, dev).
        let initial_port: i64 = std::env::var("KINO_PORT")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|p| *p > 0 && *p < 65_536)
            .unwrap_or(80);

        // Seed mDNS hostname from KINO_MDNS_HOSTNAME env (so the dev
        // container's docker-compose value `kino-dev` lands in the
        // DB on first run). Schema default is "kino" for everything
        // else. After first-run, Settings → Networking is the
        // authoritative source — env is only consulted here.
        let initial_mdns_hostname: Option<String> = std::env::var("KINO_MDNS_HOSTNAME")
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        sqlx::query(
            "INSERT INTO config (id, api_key, data_path, tmdb_api_key, media_library_path, download_path, hw_acceleration, cast_receiver_app_id, backup_location_path, listen_port, mdns_hostname) VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, 'kino'))",
        )
        .bind(&api_key)
        .bind(data_path)
        .bind(&tmdb_key)
        .bind(&media_path)
        .bind(&download_path)
        .bind(&initial_hw)
        .bind(cast_app_id.as_deref())
        .bind(&default_backup_location)
        .bind(initial_port)
        .bind(initial_mdns_hostname.as_deref())
        .execute(pool)
        .await?;

        if !tmdb_key.is_empty() {
            tracing::info!("TMDB API key set from environment");
        }
        if let Some(ref id) = cast_app_id {
            tracing::info!(app_id = %id, "Cast receiver application ID set from environment");
        }
        if !media_path.is_empty() {
            tracing::info!(path = %media_path, "media library path set from environment");
        }
    }

    // Belt-and-braces directory creation. The .deb postinst already
    // creates /var/lib/kino/{library,downloads}, but cargo install /
    // portable / Docker users don't get the postinst. Read whatever
    // paths the config row currently holds and create_dir_all each;
    // EACCES is silently swallowed (the path picker surfaces the real
    // error if kino can't actually use the path). Cheap idempotent
    // op — runs on every boot.
    let cfg_paths: Option<(String, String)> =
        sqlx::query_as("SELECT media_library_path, download_path FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if let Some((media, downloads)) = cfg_paths {
        for p in [&media, &downloads] {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let _ = std::fs::create_dir_all(trimmed);
            }
        }
    }

    // Default quality profile
    let profile_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM quality_profile")
        .fetch_one(pool)
        .await?;

    if profile_count == 0 {
        let items = default_quality_items();
        sqlx::query(
            "INSERT INTO quality_profile (name, upgrade_allowed, cutoff, items, accepted_languages, is_default) VALUES ('Default', 1, 'bluray_1080p', ?, '[\"en\"]', 1)",
        )
        .bind(&items)
        .execute(pool)
        .await?;

        tracing::info!("created default quality profile");
    }

    Ok(was_first_run)
}

/// If VPN is enabled in config and the tunnel comes up, return a
/// `VpnManager`. Returns `Ok(None)` **only** when VPN is disabled in
/// config — that's the user's explicit "no VPN" choice and the
/// caller is free to start the torrent client on the default route.
///
/// When `vpn_enabled = true` but the tunnel can't start (missing
/// fields, boringtun error, network down, etc.), this returns
/// `Err(_)`. The caller MUST treat that as fail-closed: do not
/// start the torrent client, because binding to the default route
/// would leak the user's real IP on every peer handshake.
pub async fn maybe_start_vpn(pool: &SqlitePool) -> anyhow::Result<Option<VpnManager>> {
    let enabled: bool =
        sqlx::query_scalar::<_, bool>("SELECT vpn_enabled FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or(false);
    if !enabled {
        tracing::info!("VPN disabled in config — torrents will bind to the default interface");
        return Ok(None);
    }

    let cfg = health::load_config(pool).await.map_err(|e| {
        anyhow::anyhow!(
            "VPN enabled but config incomplete: {e} — refusing to continue (would fail open)"
        )
    })?;

    let manager = VpnManager::new("wg0");
    manager.connect(&cfg).await?;
    tracing::info!(iface = %manager.interface_name(), "VPN tunnel up");

    // Port forwarding (best-effort). Failure here is non-fatal: the
    // tunnel still protects the user's IP even without a forwarded
    // port — they just won't be reachable as a BT peer, which costs
    // throughput but not privacy. Any required ID / key params for
    // provider APIs come off the same config row that `load_config`
    // read.
    if !cfg.port_forward_provider.is_empty() && cfg.port_forward_provider != "none" {
        if let Some(gateway) = crate::download::vpn::port_forward::derive_ipv4_gateway(&cfg.address)
        {
            if let Err(e) = manager
                .start_port_forwarding(
                    &cfg.port_forward_provider,
                    cfg.port_forward_api_key.as_deref(),
                    gateway,
                    6881,
                )
                .await
            {
                tracing::warn!(
                    provider = %cfg.port_forward_provider,
                    gateway = %gateway,
                    error = %e,
                    "port forwarding failed — tunnel remains up without external port"
                );
            }
        } else {
            tracing::warn!(
                address = %cfg.address,
                "could not derive gateway from tunnel address (IPv6 / malformed?)"
            );
        }
    }

    Ok(Some(manager))
}

/// Create TMDB client if API key is configured.
pub async fn create_tmdb_client(pool: &SqlitePool) -> Option<TmdbClient> {
    let key: Option<String> = sqlx::query_scalar("SELECT tmdb_api_key FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    match key {
        Some(k) if !k.is_empty() => {
            tracing::info!("TMDB client initialized");
            Some(TmdbClient::new(k))
        }
        _ => {
            tracing::warn!("TMDB API key not configured — metadata features disabled");
            None
        }
    }
}
