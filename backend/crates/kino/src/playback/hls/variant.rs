//! `GET /api/v1/play/{kind}/{entity_id}/variant.m3u8` — serves the
//! variant playlist by reading ffmpeg's `playlist.m3u8` from the
//! session's temp dir and rewriting filename references to point at
//! Kino's own `/segments/{slug}` route. The rewrite handles both
//! initial-spawn (`init.mp4`, `segment_NNN.m4s`) and respawn-
//! generation (`init_v{N}.mp4`, `segment_v{N}_NNN.m4s`) names.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

use crate::error::{AppError, AppResult};
use crate::playback::PlayKind;
use crate::playback::handlers::{PlayTabParams, play_query_suffix, play_session_id};
use crate::playback::transcode;
use crate::state::AppState;

// ─── HLS variant ────────────────────────────────────────────────────

/// Rewrite `playlist.m3u8` so the client fetches segments through Kino's
/// segment route instead of opening raw filenames. Handles both the initial
/// generation (`init.mp4` / `segment_NNN.m4s`) and post-HW→SW-respawn
/// generations (`init_v{N}.mp4` / `segment_v{N}_NNN.m4s`) — without the
/// versioned-filename branch every post-respawn segment URL collapsed to
/// `/segments/0` because the old `unwrap_or(0)` parser fell through, so the
/// player just looped on segment 0 forever.
fn rewrite_variant_playlist(content: &str, kind: &str, entity_id: i64, tab_qs: &str) -> String {
    content
        .lines()
        .map(|line| {
            // `#EXT-X-MAP:URI="<init filename>"` — rewrite both bare
            // (`init.mp4`) and respawn-versioned (`init_v{N}.mp4`) forms.
            // Each generation has its own init segment so previously-
            // cached client URLs stay valid across the discontinuity.
            if let Some(uri) = line
                .strip_prefix("#EXT-X-MAP:URI=\"")
                .and_then(|s| s.strip_suffix('"'))
                && let Some(token) = transcode::segment_token_for_filename(uri)
            {
                let slug = transcode::segment_token_slug(token);
                return format!(
                    "#EXT-X-MAP:URI=\"/api/v1/play/{kind}/{entity_id}/segments/{slug}{tab_qs}\""
                );
            }
            // Data segment lines — bare `segment_NNN.m4s` from the initial
            // generation or `segment_v{N}_NNN.m4s` from a respawn.
            if line.starts_with("segment_")
                && let Some(token) = transcode::segment_token_for_filename(line)
            {
                let slug = transcode::segment_token_slug(token);
                return format!("/api/v1/play/{kind}/{entity_id}/segments/{slug}{tab_qs}");
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `GET /api/v1/play/{kind}/{entity_id}/variant.m3u8`
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/variant.m3u8",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        ("tab" = Option<String>, Query),
    ),
    responses(
        (status = 200, description = "Variant playlist", content_type = "application/vnd.apple.mpegurl"),
        (status = 404, description = "No active transcode session"),
    ),
    tag = "playback", security(("api_key" = []))
)]
pub async fn hls_variant(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
    axum::extract::Query(params): axum::extract::Query<PlayTabParams>,
) -> AppResult<Response> {
    let transcode = state.require_transcode()?;
    let session_id = play_session_id(kind, entity_id, params.tab.as_deref());
    let tab_qs = play_query_suffix(params.tab.as_deref(), params.cast_token.as_deref());
    let temp_dir = transcode
        .session_temp_dir(&session_id)
        .await
        .ok_or_else(|| {
            tracing::warn!(%session_id, "play variant requested but no transcode session exists");
            AppError::NotFound("no active play transcode session".into())
        })?;
    let playlist_path = temp_dir.join("playlist.m3u8");
    if !playlist_path.exists() {
        return Err(AppError::NotFound("playlist not ready yet".into()));
    }
    transcode.touch_session(&session_id).await;

    let content = tokio::fs::read_to_string(&playlist_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read playlist: {e}")))?;

    let rewritten = rewrite_variant_playlist(&content, kind.as_str(), entity_id, &tab_qs);

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.apple.mpegurl"),
        )],
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
        rewritten,
    )
        .into_response())
}

