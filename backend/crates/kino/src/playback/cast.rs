//! Cast-token issuance + stream-URL resolution.
//!
//! The Cast SDK on the frontend calls `POST /api/v1/playback/cast-token`
//! before handing a URL to the TV. The response carries both the
//! token and a ready-to-use `stream_url` so the client doesn't have
//! to decide between direct-play and HLS; the backend picks for
//! it (forcing HLS for containers Chromecast can't demux natively,
//! notably MKV).

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::playback::cast_token;
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CastTokenRequest {
    pub media_id: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CastTokenReply {
    pub token: String,
    pub expires_at: i64,
    /// Absolute URL the Cast receiver should fetch. Already carries
    /// the `cast_token` query param — callers can hand it directly
    /// to `chrome.cast.media.MediaInfo`.
    pub stream_url: String,
    pub content_type: String,
}

/// `POST /api/v1/playback/cast-token` — issues a short-lived HMAC
/// token for a specific media file and returns the Cast-safe
/// stream URL to load on the receiver.
///
/// The returned URL targets the live `/api/v1/play/{kind}/{entity_id}/…`
/// routes, resolving `media_id` to its `(kind, entity_id)` pair via
/// `media` (for movies) or `media_episode` (for TV). The token's
/// subject is the play-route pair (e.g. `movie/42`) so the
/// receiver can fetch `/master.m3u8`, variant playlists, segments,
/// subtitles, trickplay, and `/progress` under that prefix — and
/// only that prefix — without the API key. The auth middleware
/// (`auth::require_api_key`) verifies the `?cast_token=` query
/// param against the URL path on every request.
#[utoipa::path(
    post, path = "/api/v1/playback/cast-token",
    request_body = CastTokenRequest,
    responses((status = 200, body = CastTokenReply), (status = 404)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn issue_cast_token(
    State(state): State<AppState>,
    Json(body): Json<CastTokenRequest>,
) -> AppResult<Json<CastTokenReply>> {
    cast_token_for_media(&state, body.media_id).await.map(Json)
}

/// Pure helper used by both the HTTP handler above and the
/// server-side Cast sender (subsystem 32). Resolves a media id to
/// a Cast-safe stream URL + HMAC token.
pub async fn cast_token_for_media(state: &AppState, media_id: i64) -> AppResult<CastTokenReply> {
    cast_token_inner(state, media_id).await
}

async fn cast_token_inner(state: &AppState, media_id: i64) -> AppResult<CastTokenReply> {
    // Resolve media_id → (kind, entity_id) in one join. A media row
    // either links to a movie directly via `media.movie_id`, or to
    // one-or-more episodes via `media_episode(media_id, episode_id)`.
    // Multi-episode files pick the lowest episode_id: a multi-ep
    // remux casts as the first episode, and the user can refine
    // by casting from the episode detail page directly (which
    // bypasses this endpoint's media-id input and issues a token
    // for the specific episode they picked).
    #[derive(sqlx::FromRow)]
    struct Row {
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        file_path: String,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT m.movie_id,
                (SELECT episode_id FROM media_episode WHERE media_id = m.id ORDER BY episode_id LIMIT 1) AS episode_id,
                m.file_path
         FROM media m
         WHERE m.id = ?",
    )
    .bind(media_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("media {media_id} not found")))?;

    let (kind, entity_id) = match (row.movie_id, row.episode_id) {
        (Some(mid), _) => ("movie", mid),
        (None, Some(eid)) => ("episode", eid),
        (None, None) => {
            return Err(AppError::NotFound(format!(
                "media {media_id} has no linked movie or episode"
            )));
        }
    };

    let api_key: Option<String> = sqlx::query_scalar("SELECT api_key FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let api_key =
        api_key.ok_or_else(|| AppError::Internal(anyhow::anyhow!("api_key not configured")))?;
    let secret = cast_token::derive_secret(&api_key);

    let (token, expires_at) = cast_token::issue(kind, entity_id, &secret)
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Chromecast default receiver handles MP4 and HLS fMP4 fine. MKV
    // direct-plays in-browser but not on Chromecast, so force HLS
    // for anything that isn't mp4/m4v regardless of the
    // direct-play eligibility computed for the browser. Full
    // decision-engine integration (per-client codec probe, remux
    // path, HDR fallback) lands with the rest of #05.
    let ext = std::path::Path::new(&row.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let cast_safe_direct = matches!(ext.as_str(), "mp4" | "m4v");

    // URL carries ONLY the cast token — no raw API key ever
    // reaches the Chromecast. The auth middleware accepts the
    // token in lieu of an API key exclusively on
    // `/api/v1/play/{kind}/{entity_id}/*` routes, and only when
    // the token's subject matches the URL path.
    let (stream_url, content_type) = if cast_safe_direct {
        (
            format!("/api/v1/play/{kind}/{entity_id}/direct?cast_token={token}"),
            "video/mp4".to_owned(),
        )
    } else {
        (
            format!("/api/v1/play/{kind}/{entity_id}/master.m3u8?cast_token={token}"),
            "application/vnd.apple.mpegurl".to_owned(),
        )
    };

    tracing::info!(
        media_id = media_id,
        kind,
        entity_id,
        cast_safe_direct,
        "issued cast token"
    );

    Ok(CastTokenReply {
        token,
        expires_at,
        stream_url,
        content_type,
    })
}
