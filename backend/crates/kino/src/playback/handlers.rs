//! Unified play API — one URL per entity, backend dispatches to the
//! right byte source (library file OR in-progress torrent) per request.
//!
//! The old `stream_*` (download-id-keyed) and `playback_*` (media-id-
//! keyed) endpoints live in parallel until frontend migrates off them.
//! This module is the canonical shape for everything new.
//!
//! Architecture notes:
//!
//! - **Per-request dispatch.** `resolve_byte_source` runs on every
//!   request. An in-flight request keeps reading from whichever source
//!   was picked at open time (axum streams the body body through the
//!   original connection); the NEXT request gets a fresh resolve.
//!   That's how the stream→library transition ends up invisible
//!   to the client — nothing is swapped mid-read, only between reads.
//!
//! - **Library always wins when available.** If an imported `media`
//!   row exists with a readable file on disk, we serve from there
//!   regardless of any lingering download. Library bytes are
//!   authoritative; the torrent's hardlinked copy is identical.
//!
//! - **HLS session key uses the entity identity** (`play-{kind}-{id}-
//!   {tab}`). That means a session started during streaming doesn't
//!   get keyed on `download_id` — post-import the SAME session id stays
//!   valid, so no restart is needed. ffmpeg's already-open input
//!   connection keeps reading from librqbit's file stream until the
//!   session naturally ends.

use std::io::SeekFrom;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf};
use tokio_util::io::ReaderStream;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use utoipa::ToSchema;

use crate::download::DownloadPhase;
use crate::error::{AppError, AppResult};
use crate::playback::progress;
use crate::playback::source::{
    ByteSource, ResolveError, resolve_byte_source, resolve_error_to_app_error,
};
use crate::state::AppState;

pub use crate::playback::PlayKind;

/// Unified state reported by `/prepare`. Mirrors the five user-
/// facing states the frontend chip + overlays discriminate on, plus
/// enough detail to drive them without a second round-trip.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlayState {
    /// Initial watch-now: indexer search is still running.
    Searching,
    /// Release picked, queued for `download_monitor` to pick up.
    Queued,
    /// Magnet handed to librqbit, still resolving info-dict.
    Grabbing,
    /// Bytes are flowing — play is live. `downloaded` + `speed`
    /// populated. Covers `downloading` *and* `seeding` pre-import.
    Streaming,
    /// Download paused by user from the downloads tab.
    Paused,
    /// Download terminally failed. `error_message` describes why.
    Failed,
    /// Imported media file on disk — library-native playback.
    Downloaded,
}

/// Unified `/prepare` response. Everything the player chrome needs
/// to render in one payload; the frontend never has to branch on
/// "which identity does this page have."
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PlayPrepareReply {
    pub kind: PlayKind,
    pub entity_id: i64,

    pub state: PlayState,

    /// Free-text title for the loading shell. Movie title or show
    /// title (for an episode).
    pub title: String,
    /// Episodes only: "S01E04 · Pilot" — the `SxxExx` + episode title
    /// shown as a second line under `title`. `None` for movies. Keeps
    /// the document title / header able to render the full "Show ·
    /// S01E04 · Pilot" line without a separate backend call.
    pub episode_label: Option<String>,

    // ── Loading-shell identity ──
    pub backdrop_path: Option<String>,
    pub logo_content_type: Option<String>,
    pub logo_entity_id: Option<i64>,
    pub logo_palette: Option<String>,

    // ── Playback descriptors (when bytes are available) ──
    /// Container extension (`mkv`, `mp4`, ...) when resolved.
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// What we concluded about the client from its `User-Agent`
    /// (or `?target=` override). Drives the capability profile
    /// the decision engine uses. Surfaced on the reply so the
    /// info chip can show "Detected: Firefox on Linux · profile
    /// firefox" — lets users verify the engine is planning for
    /// the right codec matrix.
    pub detected_client: Option<crate::playback::DetectedClient>,
    /// Full video-track details (resolution, HDR, bitrate, bit
    /// depth, colour space). `None` for streaming sources — the
    /// torrent's streams aren't probed until import. Populated
    /// for imported media even when it has no audio.
    pub video: Option<crate::playback::VideoTrackInfo>,
    /// Total file size in bytes for library files; `None` for
    /// streaming (use `total_bytes` for the download size instead).
    pub file_size_bytes: Option<i64>,
    /// File-level bitrate in bits/second — derived from
    /// `file_size_bytes * 8 / duration_secs` when both are known.
    /// Coarse but honest; ffmpeg's `format.bit_rate` lands in a
    /// later pass.
    pub total_bitrate_bps: Option<i64>,
    /// Configured hardware-acceleration backend at prepare time
    /// (`"nvenc"`, `"vaapi"`, `"qsv"`, `"videotoolbox"`, `"amf"`).
    /// `None` when software-only — not a warning, just the
    /// current setting. The player chip renders this in the
    /// Playback section so users can tell whether the session
    /// they're watching is CPU- or GPU-encoded.
    pub hw_backend: Option<String>,
    /// Live progress of an active transcode session for this
    /// entity, if one exists. `None` when Direct Play / Remux,
    /// or when the session hasn't produced a complete progress
    /// block yet (first second or two).
    pub live_progress: Option<crate::playback::TranscodeProgress>,
    /// Decision engine's verdict for browser-default capabilities.
    /// `None` for non-playable states (failed resolve, still
    /// resolving). Frontend gates direct-play vs HLS on
    /// `plan?.method === 'direct_play'`; the reason set drives
    /// the "transcoding because: …" UI copy.
    pub plan: Option<crate::playback::PlaybackPlan>,
    /// Selectable audio tracks from the source. Empty for streaming
    /// states (the torrent's streams haven't been probed yet) and
    /// for library files that happen to have no audio. The
    /// player's audio-track picker renders from this list;
    /// switching writes `?audio_stream=N` to the HLS URL.
    #[serde(default)]
    pub audio_tracks: Vec<crate::playback::AudioTrack>,
    /// Selectable subtitle tracks from the source. Each text
    /// track has a `vtt_url`; image subs (PGS/VOBSUB) have `None`
    /// for `vtt_url` because they need server-side burn-in, not
    /// a `<track>` element.
    #[serde(default)]
    pub subtitle_tracks: Vec<crate::playback::SubtitleTrack>,
    /// Detected intro / credits timestamps for TV episodes.
    /// Populated only for `kind=episode` when the intro-skipper
    /// subsystem has analysed the season. All four fields are
    /// independent — a show may have a detected intro but no
    /// credits, or vice versa. Frontend `SkipButton` renders
    /// when the playhead enters the intro range or passes the
    /// credits start. `skip_enabled_for_show` gates rendering
    /// per-show so the user can keep watching themes they like.
    pub intro_start_ms: Option<i64>,
    pub intro_end_ms: Option<i64>,
    pub credits_start_ms: Option<i64>,
    pub credits_end_ms: Option<i64>,
    /// Whether the user has the skip button enabled for this
    /// show. `None` for movies or when the show row is missing.
    pub skip_enabled_for_show: Option<bool>,
    /// Parent show id for TV episodes — populated solely so the
    /// frontend can key a per-session "intro already watched
    /// through" flag on `(show_id, season_number)` for smart auto-
    /// skip. `None` for movies.
    pub show_id: Option<i64>,
    /// Parent season number for TV episodes. `None` for movies.
    pub season_number: Option<i64>,
    /// True when at least one episode in this season has already
    /// been watched (`play_count > 0`). Smart-mode auto-skip uses
    /// this to distinguish "first time seeing this show" (show the
    /// button) from "user has seen the intro before" (auto-skip).
    /// Always `false` for movies.
    #[serde(default)]
    pub season_any_watched: bool,
    /// Raw `auto_skip_intros` config value (`"off"` / `"on"` /
    /// `"smart"`) echoed onto every prepare response so the
    /// frontend doesn't need a parallel config fetch. Always
    /// populated — the backend falls back to `"smart"` (schema
    /// default) if the config row is missing.
    pub auto_skip_intros: String,
    /// Container-authored chapter markers in presentation
    /// order. Empty for sources without chapter metadata or
    /// for stream-mode sources (pre-import). Drives the
    /// chapter-list affordance in the player.
    #[serde(default)]
    pub chapters: Vec<crate::playback::chapter_model::Chapter>,
    /// Total runtime in seconds if known (from TMDB for streaming,
    /// from ffprobe for library).
    pub duration_secs: Option<i64>,
    /// Saved resume point in seconds. Non-null for entities the
    /// user has watched before. Frontend uses this to drive the
    /// Resume dialog when appropriate (external `/play/` entry +
    /// no explicit `?resume_at=`).
    pub resume_position_secs: Option<i64>,
    /// Trickplay VTT URL when available — same endpoint shape
    /// regardless of whether it's stream-live or library-generated.
    pub trickplay_url: Option<String>,

    // ── Stream-only descriptors (populated when state != Downloaded) ──
    /// Download row id for actions like Cancel / Resume Torrent.
    pub download_id: Option<i64>,
    pub downloaded_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    /// Bytes/sec.
    pub download_speed: Option<i64>,

    // ── Library-only descriptors (populated when state = Downloaded) ──
    /// Media row id — drives Cast handoff + sub-resource URLs
    /// that need a media reference (e.g. some log lookups).
    pub media_id: Option<i64>,

    // ── Error descriptors ──
    pub error_message: Option<String>,
}

/// Query params on `/prepare`. The only knob today is the
/// Cast target override — the frontend's Cast integration passes
/// the receiver device name so the decision engine plans for that
/// device instead of for the controlling browser. UA-based
/// detection handles everything else.
#[derive(Debug, Deserialize, Default, utoipa::IntoParams)]
pub struct PrepareParams {
    /// Explicit capability preset override (e.g. `"chromecast_gtv"`,
    /// `"apple_tv_4k"`). Set by the frontend Cast SDK layer when a
    /// Cast session is active; unset for normal browser playback.
    #[serde(default)]
    pub target: Option<String>,
}

/// `GET /api/v1/play/{kind}/{entity_id}/prepare`
///
/// Returns everything the unified player needs to render. Never
/// fails with a raw 500 if possible — errors become `state: failed`
/// + `error_message`.
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/prepare",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        PrepareParams,
    ),
    responses(
        (status = 200, body = PlayPrepareReply),
        (status = 202, description = "Metadata still resolving — keep polling"),
        (status = 404, description = "Entity not tracked"),
    ),
    tag = "playback", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)] // each `ResolveError` arm is linear; splitting obscures the state→response mapping
