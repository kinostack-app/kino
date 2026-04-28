use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::state::AppState;

/// One entry in the status warning list. Carries a human message and
/// the route on the frontend that remediates it — so the `HealthBanner`
/// can render a "Fix" link that lands the user on the exact settings
/// page instead of a generic /settings jump.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StatusWarning {
    pub message: String,
    /// Frontend route that fixes this warning (e.g. `/settings/playback`).
    /// Omitted when no single page fixes it — the banner then falls
    /// back to the top-level settings index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

impl StatusWarning {
    fn new(message: &str, route: Option<&str>) -> Self {
        Self {
            message: message.to_owned(),
            route: route.map(str::to_owned),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct StatusResponse {
    pub version: String,
    pub status: String,
    /// True only on genuine first-run: config row missing or the core
    /// settings (TMDB key, download path, media library path) are blank.
    /// The setup wizard gates on this — post-setup warnings (no indexers,
    /// stuck downloads, etc.) surface in `warnings` instead, not by
    /// re-triggering the wizard.
    pub first_time_setup: bool,
    /// Broader "something the user should know about" flag — includes
    /// first-time setup plus runtime issues (no enabled indexer, TMDB
    /// client failed to init, …). Frontend banner uses `warnings`.
    pub setup_required: bool,
    pub warnings: Vec<StatusWarning>,
    /// Install descriptor — read once at boot from `KINO_INSTALL_KIND`
    /// (set by the systemd unit / launchd plist / Windows Service /
    /// Dockerfile / AppImage launcher). Drives platform-specific UX
    /// in the SPA (Storage-step copy, permission-banner remediation,
    /// docs links). `None` when the env var is unset (cargo install,
    /// portable, dev container) — SPA falls back to neutral copy.
    /// Values: `linux-systemd` | `macos-launchd` | `windows-service`
    /// | `appimage` | `docker` | `homebrew` | `cargo` | other.
    pub install_kind: Option<String>,
}

/// Returns server status with health warnings (no auth required).
#[utoipa::path(
    get,
    path = "/api/v1/status",
    responses(
        (status = 200, description = "Server status", body = StatusResponse)
    ),
    tag = "system"
)]
pub async fn get_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let mut warnings: Vec<StatusWarning> = Vec::new();
    let mut setup_required = false;
    let mut first_time_setup = false;

    check_config(
        &state,
        &mut warnings,
        &mut setup_required,
        &mut first_time_setup,
    )
    .await;
    check_paths(&state, &mut warnings).await;
    check_services(&state, &mut warnings, &mut setup_required).await;
    check_ffmpeg(&mut warnings);
    check_hw_acceleration(&state, &mut warnings).await;
    check_stuck_downloads(&state, &mut warnings).await;
    check_reconcile(&state, &mut warnings).await;

    let status = if setup_required {
        "setup_required"
    } else if warnings.is_empty() {
        "ok"
    } else {
        "degraded"
    };

    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        status: status.to_owned(),
        first_time_setup,
        setup_required,
        warnings,
        install_kind: std::env::var("KINO_INSTALL_KIND")
            .ok()
            .filter(|s| !s.trim().is_empty()),
    })
}

