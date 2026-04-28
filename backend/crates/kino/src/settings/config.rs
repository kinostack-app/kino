//! Application configuration — the singleton `config` row plus the
//! GET / PATCH endpoints. Sensitive fields (API keys, VPN
//! credentials) are masked in the response shape.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::events::{AppEvent, ConfigScope};
use crate::state::AppState;

/// Raw config row from database.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ConfigRow {
    pub id: i64,
    pub listen_address: String,
    pub listen_port: i64,
    pub api_key: String,
    pub base_url: String,
    pub data_path: String,
    pub media_library_path: String,
    pub download_path: String,
    pub vpn_enabled: bool,
    pub vpn_private_key: Option<String>,
    pub vpn_address: Option<String>,
    pub vpn_server_public_key: Option<String>,
    pub vpn_server_endpoint: Option<String>,
    pub vpn_dns: Option<String>,
    pub vpn_port_forward_provider: String,
    pub vpn_port_forward_api_key: Option<String>,
    pub vpn_killswitch_enabled: bool,
    pub vpn_killswitch_check_url: String,
    pub tmdb_api_key: String,
    pub opensubtitles_api_key: Option<String>,
    pub opensubtitles_username: Option<String>,
    pub opensubtitles_password: Option<String>,
    pub max_concurrent_downloads: i64,
    pub download_speed_limit: i64,
    pub upload_speed_limit: i64,
    pub seed_ratio_limit: f64,
    pub seed_time_limit: i64,
    /// Free-space threshold (GB) below which `/status` raises a
    /// "low free space" warning for the download path. Default 5.
    pub low_disk_threshold_gb: i64,
    pub transcoding_enabled: bool,
    pub ffmpeg_path: String,
    pub hw_acceleration: String,
    pub max_concurrent_transcodes: i64,
    pub cast_receiver_app_id: Option<String>,
    pub auto_cleanup_enabled: bool,
    pub auto_cleanup_movie_delay: i64,
    pub auto_cleanup_episode_delay: i64,
    pub auto_upgrade_enabled: bool,
    pub auto_search_interval: i64,
    pub stall_timeout: i64,
    pub dead_timeout: i64,
    pub use_hardlinks: bool,
    pub movie_naming_format: String,
    pub episode_naming_format: String,
    pub multi_episode_naming_format: String,
    pub season_folder_format: String,
    // Trakt (docs/subsystems/16-trakt.md § Configuration).
    pub trakt_client_id: Option<String>,
    pub trakt_client_secret: Option<String>,
    pub trakt_scrobble: bool,
    pub trakt_sync_watched: bool,
    pub trakt_sync_ratings: bool,
    pub trakt_sync_watchlist: bool,
    pub trakt_sync_collection: bool,
    pub trakt_resume_sync_enabled: bool,
    pub trakt_recommendations_enabled: bool,
    // Lists (subsystem 17).
    pub mdblist_api_key: Option<String>,
    pub list_bulk_growth_threshold: i64,
    // Intro-skipper (subsystem 15).
    pub intro_detect_enabled: bool,
    pub credits_detect_enabled: bool,
    /// 'off' / 'on' / 'smart'.
    pub auto_skip_intros: String,
    pub auto_skip_credits: bool,
    pub intro_min_length_s: i64,
    pub intro_analysis_limit_s: i64,
    pub credits_analysis_limit_s: i64,
    pub intro_match_score_threshold: f64,
    pub max_concurrent_intro_analyses: i64,
    // mDNS (subsystem 25)
    pub mdns_enabled: bool,
    pub mdns_hostname: String,
    pub mdns_service_name: String,
    // Backup & restore (subsystem 19)
    pub backup_enabled: bool,
    pub backup_schedule: String,
    pub backup_time: String,
    pub backup_location_path: String,
    pub backup_retention_count: i64,
}

/// Config response with sensitive fields masked.
#[derive(Debug, Serialize, ToSchema)]