pub async fn prepare(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
    headers: HeaderMap,
    Query(params): Query<PrepareParams>,
) -> AppResult<Response> {
    let identity = resolve_identity(&state, kind, entity_id).await;
    // Current auto-skip mode — echoed on every reply branch so the
    // frontend doesn't need a parallel `/config` fetch to decide
    // whether to auto-skip on intro entry.
    let auto_skip_intros = load_auto_skip_mode(&state).await;
    // Pick the capability profile for this request. `target=` query
    // override wins — the frontend sets it when a Cast session is
    // active so we plan for the receiver, not the controlling
    // browser. Falls back to UA-based detection otherwise.
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let (client_caps, detected_client) = params
        .target
        .as_deref()
        .and_then(crate::playback::ClientCapabilities::from_target_override)
        .unwrap_or_else(|| crate::playback::ClientCapabilities::from_user_agent(ua));
    tracing::debug!(
        kind = %kind.as_str(),
        entity_id,
        family = ?detected_client.family,
        os = ?detected_client.os,
        preset = %detected_client.preset,
        target_override = params.target.is_some(),
        "prepare: client detection",
    );

    match resolve_byte_source(&state, kind, entity_id).await {
        Ok(ByteSource::Library {
            media_id,
            file_path,
            container,
            video_codec,
            audio_codec,
            runtime_ticks,
            trickplay_generated,
        }) => {
            // `/prepare` uses load_streams both for audio+subtitle
            // lists and to populate the decision-engine's HDR
            // inputs. Single query, consumed twice.
            let streams =
                crate::playback::load_streams(&state.db, media_id, kind.as_str(), entity_id)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            media_id,
                            error = %e,
                            "prepare: failed to load streams, continuing with empty lists",
                        );
                        crate::playback::LoadedStreams::default()
                    });
            let source_info = crate::playback::SourceInfo {
                container: container.clone(),
                video_codec: video_codec.clone(),
                audio_tracks: streams
                    .audio
                    .iter()
                    .map(crate::playback::AudioTrack::to_candidate)
                    .collect(),
                color_transfer: streams
                    .video
                    .as_ref()
                    .and_then(|v| v.color_transfer.clone()),
                pix_fmt: streams.video.as_ref().and_then(|v| v.pixel_format.clone()),
                hdr_format: streams.video.as_ref().and_then(|v| v.hdr_format.clone()),
            };
            let plan = crate::playback::plan_playback(
                &source_info,
                &client_caps,
                &crate::playback::PlaybackOptions::default(),
            );
            // INFO level (not debug) — the decision the engine
            // picks is load-bearing for playback behaviour, so a
            // log hunt for "why did this direct-play silently /
            // transcode unnecessarily" wants this line front +
            // centre. Includes the audio-tracks count because
            // an empty list is a strong tell that streams
            // weren't populated at import time.
            tracing::info!(
                media_id,
                method = ?plan.method,
                reasons = %plan.transcode_reasons,
                selected_audio = ?plan.selected_audio_stream,
                audio_passthrough = plan.audio_passthrough,
                audio_tracks_count = source_info.audio_tracks.len(),
                container = ?source_info.container,
                video_codec = ?source_info.video_codec,
                "prepare: decision engine",
            );
            if source_info.audio_tracks.is_empty() {
                tracing::warn!(
                    media_id,
                    "prepare: source has no audio tracks in stream table — \
                     decision engine treated as silent clip. This is usually a \
                     probe / import failure rather than a genuinely silent file.",
                );
            }
            let chapters = sqlx::query_as::<_, crate::playback::chapter_model::Chapter>(
                "SELECT id, media_id, idx, start_secs, end_secs, title
                   FROM chapter
                  WHERE media_id = ?
                  ORDER BY idx",
            )
            .bind(media_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(media_id, error = %e, "prepare: chapter load failed");
                Vec::new()
            });
            let resume_position_secs = lookup_resume_seconds(&state, kind, entity_id).await;
            // Always advertise the URL — the endpoint itself returns
            // 404 while `trickplay_generated` is still false, and
            // `useTrickplay` handles that by showing the skeleton
            // until cues land. Gating here hid the UI entirely during
            // the post-import / pre-generation window.
            let _ = trickplay_generated;
            let trickplay_url = Some(unified_trickplay_url(kind, entity_id));
            let _ = file_path; // kept in ByteSource for the byte endpoints
            let skip = load_skip_data(&state, kind, entity_id).await;
            tracing::debug!(
                media_id,
                audio_tracks = streams.audio.len(),
                subtitle_tracks = streams.subtitles.len(),
                intro_ms = ?skip.intro_start_ms,
                credits_ms = ?skip.credits_start_ms,
                "prepare: loaded streams + skip data"
            );
            let file_size_bytes: Option<i64> =
                sqlx::query_scalar("SELECT size FROM media WHERE id = ?")
                    .bind(media_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
            let duration_secs = runtime_ticks.map(|t| t / 10_000_000);
            let total_bitrate_bps = match (file_size_bytes, duration_secs) {
                (Some(size), Some(dur)) if dur > 0 => Some((size * 8) / dur),
                _ => None,
            };
            let hw_backend = fetch_hw_backend(&state).await;
            let live_progress = if matches!(plan.method, crate::playback::PlaybackMethod::Transcode)
                && let Some(ref tm) = state.transcode
            {
                tm.progress_for_media(media_id).await
            } else {
                None
            };
            let video = streams.video.clone();
            Ok(Json(PlayPrepareReply {
                kind,
                entity_id,
                state: PlayState::Downloaded,
                title: identity.title,
                episode_label: identity.episode_label.clone(),
                backdrop_path: identity.backdrop_path,
                logo_content_type: identity.logo_content_type,
                logo_entity_id: identity.logo_entity_id,
                logo_palette: identity.logo_palette,
                container,
                video_codec,
                audio_codec,
                video,
                file_size_bytes,
                total_bitrate_bps,
                hw_backend,
                live_progress,
                detected_client: Some(detected_client.clone()),
                plan: Some(plan),
                duration_secs,
                resume_position_secs,
                trickplay_url,
                download_id: None,
                downloaded_bytes: None,
                total_bytes: None,
                download_speed: None,
                media_id: Some(media_id),
                error_message: None,
                audio_tracks: streams.audio,
                subtitle_tracks: streams.subtitles,
                intro_start_ms: skip.intro_start_ms,
                intro_end_ms: skip.intro_end_ms,
                credits_start_ms: skip.credits_start_ms,
                credits_end_ms: skip.credits_end_ms,
                skip_enabled_for_show: skip.skip_enabled_for_show,
                show_id: skip.show_id,
                season_number: skip.season_number,
                season_any_watched: skip.season_any_watched,
                auto_skip_intros: auto_skip_intros.clone(),
                chapters,
            })
            .into_response())
        }
        Ok(ByteSource::Stream {
            download_id,
            torrent_hash,
            file_idx,
            file_size,
            state: dl_state,
            error_message,
            downloaded,
            download_speed,
        }) => {
            // State mapping for the chip / overlays. `seeding` /
            // `finished` are post-complete but pre-import (torrent
            // finished downloading, import not yet committed) —
            // still "streaming" from the UX's perspective.
            let play_state = if dl_state == "paused" {
                PlayState::Paused
            } else {
                PlayState::Streaming
            };
            // Metadata-not-ready is a legitimate "keep polling" signal
            // from the unified prepare — the frontend stepper renders
            // the pre-bytes stage.
            let Some(file_idx_value) = file_idx else {
                return Ok(StatusCode::ACCEPTED.into_response());
            };
            let resume_position_secs = lookup_resume_seconds(&state, kind, entity_id).await;
            let duration_secs = lookup_runtime_secs(&state, kind, entity_id).await;

            // Kick the stream-trickplay background task. It's
            // idempotent (`ensure_running` no-ops when a task for
            // this download is already in flight), and we need this
            // here because the legacy `/stream/.../prepare` was the
            // previous trigger — without it, the stream source never
            // gets hover thumbnails during the live-download window.
            if let Some(dur) = duration_secs {
                state
                    .stream_trickplay
                    .ensure_running(&state, download_id, file_idx_value, dur)
                    .await;
            }

            // Resolve the partial file's on-disk path and kick the
            // lazy probe. Once ≥5 MB is available the first caller
            // runs ffprobe; subsequent callers (every 3s poll) see
            // the cached result. The probe enables feature parity
            // with the library path — video / audio / subtitle
            // tracks, HDR detection, proper plan — instead of the
            // empty placeholder the streaming arm used to return.
            let partial_file_path =
                resolve_partial_file_path(&state, &torrent_hash, file_idx_value).await;
            let probe_result = match &partial_file_path {
                Some(path) => {
                    state
                        .stream_probe
                        .get_or_probe(download_id, file_idx_value, path, downloaded)
                        .await
                }
                None => None,
            };
            // When the probe is ready we can mirror the library
            // branch: load typed streams, pick an audio track for
            // the decision engine, run plan_playback, surface the
            // lot on the reply. Until then, fall back to empty
            // lists — the frontend polls every 3s and the next
            // poll picks up the probe.
            let (streams, plan, container, video_codec, audio_codec) = match probe_result {
                Some(p) => {
                    let loaded =
                        crate::playback::load_streams_from_probe(&p, kind.as_str(), entity_id);
                    let container = p
                        .format
                        .as_ref()
                        .and_then(|f| f.format_name.as_deref())
                        .and_then(|s| s.split(',').next())
                        .map(str::to_owned);
                    let video_codec = loaded.video.as_ref().map(|v| v.codec.clone());
                    let audio_codec = loaded.audio.first().map(|a| a.codec.clone());
                    let source_info = crate::playback::SourceInfo {
                        container: container.clone(),
                        video_codec: video_codec.clone(),
                        audio_tracks: loaded
                            .audio
                            .iter()
                            .map(crate::playback::AudioTrack::to_candidate)
                            .collect(),
                        color_transfer: loaded
                            .video
                            .as_ref()
                            .and_then(|v| v.color_transfer.clone()),
                        pix_fmt: loaded.video.as_ref().and_then(|v| v.pixel_format.clone()),
                        hdr_format: loaded.video.as_ref().and_then(|v| v.hdr_format.clone()),
                    };
                    let plan = crate::playback::plan_playback(
                        &source_info,
                        &client_caps,
                        &crate::playback::PlaybackOptions::default(),
                    );
                    tracing::info!(
                        download_id,
                        method = ?plan.method,
                        reasons = %plan.transcode_reasons,
                        container = ?source_info.container,
                        video_codec = ?source_info.video_codec,
                        hdr = ?source_info.hdr_format,
                        audio_tracks = loaded.audio.len(),
                        "prepare: streaming decision engine (probe ready)",
                    );
                    (loaded, Some(plan), container, video_codec, audio_codec)
                }
                None => (
                    crate::playback::LoadedStreams::default(),
                    None,
                    None,
                    None,
                    None,
                ),
            };

            // See the library branch — always advertise the URL so
            // VideoShell renders the hover skeleton while the stream-
            // trickplay background task catches up.
            let trickplay_url = Some(unified_trickplay_url(kind, entity_id));
            Ok(Json(PlayPrepareReply {
                kind,
                entity_id,
                state: play_state,
                title: identity.title,
                episode_label: identity.episode_label.clone(),
                backdrop_path: identity.backdrop_path,
                logo_content_type: identity.logo_content_type,
                logo_entity_id: identity.logo_entity_id,
                logo_palette: identity.logo_palette,
                container,
                video_codec,
                audio_codec,
                video: streams.video,
                file_size_bytes: None,
                total_bitrate_bps: None,
                hw_backend: fetch_hw_backend(&state).await,
                // Streaming sessions tag with media_id=0 (no library
                // row exists yet); progress_for_media(0) finds the
                // most-recently-active stream transcode, which is
                // what the user is watching.
                live_progress: if let Some(ref tm) = state.transcode {
                    tm.progress_for_media(0).await
                } else {
                    None
                },
                detected_client: Some(detected_client.clone()),
                plan,
                duration_secs,
                resume_position_secs,
                trickplay_url,
                download_id: Some(download_id),
                downloaded_bytes: Some(downloaded),
                total_bytes: file_size.map(|s| i64::try_from(s).unwrap_or(i64::MAX)),
                download_speed: Some(download_speed),
                media_id: None,
                error_message,
                audio_tracks: streams.audio,
                subtitle_tracks: streams.subtitles,
                intro_start_ms: None,
                intro_end_ms: None,
                credits_start_ms: None,
                credits_end_ms: None,
                skip_enabled_for_show: None,
                show_id: None,
                season_number: None,
                season_any_watched: false,
                auto_skip_intros: auto_skip_intros.clone(),
                chapters: Vec::new(),
            })
            .into_response())
        }
        Err(ResolveError::EntityNotFound) => Err(AppError::NotFound(format!(
            "{kind_str} {entity_id} not tracked",
            kind_str = kind.as_str()
        ))),
        Err(ResolveError::LibraryFileMissing {
            media_id,
            file_path,
        }) => Ok(Json(PlayPrepareReply {
            kind,
            entity_id,
            state: PlayState::Failed,
            title: identity.title,
            episode_label: identity.episode_label.clone(),
            backdrop_path: identity.backdrop_path,
            logo_content_type: identity.logo_content_type,
            logo_entity_id: identity.logo_entity_id,
            logo_palette: identity.logo_palette,
            container: None,
            video_codec: None,
            audio_codec: None,
            video: None,
            file_size_bytes: None,
            total_bitrate_bps: None,
            hw_backend: None,
            live_progress: None,
            detected_client: Some(detected_client.clone()),
            plan: None,
            duration_secs: None,
            resume_position_secs: None,
            trickplay_url: None,
            download_id: None,
            downloaded_bytes: None,
            total_bytes: None,
            download_speed: None,
            media_id: Some(media_id),
            error_message: Some(format!(
                "Library file missing on disk: {file_path}. Re-import or check storage."
            )),
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            intro_start_ms: None,
            intro_end_ms: None,
            credits_start_ms: None,
            credits_end_ms: None,
            skip_enabled_for_show: None,
            show_id: None,
            season_number: None,
            season_any_watched: false,
            auto_skip_intros: auto_skip_intros.clone(),
            chapters: Vec::new(),
        })
        .into_response()),
        Err(ResolveError::DownloadNotReady {
            download_id,
            state: dl_state,
            error_message,
        }) => {
            let play_state = match DownloadPhase::parse(&dl_state) {
                Some(DownloadPhase::Searching) => PlayState::Searching,
                Some(DownloadPhase::Queued) => PlayState::Queued,
                // `grabbing` and any other pre-bytes state collapse
                // into "grabbing" from the stepper's perspective.
                _ => PlayState::Grabbing,
            };
            Ok(Json(PlayPrepareReply {
                kind,
                entity_id,
                state: play_state,
                title: identity.title,
                episode_label: identity.episode_label.clone(),
                backdrop_path: identity.backdrop_path,
                logo_content_type: identity.logo_content_type,
                logo_entity_id: identity.logo_entity_id,
                logo_palette: identity.logo_palette,
                container: None,
                video_codec: None,
                audio_codec: None,
                video: None,
                file_size_bytes: None,
                total_bitrate_bps: None,
                hw_backend: None,
                live_progress: None,
                detected_client: Some(detected_client.clone()),
                plan: None,
                duration_secs: lookup_runtime_secs(&state, kind, entity_id).await,
                resume_position_secs: lookup_resume_seconds(&state, kind, entity_id).await,
                trickplay_url: None,
                download_id: Some(download_id),
                downloaded_bytes: None,
                total_bytes: None,
                download_speed: None,
                media_id: None,
                error_message,
                // Stream sources: no typed stream probe yet
                // (ffprobe runs on the partial file lazily). Empty
                // lists are honest here rather than guessing.
                audio_tracks: Vec::new(),
                subtitle_tracks: Vec::new(),
                intro_start_ms: None,
                intro_end_ms: None,
                credits_start_ms: None,
                credits_end_ms: None,
                skip_enabled_for_show: None,
                show_id: None,
                season_number: None,
                season_any_watched: false,
                auto_skip_intros: auto_skip_intros.clone(),
                chapters: Vec::new(),
            })
            .into_response())
        }
        Err(ResolveError::DownloadFailed {
            download_id,
            error_message,
        }) => Ok(Json(PlayPrepareReply {
            kind,
            entity_id,
            state: PlayState::Failed,
            title: identity.title,
            episode_label: identity.episode_label.clone(),
            backdrop_path: identity.backdrop_path,
            logo_content_type: identity.logo_content_type,
            logo_entity_id: identity.logo_entity_id,
            logo_palette: identity.logo_palette,
            container: None,
            video_codec: None,
            audio_codec: None,
            video: None,
            file_size_bytes: None,
            total_bitrate_bps: None,
            hw_backend: None,
            live_progress: None,
            detected_client: Some(detected_client.clone()),
            plan: None,
            duration_secs: lookup_runtime_secs(&state, kind, entity_id).await,
            resume_position_secs: None,
            trickplay_url: None,
            download_id: Some(download_id),
            downloaded_bytes: None,
            total_bytes: None,
            download_speed: None,
            media_id: None,
            error_message,
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            intro_start_ms: None,
            intro_end_ms: None,
            credits_start_ms: None,
            credits_end_ms: None,
            skip_enabled_for_show: None,
            show_id: None,
            season_number: None,
            season_any_watched: false,
            auto_skip_intros: auto_skip_intros.clone(),
            chapters: Vec::new(),
        })
        .into_response()),
        Err(ResolveError::NoSource) => Ok(Json(PlayPrepareReply {
            kind,
            entity_id,
            state: PlayState::Failed,
            title: identity.title,
            episode_label: identity.episode_label.clone(),
            backdrop_path: identity.backdrop_path,
            logo_content_type: identity.logo_content_type,
            logo_entity_id: identity.logo_entity_id,
            logo_palette: identity.logo_palette,
            container: None,
            video_codec: None,
            audio_codec: None,
            video: None,
            file_size_bytes: None,
            total_bitrate_bps: None,
            hw_backend: None,
            live_progress: None,
            detected_client: Some(detected_client.clone()),
            plan: None,
            duration_secs: lookup_runtime_secs(&state, kind, entity_id).await,
            resume_position_secs: lookup_resume_seconds(&state, kind, entity_id).await,
            trickplay_url: None,
            download_id: None,
            downloaded_bytes: None,
            total_bytes: None,
            download_speed: None,
            media_id: None,
            error_message: Some(
                "Nothing to play yet. Start watching to begin the download.".to_owned(),
            ),
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            intro_start_ms: None,
            intro_end_ms: None,
            credits_start_ms: None,
            credits_end_ms: None,
            skip_enabled_for_show: None,
            show_id: None,
            season_number: None,
            season_any_watched: false,
            auto_skip_intros: auto_skip_intros.clone(),
            chapters: Vec::new(),
        })
        .into_response()),
    }
}