async fn check_config(
    state: &AppState,
    warnings: &mut Vec<StatusWarning>,
    setup_required: &mut bool,
    first_time_setup: &mut bool,
) {
    let Ok(config) = sqlx::query_as::<_, ConfigCheck>(
        "SELECT tmdb_api_key, download_path, media_library_path FROM config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await
    else {
        return;
    };
    let Some(c) = config else {
        *setup_required = true;
        *first_time_setup = true;
        warnings.push(StatusWarning::new(
            "No configuration found — first-time setup required",
            Some("/settings/general"),
        ));
        return;
    };
    if c.tmdb_api_key.as_deref().unwrap_or("").is_empty() {
        warnings.push(StatusWarning::new(
            "TMDB API key not configured — browse and metadata disabled",
            Some("/settings/metadata"),
        ));
        *setup_required = true;
        *first_time_setup = true;
    }
    if c.download_path.as_deref().unwrap_or("").is_empty() {
        warnings.push(StatusWarning::new(
            "Download path not configured — torrents cannot download",
            Some("/settings/library"),
        ));
        *setup_required = true;
        *first_time_setup = true;
    }
    if c.media_library_path.as_deref().unwrap_or("").is_empty() {
        warnings.push(StatusWarning::new(
            "Media library path not configured",
            Some("/settings/library"),
        ));
        *setup_required = true;
        *first_time_setup = true;
    }
}

async fn check_services(
    state: &AppState,
    warnings: &mut Vec<StatusWarning>,
    setup_required: &mut bool,
) {
    if let Ok(count) =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM indexer WHERE enabled = 1")
            .fetch_one(&state.db)
            .await
        && count == 0
    {
        warnings.push(StatusWarning::new(
            "No indexers configured — search will not find releases",
            Some("/settings/indexers"),
        ));
        *setup_required = true;
    }

    if state.torrent.is_none() {
        warnings.push(StatusWarning::new(
            "Torrent client not running — check download path",
            Some("/settings/library"),
        ));
    }

    if state.tmdb.is_none() {
        warnings.push(StatusWarning::new(
            "TMDB client not initialized — check API key",
            Some("/settings/metadata"),
        ));
        *setup_required = true;
    }
}

/// Emits at most one ffmpeg-specific warning per probe, picking
/// the most serious condition. Using the cached probe result —
/// not a fresh `ffmpeg -version` — means the build-info fields
/// (version major, libplacebo, libass, Jellyfin-build flag) are
/// available at zero extra cost.
///
/// Severity priority (highest to lowest):
/// 1. ffmpeg missing (banner is the only signal — blocks playback)
/// 2. version major < 4 (hard-error — kino requires ≥ 7.x for
///    modern hardware + HDR features)
/// 3. version major < 7 (Blackwell-class compat issues + missing
///    libplacebo APIs)
/// 4. libplacebo missing (HDR tone-mapping falls back to SW-only
///    path — slower, lower quality)
/// 5. libass missing (styled ASS subs render unstyled)
///
/// Only the highest severity fires — the settings-page panel
/// renders the full per-feature breakdown for operators who
/// want to audit everything at once.
fn check_ffmpeg(warnings: &mut Vec<StatusWarning>) {
    let Some(caps) = crate::playback::hw_probe_cache::cached() else {
        return; // probe hasn't run yet, banner stays quiet
    };
    if !caps.ffmpeg_ok {
        warnings.push(StatusWarning::new(
            "FFmpeg not found — transcoding and playback will fail. \
             Install jellyfin-ffmpeg or configure a path in Playback settings.",
            Some("/settings/playback"),
        ));
        return;
    }
    match caps.ffmpeg_major {
        Some(m) if m < 4 => {
            warnings.push(StatusWarning::new(
                &format!(
                    "FFmpeg {m}.x detected — kino requires 7.x or newer. \
                     Download the bundled jellyfin-ffmpeg from Playback settings."
                ),
                Some("/settings/playback"),
            ));
            return;
        }
        Some(m) if m < 7 => {
            warnings.push(StatusWarning::new(
                &format!(
                    "FFmpeg {m}.x detected — 2024+ GPUs (Blackwell etc.) and HDR \
                     features may not work correctly. Download jellyfin-ffmpeg 7.x \
                     from Playback settings."
                ),
                Some("/settings/playback"),
            ));
            return;
        }
        _ => {}
    }
    if !caps.has_libplacebo {
        warnings.push(StatusWarning::new(
            "FFmpeg build lacks libplacebo — HDR tone-mapping uses the software \
             path only (slower, lower quality). Download jellyfin-ffmpeg for \
             HW-accelerated tone-mapping.",
            Some("/settings/playback"),
        ));
        return;
    }
    if !caps.has_libass {
        warnings.push(StatusWarning::new(
            "FFmpeg build lacks libass — styled ASS / SSA subtitles will render \
             unstyled. Download jellyfin-ffmpeg for styled subtitle support.",
            Some("/settings/playback"),
        ));
    }
}

/// Warn when the transcoder is running on software but the probe
/// found a hardware backend available. Cached probe result means
/// this check is free — no ffmpeg is launched here. Skipped
/// entirely when the probe hasn't run yet so the banner doesn't
/// flicker on fresh boots.
async fn check_hw_acceleration(state: &AppState, warnings: &mut Vec<StatusWarning>) {
    let Some(caps) = crate::playback::hw_probe_cache::cached() else {
        return;
    };
    if !caps.ffmpeg_ok || !caps.any_available() {
        return;
    }
    let Some(suggested) = caps.suggested() else {
        return;
    };
    let current: String = sqlx::query_scalar("SELECT hw_acceleration FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "none".to_string());
    if current == "none" {
        warnings.push(StatusWarning::new(
            &format!(
                "Software transcoding in use — {} hardware acceleration is available on this host",
                suggested.label()
            ),
            Some("/settings/playback"),
        ));
    }
}

/// Verify that `download_path` + `media_library_path` exist and are
/// writable, and that the download volume has enough free space (per
/// the user-configurable `low_disk_threshold_gb` config field).
/// Catches the "typoed env var", "volume unmounted", and "disk full"
/// failure modes early — previously these surfaced only as confusing
/// errors on the first grab or first import, long after the actual
/// setup mistake.
async fn check_paths(state: &AppState, warnings: &mut Vec<StatusWarning>) {
    let Ok(Some(paths)) = sqlx::query_as::<_, (Option<String>, Option<String>, i64)>(
        "SELECT download_path, media_library_path, low_disk_threshold_gb FROM config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await
    else {
        return;
    };
    let low_disk_warning_bytes = u64::try_from(paths.2.max(0))
        .unwrap_or(5)
        .saturating_mul(1024 * 1024 * 1024);

    for (label, path, route, check_space) in [
        (
            "Download path",
            paths.0.as_deref(),
            "/settings/library",
            true,
        ),
        (
            "Media library path",
            paths.1.as_deref(),
            "/settings/library",
            false,
        ),
    ] {
        let Some(path) = path else { continue };
        if path.is_empty() {
            // check_config already flagged missing paths.
            continue;
        }
        match probe_writable(path) {
            Ok(()) => {}
            Err(reason) => {
                warnings.push(StatusWarning::new(
                    &format!("{label} not usable: {reason}"),
                    Some(route),
                ));
                continue;
            }
        }
        if check_space
            && let Some(free) = fs4::available_space(std::path::Path::new(path)).ok()
            && free < low_disk_warning_bytes
        {
            warnings.push(StatusWarning::new(
                &format!(
                    "Download path low on free space: {} free",
                    human_bytes(free),
                ),
                Some(route),
            ));
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.0} MB", n / MB)
    } else {
        format!("{n:.0} B")
    }
}

/// Best-effort directory write check. Creates a tiny probe file with
/// the PID in the name to avoid collisions with concurrent callers,
/// then removes it. Returns Err with a human message on any failure.
fn probe_writable(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    let md = std::fs::metadata(p).map_err(|e| format!("cannot access {path}: {e}"))?;
    if !md.is_dir() {
        return Err(format!("{path} is not a directory"));
    }
    let probe = p.join(format!(".kino-write-probe-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|e| format!("not writable: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

async fn check_stuck_downloads(state: &AppState, warnings: &mut Vec<StatusWarning>) {
    // Routed to the Downloads page rather than /settings — the user
    // unsticks them from the live downloads list.
    if let Ok(stuck) = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM download WHERE state = 'importing' AND datetime(added_at) < datetime('now', '-10 minutes')",
    )
    .fetch_one(&state.db)
    .await
        && stuck > 0
    {
        warnings.push(StatusWarning::new(
            &format!("{stuck} download(s) stuck in importing state"),
            Some("/downloads"),
        ));
    }
}

/// Surface invariant drift caught by the 60s reconcile task.
/// One warning per distinct invariant with at least one violation;
/// the message includes the count for the user to triage.
async fn check_reconcile(state: &AppState, warnings: &mut Vec<StatusWarning>) {
    let Some(report) = state.last_reconcile.read().await.clone() else {
        return;
    };
    if report.invariant_violations.is_empty() {
        return;
    }
    let mut by_invariant: std::collections::BTreeMap<&'static str, u64> =
        std::collections::BTreeMap::new();
    for v in &report.invariant_violations {
        *by_invariant.entry(v.invariant).or_insert(0) += 1;
    }
    for (name, count) in by_invariant {
        warnings.push(StatusWarning::new(
            &format!("{count} invariant violation(s): {name}"),
            // No single fix-route — the admin needs to inspect the
            // affected rows. Future: a /admin/invariants page.
            None,
        ));
    }
}

#[derive(sqlx::FromRow)]
struct ConfigCheck {
    tmdb_api_key: Option<String>,
    download_path: Option<String>,
    media_library_path: Option<String>,
}