pub struct ConfigResponse {
    pub id: i64,
    pub listen_address: String,
    pub listen_port: i64,
    pub api_key: String,
    pub base_url: String,
    pub data_path: String,
    pub media_library_path: String,
    pub download_path: String,
    pub vpn_enabled: bool,
    pub vpn_private_key: String,
    pub vpn_address: Option<String>,
    pub vpn_server_public_key: Option<String>,
    pub vpn_server_endpoint: Option<String>,
    pub vpn_dns: Option<String>,
    pub vpn_port_forward_provider: String,
    pub vpn_port_forward_api_key: String,
    pub vpn_killswitch_enabled: bool,
    pub vpn_killswitch_check_url: String,
    pub tmdb_api_key: String,
    pub opensubtitles_api_key: Option<String>,
    pub opensubtitles_username: Option<String>,
    pub opensubtitles_password: String,
    pub max_concurrent_downloads: i64,
    pub download_speed_limit: i64,
    pub upload_speed_limit: i64,
    pub seed_ratio_limit: f64,
    pub seed_time_limit: i64,
    pub low_disk_threshold_gb: i64,
    pub transcoding_enabled: bool,
    pub ffmpeg_path: String,
    pub hw_acceleration: String,
    pub max_concurrent_transcodes: i64,
    pub cast_receiver_app_id: Option<String>,
    pub auto_cleanup_enabled: bool,
    pub auto_cleanup_movie_delay: i64,
    pub auto_cleanup_episode_delay: i64,
    pub auto_upgrade_enabled: bool,
    pub auto_search_interval: i64,
    pub stall_timeout: i64,
    pub dead_timeout: i64,
    pub use_hardlinks: bool,
    pub movie_naming_format: String,
    pub episode_naming_format: String,
    pub multi_episode_naming_format: String,
    pub season_folder_format: String,
    // Trakt — client_id/secret surfaced as empty string when unset so
    // the frontend can treat them like other optional-secret inputs
    // without null checks. Toggles are always present.
    pub trakt_client_id: String,
    pub trakt_client_secret: String,
    pub trakt_scrobble: bool,
    pub trakt_sync_watched: bool,
    pub trakt_sync_ratings: bool,
    pub trakt_sync_watchlist: bool,
    pub trakt_sync_collection: bool,
    pub trakt_resume_sync_enabled: bool,
    pub trakt_recommendations_enabled: bool,
    pub mdblist_api_key: String,
    pub list_bulk_growth_threshold: i64,
    // Intro-skipper (subsystem 15).
    pub intro_detect_enabled: bool,
    pub credits_detect_enabled: bool,
    pub auto_skip_intros: String,
    pub auto_skip_credits: bool,
    pub intro_min_length_s: i64,
    pub intro_analysis_limit_s: i64,
    pub credits_analysis_limit_s: i64,
    pub intro_match_score_threshold: f64,
    pub max_concurrent_intro_analyses: i64,
    // mDNS (subsystem 25)
    pub mdns_enabled: bool,
    pub mdns_hostname: String,
    pub mdns_service_name: String,
    // Backup & restore (subsystem 19)
    pub backup_enabled: bool,
    pub backup_schedule: String,
    pub backup_time: String,
    pub backup_location_path: String,
    pub backup_retention_count: i64,
}

/// Placeholder for masked secret fields. The frontend treats this
/// exact string as "secret already set, don't change it"; any other
/// value (including empty) flows through as a real update. Keeping
/// it three chars avoids accidental confusion with real keys (no
/// legitimate API key or password is `***`).
pub const REDACTED: &str = "***";

/// Returns `REDACTED` if the field has a value; empty otherwise so
/// the UI can distinguish "never set" from "set but masked".
fn mask_opt(v: Option<&str>) -> String {
    match v {
        Some(s) if !s.is_empty() => REDACTED.to_owned(),
        _ => String::new(),
    }
}