// ── Shared helpers ──

#[derive(Debug, Default)]
struct LoadingIdentity {
    title: String,
    /// Episodes only: "S01E04 · Pilot". None for movies.
    episode_label: Option<String>,
    backdrop_path: Option<String>,
    logo_content_type: Option<String>,
    logo_entity_id: Option<i64>,
    logo_palette: Option<String>,
}

/// Resolve the loading-shell identity (title, backdrop, logo refs)
/// for the entity. Episodes inherit from their parent show and
/// carry an extra `episode_label` line so the player chrome can
/// render "Show · `SxxExx` · Episode Title" in full.
async fn resolve_identity(state: &AppState, kind: PlayKind, entity_id: i64) -> LoadingIdentity {
    #[derive(sqlx::FromRow)]
    struct MovieRow {
        title: String,
        backdrop_path: Option<String>,
        logo_palette: Option<String>,
    }
    #[derive(sqlx::FromRow)]
    struct ShowRow {
        show_id: i64,
        title: String,
        backdrop_path: Option<String>,
        logo_palette: Option<String>,
        season_number: i64,
        episode_number: i64,
        episode_title: Option<String>,
    }
    match kind {
        PlayKind::Movie => {
            let row: Option<MovieRow> =
                sqlx::query_as("SELECT title, backdrop_path, logo_palette FROM movie WHERE id = ?")
                    .bind(entity_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
            row.map(|r| LoadingIdentity {
                title: r.title,
                episode_label: None,
                backdrop_path: r.backdrop_path,
                logo_content_type: Some("movies".to_owned()),
                logo_entity_id: Some(entity_id),
                logo_palette: r.logo_palette,
            })
            .unwrap_or_default()
        }
        PlayKind::Episode => {
            let row: Option<ShowRow> = sqlx::query_as(
                "SELECT s.id AS show_id, s.title, s.backdrop_path, s.logo_palette,
                        e.season_number, e.episode_number, e.title AS episode_title
                 FROM show s JOIN episode e ON e.show_id = s.id
                 WHERE e.id = ? LIMIT 1",
            )
            .bind(entity_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            row.map(|r| {
                let sxe = format!("S{:02}E{:02}", r.season_number, r.episode_number);
                let episode_label = match r.episode_title {
                    Some(t) if !t.is_empty() => Some(format!("{sxe} · {t}")),
                    _ => Some(sxe),
                };
                LoadingIdentity {
                    title: r.title,
                    episode_label,
                    backdrop_path: r.backdrop_path,
                    logo_content_type: Some("shows".to_owned()),
                    logo_entity_id: Some(r.show_id),
                    logo_palette: r.logo_palette,
                }
            })
            .unwrap_or_default()
        }
    }
}

async fn lookup_resume_seconds(state: &AppState, kind: PlayKind, entity_id: i64) -> Option<i64> {
    let ticks: Option<i64> = match kind {
        PlayKind::Movie => {
            sqlx::query_scalar("SELECT playback_position_ticks FROM movie WHERE id = ?")
                .bind(entity_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
        }
        PlayKind::Episode => {
            sqlx::query_scalar("SELECT playback_position_ticks FROM episode WHERE id = ?")
                .bind(entity_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
        }
    };
    ticks.filter(|t| *t > 0).map(|t| t / 10_000_000)
}

async fn lookup_runtime_secs(state: &AppState, kind: PlayKind, entity_id: i64) -> Option<i64> {
    let minutes: Option<i64> = match kind {
        PlayKind::Movie => sqlx::query_scalar("SELECT runtime FROM movie WHERE id = ?")
            .bind(entity_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        PlayKind::Episode => sqlx::query_scalar("SELECT runtime FROM episode WHERE id = ?")
            .bind(entity_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
    };
    minutes.map(|m| m.saturating_mul(60))
}

/// Intro/credits + per-show skip-enabled flag for the prepare
/// response. Movies return `all-None`; episodes may have any
/// subset of the four timestamps populated depending on which
/// detection passes the intro-skipper has completed.
///
/// `show_id` / `season_number` are included so the frontend can key
/// a per-session "intro already watched through" flag on them — part
/// of the smart auto-skip decision. `season_any_watched` short-
/// circuits that flag when any episode in the same season already
/// has `play_count > 0`: the user has seen this show's intro before,
/// so smart mode should auto-skip from the first episode this
/// session.
#[derive(Debug, Default)]
struct SkipData {
    intro_start_ms: Option<i64>,
    intro_end_ms: Option<i64>,
    credits_start_ms: Option<i64>,
    credits_end_ms: Option<i64>,
    skip_enabled_for_show: Option<bool>,
    show_id: Option<i64>,
    season_number: Option<i64>,
    season_any_watched: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct SkipRow {
    intro_start_ms: Option<i64>,
    intro_end_ms: Option<i64>,
    credits_start_ms: Option<i64>,
    credits_end_ms: Option<i64>,
    skip_intros: Option<bool>,
    show_id: i64,
    season_number: i64,
}

async fn load_skip_data(state: &AppState, kind: PlayKind, entity_id: i64) -> SkipData {
    if !matches!(kind, PlayKind::Episode) {
        // Movies don't have intros / credits analysis — return
        // empty. Keep the `skip_enabled_for_show` None so the
        // frontend doesn't render skip UI on movies.
        return SkipData::default();
    }
    let row: Option<SkipRow> = sqlx::query_as(
        "SELECT e.intro_start_ms, e.intro_end_ms,
                e.credits_start_ms, e.credits_end_ms,
                s.skip_intros,
                e.show_id, e.season_number
         FROM episode e
         JOIN show s ON s.id = e.show_id
         WHERE e.id = ?",
    )
    .bind(entity_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some(r) = row else {
        return SkipData::default();
    };
    // "Has the user watched any intro from this season already" —
    // used by smart-mode auto-skip. `play_count > 0` on any sibling
    // episode satisfies the condition; the current episode being
    // re-watched also counts (the user has seen the intro before).
    let season_any_watched: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM episode
         WHERE show_id = ? AND season_number = ? AND play_count > 0",
    )
    .bind(r.show_id)
    .bind(r.season_number)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0)
        > 0;
    SkipData {
        intro_start_ms: r.intro_start_ms,
        intro_end_ms: r.intro_end_ms,
        credits_start_ms: r.credits_start_ms,
        credits_end_ms: r.credits_end_ms,
        skip_enabled_for_show: Some(r.skip_intros.unwrap_or(true)),
        show_id: Some(r.show_id),
        season_number: Some(r.season_number),
        season_any_watched,
    }
}

/// Echo the current `auto_skip_intros` config on every prepare
/// response so the frontend doesn't need a parallel `/config` fetch
/// just to decide whether to auto-skip. Defaults to `"smart"` when
/// the config row is missing — matches the schema default.
async fn load_auto_skip_mode(state: &AppState) -> String {
    sqlx::query_scalar::<_, String>("SELECT auto_skip_intros FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "smart".to_owned())
}

/// Which HDR bucket the output stream lives in — drives the
/// master playlist's `VIDEO-RANGE` + optional
/// `SUPPLEMENTAL-CODECS` tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputRange {
    /// Standard dynamic range, BT.709. The bulk of library
    /// content + every output of our SDR transcode path.
    Sdr,
    /// HDR10 / Dolby Vision with HDR10 fallback / any PQ
    /// passthrough output. Signaled as `VIDEO-RANGE=PQ`.
    Pq,
    /// Hybrid Log-Gamma (broadcast HDR). Signaled as
    /// `VIDEO-RANGE=HLG`.
    Hlg,
}

impl OutputRange {
    pub(crate) fn as_tag(self) -> &'static str {
        match self {
            Self::Sdr => "SDR",
            Self::Pq => "PQ",
            Self::Hlg => "HLG",
        }
    }
}

/// Derive the master playlist's `VIDEO-RANGE` from source
/// characteristics + chosen playback method.
///
/// * `Transcode` → always SDR. The libx264 / h264_* encoder
///   chain with `-pix_fmt yuv420p` + tone-map filter produces
///   BT.709 output regardless of source.
/// * `Remux` / `DirectPlay` → source-equivalent. Stream-copy
///   or raw-byte serve preserves whatever HDR the source
///   carries, so we signal it honestly.
/// * Missing color metadata → SDR (conservative: tells HDR
///   clients "don't switch panels into HDR mode for this
///   stream"; the alternative would strand SDR content on
///   HDR-capable clients with flickery mode switching).
pub(crate) fn resolve_output_range(
    source: &crate::playback::SourceInfo,
    method: crate::playback::PlaybackMethod,
) -> OutputRange {
    use crate::playback::PlaybackMethod;
    if matches!(method, PlaybackMethod::Transcode) {
        return OutputRange::Sdr;
    }
    if let Some(t) = source.color_transfer.as_deref() {
        let lc = t.to_ascii_lowercase();
        if lc == "smpte2084" {
            return OutputRange::Pq;
        }
        if lc == "arib-std-b67" {
            return OutputRange::Hlg;
        }
    }
    if let Some(f) = source.hdr_format.as_deref() {
        let lc = f.to_ascii_lowercase();
        if lc.contains("dolby vision") || lc.contains("dovi") || lc.contains("hdr10") {
            return OutputRange::Pq;
        }
        if lc.contains("hlg") {
            return OutputRange::Hlg;
        }
    }
    OutputRange::Sdr
}

/// RFC 6381 / ISOBMFF `CODECS` attribute value for the output
/// audio track.
///
/// * Transcode + no passthrough → `mp4a.40.2` (AAC-LC, stereo
///   re-encode).
/// * Transcode + passthrough or Remux / `DirectPlay` → the
///   selected source track's codec:
///   * `aac` → `mp4a.40.2`
///   * `ac3` → `ac-3`
///   * `eac3` → `ec-3`
///   * anything unrecognised falls back to `mp4a.40.2` (we
///     would never emit a file that needed this path — the
///     decision engine's passthrough detection is the gate —
///     but the fallback keeps `CODECS` valid if a future
///     codec lands before it's wired up here).
///
/// Direct clients index on this string for decoder selection;
/// strict HLS validators (iOS / CAF) reject mismatches, hence
/// the honesty.
pub(crate) fn resolve_audio_codec_tag(
    source: &crate::playback::SourceInfo,
    plan: &crate::playback::PlaybackPlan,
) -> &'static str {
    use crate::playback::PlaybackMethod;
    let using_source_audio = match plan.method {
        PlaybackMethod::Transcode => plan.audio_passthrough,
        PlaybackMethod::Remux | PlaybackMethod::DirectPlay => true,
    };
    if !using_source_audio {
        return "mp4a.40.2";
    }
    let Some(idx) = plan.selected_audio_stream else {
        return "mp4a.40.2";
    };
    let track = source.audio_tracks.iter().find(|t| t.stream_index == idx);
    let codec = track.map_or("", |t| t.codec.as_str());
    let profile = track.and_then(|t| t.profile.as_deref()).unwrap_or("");
    match codec {
        "ac3" => "ac-3",
        "eac3" => "ec-3",
        // DTS family: `dtsc` covers DTS Core; DTS-HD MA and
        // DTS-X (which rides inside a DTS-HD MA bitstream) get
        // `dtsh` so Apple TV's CAF validator unlocks the
        // lossless decode path. ffprobe surfaces the variants
        // as `codec_name="dts"` + a `profile` string — we
        // promote to `dtsh` only when the profile string says
        // HD-MA; DTS-HD HRA + plain DTS Core stay on `dtsc`.
        "dts" if is_dts_hd_ma_profile(profile) => "dtsh",
        "dts" => "dtsc",
        // AAC + anything unrecognised → safe default. We
        // bucket "aac" and the fallback case together
        // because both emit the same CODECS value; splitting
        // them would be noise.
        _ => "mp4a.40.2",
    }
}

/// DTS-HD MA detection from the ffprobe `profile` string. ffmpeg
/// emits `"DTS-HD MA"` for DTS-HD Master Audio (and for DTS-X,
/// which is layered inside an HD-MA core — so the HD-MA HLS tag
/// covers both). Other DTS profiles (`"DTS"`, `"DTS-ES"`,
/// `"DTS 96/24"`, `"DTS-HD HRA"`) stay on the `dtsc` tag because
/// they're lossy and broadly compatible with the core-profile
/// decoder path clients expect when they see `dtsc`.
pub(crate) fn is_dts_hd_ma_profile(profile: &str) -> bool {
    let lc = profile.to_ascii_lowercase();
    lc.contains("dts-hd ma") || lc.contains("dts-hd master audio")
}

/// Build the HLS `SUPPLEMENTAL-CODECS` attribute for Dolby
/// Vision content being passed through.
///
/// Format: `"dvh1.<profile>.<level>/<fallback>"` where
/// * `dvh1` is the sample-entry `FourCC` for DV-in-HEVC;
///   `dvav` would be DV-in-AVC (rare), `dav1` for DV-in-AV1
///   (profile 10).
/// * `<profile>` is the zero-padded DV profile byte (05 / 07
///   / 08 / 10).
/// * `<level>` is the DV level byte — we don't track this
///   precisely; 06 (≤ 60 Mbps) covers most UHD library
///   content.
/// * `<fallback>` is `db1p` (HDR10 fallback), `db2p` (SDR
///   fallback), `db4p` (HLG fallback), `dbap` (absent).
///   Profile 5 has no fallback → `dbap`; profile 8.1 ships
///   an HDR10 base layer → `db1p`.
///
/// Returns `None` when the source isn't DV — regular HDR10
/// and HLG don't need this tag, the `VIDEO-RANGE=PQ/HLG`
/// signaling alone is enough. Also returns `None` when the
/// plan carries a bitstream filter that strips the DV RPU:
/// the output is pure HDR10 at that point, advertising DV
/// would lie to clients.
pub(crate) fn resolve_supplemental_codecs(
    source: &crate::playback::SourceInfo,
    plan: &crate::playback::PlaybackPlan,
) -> Option<String> {
    let fmt = source.hdr_format.as_deref()?.to_ascii_lowercase();
    if !(fmt.contains("dolby vision") || fmt.contains("dovi")) {
        return None;
    }
    if plan
        .video_bitstream_filter
        .as_deref()
        .is_some_and(|f| f.contains("remove_dovi"))
    {
        return None;
    }
    // Profile detection from the hdr_format string. The import
    // layer surfaces strings like "Dolby Vision Profile 5" or
    // "Dolby Vision Profile 8.1" — pull the first digit sequence.
    let profile = fmt
        .split_whitespace()
        .find_map(|tok| {
            let head: String = tok.chars().take_while(char::is_ascii_digit).collect();
            head.parse::<u8>().ok()
        })
        .unwrap_or(8);
    // Fallback tag: profile 5 has no HDR10 fallback in the
    // bitstream, so advertise `dbap` (absent); the common
    // streaming profiles 8.1 / 8.4 carry HDR10 / HLG
    // fallbacks respectively. We default to `db1p` which is
    // correct for 8.1 (by far the most common DV encountered
    // in library content).
    let fallback = match profile {
        5 => "dbap",
        _ if fmt.contains("8.4") => "db4p",
        _ => "db1p",
    };
    Some(format!("dvh1.{profile:02}.06/{fallback}"))
}

/// Render the single-variant master playlist. Kept in one
/// place so the `hls_master` reuse path (existing session,
/// no-restart) and the initial-spawn path stay in lockstep.
///
/// The video leg of `CODECS` is `avc1.640028` — H.264 High L4.0,
/// a safe conservative match for our libx264 encode config
/// (`-profile:v high`, no explicit `-level`) and for typical
/// library sources that remux. Strict HLS validators + CAF
/// receivers reject mismatched CODECS strings; the previous
/// hardcoded `avc1.640029` (High L4.1) claimed a level we never
/// pinned.
///
/// The audio leg of `CODECS` reflects what's actually in the
/// segments: `mp4a.40.2` (AAC-LC) when we're re-encoding,
/// `ac-3` / `ec-3` when we're passing through AC-3 / EAC-3 for
/// a client that advertised them. Passing through `ec-3`
/// preserves 5.1 + Atmos-in-EAC-3 without touching the audio
/// bitstream.
///
/// `BANDWIDTH=5000000` is a placeholder — real bitrate
/// measurement lands with the profile-chain work that tracks
/// encoder output. 5Mbps is in range for libx264 veryfast CRF
/// 23 at 1080p.
///
/// `VIDEO-RANGE` is computed from `(source, method)` via
/// `resolve_output_range`: SDR for any transcode output, PQ /
/// HLG for HDR passthrough on remux / direct-play paths. An
/// optional `SUPPLEMENTAL-CODECS` tag advertises Dolby Vision
/// with its HDR10 / HLG / SDR fallback so DV-capable clients
/// (Apple TV, LG OLED) can opt in while non-DV clients render
/// the base layer.
pub(crate) fn build_master_playlist(
    kind: PlayKind,
    entity_id: i64,
    tab_qs: &str,
    range: OutputRange,
    supplemental_codecs: Option<&str>,
    audio_codec_tag: &str,
) -> String {
    let k = kind.as_str();
    let mut attrs = format!(
        "BANDWIDTH=5000000,CODECS=\"avc1.640028,{audio_codec_tag}\",VIDEO-RANGE={}",
        range.as_tag(),
    );
    if let Some(sup) = supplemental_codecs {
        use std::fmt::Write;
        let _ = write!(attrs, ",SUPPLEMENTAL-CODECS=\"{sup}\"");
    }
    format!(
        "#EXTM3U\n\
         #EXT-X-STREAM-INF:{attrs}\n\
         /api/v1/play/{k}/{entity_id}/variant.m3u8{tab_qs}\n"
    )
}

pub(crate) fn unified_trickplay_url(kind: PlayKind, entity_id: i64) -> String {
    format!(
        "/api/v1/play/{kind}/{entity_id}/trickplay.vtt",
        kind = kind.as_str()
    )
}

/// Resolve the on-disk absolute path for a torrent file at a
/// given index. `librqbit`'s `files()` returns paths *relative*
/// to the download root AND strips the torrent's wrapper
/// directory (info.name) for most torrent shapes — so a
/// Fellowship torrent reports just
/// `"The.Lord.of.the.Rings.2001...mkv"` while librqbit actually
/// writes it to
/// `<download_path>/<torrent_name>/The.Lord...2001...mkv`.
/// Mirrors `import_trigger`'s path logic: try the
/// `download_path / torrent_name / relative` layout first, fall
/// back to `download_path / relative` when the wrapper is
/// absent (bare single-file torrents).
///
/// Returns `None` when the torrent isn't in the session, metadata
/// hasn't resolved, `config.download_path` is unset, or neither
/// candidate path exists on disk yet.
pub(crate) async fn resolve_partial_file_path(
    state: &AppState,
    torrent_hash: &str,
    file_idx: usize,
) -> Option<std::path::PathBuf> {
    let tc = state.torrent.as_ref()?;
    let relative = tc.files(torrent_hash).and_then(|files| {
        files
            .into_iter()
            .find(|(i, _, _)| *i == file_idx)
            .map(|(_, p, _)| p)
    })?;
    let download_root: String = sqlx::query_scalar("SELECT download_path FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .filter(|s: &String| !s.is_empty())?;
    let root = std::path::Path::new(&download_root);
    // Wrapped layout first — librqbit creates a subdirectory
    // named after the torrent's info.name for most torrents
    // (even single-file ones from many trackers). `files()`
    // returns the path relative to that subdirectory, so we
    // have to prepend the torrent name ourselves.
    if let Some(name) = tc.torrent_name(torrent_hash) {
        let wrapped = root.join(&name).join(&relative);
        if wrapped.exists() {
            return Some(wrapped);
        }
    }
    let bare = root.join(&relative);
    if bare.exists() {
        return Some(bare);
    }
    tracing::debug!(
        torrent_hash,
        file_idx,
        relative = %relative.display(),
        download_root = %download_root,
        "resolve_partial_file_path: neither layout candidate exists on disk yet",
    );
    None
}

/// Configured hardware-acceleration backend for the info chip's
/// Playback section. Treats `"auto"`, empty, and `"none"` all as
/// `None` — the chip renders "Software" in that case rather than
/// echoing the raw setting.
pub(crate) async fn fetch_hw_backend(state: &AppState) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT hw_acceleration FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten()
        .filter(|s| !s.is_empty() && s != "auto" && s != "none")
}

// ─── Tab-nonce + session-id helpers ────────────────────────────────

/// Per-tab nonce, same shape as the legacy stream/playback handlers.
/// `[A-Za-z0-9_-]{1,32}` so it can't path-traverse through a temp dir.
pub(crate) fn sanitize_tab(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Session id for the unified HLS transcode. Keyed on entity so a
/// session that started pre-import keeps running post-import (the
/// underlying file is identical via hardlink) without a restart.
pub(crate) fn play_session_id(kind: PlayKind, entity_id: i64, tab: Option<&str>) -> String {
    let k = kind.as_str();
    match tab.and_then(sanitize_tab) {
        Some(nonce) => format!("play-{k}-{entity_id}-{nonce}"),
        None => format!("play-{k}-{entity_id}"),
    }
}

/// Build the `?tab=...&cast_token=...` suffix that every play subroute
/// URL carries. Cast tokens have to round-trip through the master /
/// variant / segment chain — the Cast SDK can't attach auth headers
/// to playlist subrequests, so dropping the token anywhere in the
/// chain breaks playback for every non-MP4 cast target.
pub(crate) fn play_query_suffix(tab: Option<&str>, cast_token: Option<&str>) -> String {
    let tab = tab.and_then(sanitize_tab);
    let token = cast_token.and_then(sanitize_cast_token);
    match (tab, token) {
        (None, None) => String::new(),
        (Some(t), None) => format!("?tab={t}"),
        (None, Some(c)) => format!("?cast_token={c}"),
        (Some(t), Some(c)) => format!("?tab={t}&cast_token={c}"),
    }
}

/// Allow only the characters cast tokens use (base64url + `.`). Any
/// other input is dropped — keeps `?cast_token=` injection-safe even
/// though the token is surfaced through our own auth middleware
/// before it reaches the route handler.
pub(crate) fn sanitize_cast_token(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        .then(|| t.to_owned())
}

pub(crate) async fn load_api_key(db: &sqlx::SqlitePool) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

// ─── Query params ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct PlayHlsMasterParams {
    #[serde(default)]
    pub at: Option<f64>,
    #[serde(default)]
    pub audio_stream: Option<i64>,
    /// Subtitle stream index to burn into the video. Only has an
    /// effect when the stream is image-based (PGS / VOBSUB /
    /// DVB); for text-based subs the frontend renders via
    /// `<track>` from the `/subtitles/{idx}` endpoint and this
    /// param is ignored. Setting → unsetting forces an ffmpeg
    /// restart (adds / removes the `-filter_complex overlay`
    /// chain).
    #[serde(default)]
    pub subtitle_stream: Option<i64>,
    #[serde(default)]
    pub tab: Option<String>,
    /// Cast target preset override — mirrors `PrepareParams::target`
    /// so the session spawned by this handler matches the plan the
    /// frontend got from `/prepare`. Typically sent through by the
    /// URL construction in `PlayerRoot` when a Cast session is active.
    #[serde(default)]
    pub target: Option<String>,
    /// Echoed back into every URL we hand to the receiver — variant
    /// playlist, init segment, media segments. The Cast SDK can't add
    /// auth headers to playlist subrequests, so the token has to live
    /// in the URL or the receiver gets 401 on the very next fetch.
    #[serde(default)]
    pub cast_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayTabParams {
    #[serde(default)]
    pub tab: Option<String>,
    /// Same purpose as `PlayHlsMasterParams::cast_token`. Variant +
    /// segment routes echo it into their generated URLs so the entire
    /// HLS subtree stays auth-attached on the receiver.
    #[serde(default)]
    pub cast_token: Option<String>,
}

// ─── Diagnostic stream wrapper (copy of the one in api/stream.rs) ───

/// Wraps a byte-serving `AsyncRead` so truncation (body dropped
/// before `expected` bytes landed) shows up in logs with the
/// reason attached. Mirrors the logic in the legacy `stream_file`
/// handler — duplicated here to keep the unified module self-
/// contained; both will live in parallel until the old endpoints
/// are removed.
struct LoggingReader {
    inner: Box<dyn AsyncRead + Send + Unpin>,
    bytes_sent: u64,
    expected: u64,
    started_at: Instant,
    label: String,
    range_start: u64,
    ever_read: bool,
    eof_warned: bool,
}

impl LoggingReader {
    fn new(
        inner: Box<dyn AsyncRead + Send + Unpin>,
        expected: u64,
        label: String,
        range_start: u64,
    ) -> Self {
        tracing::info!(label = %label, range_start, expected, "play body opened");
        Self {
            inner,
            bytes_sent: 0,
            expected,
            started_at: Instant::now(),
            label,
            range_start,
            ever_read: false,
            eof_warned: false,
        }
    }
}

impl AsyncRead for LoggingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        match &result {
            Poll::Ready(Ok(())) => {
                let delta = (buf.filled().len() - before) as u64;
                self.bytes_sent += delta;
                if delta > 0 {
                    self.ever_read = true;
                } else if self.bytes_sent < self.expected && !self.eof_warned {
                    self.eof_warned = true;
                    tracing::warn!(
                        label = %self.label,
                        sent = self.bytes_sent,
                        expected = self.expected,
                        "file stream returned EOF early (0-byte read)"
                    );
                }
            }
            Poll::Ready(Err(e)) => {
                tracing::warn!(
                    label = %self.label,
                    sent = self.bytes_sent,
                    expected = self.expected,
                    error = %e,
                    kind = ?e.kind(),
                    "play stream read error"
                );
            }
            Poll::Pending => {}
        }
        result
    }
}

impl Drop for LoggingReader {
    fn drop(&mut self) {
        let duration = self.started_at.elapsed();
        let short = self.bytes_sent < self.expected;
        // "TRUNCATED" only fires when the file stream signaled an
        // early EOF (librqbit returned `Poll::Ready(Ok(()))` with 0
        // bytes before `expected`). That's the server-side bug class
        // we care about. A Drop with `short=true` but `!eof_warned`
        // means the HTTP consumer cancelled — Firefox's probe
        // pattern (open full-file, sniff, close, range-request) hits
        // this every stream start, and it's benign.
        if short && self.eof_warned {
            tracing::warn!(
                label = %self.label,
                range_start = self.range_start,
                sent = self.bytes_sent,
                expected = self.expected,
                short_by = self.expected - self.bytes_sent,
                duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                "play body dropped TRUNCATED (server-side EOF)"
            );
        } else {
            tracing::debug!(
                label = %self.label,
                range_start = self.range_start,
                sent = self.bytes_sent,
                expected = self.expected,
                short,
                duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                "play body closed"
            );
        }
    }
}
// ─── /direct ────────────────────────────────────────────────────────

/// `GET /api/v1/play/{kind}/{entity_id}/direct`
///
/// Range-aware byte stream of the resolved source. Library source
/// goes through `tower_http::ServeFile` (local disk). Stream source
/// goes through librqbit's piece-prioritised `FileStream`. The
/// dispatcher decides fresh on every request — a browser issuing
/// sequential Range requests during playback transparently flips
/// from stream → library the moment the import lands.
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/direct",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
    ),
    responses(
        (status = 200, description = "Full file"),
        (status = 206, description = "Partial content"),
        (status = 404, description = "No playable source"),
    ),
    tag = "playback", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)] // range handling + two source branches; splitting obscures flow
