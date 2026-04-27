//! `GET /api/v1/play/{kind}/{entity_id}/master.m3u8` — entry point
//! for HLS playback. Resolves the byte source, picks a profile chain,
//! spawns or reuses a transcode session, and emits the master
//! playlist with VIDEO-RANGE / SUPPLEMENTAL-CODECS signaling derived
//! from the source HDR metadata + selected method.
//!
//! Reuse path: when a session for `(entity, tab)` already exists and
//! no `?at=` was requested, we touch the session and return the
//! cached master without touching ffmpeg — hls.js's recovery
//! `startLoad()` re-fetches master, and we don't want that to
//! restart the encoder.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};

use crate::error::{AppError, AppResult};
use crate::playback::PlayKind;
use std::time::{Duration, Instant};

use crate::playback::handlers::{
    OutputRange, PlayHlsMasterParams, build_master_playlist, load_api_key, play_query_suffix,
    play_session_id, resolve_audio_codec_tag, resolve_output_range, resolve_partial_file_path,
    resolve_supplemental_codecs,
};
use crate::playback::source::{ByteSource, resolve_byte_source, resolve_error_to_app_error};
use crate::state::AppState;

// ─── HLS master ─────────────────────────────────────────────────────

/// `GET /api/v1/play/{kind}/{entity_id}/master.m3u8`
///
/// Starts or reuses an HLS transcode session keyed on the entity.
/// Library source → ffmpeg reads the file path directly. Stream
/// source → ffmpeg reads our own `/direct` URL so librqbit's piece
/// prioritiser stays in the loop. Optional `?at=<secs>` sets
/// ffmpeg `-ss` for seek restarts; `?audio_stream=N` picks an
/// alternate audio track; `?tab=<nonce>` segregates sessions so
/// two browser tabs playing the same entity don't kill each
/// other's ffmpeg.
#[utoipa::path(
    get, path = "/api/v1/play/{kind}/{entity_id}/master.m3u8",
    params(
        ("kind" = PlayKind, Path),
        ("entity_id" = i64, Path),
        ("at" = Option<f64>, Query),
        ("audio_stream" = Option<i64>, Query),
        ("tab" = Option<String>, Query),
    ),
    responses(
        (status = 200, description = "Master playlist", content_type = "application/vnd.apple.mpegurl"),
        (status = 404, description = "No playable source"),
        (status = 409, description = "Transcode concurrency cap reached"),
    ),
    tag = "playback", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn hls_master(
    State(state): State<AppState>,
    Path((kind, entity_id)): Path<(PlayKind, i64)>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<PlayHlsMasterParams>,
) -> AppResult<Response> {
    let transcode = state.require_transcode()?;
    // Same UA-driven capability selection as `/prepare` so the
    // plan used at session-spawn time matches what the decision
    // engine told the frontend.
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let (client_caps, _detected_client) = params
        .target
        .as_deref()
        .and_then(crate::playback::ClientCapabilities::from_target_override)
        .unwrap_or_else(|| crate::playback::ClientCapabilities::from_user_agent(ua));
    let source = resolve_byte_source(&state, kind, entity_id)
        .await
        .map_err(resolve_error_to_app_error)?;

    // Entity-keyed session so a session that started during streaming
    // stays valid post-import (bytes are the same file via hardlink).
    let session_id = play_session_id(kind, entity_id, params.tab.as_deref());
    let tab_qs = play_query_suffix(params.tab.as_deref(), params.cast_token.as_deref());

    // Only tear down the existing session when we've been asked to
    // restart at a specific `-ss` offset (user-triggered seek).
    // hls.js calls `startLoad()` on fatal NETWORK_ERRORs to recover,
    // which refetches `master.m3u8` with no `?at=` — we don't want
    // to kill ffmpeg in that case; reusing the session lets the
    // client pick up where the existing playlist leaves off.
    //
    // The short-circuit for the reuse case is at the bottom of the
    // handler so we still hit the concurrency-cap check + log line
    // for the active session.
    let restart_requested = params.at.is_some();
    let already_running = transcode.has_session(&session_id).await;
    if restart_requested
        && already_running
        && let Err(e) = transcode.stop_session(&session_id).await
    {
        tracing::warn!(%session_id, error = %e, "failed to stop prior play transcode");
    }

    // Reuse path: existing session + no restart requested → return
    // the master playlist without touching ffmpeg. This is the
    // steady state when hls.js recovers from a fatal NETWORK_ERROR
    // via `startLoad()` — re-fetching master should be idempotent.
    if already_running && !restart_requested {
        tracing::debug!(
            %session_id,
            kind = %kind.as_str(),
            entity_id,
            "master refetch — reusing existing transcode session",
        );
        transcode.touch_session(&session_id).await;
        // Cached playlist preserves the VIDEO-RANGE /
        // SUPPLEMENTAL-CODECS signaling from spawn — re-deriving
        // here would need another DB round-trip for source HDR
        // info. Fall back to a plain SDR master only if the
        // session predates the cache (empty string).
        let master = transcode
            .session_master_playlist(&session_id)
            .await
            .unwrap_or_else(|| {
                build_master_playlist(
                    kind,
                    entity_id,
                    &tab_qs,
                    OutputRange::Sdr,
                    None,
                    "mp4a.40.2",
                )
            });
        return Ok((
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/vnd.apple.mpegurl"),
            )],
            [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
            master,
        )
            .into_response());
    }

    // Concurrency cap — same behavior as the legacy paths.
    let cap: i64 = sqlx::query_scalar("SELECT max_concurrent_transcodes FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?
        .unwrap_or(2);
    let active = transcode.active_session_count().await;
    if i64::try_from(active).unwrap_or(i64::MAX) >= cap {
        tracing::warn!(
            %session_id,
            active,
            cap,
            "play transcode rejected — concurrency cap reached",
        );
        return Err(AppError::Conflict(format!(
            "max concurrent transcodes reached ({active}/{cap}) — stop another session or raise the cap in settings",
        )));
    }

    // Build ffmpeg input. Library → direct file path. Stream → our
    // own `/direct` endpoint so librqbit's piece prioritiser gets
    // visibility into what ffmpeg needs next.
    //
    // Plan construction: the decision engine decides remux vs
    // transcode based on source codec info. Library sources know
    // their codecs (import populated `media.video_codec` etc.).
    // Stream sources haven't been ffprobed yet, so we fall
    // through to a safe "Transcode" plan — the HLS path always
    // re-encodes those.
    let (input, media_id_for_tag, mut plan, source_info) = match &source {
        ByteSource::Library {
            file_path,
            media_id,
            container,
            video_codec,
            audio_codec,
            ..
        } => {
            // Load the video-stream color metadata + audio track
            // list so the decision engine can see HDR / 10-bit +
            // pick a compatible audio track. Falls through
            // gracefully when the stream table is empty
            // (pre-import or DB race) — the HDR branch just
            // won't fire and audio defaults to empty (silent).
            let loaded =
                crate::playback::load_streams(&state.db, *media_id, kind.as_str(), entity_id)
                    .await
                    .unwrap_or_default();
            let _ = audio_codec; // superseded by audio_tracks below
            let source_info = crate::playback::SourceInfo {
                container: container.clone(),
                video_codec: video_codec.clone(),
                audio_tracks: loaded
                    .audio
                    .iter()
                    .map(crate::playback::AudioTrack::to_candidate)
                    .collect(),
                color_transfer: loaded.video.as_ref().and_then(|v| v.color_transfer.clone()),
                pix_fmt: loaded.video.as_ref().and_then(|v| v.pixel_format.clone()),
                hdr_format: loaded.video.as_ref().and_then(|v| v.hdr_format.clone()),
            };
            let plan = crate::playback::plan_playback(
                &source_info,
                &client_caps,
                &crate::playback::PlaybackOptions::default(),
            );
            (file_path.clone(), *media_id, plan, source_info)
        }
        ByteSource::Stream {
            download_id,
            torrent_hash,
            file_idx,
            downloaded,
            ..
        } => {
            let Some(idx) = file_idx else {
                return Err(AppError::BadRequest(
                    "torrent metadata not ready yet — keep polling /prepare".into(),
                ));
            };
            let api_key = load_api_key(&state.db).await.unwrap_or_default();
            let url = format!(
                "http://127.0.0.1:{port}/api/v1/play/{k}/{entity_id}/direct?api_key={api_key}",
                port = state.http_port,
                k = kind.as_str()
            );
            // Consult the stream probe cache the same way /prepare
            // does. When the probe is ready we can build a proper
            // plan (tonemap on HDR, right audio codec, right
            // passthrough) instead of the default-everything-goes
            // transcode. When it's not ready, fall back to the old
            // "assume Transcode, empty reasons" — the in-session
            // respawn will pick up the correct plan when the probe
            // lands on the next prepare poll.
            let partial_file_path = resolve_partial_file_path(&state, torrent_hash, *idx).await;
            let probe_result = match &partial_file_path {
                Some(path) => {
                    state
                        .stream_probe
                        .get_or_probe(*download_id, *idx, path, *downloaded)
                        .await
                }
                None => None,
            };
            let (plan, source_info) = if let Some(p) = probe_result {
                let loaded = crate::playback::load_streams_from_probe(&p, kind.as_str(), entity_id);
                let container = p
                    .format
                    .as_ref()
                    .and_then(|f| f.format_name.as_deref())
                    .and_then(|s| s.split(',').next())
                    .map(str::to_owned);
                let video_codec = loaded.video.as_ref().map(|v| v.codec.clone());
                let source_info = crate::playback::SourceInfo {
                    container,
                    video_codec,
                    audio_tracks: loaded
                        .audio
                        .iter()
                        .map(crate::playback::AudioTrack::to_candidate)
                        .collect(),
                    color_transfer: loaded.video.as_ref().and_then(|v| v.color_transfer.clone()),
                    pix_fmt: loaded.video.as_ref().and_then(|v| v.pixel_format.clone()),
                    hdr_format: loaded.video.as_ref().and_then(|v| v.hdr_format.clone()),
                };
                let plan = crate::playback::plan_playback(
                    &source_info,
                    &client_caps,
                    &crate::playback::PlaybackOptions::default(),
                );
                (plan, source_info)
            } else {
                let plan = crate::playback::PlaybackPlan {
                    method: crate::playback::PlaybackMethod::Transcode,
                    transcode_reasons: crate::playback::TranscodeReasons::new(),
                    selected_audio_stream: None,
                    video_bitstream_filter: None,
                    audio_passthrough: false,
                };
                (plan, crate::playback::SourceInfo::default())
            };
            (url, 0, plan, source_info)
        }
    };

    // Subtitle burn-in resolution. The frontend passes
    // `?subtitle_stream=N` when the user picks any subtitle
    // track; text-based codecs serve via `<track>` without any
    // server-side effect so we only act when the selected
    // stream is image-based (PGS / VOBSUB / DVB). Image-sub
    // burn-in forces a full Transcode — can't mix `-c:v copy`
    // with a `-filter_complex overlay` chain.
    let burn_in_subtitle = if let (Some(sub_idx), true) = (
        params.subtitle_stream,
        matches!(&source, ByteSource::Library { .. }),
    ) {
        let codec: Option<String> = sqlx::query_scalar(
            "SELECT codec FROM stream WHERE media_id = ? AND stream_index = ? AND stream_type = 'subtitle'",
        )
        .bind(media_id_for_tag)
        .bind(sub_idx)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        let is_image = codec
            .as_deref()
            .is_some_and(crate::playback::stream::is_image_subtitle_codec);
        is_image.then(|| {
            tracing::info!(
                %session_id,
                subtitle_stream = sub_idx,
                codec = ?codec,
                "burn-in subtitle selected — forcing Transcode",
            );
            // Image subtitles can't ride along with Remux — the
            // overlay filter is incompatible with `-c:v copy`.
            // Upgrade the plan + surface the reason.
            if matches!(plan.method, crate::playback::PlaybackMethod::Remux) {
                plan.method = crate::playback::PlaybackMethod::Transcode;
                plan.transcode_reasons
                    .add(crate::playback::TranscodeReason::SubtitleCodecNotSupported);
            }
            sub_idx
        })
    } else {
        None
    };

    // Upgrade DirectPlay → Remux inside the HLS path. Reaching
    // `hls_master` means the client explicitly wants HLS —
    // either the frontend preferred it, or direct play hit a
    // code=4 media error and auto-flipped to `forceHls`. The
    // decision engine's DirectPlay verdict is correct for
    // "source is fully client-compatible", but a DirectPlay
    // plan yields an empty profile chain + start_hls errors
    // out. Remux (stream-copy into fMP4 HLS) is the
    // zero-cost path that satisfies both: the bitstreams are
    // already compatible so they copy unchanged, just
    // wrapped in the HLS container format the client asked
    // for.
    if matches!(plan.method, crate::playback::PlaybackMethod::DirectPlay) {
        tracing::debug!(
            %session_id,
            "DirectPlay plan reached HLS path — upgrading to Remux for this session",
        );
        plan.method = crate::playback::PlaybackMethod::Remux;
    }

    // Build the profile chain from the plan + configured HWA +
    // live probe cache. The chain decides whether to try the
    // configured hardware backend first (with SW fallback) or
    // skip straight to SW when the probe says HW isn't actually
    // working. A fresh cache read per session means the chain
    // picks up on-the-fly driver installs / removals the next
    // time the operator hits `Test FFmpeg` in settings.
    let caps = crate::playback::hw_probe_cache::cached();
    let chain = if let Some(caps) = caps.as_deref() {
        transcode.chain_for(&plan, caps)
    } else {
        // No probe yet (startup race) — treat every backend as
        // unavailable so the chain is SW-only. The probe will
        // warm the cache within a few seconds; subsequent
        // sessions pick up HW rungs automatically.
        transcode.chain_for(
            &plan,
            &crate::playback::HwCapabilities {
                ffmpeg_ok: true,
                ffmpeg_version: None,
                ffmpeg_major: None,
                is_jellyfin_build: false,
                has_libplacebo: false,
                has_libass: false,
                software_codecs: vec!["libx264".into()],
                backends: Vec::new(),
            },
        )
    };
    let first_profile_kind = chain.current().map(|p| p.kind);

    tracing::info!(
        %session_id,
        kind = %kind.as_str(),
        entity_id,
        start_time = ?params.at,
        audio_stream = ?params.audio_stream,
        burn_in_subtitle = ?burn_in_subtitle,
        active_before = active,
        cap,
        method = ?plan.method,
        reasons = %plan.transcode_reasons,
        profile_kind = ?first_profile_kind,
        chain_rungs = chain.len(),
        "play transcode starting",
    );
    // Audio stream selection: explicit user pick wins, otherwise
    // fall back to the decision engine's compatibility-based
    // pick. That lets a library with [TrueHD, AC-3] stream the
    // AC-3 track automatically on an AC-3-capable client without
    // the user having to flip the picker manually.
    let audio_stream = params.audio_stream.or(plan.selected_audio_stream);

    // Build the master playlist once, cache it on the session
    // so refetches return the same signaling without
    // re-deriving. VIDEO-RANGE is computed from (source, method)
    // so an HDR source served via Remux signals PQ / HLG; a
    // tone-mapped transcode output always signals SDR.
    let output_range = resolve_output_range(&source_info, plan.method);
    let supplemental_codecs = resolve_supplemental_codecs(&source_info, &plan);
    let audio_codec_tag = resolve_audio_codec_tag(&source_info, &plan);
    tracing::debug!(
        %session_id,
        output_range = %output_range.as_tag(),
        supplemental_codecs = ?supplemental_codecs,
        audio_codec_tag,
        audio_passthrough = plan.audio_passthrough,
        "master playlist signaling",
    );
    let master = build_master_playlist(
        kind,
        entity_id,
        &tab_qs,
        output_range,
        supplemental_codecs.as_deref(),
        audio_codec_tag,
    );

    // BS.775 downmix filter for the re-encode path. Only
    // applied when (a) we're re-encoding audio (passthrough
    // is off) and (b) the selected track's channel layout is
    // in the known downmix table — stereo / mono / unknown
    // layouts fall through to ffmpeg's `-ac 2`. No-op when
    // passthrough fires since `-c:a copy` skips the filter
    // graph entirely.
    let audio_filter = if plan.audio_passthrough {
        None
    } else {
        audio_stream
            .and_then(|idx| {
                source_info
                    .audio_tracks
                    .iter()
                    .find(|t| t.stream_index == idx)
            })
            .and_then(|t| t.channel_layout.as_deref())
            .and_then(|layout| {
                crate::playback::downmix::build_downmix_filter(
                    Some(layout),
                    crate::playback::downmix::DownmixAlgorithm::default(),
                )
            })
    };
    let playlist_path = transcode
        .start_hls(
            &session_id,
            &input,
            media_id_for_tag,
            params.at,
            audio_stream,
            audio_filter.as_deref(),
            &plan,
            chain,
            burn_in_subtitle,
            master.clone(),
            0, // fresh spawn — respawn generation starts at 0
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("start play transcode: {e}")))?;

    // Wait for ffmpeg to produce the playlist. 20s is generous even
    // for cold-start on a barely-downloaded torrent.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !playlist_path.exists() {
        if Instant::now() > deadline {
            return Err(AppError::Internal(anyhow::anyhow!(
                "play transcode playlist not ready in time",
            )));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.apple.mpegurl"),
        )],
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
        master,
    )
        .into_response())
}

