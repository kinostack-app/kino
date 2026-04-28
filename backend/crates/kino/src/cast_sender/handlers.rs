//! HTTP handlers for `/api/v1/cast/*`.
//!
//! Phase 1 surface — minimum needed for browser-agnostic cast:
//!
//! - `GET    /api/v1/cast/devices`               — list devices
//! - `POST   /api/v1/cast/devices`               — manually add by IP
//! - `DELETE /api/v1/cast/devices/{id}`          — forget a device
//! - `POST   /api/v1/cast/sessions`              — start session
//! - `GET    /api/v1/cast/sessions/{id}`         — current session row
//! - `DELETE /api/v1/cast/sessions/{id}`         — stop the session
//! - `POST   /api/v1/cast/sessions/{id}/play`
//! - `POST   /api/v1/cast/sessions/{id}/pause`
//! - `POST   /api/v1/cast/sessions/{id}/seek`    body: { `position_ms` }
//!
//! Phase 2 (deferred): queue ops, custom message channel, volume,
//! resume-on-restart.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use super::device::{CastDevice, CastSession, CastSessionStatus};
use super::session::{SessionCommand, SessionConfig, SessionEvent};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Default Cast app id when none is configured. Google's stock
/// media receiver — works for vanilla MP4/HLS playback without a
/// registered receiver. The custom kino receiver overrides this
/// when `config.cast_receiver_app_id` is set.
const DEFAULT_RECEIVER_APP_ID: &str = "CC1AD845";

// ─── Devices ─────────────────────────────────────────────────────