pub async fn direct(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
    headers: HeaderMap,
    request: Request,
) -> AppResult<Response> {
    let source = resolve_byte_source(&state, kind, entity_id)
        .await
        .map_err(resolve_error_to_app_error)?;

    match source {
        ByteSource::Library { file_path, .. } => {
            if !tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
                return Err(AppError::NotFound("library file missing on disk".into()));
            }
            let response = ServeFile::new(&file_path)
                .oneshot(request)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!("serve file: {e}")))?;
            Ok(response.into_response())
        }
        ByteSource::Stream {
            download_id,
            torrent_hash,
            file_idx,
            ..
        } => {
            let Some(file_idx) = file_idx else {
                return Err(AppError::BadRequest(
                    "torrent metadata not ready yet — keep polling /prepare".into(),
                ));
            };
            let torrent = state
                .torrent
                .as_ref()
                .ok_or_else(|| AppError::BadRequest("torrent client not running".into()))?;

            let mut file_stream = torrent
                .open_file_stream(&torrent_hash, file_idx)
                .await
                .map_err(AppError::Internal)?;
            let total_len = file_stream.total_len();

            let mime = mime_from_idx(torrent.as_ref(), &torrent_hash, file_idx);
            let mut out = HeaderMap::new();
            out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            if let Some(m) = &mime
                && let Ok(val) = HeaderValue::from_str(m)
            {
                out.insert(header::CONTENT_TYPE, val);
            }

            let range = parse_range_header(headers.get(header::RANGE), total_len);
            let range_str = headers
                .get(header::RANGE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("(none)");
            let label = format!(
                "play/{k}/{entity_id}/direct (download_id={download_id}, file_idx={file_idx})",
                k = kind.as_str()
            );
            tracing::info!(
                label = %label,
                total_len,
                range = %range_str,
                "direct stream request",
            );

            let (status, body): (StatusCode, Body) = if let Some((start, end_exclusive)) = range {
                if start >= total_len || end_exclusive <= start || end_exclusive > total_len {
                    return Ok(StatusCode::RANGE_NOT_SATISFIABLE.into_response());
                }
                file_stream
                    .seek(SeekFrom::Start(start))
                    .await
                    .map_err(|e| AppError::Internal(e.into()))?;
                let to_take = end_exclusive - start;
                out.insert(
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&to_take.to_string())
                        .expect("numeric string is always a valid header value"),
                );
                out.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!(
                        "bytes {}-{}/{}",
                        start,
                        end_exclusive.saturating_sub(1),
                        total_len
                    ))
                    .expect("ascii-only content-range value"),
                );
                let taken: Box<dyn AsyncRead + Send + Unpin> = Box::new(file_stream.take(to_take));
                let logging = LoggingReader::new(taken, to_take, label, start);
                (
                    StatusCode::PARTIAL_CONTENT,
                    Body::from_stream(ReaderStream::with_capacity(logging, 65_536)),
                )
            } else {
                out.insert(
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&total_len.to_string())
                        .expect("numeric string is always a valid header value"),
                );
                let full: Box<dyn AsyncRead + Send + Unpin> = Box::new(file_stream);
                let logging = LoggingReader::new(full, total_len, label, 0);
                (
                    StatusCode::OK,
                    Body::from_stream(ReaderStream::with_capacity(logging, 65_536)),
                )
            };

            Ok((status, out, body).into_response())
        }
    }
}