fn mask_req(v: &str) -> String {
    if v.is_empty() {
        String::new()
    } else {
        REDACTED.to_owned()
    }
}

impl ConfigResponse {
    pub fn from_row(c: ConfigRow) -> Self {
        Self {
            id: c.id,
            listen_address: c.listen_address,
            listen_port: c.listen_port,
            // Sensitive fields are masked with `***`. Confirms to
            // the UI that the value is set without exposing it via
            // the API. Re-saving with `***` is a no-op on the write
            // side; sending a real string replaces the value.
            api_key: mask_req(&c.api_key),
            base_url: c.base_url,
            data_path: c.data_path,
            media_library_path: c.media_library_path,
            download_path: c.download_path,
            vpn_enabled: c.vpn_enabled,
            vpn_private_key: mask_opt(c.vpn_private_key.as_deref()),
            vpn_address: c.vpn_address,
            vpn_server_public_key: c.vpn_server_public_key,
            vpn_server_endpoint: c.vpn_server_endpoint,
            vpn_dns: c.vpn_dns,
            vpn_port_forward_provider: c.vpn_port_forward_provider,
            vpn_port_forward_api_key: mask_opt(c.vpn_port_forward_api_key.as_deref()),
            vpn_killswitch_enabled: c.vpn_killswitch_enabled,
            vpn_killswitch_check_url: c.vpn_killswitch_check_url,
            tmdb_api_key: mask_req(&c.tmdb_api_key),
            opensubtitles_api_key: c.opensubtitles_api_key.as_deref().map(mask_req),
            opensubtitles_username: c.opensubtitles_username,
            opensubtitles_password: mask_opt(c.opensubtitles_password.as_deref()),
            max_concurrent_downloads: c.max_concurrent_downloads,
            download_speed_limit: c.download_speed_limit,
            upload_speed_limit: c.upload_speed_limit,
            seed_ratio_limit: c.seed_ratio_limit,
            seed_time_limit: c.seed_time_limit,
            low_disk_threshold_gb: c.low_disk_threshold_gb,
            transcoding_enabled: c.transcoding_enabled,
            ffmpeg_path: c.ffmpeg_path,
            hw_acceleration: c.hw_acceleration,
            max_concurrent_transcodes: c.max_concurrent_transcodes,
            cast_receiver_app_id: c.cast_receiver_app_id,
            auto_cleanup_enabled: c.auto_cleanup_enabled,
            auto_cleanup_movie_delay: c.auto_cleanup_movie_delay,
            auto_cleanup_episode_delay: c.auto_cleanup_episode_delay,
            auto_upgrade_enabled: c.auto_upgrade_enabled,
            auto_search_interval: c.auto_search_interval,
            stall_timeout: c.stall_timeout,
            dead_timeout: c.dead_timeout,
            use_hardlinks: c.use_hardlinks,
            movie_naming_format: c.movie_naming_format,
            episode_naming_format: c.episode_naming_format,
            multi_episode_naming_format: c.multi_episode_naming_format,
            season_folder_format: c.season_folder_format,
            trakt_client_id: c.trakt_client_id.unwrap_or_default(),
            trakt_client_secret: mask_opt(c.trakt_client_secret.as_deref()),
            trakt_scrobble: c.trakt_scrobble,
            trakt_sync_watched: c.trakt_sync_watched,
            trakt_sync_ratings: c.trakt_sync_ratings,
            trakt_sync_watchlist: c.trakt_sync_watchlist,
            trakt_sync_collection: c.trakt_sync_collection,
            trakt_resume_sync_enabled: c.trakt_resume_sync_enabled,
            trakt_recommendations_enabled: c.trakt_recommendations_enabled,
            mdblist_api_key: mask_opt(c.mdblist_api_key.as_deref()),
            list_bulk_growth_threshold: c.list_bulk_growth_threshold,
            intro_detect_enabled: c.intro_detect_enabled,
            credits_detect_enabled: c.credits_detect_enabled,
            auto_skip_intros: c.auto_skip_intros,
            auto_skip_credits: c.auto_skip_credits,
            intro_min_length_s: c.intro_min_length_s,
            intro_analysis_limit_s: c.intro_analysis_limit_s,
            credits_analysis_limit_s: c.credits_analysis_limit_s,
            intro_match_score_threshold: c.intro_match_score_threshold,
            max_concurrent_intro_analyses: c.max_concurrent_intro_analyses,
            mdns_enabled: c.mdns_enabled,
            mdns_hostname: c.mdns_hostname,
            mdns_service_name: c.mdns_service_name,
            backup_enabled: c.backup_enabled,
            backup_schedule: c.backup_schedule,
            backup_time: c.backup_time,
            backup_location_path: c.backup_location_path,
            backup_retention_count: c.backup_retention_count,
        }
    }
}

