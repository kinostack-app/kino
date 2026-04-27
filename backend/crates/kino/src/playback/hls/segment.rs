//! `GET /api/v1/play/{kind}/{entity_id}/segments/{slug}` — serves a
//! single fMP4 segment (init or numbered) from the active HLS
//! transcode session. The slug comes from the variant playlist
//! rewrite; supports both initial-spawn and respawn-generation
//! filenames (`init`, `init_v{N}`, `NNN`, `v{N}_NNN`).

use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

use crate::error::{AppError, AppResult};
use crate::playback::PlayKind;
use crate::playback::handlers::{PlayTabParams, play_session_id};
use crate::playback::transcode;
use crate::state::AppState;

/// `GET /api/v1/play/{kind}/{entity_id}/segments/{index}`
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/segments/{index}",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        ("index" = String, Path),
        ("tab" = Option<String>, Query),
    ),
    responses(
        (status = 200, description = "fMP4 segment"),
        (status = 404, description = "Segment unavailable"),
    ),
    tag = "playback", security(("api_key" = []))
)]
pub async fn hls_segment(
    State(state): State<AppState>,
    Path((kind, entity_id, index)): Path<(PlayKind, i64, String)>,
    axum::extract::Query(params): axum::extract::Query<PlayTabParams>,
) -> AppResult<Response> {
    let transcode = state.require_transcode()?;
    let session_id = play_session_id(kind, entity_id, params.tab.as_deref());
    let temp_dir = transcode
        .session_temp_dir(&session_id)
        .await
        .ok_or_else(|| {
            tracing::warn!(%session_id, "play segment requested but no transcode session exists");
            AppError::NotFound("no active play transcode session".into())
        })?;

    let token = transcode::SegmentToken::parse(&index)
        .ok_or_else(|| AppError::BadRequest("invalid segment slug".into()))?;
    let segment_path = match token {
        transcode::SegmentToken::InitBase | transcode::SegmentToken::InitVersioned(_) => {
            temp_dir.join(token.filename())
        }
        transcode::SegmentToken::Numbered(_)
        | transcode::SegmentToken::VersionedNumbered { .. } => transcode
            .get_segment(&session_id, token)
            .await
            .map_err(|e| {
                tracing::warn!(
                    %session_id,
                    slug = %index,
                    error = %e,
                    "play segment fetch failed — ffmpeg likely died",
                );
                AppError::NotFound(format!("segment: {e}"))
            })?,
    };
    if !segment_path.exists() {
        return Err(AppError::NotFound("segment not found".into()));
    }
    let bytes = tokio::fs::read(&segment_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read segment: {e}")))?;
    tracing::debug!(
        %session_id,
        index = %index,
        bytes = bytes.len(),
        "served play segment",
    );

    // `no-store` — seek restarts reuse `segment_N.m4s` filenames so
    // the same URL serves different content across sessions.
    Ok((
        [(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"))],
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        bytes,
    )
        .into_response())
}