fn parse_range_header(value: Option<&HeaderValue>, total: u64) -> Option<(u64, u64)> {
    let s = value?.to_str().ok()?.strip_prefix("bytes=")?;
    let (start, end) = s.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end_exclusive = if end.is_empty() {
        total
    } else {
        end.parse::<u64>().ok()?.saturating_add(1)
    };
    Some((start, end_exclusive))
}

fn mime_from_idx(
    client: &dyn crate::download::TorrentSession,
    hash: &str,
    file_idx: usize,
) -> Option<String> {
    let files = client.files(hash)?;
    let (_, path, _) = files.into_iter().find(|(i, _, _)| *i == file_idx)?;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(
        match ext.as_str() {
            "mp4" | "m4v" => "video/mp4",
            "mkv" => "video/x-matroska",
            "webm" => "video/webm",
            "avi" => "video/x-msvideo",
            "mov" => "video/quicktime",
            "ts" => "video/mp2t",
            "mpg" | "mpeg" => "video/mpeg",
            _ => "application/octet-stream",
        }
        .to_owned(),
    )
}

// ─── Stop transcode ─────────────────────────────────────────────────

/// `DELETE /api/v1/play/{kind}/{entity_id}/transcode`
///
/// Called by the frontend on player unmount so the encoder budget
/// doesn't linger behind a closed tab.
#[utoipa::path(
    delete, path = "/api/v1/play/{kind}/{entity_id}/transcode",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        ("tab" = Option<String>, Query),
    ),
    responses((status = 204)),
    tag = "playback", security(("api_key" = []))
)]
pub async fn stop_transcode(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
    axum::extract::Query(params): axum::extract::Query<PlayTabParams>,
) -> AppResult<StatusCode> {
    if let Some(ref transcode) = state.transcode {
        let session_id = play_session_id(kind, entity_id, params.tab.as_deref());
        if let Err(e) = transcode.stop_session(&session_id).await {
            tracing::warn!(%session_id, error = %e, "failed to stop play transcode");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

// ─── Subtitles ──────────────────────────────────────────────────────

/// Query params on the subtitle endpoint.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SubtitleParams {
    /// HLS transcode `-ss` offset in seconds. Shifts every
    /// VTT cue back by this amount so cue timestamps line
    /// up with the trimmed output's `video.currentTime`
    /// (which starts at 0 even though content starts at
    /// `offset`). Cues ending before the offset are
    /// dropped; cues straddling the cut are clamped to
    /// start at 0. When omitted or 0, the VTT is served
    /// unchanged.
    pub offset: Option<f64>,
}

/// `GET /api/v1/play/{kind}/{entity_id}/subtitles/{stream_index}`
///
/// Serves a `WebVTT` rendition of the requested subtitle stream.
/// Library sources read codec + path from the DB `stream` table;
/// streaming sources pull the same info from the in-memory probe
/// cache + partial torrent file. Both paths route through the
/// same `get_subtitle_path` extractor so SRT/ASS conversions
/// behave identically; cache roots differ
/// (`cache/subs/{media_id}` vs `cache/subs-stream/{download_id}`)
/// so a partial streaming extract doesn't collide with a library
/// rebuild if the user imports mid-stream.
///
/// Accepts `?offset=<secs>` for HLS playback that started mid-file
/// via `ffmpeg -ss`. The VTT's native timestamps are source-time;
/// without the shift the browser would wait for the real-clock cue
/// time against the trimmed output's zero-based
/// `video.currentTime`, displaying subtitles minutes late (or not
/// at all if the trim is far enough in).
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/subtitles/{stream_index}",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        ("stream_index" = i64, Path),
        SubtitleParams,
    ),
    responses(
        (status = 200, description = "WebVTT subtitle", content_type = "text/vtt"),
        (status = 404, description = "No subs for this source"),
    ),
    tag = "playback", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)] // library vs streaming branches + cache-dir resolution; splitting scatters the flow