/// Partial update payload for config.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ConfigUpdate {
    pub listen_address: Option<String>,
    pub listen_port: Option<i64>,
    pub base_url: Option<String>,
    pub media_library_path: Option<String>,
    pub download_path: Option<String>,
    pub vpn_enabled: Option<bool>,
    pub vpn_private_key: Option<String>,
    pub vpn_address: Option<String>,
    pub vpn_server_public_key: Option<String>,
    pub vpn_server_endpoint: Option<String>,
    pub vpn_dns: Option<String>,
    pub vpn_port_forward_provider: Option<String>,
    pub vpn_port_forward_api_key: Option<String>,
    pub vpn_killswitch_enabled: Option<bool>,
    pub vpn_killswitch_check_url: Option<String>,
    pub tmdb_api_key: Option<String>,
    pub opensubtitles_api_key: Option<String>,
    pub opensubtitles_username: Option<String>,
    pub opensubtitles_password: Option<String>,
    pub max_concurrent_downloads: Option<i64>,
    pub download_speed_limit: Option<i64>,
    pub upload_speed_limit: Option<i64>,
    pub seed_ratio_limit: Option<f64>,
    pub seed_time_limit: Option<i64>,
    pub low_disk_threshold_gb: Option<i64>,
    pub transcoding_enabled: Option<bool>,
    pub ffmpeg_path: Option<String>,
    pub hw_acceleration: Option<String>,
    pub max_concurrent_transcodes: Option<i64>,
    pub cast_receiver_app_id: Option<String>,
    pub auto_cleanup_enabled: Option<bool>,
    pub auto_cleanup_movie_delay: Option<i64>,
    pub auto_cleanup_episode_delay: Option<i64>,
    pub auto_upgrade_enabled: Option<bool>,
    pub auto_search_interval: Option<i64>,
    pub stall_timeout: Option<i64>,
    pub dead_timeout: Option<i64>,
    pub use_hardlinks: Option<bool>,
    pub movie_naming_format: Option<String>,
    pub episode_naming_format: Option<String>,
    pub multi_episode_naming_format: Option<String>,
    pub season_folder_format: Option<String>,
    pub trakt_client_id: Option<String>,
    pub trakt_client_secret: Option<String>,
    pub trakt_scrobble: Option<bool>,
    pub trakt_sync_watched: Option<bool>,
    pub trakt_sync_ratings: Option<bool>,
    pub trakt_sync_watchlist: Option<bool>,
    pub trakt_sync_collection: Option<bool>,
    pub trakt_resume_sync_enabled: Option<bool>,
    pub trakt_recommendations_enabled: Option<bool>,
    pub mdblist_api_key: Option<String>,
    pub list_bulk_growth_threshold: Option<i64>,
    pub intro_detect_enabled: Option<bool>,
    pub credits_detect_enabled: Option<bool>,
    pub auto_skip_intros: Option<String>,
    pub auto_skip_credits: Option<bool>,
    pub intro_min_length_s: Option<i64>,
    pub intro_analysis_limit_s: Option<i64>,
    pub credits_analysis_limit_s: Option<i64>,
    pub intro_match_score_threshold: Option<f64>,
    pub max_concurrent_intro_analyses: Option<i64>,
    pub mdns_enabled: Option<bool>,
    pub mdns_hostname: Option<String>,
    pub mdns_service_name: Option<String>,
    // Backup & restore (subsystem 19)
    pub backup_enabled: Option<bool>,
    pub backup_schedule: Option<String>,
    pub backup_time: Option<String>,
    pub backup_location_path: Option<String>,
    pub backup_retention_count: Option<i64>,
}