#[cfg(test)]
mod master_playlist_tests {
    use super::*;
    use crate::playback::{PlaybackMethod, SourceInfo};

    fn sdr_mp4() -> SourceInfo {
        SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            color_transfer: Some("bt709".into()),
            ..Default::default()
        }
    }

    fn hdr10_hevc() -> SourceInfo {
        SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("hevc".into()),
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("HDR10".into()),
            ..Default::default()
        }
    }

    fn hlg_source() -> SourceInfo {
        SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("hevc".into()),
            color_transfer: Some("arib-std-b67".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("HLG".into()),
            ..Default::default()
        }
    }

    #[test]
    fn transcode_always_outputs_sdr_even_from_hdr_source() {
        // Our transcode path tonemaps to BT.709 unconditionally,
        // so the master must signal SDR regardless of source.
        assert_eq!(
            resolve_output_range(&hdr10_hevc(), PlaybackMethod::Transcode),
            OutputRange::Sdr
        );
        assert_eq!(
            resolve_output_range(&hlg_source(), PlaybackMethod::Transcode),
            OutputRange::Sdr
        );
    }

    #[test]
    fn remux_preserves_source_range() {
        assert_eq!(
            resolve_output_range(&sdr_mp4(), PlaybackMethod::Remux),
            OutputRange::Sdr
        );
        assert_eq!(
            resolve_output_range(&hdr10_hevc(), PlaybackMethod::Remux),
            OutputRange::Pq
        );
        assert_eq!(
            resolve_output_range(&hlg_source(), PlaybackMethod::Remux),
            OutputRange::Hlg
        );
    }

    #[test]
    fn direct_play_preserves_source_range() {
        assert_eq!(
            resolve_output_range(&hdr10_hevc(), PlaybackMethod::DirectPlay),
            OutputRange::Pq
        );
    }

    #[test]
    fn missing_color_metadata_defaults_sdr() {
        let bare = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_output_range(&bare, PlaybackMethod::Remux),
            OutputRange::Sdr
        );
    }

    #[test]
    fn hdr_format_fallback_when_color_transfer_missing() {
        // Some containers carry the DV / HDR10 box as metadata
        // without a video-stream color_transfer — hdr_format is
        // the fallback signal.
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("hevc".into()),
            color_transfer: None,
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_output_range(&s, PlaybackMethod::Remux),
            OutputRange::Pq
        );
    }

    fn passthrough_plan() -> crate::playback::PlaybackPlan {
        crate::playback::PlaybackPlan {
            method: PlaybackMethod::Remux,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: None,
            video_bitstream_filter: None,
            audio_passthrough: false,
        }
    }

    #[test]
    fn dv_profile_5_signals_dbap_no_fallback() {
        let s = SourceInfo {
            hdr_format: Some("Dolby Vision Profile 5".into()),
            ..Default::default()
        };
        let sc = resolve_supplemental_codecs(&s, &passthrough_plan())
            .expect("DV source has supplemental codecs");
        assert!(sc.contains("dvh1.05"), "got {sc}");
        assert!(sc.ends_with("/dbap"), "profile 5 has no fallback: {sc}");
    }

    #[test]
    fn dv_profile_81_signals_db1p_hdr10_fallback() {
        let s = SourceInfo {
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
            ..Default::default()
        };
        let sc = resolve_supplemental_codecs(&s, &passthrough_plan()).expect("DV source");
        assert!(sc.contains("dvh1.08"));
        assert!(sc.ends_with("/db1p"));
    }

    #[test]
    fn non_dv_hdr_has_no_supplemental_codecs() {
        // HDR10 + HLG don't need SUPPLEMENTAL-CODECS — the
        // VIDEO-RANGE tag alone is enough. SUPPLEMENTAL-CODECS
        // is specifically for DV-with-fallback signaling.
        let plan = passthrough_plan();
        assert!(resolve_supplemental_codecs(&hdr10_hevc(), &plan).is_none());
        assert!(resolve_supplemental_codecs(&hlg_source(), &plan).is_none());
    }

    #[test]
    fn rpu_strip_drops_supplemental_codecs() {
        // When the plan strips the DV RPU, the output is pure
        // HDR10 — advertising DV would lie to the client.
        let s = SourceInfo {
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
            ..Default::default()
        };
        let mut plan = passthrough_plan();
        plan.video_bitstream_filter = Some("hevc_metadata=remove_dovi=1".into());
        assert!(resolve_supplemental_codecs(&s, &plan).is_none());
    }

    #[test]
    fn master_emits_video_range_tag() {
        let master = build_master_playlist(
            PlayKind::Movie,
            42,
            "&tab=abc",
            OutputRange::Pq,
            None,
            "mp4a.40.2",
        );
        assert!(master.contains("VIDEO-RANGE=PQ"), "got {master}");
        assert!(!master.contains("SUPPLEMENTAL-CODECS"));
    }

    #[test]
    fn master_emits_supplemental_codecs_when_present() {
        let master = build_master_playlist(
            PlayKind::Movie,
            42,
            "",
            OutputRange::Pq,
            Some("dvh1.08.06/db1p"),
            "mp4a.40.2",
        );
        assert!(master.contains("VIDEO-RANGE=PQ"));
        assert!(
            master.contains("SUPPLEMENTAL-CODECS=\"dvh1.08.06/db1p\""),
            "got {master}"
        );
    }

    fn source_with_audio(codec: &str) -> SourceInfo {
        source_with_audio_profile(codec, None)
    }

    fn source_with_audio_profile(codec: &str, profile: Option<&str>) -> SourceInfo {
        SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("h264".into()),
            audio_tracks: vec![crate::playback::decision::AudioCandidate {
                stream_index: 1,
                codec: codec.into(),
                channels: Some(6),
                channel_layout: Some("5.1".into()),
                profile: profile.map(str::to_owned),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn codec_tag_transcode_no_passthrough_is_aac() {
        let s = source_with_audio("eac3");
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Transcode,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: false,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "mp4a.40.2");
    }

    #[test]
    fn codec_tag_transcode_passthrough_eac3_is_ec3() {
        // The headline passthrough case: 5.1 EAC-3 preserved
        // through a video-only re-encode.
        let s = source_with_audio("eac3");
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Transcode,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: true,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "ec-3");
    }

    #[test]
    fn codec_tag_remux_uses_source_codec() {
        let s = source_with_audio("ac3");
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Remux,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: false,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "ac-3");
    }

    #[test]
    fn codec_tag_direct_play_uses_source_codec() {
        let s = source_with_audio("aac");
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::DirectPlay,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: false,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "mp4a.40.2");
    }

    #[test]
    fn codec_tag_unknown_source_codec_falls_back_to_aac() {
        // Defensive — if a future codec slips past the
        // passthrough detection we still emit a valid CODECS.
        let s = source_with_audio("truehd");
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Remux,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: false,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "mp4a.40.2");
    }

    #[test]
    fn codec_tag_dts_passthrough_emits_dtsc() {
        // Apple TV + DTS source + passthrough plan → CODECS
        // string carries `dtsc` so the CAF receiver's strict
        // validator accepts the playlist. Emitting a misleading
        // `mp4a.40.2` would make the receiver try to decode DTS
        // segments as AAC and fail.
        let s = source_with_audio("dts");
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Transcode,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: true,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "dtsc");
    }

    #[test]
    fn codec_tag_dts_hd_ma_passthrough_emits_dtsh() {
        // DTS-HD MA (and DTS-X inside it) get `dtsh` so Apple TV
        // routes to the lossless decoder instead of the DTS Core
        // fallback. ffprobe emits `profile="DTS-HD MA"` for both
        // HD-MA and DTS-X masters; the `dtsh` tag covers both.
        let s = source_with_audio_profile("dts", Some("DTS-HD MA"));
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Transcode,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: true,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "dtsh");
    }

    #[test]
    fn codec_tag_dts_hd_hra_stays_on_dtsc() {
        // HRA is lossy and lives on the core-decoder path — keep
        // `dtsc` so clients don't advertise lossless decode they
        // can't deliver. Only HD-MA flips to `dtsh`.
        let s = source_with_audio_profile("dts", Some("DTS-HD HRA"));
        let plan = crate::playback::PlaybackPlan {
            method: PlaybackMethod::Transcode,
            transcode_reasons: crate::playback::TranscodeReasons::new(),
            selected_audio_stream: Some(1),
            video_bitstream_filter: None,
            audio_passthrough: true,
        };
        assert_eq!(resolve_audio_codec_tag(&s, &plan), "dtsc");
    }
}