pub async fn subtitle(
    State(state): State<AppState>,
    Path((kind, entity_id, stream_index)): Path<(PlayKind, i64, i64)>,
    Query(params): Query<SubtitleParams>,
) -> AppResult<Response> {
    let source = resolve_byte_source(&state, kind, entity_id)
        .await
        .map_err(resolve_error_to_app_error)?;

    let ffmpeg_path: String = state.transcode.as_ref().map_or_else(
        || "ffmpeg".to_owned(),
        super::super::playback::transcode::TranscodeManager::ffmpeg_path,
    );

    // Resolve (file_path, codec, is_external, external_path,
    // cache_dir) from whichever source we got. Library: DB
    // lookup. Streaming: probe cache + partial file resolution.
    let (file_path, codec, is_external, external_path, temp_dir) = match source {
        ByteSource::Library {
            media_id,
            file_path,
            ..
        } => {
            #[derive(sqlx::FromRow)]
            struct StreamRow {
                codec: String,
                is_external: bool,
                path: Option<String>,
            }
            let row: StreamRow = sqlx::query_as(
                "SELECT codec, is_external, path FROM stream
                 WHERE media_id = ? AND stream_index = ? AND stream_type = 'subtitle'",
            )
            .bind(media_id)
            .bind(stream_index)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "subtitle stream {stream_index} not found for media {media_id}"
                ))
            })?;
            (
                std::path::PathBuf::from(file_path),
                row.codec,
                row.is_external,
                row.path,
                crate::playback::subtitle::cache_dir(&state.data_path, media_id),
            )
        }
        ByteSource::Stream {
            download_id,
            torrent_hash,
            file_idx,
            downloaded,
            ..
        } => {
            let Some(idx) = file_idx else {
                return Err(AppError::NotFound(
                    "torrent metadata not ready — cannot extract subtitles yet".into(),
                ));
            };
            let file_path = resolve_partial_file_path(&state, &torrent_hash, idx)
                .await
                .ok_or_else(|| {
                    AppError::NotFound(
                        "partial download file not on disk yet — cannot extract subtitles".into(),
                    )
                })?;
            // Probe tells us the subtitle codec. Force-probe if
            // not yet cached (we have enough bytes for the header
            // since we're able to resolve a file path).
            let probe = state
                .stream_probe
                .get_or_probe(download_id, idx, &file_path, downloaded)
                .await
                .ok_or_else(|| {
                    AppError::NotFound(
                        "probe not ready yet — try again after a few seconds of download".into(),
                    )
                })?;
            let codec = probe
                .streams
                .as_ref()
                .and_then(|streams| streams.iter().find(|s| s.index == stream_index))
                .and_then(|s| s.codec_name.clone())
                .ok_or_else(|| {
                    AppError::NotFound(format!(
                        "subtitle stream {stream_index} not in probe result"
                    ))
                })?;
            (
                file_path,
                codec,
                false, // embedded in the container
                None,
                crate::playback::subtitle::stream_cache_dir(&state.data_path, download_id),
            )
        }
    };

    let vtt_path = crate::playback::subtitle::get_subtitle_path(
        &file_path,
        stream_index,
        &codec,
        is_external,
        external_path.as_deref(),
        &temp_dir,
        &ffmpeg_path,
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;
    let raw = tokio::fs::read(&vtt_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read subtitle: {e}")))?;

    // Apply the HLS offset shift if the caller supplied
    // one. We operate on UTF-8; if the file is somehow
    // non-UTF-8 we pass it through as-is (extraction
    // produces UTF-8 VTTs, so this is defensive).
    let body: Vec<u8> = match (params.offset, std::str::from_utf8(&raw)) {
        (Some(offset), Ok(text)) if offset > 0.0 => {
            crate::playback::subtitle::shift_vtt_timestamps(text, offset).into_bytes()
        }
        _ => raw,
    };

    // Cache is offset-aware via the query param, so each
    // distinct offset lives in a different cache entry.
    // A short max-age still wins within a session while
    // letting an HLS restart at a new `-ss` invalidate
    // naturally.
    let cache_control = if params.offset.unwrap_or(0.0) > 0.0 {
        "private, max-age=3600"
    } else {
        "public, max-age=86400"
    };

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/vtt; charset=utf-8"),
        )],
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_str(cache_control).expect("static ASCII"),
        )],
        body,
    )
        .into_response())
}