// ─── HTTP handlers ──────────────────────────────────────────────────

/// Get application configuration (sensitive fields masked).
#[utoipa::path(
    get,
    path = "/api/v1/config",
    responses(
        (status = 200, description = "Current configuration", body = ConfigResponse),
        (status = 404, description = "Config not initialized")
    ),
    tag = "config",
    security(("api_key" = []))
)]
pub async fn get_config(State(state): State<AppState>) -> AppResult<Json<ConfigResponse>> {
    let config = sqlx::query_as::<_, ConfigRow>("SELECT * FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("config not initialized".into()))?;

    Ok(Json(ConfigResponse::from_row(config)))
}

/// Update application configuration (partial update via COALESCE).
/// Drop the value when the caller sent the masking sentinel `***`
/// (the frontend round-trips the masked `GET /config` response).
/// Keeping the sentinel would overwrite the real secret with the
/// string `"***"` and silently break VPN / Trakt / etc. Applied
/// only to the secret-bearing fields; everything else passes
/// through untouched.
fn strip_mask_sentinel(mut update: ConfigUpdate) -> ConfigUpdate {
    fn f(v: &mut Option<String>) {
        if matches!(v.as_deref(), Some(crate::settings::config::REDACTED)) {
            *v = None;
        }
    }
    f(&mut update.vpn_private_key);
    f(&mut update.vpn_port_forward_api_key);
    f(&mut update.tmdb_api_key);
    f(&mut update.opensubtitles_api_key);
    f(&mut update.opensubtitles_password);
    f(&mut update.trakt_client_secret);
    f(&mut update.mdblist_api_key);
    update
}

#[utoipa::path(
    put,
    path = "/api/v1/config",
    request_body = ConfigUpdate,
    responses(
        (status = 200, description = "Updated configuration", body = ConfigResponse),
        (status = 404, description = "Config not initialized")
    ),
    tag = "config",
    security(("api_key" = []))
)]
// Long COALESCE SQL + matching bind chain; splitting them would just
// move the column list somewhere else without making the body any
// clearer.
#[allow(clippy::too_many_lines)]
pub async fn update_config(
    State(state): State<AppState>,
    Json(update): Json<ConfigUpdate>,
) -> AppResult<Json<ConfigResponse>> {
    let update = strip_mask_sentinel(update);
    // COALESCE pattern: pass new value or NULL; SQL keeps existing when NULL
    let result = sqlx::query(
        r"UPDATE config SET
            listen_address             = COALESCE(?, listen_address),
            listen_port                = COALESCE(?, listen_port),
            base_url                   = COALESCE(?, base_url),
            media_library_path         = COALESCE(?, media_library_path),
            download_path              = COALESCE(?, download_path),
            vpn_enabled                = COALESCE(?, vpn_enabled),
            vpn_private_key            = COALESCE(?, vpn_private_key),
            vpn_address                = COALESCE(?, vpn_address),
            vpn_server_public_key      = COALESCE(?, vpn_server_public_key),
            vpn_server_endpoint        = COALESCE(?, vpn_server_endpoint),
            vpn_dns                    = COALESCE(?, vpn_dns),
            vpn_port_forward_provider  = COALESCE(?, vpn_port_forward_provider),
            vpn_port_forward_api_key   = COALESCE(?, vpn_port_forward_api_key),
            vpn_killswitch_enabled     = COALESCE(?, vpn_killswitch_enabled),
            vpn_killswitch_check_url   = COALESCE(?, vpn_killswitch_check_url),
            tmdb_api_key               = COALESCE(?, tmdb_api_key),
            opensubtitles_api_key      = COALESCE(?, opensubtitles_api_key),
            opensubtitles_username     = COALESCE(?, opensubtitles_username),
            opensubtitles_password     = COALESCE(?, opensubtitles_password),
            max_concurrent_downloads   = COALESCE(?, max_concurrent_downloads),
            download_speed_limit       = COALESCE(?, download_speed_limit),
            upload_speed_limit         = COALESCE(?, upload_speed_limit),
            seed_ratio_limit           = COALESCE(?, seed_ratio_limit),
            seed_time_limit            = COALESCE(?, seed_time_limit),
            low_disk_threshold_gb      = COALESCE(?, low_disk_threshold_gb),
            transcoding_enabled        = COALESCE(?, transcoding_enabled),
            ffmpeg_path                = COALESCE(?, ffmpeg_path),
            hw_acceleration            = COALESCE(?, hw_acceleration),
            max_concurrent_transcodes  = COALESCE(?, max_concurrent_transcodes),
            cast_receiver_app_id       = COALESCE(?, cast_receiver_app_id),
            auto_cleanup_enabled       = COALESCE(?, auto_cleanup_enabled),
            auto_cleanup_movie_delay   = COALESCE(?, auto_cleanup_movie_delay),
            auto_cleanup_episode_delay = COALESCE(?, auto_cleanup_episode_delay),
            auto_upgrade_enabled       = COALESCE(?, auto_upgrade_enabled),
            auto_search_interval       = COALESCE(?, auto_search_interval),
            stall_timeout              = COALESCE(?, stall_timeout),
            dead_timeout               = COALESCE(?, dead_timeout),
            use_hardlinks              = COALESCE(?, use_hardlinks),
            movie_naming_format        = COALESCE(?, movie_naming_format),
            episode_naming_format      = COALESCE(?, episode_naming_format),
            multi_episode_naming_format = COALESCE(?, multi_episode_naming_format),
            season_folder_format       = COALESCE(?, season_folder_format),
            trakt_client_id            = COALESCE(?, trakt_client_id),
            trakt_client_secret        = COALESCE(?, trakt_client_secret),
            trakt_scrobble             = COALESCE(?, trakt_scrobble),
            trakt_sync_watched         = COALESCE(?, trakt_sync_watched),
            trakt_sync_ratings         = COALESCE(?, trakt_sync_ratings),
            trakt_sync_watchlist       = COALESCE(?, trakt_sync_watchlist),
            trakt_sync_collection      = COALESCE(?, trakt_sync_collection),
            trakt_resume_sync_enabled  = COALESCE(?, trakt_resume_sync_enabled),
            trakt_recommendations_enabled = COALESCE(?, trakt_recommendations_enabled),
            mdblist_api_key            = COALESCE(?, mdblist_api_key),
            list_bulk_growth_threshold = COALESCE(?, list_bulk_growth_threshold),
            intro_detect_enabled       = COALESCE(?, intro_detect_enabled),
            credits_detect_enabled     = COALESCE(?, credits_detect_enabled),
            auto_skip_intros           = COALESCE(?, auto_skip_intros),
            auto_skip_credits          = COALESCE(?, auto_skip_credits),
            intro_min_length_s         = COALESCE(?, intro_min_length_s),
            intro_analysis_limit_s     = COALESCE(?, intro_analysis_limit_s),
            credits_analysis_limit_s   = COALESCE(?, credits_analysis_limit_s),
            intro_match_score_threshold = COALESCE(?, intro_match_score_threshold),
            max_concurrent_intro_analyses = COALESCE(?, max_concurrent_intro_analyses),
            mdns_enabled               = COALESCE(?, mdns_enabled),
            mdns_hostname              = COALESCE(?, mdns_hostname),
            mdns_service_name          = COALESCE(?, mdns_service_name),
            backup_enabled             = COALESCE(?, backup_enabled),
            backup_schedule            = COALESCE(?, backup_schedule),
            backup_time                = COALESCE(?, backup_time),
            backup_location_path       = COALESCE(?, backup_location_path),
            backup_retention_count     = COALESCE(?, backup_retention_count)
        WHERE id = 1",
    )
    .bind(update.listen_address.as_deref())
    .bind(update.listen_port)
    .bind(update.base_url.as_deref())
    .bind(update.media_library_path.as_deref())
    .bind(update.download_path.as_deref())
    .bind(update.vpn_enabled)
    .bind(update.vpn_private_key.as_deref())
    .bind(update.vpn_address.as_deref())
    .bind(update.vpn_server_public_key.as_deref())
    .bind(update.vpn_server_endpoint.as_deref())
    .bind(update.vpn_dns.as_deref())
    .bind(update.vpn_port_forward_provider.as_deref())
    .bind(update.vpn_port_forward_api_key.as_deref())
    .bind(update.vpn_killswitch_enabled)
    .bind(update.vpn_killswitch_check_url.as_deref())
    .bind(update.tmdb_api_key.as_deref())
    .bind(update.opensubtitles_api_key.as_deref())
    .bind(update.opensubtitles_username.as_deref())
    .bind(update.opensubtitles_password.as_deref())
    .bind(update.max_concurrent_downloads)
    .bind(update.download_speed_limit)
    .bind(update.upload_speed_limit)
    .bind(update.seed_ratio_limit)
    .bind(update.seed_time_limit)
    .bind(update.low_disk_threshold_gb)
    .bind(update.transcoding_enabled)
    .bind(update.ffmpeg_path.as_deref())
    .bind(update.hw_acceleration.as_deref())
    .bind(update.max_concurrent_transcodes)
    .bind(update.cast_receiver_app_id.as_deref())
    .bind(update.auto_cleanup_enabled)
    .bind(update.auto_cleanup_movie_delay)
    .bind(update.auto_cleanup_episode_delay)
    .bind(update.auto_upgrade_enabled)
    .bind(update.auto_search_interval)
    .bind(update.stall_timeout)
    .bind(update.dead_timeout)
    .bind(update.use_hardlinks)
    .bind(update.movie_naming_format.as_deref())
    .bind(update.episode_naming_format.as_deref())
    .bind(update.multi_episode_naming_format.as_deref())
    .bind(update.season_folder_format.as_deref())
    .bind(update.trakt_client_id.as_deref())
    .bind(update.trakt_client_secret.as_deref())
    .bind(update.trakt_scrobble)
    .bind(update.trakt_sync_watched)
    .bind(update.trakt_sync_ratings)
    .bind(update.trakt_sync_watchlist)
    .bind(update.trakt_sync_collection)
    .bind(update.trakt_resume_sync_enabled)
    .bind(update.trakt_recommendations_enabled)
    .bind(update.mdblist_api_key.as_deref())
    .bind(update.list_bulk_growth_threshold)
    .bind(update.intro_detect_enabled)
    .bind(update.credits_detect_enabled)
    .bind(update.auto_skip_intros.as_deref())
    .bind(update.auto_skip_credits)
    .bind(update.intro_min_length_s)
    .bind(update.intro_analysis_limit_s)
    .bind(update.credits_analysis_limit_s)
    .bind(update.intro_match_score_threshold)
    .bind(update.max_concurrent_intro_analyses)
    .bind(update.mdns_enabled)
    .bind(update.mdns_hostname.as_deref())
    .bind(update.mdns_service_name.as_deref())
    .bind(update.backup_enabled)
    .bind(update.backup_schedule.as_deref())
    .bind(update.backup_time.as_deref())
    .bind(update.backup_location_path.as_deref())
    .bind(update.backup_retention_count)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("config not initialized".into()));
    }

    for new_path in [
        update.media_library_path.as_deref(),
        update.download_path.as_deref(),
        update.backup_location_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    {
        let _ = std::fs::create_dir_all(new_path);
    }

    if let Some(key) = update.tmdb_api_key.as_deref() {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            state.set_tmdb(None);
        } else {
            state.set_tmdb(Some(crate::tmdb::TmdbClient::new(trimmed.to_owned())));
        }
    }

    sync_scheduler_intervals(&state, &update).await;

    // Broad scope — the PATCH body can touch any column. Individual
    // settings screens could eventually fan in with narrower scopes if
    // invalidation granularity matters.
    let _ = state.event_tx.send(AppEvent::ConfigChanged {
        scope: ConfigScope::All,
    });

    get_config(State(state)).await
}

