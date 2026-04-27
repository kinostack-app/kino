//! Playback settings probe + stats endpoints. The UI uses these
//! to verify ffmpeg is wired up, pick a hardware acceleration
//! that actually exists on this machine, and watch live
//! transcode sessions.
//!
//! The probe logic + typed `HwCapabilities` shape live in
//! `playback::hw_probe`; this module only handles HTTP framing,
//! live-session snapshots, and the `/test-transcode` convenience
//! endpoint the settings page exercises.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::AppResult;
use crate::playback::HwCapabilities;
use crate::state::AppState;

/// `POST /api/v1/playback/probe` — runs the configured ffmpeg
/// binary to confirm it exists, report its version, and run a
/// trial encode through every hardware backend. The response is
/// the same typed `HwCapabilities` shape the status banner +
/// (future) profile chain consume from the cache.
#[utoipa::path(
    post, path = "/api/v1/playback/probe",
    responses((status = 200, body = HwCapabilities)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn probe(State(state): State<AppState>) -> AppResult<Json<HwCapabilities>> {
    let ffmpeg: String = sqlx::query_scalar("SELECT ffmpeg_path FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?
        .flatten()
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "ffmpeg".to_string());

    let caps = crate::playback::hw_probe::run_probe(&ffmpeg).await;
    // Refresh the process-wide cache so the status-banner check
    // picks up any changes (user swapped ffmpeg path, plugged in
    // a GPU, etc.).
    crate::playback::hw_probe_cache::set_cached(caps.clone());
    Ok(Json(caps))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TranscodeStats {
    pub active_sessions: i64,
    pub max_concurrent: i64,
    /// Whether transcoding is enabled overall. Lets the UI show
    /// a "disabled" state on the live card without a second query.
    pub enabled: bool,
}

/// `GET /api/v1/playback/transcode-stats` — how many transcode
/// sessions are currently running. Cheap; used by the settings card.
#[utoipa::path(
    get, path = "/api/v1/playback/transcode-stats",
    responses((status = 200, body = TranscodeStats)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn transcode_stats(State(state): State<AppState>) -> AppResult<Json<TranscodeStats>> {
    let active = match &state.transcode {
        Some(t) => t.active_session_count().await,
        None => 0,
    };
    let row: Option<(i64, bool)> = sqlx::query_as(
        "SELECT max_concurrent_transcodes, transcoding_enabled FROM config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let (max_concurrent, enabled) = row.unwrap_or((2, true));
    Ok(Json(TranscodeStats {
        active_sessions: i64::try_from(active).unwrap_or(i64::MAX),
        max_concurrent,
        enabled,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionInfo {
    pub session_id: String,
    pub media_id: i64,
    pub title: Option<String>,
    pub started_at_secs_ago: i64,
    pub idle_secs: i64,
    pub state: crate::playback::TranscodeSessionState,
}

/// `GET /api/v1/playback/transcode-sessions` — one row per live
/// session, with a title looked up from the media table so the UI
/// can show what's actually playing.
#[utoipa::path(
    get, path = "/api/v1/playback/transcode-sessions",
    responses((status = 200, body = Vec<SessionInfo>)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn transcode_sessions(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<SessionInfo>>> {
    let snapshots = match &state.transcode {
        Some(t) => t.list_sessions().await,
        None => return Ok(Json(Vec::new())),
    };

    // Look up titles for every media_id we have a session for in
    // one query; join movie + episode so we pick up both. Falls
    // back to the media_id number when a title isn't available.
    let mut media_ids: Vec<i64> = snapshots.iter().map(|s| s.media_id).collect();
    media_ids.sort_unstable();
    media_ids.dedup();

    let mut titles: HashMap<i64, String> = HashMap::new();
    if !media_ids.is_empty() {
        let placeholders = media_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        // Movie title via media → movie.
        let movie_sql = format!(
            "SELECT m.id, mo.title FROM media m \
             JOIN movie mo ON mo.id = m.movie_id \
             WHERE m.id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, String)>(&movie_sql);
        for id in &media_ids {
            q = q.bind(id);
        }
        for (id, title) in q.fetch_all(&state.db).await.unwrap_or_default() {
            titles.insert(id, title);
        }

        // Episode title via media_episode → episode → show. We
        // compose "Show — SxxEyy · Episode Title" for clarity on
        // a shared list.
        let ep_sql = format!(
            "SELECT m.id, s.title, e.season_number, e.episode_number, e.title \
             FROM media m \
             JOIN media_episode me ON me.media_id = m.id \
             JOIN episode e ON e.id = me.episode_id \
             JOIN show s ON s.id = e.show_id \
             WHERE m.id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, String, i64, i64, String)>(&ep_sql);
        for id in &media_ids {
            q = q.bind(id);
        }
        for (id, show, season, ep, ep_title) in q.fetch_all(&state.db).await.unwrap_or_default() {
            titles
                .entry(id)
                .or_insert_with(|| format!("{show} — S{season:02}E{ep:02} · {ep_title}"));
        }
    }

    let out: Vec<SessionInfo> = snapshots
        .into_iter()
        .map(|s| SessionInfo {
            title: titles.get(&s.media_id).cloned(),
            session_id: s.session_id,
            media_id: s.media_id,
            started_at_secs_ago: i64::try_from(s.started_at_secs_ago).unwrap_or(i64::MAX),
            idle_secs: i64::try_from(s.idle_secs).unwrap_or(i64::MAX),
            state: s.state,
        })
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestTranscodeResult {
    pub ok: bool,
    pub message: String,
    pub duration_ms: i64,
    pub hw_acceleration: String,
    /// Last lines of ffmpeg stderr on failure — same tail the
    /// logs viewer surfaces so the user can diagnose driver /
    /// codec issues.
    pub stderr_tail: Option<String>,
}

/// `DELETE /api/v1/playback/transcode-sessions/{session_id}` —
/// stop one session by its raw id. Used by the settings page's
/// session list, which shows a mix of `play-*` and legacy
/// `transcode-*` session ids and needs an entity-agnostic kill.
/// The `/play/.../transcode` DELETE is entity-keyed and doesn't
/// fit this UI.
#[utoipa::path(
    delete, path = "/api/v1/playback/transcode-sessions/{session_id}",
    params(("session_id" = String, Path)),
    responses((status = 204)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn stop_transcode_session(
    State(state): State<AppState>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> AppResult<axum::http::StatusCode> {
    if let Some(ref transcode) = state.transcode
        && let Err(e) = transcode.stop_session(&session_id).await
    {
        tracing::warn!(%session_id, error = %e, "failed to stop transcode session");
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /api/v1/playback/test-transcode` — actually runs ffmpeg
/// to transcode a synthetic 2s test source using the
/// currently-configured hardware acceleration. Catches broken
/// drivers / missing codec libs that the lighter `probe`
/// endpoint doesn't exercise for the configured backend.
#[utoipa::path(
    post, path = "/api/v1/playback/test-transcode",
    responses((status = 200, body = TestTranscodeResult)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn test_transcode(State(state): State<AppState>) -> AppResult<Json<TestTranscodeResult>> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT ffmpeg_path, hw_acceleration FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await?;
    let (ffmpeg, accel) = match row {
        Some((ff, hw)) => (
            ff.filter(|s| !s.is_empty())
                .unwrap_or_else(|| "ffmpeg".into()),
            hw.unwrap_or_else(|| "none".into()),
        ),
        None => ("ffmpeg".into(), "none".into()),
    };

    // Build a self-contained ffmpeg invocation:
    //   1. Generate 2s of test video via lavfi — no source file needed.
    //   2. Encode with the configured hw accel (or libx264 for "none").
    //   3. Write to /dev/null via -f null so we don't touch disk.
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostats".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        "testsrc=duration=2:size=640x360:rate=30".into(),
    ];
    match accel.as_str() {
        "vaapi" => {
            args.extend(
                [
                    "-vf",
                    "format=nv12,hwupload",
                    "-vaapi_device",
                    "/dev/dri/renderD128",
                    "-c:v",
                    "h264_vaapi",
                ]
                .map(String::from),
            );
        }
        "nvenc" => args.extend(["-c:v", "h264_nvenc"].map(String::from)),
        "qsv" => args.extend(["-c:v", "h264_qsv"].map(String::from)),
        "videotoolbox" => args.extend(["-c:v", "h264_videotoolbox"].map(String::from)),
        "amf" => args.extend(["-c:v", "h264_amf"].map(String::from)),
        _ => args.extend(["-c:v", "libx264", "-preset", "ultrafast"].map(String::from)),
    }
    args.extend(["-t", "2", "-f", "null", "-"].map(String::from));

    let start = std::time::Instant::now();
    let output = tokio::process::Command::new(&ffmpeg)
        .args(&args)
        .output()
        .await;
    let duration_ms = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);

    match output {
        Ok(out) if out.status.success() => {
            tracing::info!(hw = %accel, duration_ms, "test transcode ok");
            Ok(Json(TestTranscodeResult {
                ok: true,
                message: format!("Transcoded 2s clip via {accel} in {duration_ms} ms"),
                duration_ms,
                hw_acceleration: accel,
                stderr_tail: None,
            }))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            tracing::warn!(hw = %accel, status = %out.status, "test transcode failed");
            Ok(Json(TestTranscodeResult {
                ok: false,
                message: format!("Transcode via {accel} failed (exit {})", out.status),
                duration_ms,
                hw_acceleration: accel,
                stderr_tail: Some(
                    stderr
                        .lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .iter()
                        .rev()
                        .copied()
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            }))
        }
        Err(e) => Ok(Json(TestTranscodeResult {
            ok: false,
            message: format!("couldn't run ffmpeg: {e}"),
            duration_ms,
            hw_acceleration: accel,
            stderr_tail: None,
        })),
    }
}

// ─── FFmpeg bundle download / revert ──────────────────────────────

use crate::playback::ffmpeg_bundle::{self, FfmpegBundleError, FfmpegDownloadState};

/// `POST /api/v1/playback/ffmpeg/download` — starts a download
/// of the pinned jellyfin-ffmpeg build into `{data_path}/bin/`.
/// Returns 202 Accepted with the current state when a new
/// download starts; 409 Conflict when one is already running;
/// 400 when the host platform isn't in the pinned table.
#[utoipa::path(
    post, path = "/api/v1/playback/ffmpeg/download",
    responses(
        (status = 202, description = "Download started", body = FfmpegDownloadState),
        (status = 409, description = "A download is already in progress"),
        (status = 400, description = "Platform not supported"),
    ),
    tag = "playback", security(("api_key" = []))
)]
pub async fn start_ffmpeg_download(
    State(state): State<AppState>,
) -> AppResult<(axum::http::StatusCode, Json<FfmpegDownloadState>)> {
    match ffmpeg_bundle::start_download(
        state.ffmpeg_download.clone(),
        state.data_path.clone(),
        state.event_tx.clone(),
        state.db.clone(),
        state.transcode.clone(),
    )
    .await
    {
        Ok(()) => {
            let snapshot = state.ffmpeg_download.snapshot().await;
            Ok((axum::http::StatusCode::ACCEPTED, Json(snapshot)))
        }
        Err(FfmpegBundleError::AlreadyRunning) => Err(crate::error::AppError::Conflict(
            "an ffmpeg download is already in progress".into(),
        )),
        Err(FfmpegBundleError::UnsupportedPlatform { os, arch }) => {
            Err(crate::error::AppError::BadRequest(format!(
                "bundled ffmpeg is not available for this platform ({os} / {arch})"
            )))
        }
        Err(e) => Err(crate::error::AppError::Internal(anyhow::anyhow!(
            "ffmpeg download failed to start: {e}"
        ))),
    }
}

/// `GET /api/v1/playback/ffmpeg/download` — returns the current
/// download state. Used by late-joining clients (e.g., a browser
/// refreshed mid-download) to reconstruct the progress bar
/// without waiting for the next broadcast tick.
#[utoipa::path(
    get, path = "/api/v1/playback/ffmpeg/download",
    responses((status = 200, body = FfmpegDownloadState)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn get_ffmpeg_download(State(state): State<AppState>) -> Json<FfmpegDownloadState> {
    Json(state.ffmpeg_download.snapshot().await)
}

/// `DELETE /api/v1/playback/ffmpeg/download` — cancel an
/// in-flight download. Idempotent: safe to call when no
/// download is running (no-op). The task will observe the
/// cancellation token at its next chunk boundary and move to
/// the `Failed { reason: "cancelled" }` state.
#[utoipa::path(
    delete, path = "/api/v1/playback/ffmpeg/download",
    responses((status = 204, description = "Cancellation requested")),
    tag = "playback", security(("api_key" = []))
)]
pub async fn cancel_ffmpeg_download(State(state): State<AppState>) -> axum::http::StatusCode {
    state.ffmpeg_download.cancel().await;
    axum::http::StatusCode::NO_CONTENT
}

/// `POST /api/v1/playback/ffmpeg/revert` — remove the bundled
/// ffmpeg and revert to the system ffmpeg. Clears
/// `config.ffmpeg_path` and deletes `{data_path}/bin/`.
/// Idempotent: safe when no bundle is installed.
#[utoipa::path(
    post, path = "/api/v1/playback/ffmpeg/revert",
    responses((status = 204, description = "Reverted to system ffmpeg")),
    tag = "playback", security(("api_key" = []))
)]
pub async fn revert_ffmpeg_to_system(
    State(state): State<AppState>,
) -> AppResult<axum::http::StatusCode> {
    ffmpeg_bundle::revert_to_system(&state.data_path, &state.db, state.transcode.as_ref())
        .await
        .map_err(|e| crate::error::AppError::Internal(anyhow::anyhow!("revert failed: {e}")))?;
    // Idle the tracker so the settings panel doesn't show a
    // stale "Completed" state after revert.
    state.ffmpeg_download.set_idle().await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