#[cfg(test)]
mod variant_rewrite_tests {
    use super::rewrite_variant_playlist;

    #[test]
    fn rewrites_initial_generation_segments() {
        let playlist = "\
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-TARGETDURATION:6
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:6.000000,
segment_000.m4s
#EXTINF:6.000000,
segment_001.m4s
";
        let out = rewrite_variant_playlist(playlist, "movie", 42, "");
        assert!(
            out.contains("#EXT-X-MAP:URI=\"/api/v1/play/movie/42/segments/init\""),
            "init mapped: {out}"
        );
        assert!(
            out.contains("/api/v1/play/movie/42/segments/0"),
            "first data segment: {out}"
        );
        assert!(
            out.contains("/api/v1/play/movie/42/segments/1"),
            "second data segment: {out}"
        );
        assert!(
            !out.contains("segment_000.m4s") && !out.contains("segment_001.m4s"),
            "raw filenames replaced: {out}"
        );
    }

    #[test]
    fn rewrites_post_respawn_versioned_segments() {
        // Bug #9: ffmpeg respawns after a HW→SW fallback and writes
        // `init_v1.mp4` + `segment_v1_000.m4s`. The old rewrite parsed the
        // segment lines with `unwrap_or(0)` so every URL collapsed to
        // `/segments/0` and the player looped on segment 0.
        let playlist = "\
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-TARGETDURATION:6
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:6.000000,
segment_000.m4s
#EXTINF:6.000000,
segment_001.m4s
#EXT-X-DISCONTINUITY
#EXT-X-MAP:URI=\"init_v1.mp4\"
#EXTINF:6.000000,
segment_v1_000.m4s
#EXTINF:6.000000,
segment_v1_001.m4s
";
        let out = rewrite_variant_playlist(playlist, "episode", 7, "");
        assert!(
            out.contains("#EXT-X-MAP:URI=\"/api/v1/play/episode/7/segments/init\""),
            "init: {out}"
        );
        assert!(
            out.contains("#EXT-X-MAP:URI=\"/api/v1/play/episode/7/segments/init_v1\""),
            "respawn init: {out}"
        );
        assert!(
            out.contains("/api/v1/play/episode/7/segments/v1_0"),
            "first respawn segment: {out}"
        );
        assert!(
            out.contains("/api/v1/play/episode/7/segments/v1_1"),
            "second respawn segment: {out}"
        );
        // The discontinuity tag must survive the rewrite — clients use it
        // to know they need to re-fetch the new init segment.
        assert!(out.contains("#EXT-X-DISCONTINUITY"), "discontinuity: {out}");
    }

    #[test]
    fn appends_tab_query_suffix_to_each_url() {
        let playlist = "\
#EXTM3U
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:6.000000,
segment_000.m4s
";
        let out = rewrite_variant_playlist(playlist, "movie", 5, "?tab=foo");
        assert!(
            out.contains("/segments/init?tab=foo\""),
            "tab on init: {out}"
        );
        assert!(out.contains("/segments/0?tab=foo"), "tab on segment: {out}");
    }

    #[test]
    fn carries_cast_token_through_segment_urls() {
        // Bug #35: the master playlist linked the variant URL with
        // only `?tab=`, the variant rewrote segments with only `?tab=`
        // — the cast token never reached subrequests, so the
        // receiver got 401 the moment it followed the master.
        let playlist = "\
#EXTM3U
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:6.000000,
segment_000.m4s
";
        let out =
            rewrite_variant_playlist(playlist, "movie", 5, "?tab=foo&cast_token=abc.def-_xyz");
        assert!(
            out.contains("/segments/init?tab=foo&cast_token=abc.def-_xyz"),
            "cast_token on init: {out}"
        );
        assert!(
            out.contains("/segments/0?tab=foo&cast_token=abc.def-_xyz"),
            "cast_token on data segment: {out}"
        );
    }
}