/// Push any changed automation intervals to the running scheduler so
/// the Automation settings page takes effect without a backend
/// restart. Only touches tasks whose config field was included in the
/// PATCH body (the `Option` is `Some`) — unrelated config writes stay
/// no-ops for the scheduler.
async fn sync_scheduler_intervals(state: &AppState, update: &ConfigUpdate) {
    let Some(sched) = state.scheduler.as_ref() else {
        return;
    };
    if let Some(mins) = update.auto_search_interval {
        sched
            .set_interval(
                "wanted_search",
                std::time::Duration::from_secs(u64::try_from(mins.max(1)).unwrap_or(15) * 60),
            )
            .await;
    }
    // `metadata_refresh` no longer takes a user-configurable
    // interval: the cadence is per-row tiered in SQL (see
    // `metadata::refresh::refresh_sweep`) with a fixed 30-min
    // scheduler tick. The old `metadata_refresh_interval` config
    // field was dropped alongside the tiered-refresh work.
}

/// Rotate the API key. Generates a fresh UUID, writes it, and returns
/// the new config so the caller (the frontend, after confirming) can
/// update its stored key.
#[utoipa::path(
    post,
    path = "/api/v1/config/rotate-api-key",
    responses(
        (status = 200, description = "Updated configuration with new API key", body = ConfigResponse),
        (status = 404, description = "Config not initialized")
    ),
    tag = "config",
    security(("api_key" = []))
)]
pub async fn rotate_api_key(State(state): State<AppState>) -> AppResult<Json<ConfigResponse>> {
    let new_key = uuid::Uuid::new_v4().to_string();
    let result = sqlx::query("UPDATE config SET api_key = ? WHERE id = 1")
        .bind(&new_key)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("config not initialized".into()));
    }

    // Cookie sessions are derived from the master key — rotating
    // the key without revoking them would let a stolen cookie keep
    // working past the rotation it was supposed to invalidate. Drop
    // every session row so the user (and any other devices) re-paste
    // the new key on next request.
    if let Err(e) = crate::auth_session::revoke_all(&state.db).await {
        tracing::warn!(error = %e, "api-key rotate: failed to wipe sessions");
    }

    tracing::info!("api key rotated; all sessions revoked");
    let _ = state.event_tx.send(AppEvent::ConfigChanged {
        scope: ConfigScope::All,
    });

    // Rotate is the one `/config` endpoint that must return the
    // real (unmasked) API key — the UI needs it to re-authenticate
    // subsequent requests. Everywhere else `GET` / `PUT /config`
    // returns `***` so a cross-origin leak can't exfiltrate the
    // key via the normal read path.
    let mut resp = get_config(State(state)).await?;
    resp.0.api_key = new_key;
    Ok(resp)
}