/// `GET /api/v1/cast/devices` — every Chromecast we know about,
/// freshest mDNS sightings first then any manually-added rows.
#[utoipa::path(
    get, path = "/api/v1/cast/devices",
    responses((status = 200, body = Vec<CastDevice>)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn list_devices(State(state): State<AppState>) -> AppResult<Json<Vec<CastDevice>>> {
    // Filter out audio-only Cast targets (Google Home / Nest Mini /
    // Chromecast Audio). They announce on the same `_googlecast._tcp.`
    // service type as video Chromecasts but their `ca` capabilities
    // bitmask has bit 0 (video output) cleared. Manual rows survive
    // the filter — the user added them by IP on purpose, and we
    // don't probe TXT records for those. NULL capabilities also
    // survive (older firmwares; better to over-show than hide a
    // working TV).
    let rows = sqlx::query_as::<_, CastDevice>(
        "SELECT * FROM cast_device
         WHERE source = 'manual'
            OR capabilities IS NULL
            OR (capabilities & 1) != 0
         ORDER BY last_seen DESC NULLS LAST, name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddDeviceRequest {
    pub ip: String,
    /// Optional — defaults to the Cast control port (8009).
    pub port: Option<u16>,
    /// Optional — defaults to the IP if absent.
    pub name: Option<String>,
}

/// `POST /api/v1/cast/devices` — manually add a Chromecast by IP.
/// Useful when mDNS is blocked (Docker bridge, corporate Wi-Fi,
/// AP-isolated guest networks).
#[utoipa::path(
    post, path = "/api/v1/cast/devices",
    request_body = AddDeviceRequest,
    responses(
        (status = 201, body = CastDevice),
        (status = 400, description = "Invalid IP")
    ),
    tag = "cast", security(("api_key" = []))
)]
pub async fn add_device(
    State(state): State<AppState>,
    Json(body): Json<AddDeviceRequest>,
) -> AppResult<(StatusCode, Json<CastDevice>)> {
    let port = body.port.unwrap_or(8009);
    body.ip
        .parse::<std::net::IpAddr>()
        .map_err(|_| AppError::BadRequest(format!("invalid IP: {}", body.ip)))?;
    let id = format!("manual:{}", Uuid::new_v4());
    let name = body.name.unwrap_or_else(|| body.ip.clone());
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO cast_device
            (id, name, ip, port, source, created_at)
         VALUES (?, ?, ?, ?, 'manual', ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(&body.ip)
    .bind(i64::from(port))
    .bind(&now)
    .execute(&state.db)
    .await?;
    let row = sqlx::query_as::<_, CastDevice>("SELECT * FROM cast_device WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `DELETE /api/v1/cast/devices/{id}` — forget a manually-added
/// device. mDNS-sourced rows are managed by discovery and ignore
/// the delete.
#[utoipa::path(
    delete, path = "/api/v1/cast/devices/{id}",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn delete_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let res = sqlx::query("DELETE FROM cast_device WHERE id = ? AND source = 'manual'")
        .bind(&id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "manual cast device {id} not found"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ─── Sessions ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct StartSessionRequest {
    pub device_id: String,
    pub media_id: i64,
    /// Resume position in milliseconds. When present, the receiver
    /// seeks here on initial LOAD instead of starting from 0 — used
    /// by the in-app player's "snap to current playhead on cast"
    /// handoff so the TV picks up where the laptop left off.
    pub start_position_ms: Option<i64>,
}

/// `POST /api/v1/cast/sessions` — launch the receiver app on a
/// device and load the requested media. Returns immediately with
/// the new session row; status updates flow over WebSocket as the
/// session progresses.
#[utoipa::path(
    post, path = "/api/v1/cast/sessions",
    request_body = StartSessionRequest,
    responses(
        (status = 201, body = CastSession),
        (status = 404, description = "Device or media not found")
    ),
    tag = "cast", security(("api_key" = []))
)]
pub async fn start_session(
    State(state): State<AppState>,
    Json(body): Json<StartSessionRequest>,
) -> AppResult<(StatusCode, Json<CastSession>)> {
    let device = sqlx::query_as::<_, CastDevice>("SELECT * FROM cast_device WHERE id = ?")
        .bind(&body.device_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("cast device {} not found", body.device_id)))?;

    let token_reply = crate::playback::cast::cast_token_for_media(&state, body.media_id).await?;
    let app_id = configured_receiver_app_id(&state.db).await;

    // Build the absolute URL the receiver will fetch. cast_token_for_media
    // returns a path; the receiver needs an absolute URL it can fetch
    // over the LAN (Chromecasts can't resolve `kino.local` reliably,
    // so we use the host's reachable IP.)
    let base = receiver_facing_base_url(&state).await;
    let content_url = format!("{base}{}", token_reply.stream_url);

    let session_id = super::session::CastSessionManager::new_session_id();
    let now = Utc::now().to_rfc3339();
    let media_title = sqlx::query_scalar::<_, Option<String>>(
        "SELECT m.movie_id IS NOT NULL FROM media m WHERE m.id = ?",
    )
    .bind(body.media_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .flatten();
    let _ = media_title;

    sqlx::query(
        "INSERT INTO cast_session
            (id, device_id, media_id, status, started_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&session_id)
    .bind(&device.id)
    .bind(body.media_id)
    .bind(CastSessionStatus::Starting.as_str())
    .bind(&now)
    .execute(&state.db)
    .await?;

    let device_port_u16 = u16::try_from(device.port).unwrap_or(8009);
    let (returned_id, mut event_rx) = state
        .cast_sessions
        .spawn(SessionConfig {
            session_id: session_id.clone(),
            device_host: device.ip.clone(),
            device_port: device_port_u16,
            app_id,
            content_url,
            content_type: token_reply.content_type.clone(),
            title: format!("Media {}", body.media_id),
            subtitle: None,
            poster_url: None,
            #[allow(
                clippy::cast_precision_loss,
                reason = "ms→sec for Cast API; tens-of-hours runtime well within f64"
            )]
            start_position_sec: body.start_position_ms.map(|ms| ms as f64 / 1000.0),
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    debug_assert_eq!(returned_id, session_id);

    // Drain events from the worker thread + apply them to DB +
    // WebSocket. Lives for the session's lifetime; exits when the
    // worker drops its event sender.
    let pool = state.db.clone();
    let event_tx = state.event_tx.clone();
    let manager = state.cast_sessions.clone();
    let session_id_for_task = session_id.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            apply_event(&pool, &event_tx, &session_id_for_task, &event).await;
        }
        manager.forget(&session_id_for_task).await;
    });

    let row = sqlx::query_as::<_, CastSession>("SELECT * FROM cast_session WHERE id = ?")
        .bind(&session_id)
        .fetch_one(&state.db)
        .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `GET /api/v1/cast/sessions/{id}` — current row + last status JSON.
#[utoipa::path(
    get, path = "/api/v1/cast/sessions/{id}",
    params(("id" = String, Path)),
    responses((status = 200, body = CastSession), (status = 404)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<CastSession>> {
    let row = sqlx::query_as::<_, CastSession>("SELECT * FROM cast_session WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("cast session {id} not found")))?;
    Ok(Json(row))
}

/// `DELETE /api/v1/cast/sessions/{id}` — stop the receiver app and
/// tear the worker thread down. Idempotent.
#[utoipa::path(
    delete, path = "/api/v1/cast/sessions/{id}",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn stop_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cast_session WHERE id = ? AND status != 'ended')",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(AppError::NotFound(format!(
            "active cast session {id} not found"
        )));
    }
    state
        .cast_sessions
        .send(&id, SessionCommand::Shutdown)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/v1/cast/sessions/{id}/play",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn play(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<StatusCode> {
    if !state.cast_sessions.send(&id, SessionCommand::Play).await {
        return Err(AppError::NotFound(format!("cast session {id} not active")));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/v1/cast/sessions/{id}/pause",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn pause(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<StatusCode> {
    if !state.cast_sessions.send(&id, SessionCommand::Pause).await {
        return Err(AppError::NotFound(format!("cast session {id} not active")));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SeekRequest {
    pub position_ms: i64,
}

#[utoipa::path(
    post, path = "/api/v1/cast/sessions/{id}/seek",
    params(("id" = String, Path)),
    request_body = SeekRequest,
    responses((status = 204), (status = 404), (status = 400)),
    tag = "cast", security(("api_key" = []))
)]
pub async fn seek(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SeekRequest>,
) -> AppResult<StatusCode> {
    if body.position_ms < 0 {
        return Err(AppError::BadRequest("position_ms must be ≥ 0".into()));
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "ms → sec for Cast API; tens-of-hours runtimes still well within f64 mantissa"
    )]
    let secs = body.position_ms as f64 / 1000.0;
    if !state
        .cast_sessions
        .send(&id, SessionCommand::Seek(secs))
        .await
    {
        return Err(AppError::NotFound(format!("cast session {id} not active")));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ─── Event handling ──────────────────────────────────────────────

async fn apply_event(
    pool: &sqlx::SqlitePool,
    event_tx: &tokio::sync::broadcast::Sender<crate::events::AppEvent>,
    session_id: &str,
    event: &SessionEvent,
) {
    match event {
        SessionEvent::Launched {
            transport_id,
            session_id: cast_session_id,
        } => {
            let _ = sqlx::query(
                "UPDATE cast_session SET transport_id = ?, session_id = ?, status = 'active'
                 WHERE id = ?",
            )
            .bind(transport_id)
            .bind(cast_session_id)
            .bind(session_id)
            .execute(pool)
            .await;
            let _ = event_tx.send(crate::events::AppEvent::CastStatus {
                session_id: session_id.to_owned(),
                position_ms: None,
                status_json: serde_json::json!({"player_state": "STARTING"}).to_string(),
            });
        }
        SessionEvent::Status { position_ms, json } => {
            let _ = sqlx::query(
                "UPDATE cast_session SET last_status_json = ?, last_position_ms = ?
                 WHERE id = ?",
            )
            .bind(json)
            .bind(*position_ms)
            .bind(session_id)
            .execute(pool)
            .await;
            let _ = event_tx.send(crate::events::AppEvent::CastStatus {
                session_id: session_id.to_owned(),
                position_ms: *position_ms,
                status_json: json.clone(),
            });
        }
        SessionEvent::Reconnecting { .. } | SessionEvent::Warning(_) => {
            // No DB write — these are transient. WS broadcast only.
        }
        SessionEvent::Ended { reason } => {
            let now = Utc::now().to_rfc3339();
            let final_status = match reason {
                super::session::SessionEndReason::Stopped => CastSessionStatus::Ended,
                super::session::SessionEndReason::Failed(_) => CastSessionStatus::Errored,
            };
            let _ = sqlx::query("UPDATE cast_session SET status = ?, ended_at = ? WHERE id = ?")
                .bind(final_status.as_str())
                .bind(&now)
                .bind(session_id)
                .execute(pool)
                .await;
            let reason_str = match reason {
                super::session::SessionEndReason::Stopped => "stopped".to_owned(),
                super::session::SessionEndReason::Failed(s) => format!("failed: {s}"),
            };
            let _ = event_tx.send(crate::events::AppEvent::CastSessionEnded {
                session_id: session_id.to_owned(),
                reason: reason_str,
            });
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────

async fn configured_receiver_app_id(pool: &sqlx::SqlitePool) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT cast_receiver_app_id FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RECEIVER_APP_ID.to_owned())
}

/// Build the base URL the Chromecast will use to fetch HLS / direct
/// content. The receiver runs on the LAN, so it needs an IP-form URL
/// for kino's HTTP server, not `localhost` or `kino.local`. We try
/// the configured `base_url` first; otherwise fall back to the
/// machine's primary LAN IPv4.
async fn receiver_facing_base_url(state: &AppState) -> String {
    let configured: Option<String> = sqlx::query_scalar("SELECT base_url FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if let Some(url) = configured.filter(|s| !s.trim().is_empty()) {
        return url.trim_end_matches('/').to_owned();
    }
    // Fallback: actual bound port (NOT `config.listen_port`, which
    // may diverge if the 80→8080 runtime fallback fired) + the
    // first real LAN IPv4 (skipping docker bridges, virtual
    // interfaces, etc.). Without the virtual-interface filter, the
    // Chromecast can be handed a 172.x.x.x docker bridge address
    // that it can't possibly reach from the TV.
    let port = state.http_port;
    let (lan_ips, _virt) = crate::mdns::lan_ipv4s_with_virtual_filtered();
    let ip = lan_ips
        .first()
        .map_or_else(|| "127.0.0.1".to_owned(), std::string::ToString::to_string);
    if port == 80 {
        format!("http://{ip}")
    } else {
        format!("http://{ip}:{port}")
    }
}

/// Internal view used by the ws frame test. Re-exported for the
/// `FromRow` derive on cross-module integration tests when they land.
#[derive(Debug, FromRow, Serialize)]
pub struct InternalRow(pub i64);