// ─── Trickplay ──────────────────────────────────────────────────────

/// `GET /api/v1/play/{kind}/{entity_id}/trickplay.vtt`
///
/// Picks the right source's trickplay VTT: library-generated for
/// imported media (from `media.trickplay_generated`), stream-
/// generated for in-progress downloads (written by the streaming
/// trickplay task). During the stream→library transition the next
/// fetch naturally starts hitting the library VTT once it exists —
/// WS-driven invalidation on the frontend triggers the refetch.
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/trickplay.vtt",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
    ),
    responses(
        (status = 200, description = "WebVTT cues", content_type = "text/vtt"),
        (status = 404, description = "No trickplay available yet"),
    ),
    tag = "playback", security(("api_key" = []))
)]
pub async fn trickplay_vtt(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
) -> AppResult<Response> {
    let source = resolve_byte_source(&state, kind, entity_id)
        .await
        .map_err(resolve_error_to_app_error)?;
    let k = kind.as_str();
    let prefix = format!("/api/v1/play/{k}/{entity_id}/trickplay/");

    // Check the library trickplay dir by file-existence rather than
    // the `trickplay_generated` flag — the stream-trickplay task
    // *promotes* its partial output into this dir on import (renaming
    // `data/trickplay-stream/{download_id}` to `data/trickplay/{media_id}`),
    // and the flag only flips to 1 once the task finishes sealing any
    // remaining sheets. Gating on the flag would hide a perfectly-usable
    // partial VTT during that post-import finalising window.
    if let ByteSource::Library { media_id, .. } = &source {
        let vtt_path = crate::playback::trickplay::trickplay_dir(&state.data_path, *media_id)
            .join("trickplay.vtt");
        if let Ok(raw) = tokio::fs::read(&vtt_path).await {
            let body = String::from_utf8_lossy(&raw)
                .replace("sprite_", &format!("{prefix}sprite_"))
                .into_bytes();
            // `no-store` while the task may still be adding sheets;
            // ETag-based caching comes once we mark `generated = 1`.
            return Ok((
                [(header::CONTENT_TYPE, HeaderValue::from_static("text/vtt"))],
                [(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-store, must-revalidate"),
                )],
                body,
            )
                .into_response());
        }
    }

    // Fallback / stream source: look up any download linked to this
    // entity and try its stream-trickplay VTT. Works for both live
    // streaming AND the post-import window where the library sweep
    // hasn't produced sprites yet.
    let download_id = match &source {
        ByteSource::Stream { download_id, .. } => Some(*download_id),
        ByteSource::Library { .. } => lookup_linked_download_id(&state, kind, entity_id).await,
    };
    if let Some(dl_id) = download_id {
        let dir = crate::playback::trickplay_stream::output_dir(&state.data_path, dl_id);
        let vtt_path = dir.join("trickplay.vtt");
        if let Ok(raw) = tokio::fs::read(&vtt_path).await {
            let body = String::from_utf8_lossy(&raw)
                .replace("sprite_", &format!("{prefix}sprite_"))
                .into_bytes();
            return Ok((
                [(header::CONTENT_TYPE, HeaderValue::from_static("text/vtt"))],
                [(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-store, must-revalidate"),
                )],
                body,
            )
                .into_response());
        }
    }

    Err(AppError::NotFound("no trickplay yet".into()))
}

/// Look up any download linked to this entity, ignoring state —
/// used by trickplay fallback where we still have the stream VTT
/// on disk for a finished / imported download.
async fn lookup_linked_download_id(
    state: &AppState,
    kind: PlayKind,
    entity_id: i64,
) -> Option<i64> {
    let sql = match kind {
        PlayKind::Movie => {
            "SELECT download_id FROM download_content WHERE movie_id = ? ORDER BY download_id DESC LIMIT 1"
        }
        PlayKind::Episode => {
            "SELECT download_id FROM download_content WHERE episode_id = ? ORDER BY download_id DESC LIMIT 1"
        }
    };
    sqlx::query_scalar(sql)
        .bind(entity_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

// ─── Trickplay sprite ───────────────────────────────────────────────

/// `GET /api/v1/play/{kind}/{entity_id}/trickplay/{name}` — JPEG
/// sprite sheet referenced by the VTT. Dispatches on byte source
/// so the same URL serves library-generated sprites pre- or
/// post-import, and stream-generated sprites during the live
/// download phase.
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/trickplay/{name}",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "JPEG sprite", content_type = "image/jpeg"),
        (status = 404),
    ),
    tag = "playback", security(("api_key" = []))
)]
pub async fn trickplay_sprite(
    State(state): State<AppState>,
    Path((kind, entity_id, name)): Path<(PlayKind, i64, String)>,
) -> AppResult<Response> {
    // Defend against path traversal: only allow filenames matching
    // our deterministic `sprite_NNN.jpg` naming.
    let valid = name.starts_with("sprite_")
        && std::path::Path::new(&name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
        && !name.contains('/')
        && !name.contains("..");
    if !valid {
        return Err(AppError::BadRequest("invalid sprite name".into()));
    }

    let source = resolve_byte_source(&state, kind, entity_id)
        .await
        .map_err(resolve_error_to_app_error)?;

    // Mirror the VTT handler's fallback ladder: library sprite if
    // generated, else any linked download's stream-trickplay
    // sprites. Guarantees the sheet the VTT referenced is
    // reachable no matter which source served the VTT.
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let ByteSource::Library { media_id, .. } = &source {
        candidates.push(
            crate::playback::trickplay::trickplay_dir(&state.data_path, *media_id).join(&name),
        );
    }
    let download_id = match &source {
        ByteSource::Stream { download_id, .. } => Some(*download_id),
        ByteSource::Library { .. } => lookup_linked_download_id(&state, kind, entity_id).await,
    };
    if let Some(dl) = download_id {
        candidates
            .push(crate::playback::trickplay_stream::output_dir(&state.data_path, dl).join(&name));
    }

    let sprite_path = candidates.into_iter().find(|p| p.exists());
    let Some(sprite_path) = sprite_path else {
        return Err(AppError::NotFound("sprite not found".into()));
    };
    let bytes = tokio::fs::read(&sprite_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read sprite: {e}")))?;

    Ok((
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"))],
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=3600"),
        )],
        bytes,
    )
        .into_response())
}

// ─── Progress ───────────────────────────────────────────────────────

/// Body for `POST /api/v1/play/{kind}/{entity_id}/progress`. Single
/// endpoint replaces `/playback/progress` (media-id) and
/// `/playback/stream/{id}/progress` (download-id). Backend writes
/// to the entity row (`movie.playback_position_ticks` /
/// `episode.playback_position_ticks`) directly — both source paths
/// resolve to the same row, so the write target is the same
/// regardless of whether the byte stream came from torrent or
/// library.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PlayProgressBody {
    /// Current playhead in seconds (source-time).
    pub position_secs: f64,
    /// True on the final beacon (tab close / route change) so we
    /// emit `/scrobble/pause` to Trakt and release any session
    /// resources. Mutually exclusive with `paused` (tab-close
    /// takes precedence — we route the whole session through
    /// the pause path either way, but `final_tick` also triggers
    /// the session-end cleanup).
    #[serde(default)]
    pub final_tick: bool,
    /// True when the client is paused inside the player (as
    /// opposed to actively playing). Maps to Trakt
    /// `/scrobble/pause`; false maps to `/scrobble/start`. Before
    /// this field existed, all in-session pauses pushed
    /// `scrobble/start` ticks which made Trakt show "watching"
    /// while the user was actually paused.
    #[serde(default)]
    pub paused: bool,
    /// Per-tab incognito flag. Local progress still persists;
    /// scrobble stays silent.
    #[serde(default)]
    pub incognito: bool,
}

/// `POST /api/v1/play/{kind}/{entity_id}/progress`
#[utoipa::path(
    post, path = "/api/v1/play/{kind}/{entity_id}/progress",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
    ),
    request_body = PlayProgressBody,
    responses((status = 204), (status = 404)),
    tag = "playback", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)] // mirrors the library report_progress — scrobble branches ride inline
pub async fn play_progress(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
    Json(body): Json<PlayProgressBody>,
) -> AppResult<StatusCode> {
    let movie_id = (kind == PlayKind::Movie).then_some(entity_id);
    let episode_id = (kind == PlayKind::Episode).then_some(entity_id);

    // Runtime in minutes → ticks for threshold calculation.
    let runtime_mins: Option<i64> = match kind {
        PlayKind::Movie => sqlx::query_scalar("SELECT runtime FROM movie WHERE id = ?")
            .bind(entity_id)
            .fetch_optional(&state.db)
            .await?
            .flatten(),
        PlayKind::Episode => sqlx::query_scalar("SELECT runtime FROM episode WHERE id = ?")
            .bind(entity_id)
            .fetch_optional(&state.db)
            .await?
            .flatten(),
    };
    let runtime_ticks = runtime_mins.map(|m| m.saturating_mul(60).saturating_mul(10_000_000));

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let position_ticks = (body.position_secs * 10_000_000.0) as i64;

    let action = progress::update_progress(
        &state.db,
        movie_id,
        episode_id,
        position_ticks,
        runtime_ticks,
    )
    .await?;

    // Scrobble fanout — identical to the legacy library path.
    let progress_pct = runtime_ticks.filter(|t| *t > 0).map_or(0.0, |total| {
        #[allow(clippy::cast_precision_loss)]
        let p = (position_ticks as f64 / total as f64) * 100.0;
        p.clamp(0.0, 100.0)
    });
    // Route to scrobble/pause on either the tab-close beacon
    // or a mid-session pause; only "playing" ticks land on
    // scrobble/start. `final_tick` takes precedence because it
    // also implies session-end cleanup elsewhere in the Trakt
    // integration.
    let final_tick = body.final_tick && action != progress::WatchAction::MarkWatched;
    let emit_pause = final_tick || body.paused;
    if !body.incognito
        && let Some(mid) = movie_id
    {
        if emit_pause {
            state
                .scrobble
                .on_pause(
                    &state.db,
                    mid,
                    crate::integrations::trakt::scrobble::Kind::Movie,
                    progress_pct,
                )
                .await;
        } else {
            state
                .scrobble
                .on_progress(
                    &state.db,
                    mid,
                    crate::integrations::trakt::scrobble::Kind::Movie,
                    progress_pct,
                )
                .await;
        }
    } else if !body.incognito
        && let Some(eid) = episode_id
    {
        if emit_pause {
            state
                .scrobble
                .on_pause(
                    &state.db,
                    eid,
                    crate::integrations::trakt::scrobble::Kind::Episode,
                    progress_pct,
                )
                .await;
        } else {
            state
                .scrobble
                .on_progress(
                    &state.db,
                    eid,
                    crate::integrations::trakt::scrobble::Kind::Episode,
                    progress_pct,
                )
                .await;
        }
    }

    // Interim progress fanout — keeps Home "Up Next" + ShowDetail's
    // next-up progress bar live without a manual refresh. `Watched`
    // below covers the completion transition with its own tag, so
    // skip PlaybackProgress on that tick to avoid double-invalidation.
    if action != progress::WatchAction::MarkWatched {
        // Position is always non-negative in practice (the frontend
        // clamps) and will fit in i64 for any realistic runtime, so
        // the truncation-on-cast is safe.
        #[allow(clippy::cast_possible_truncation)]
        let position_secs = body.position_secs.max(0.0) as i64;
        state.emit(crate::events::AppEvent::PlaybackProgress {
            movie_id,
            episode_id,
            position_secs,
            // `progress_pct` above is a 0-100 scale for the scrobble
            // call; the event carries the 0.0-1.0 fraction the UI
            // consumes as a progress bar.
            progress_pct: (progress_pct / 100.0).clamp(0.0, 1.0),
        });
    }

    if action == progress::WatchAction::MarkWatched {
        let title = match kind {
            PlayKind::Movie => {
                sqlx::query_scalar::<_, String>("SELECT title FROM movie WHERE id = ?")
                    .bind(entity_id)
                    .fetch_optional(&state.db)
                    .await?
                    .unwrap_or_default()
            }
            PlayKind::Episode => {
                sqlx::query_scalar::<_, String>("SELECT title FROM episode WHERE id = ?")
                    .bind(entity_id)
                    .fetch_optional(&state.db)
                    .await?
                    .unwrap_or_default()
            }
        };

        state.emit(crate::events::AppEvent::Watched {
            movie_id,
            episode_id,
            title,
        });

        if !body.incognito {
            if let Some(mid) = movie_id {
                state
                    .scrobble
                    .on_watched(
                        &state.db,
                        mid,
                        crate::integrations::trakt::scrobble::Kind::Movie,
                    )
                    .await;
            } else if let Some(eid) = episode_id {
                state
                    .scrobble
                    .on_watched(
                        &state.db,
                        eid,
                        crate::integrations::trakt::scrobble::Kind::Episode,
                    )
                    .await;
            }
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod play_query_suffix_tests {
    use super::{play_query_suffix, sanitize_cast_token};

    #[test]
    fn empty_when_no_inputs() {
        assert_eq!(play_query_suffix(None, None), "");
    }

    #[test]
    fn tab_only_matches_legacy_shape() {
        assert_eq!(play_query_suffix(Some("abc"), None), "?tab=abc");
    }

    #[test]
    fn cast_token_only_carries_through() {
        // Master URL constructed by the cast-token endpoint has no
        // `tab=` — the receiver sees `?cast_token=` alone and the
        // rewrite has to keep that shape.
        assert_eq!(
            play_query_suffix(None, Some("abc.def-_xyz")),
            "?cast_token=abc.def-_xyz"
        );
    }

    #[test]
    fn both_join_with_ampersand() {
        // Tab nonce + cast token together — both have to land on
        // every variant / segment subrequest or playback breaks for
        // multi-tab cast users.
        assert_eq!(
            play_query_suffix(Some("foo"), Some("tok")),
            "?tab=foo&cast_token=tok"
        );
    }

    #[test]
    fn sanitize_cast_token_rejects_injection() {
        // Even though the auth middleware vets the token before the
        // route runs, the sanitizer is the second layer that keeps
        // `&`, `?`, spaces out of the URL we emit.
        assert_eq!(
            sanitize_cast_token("abc.def-_xyz"),
            Some("abc.def-_xyz".into())
        );
        assert_eq!(sanitize_cast_token(""), None);
        assert_eq!(sanitize_cast_token("ab&cd"), None);
        assert_eq!(sanitize_cast_token("ab cd"), None);
        assert_eq!(sanitize_cast_token("ab/cd"), None);
    }
}
