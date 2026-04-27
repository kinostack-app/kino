//! Streaming trickplay — per-download background task that grows a
//! trickplay VTT + sprite set as more of the torrent lands on disk.
//!
//! Two-phase lifecycle:
//!
//! **Streaming** (pre-import): reads via the unified `/play/.../direct`
//! HTTP URL and regenerates the sprite set wholesale each tick with
//! `-ss 0 -t coverage` — required because partial MKV containers have
//! their cue index at the end of the file, so ffmpeg input-seek to a
//! non-zero offset is unreliable.
//!
//! **Post-import** (incremental): on import-detection the output dir
//! is *promoted* — renamed from `data/trickplay-stream/{download_id}`
//! to `data/trickplay/{media_id}` so the streaming-era sprites become
//! the library trickplay without a full regen. The task then continues
//! against the filesystem file, sealing one sheet per tick (fast seek,
//! no wasted decode) until every sheet's worth of runtime is written.
//!
//! Claims `trickplay_generated = 3` on the media row during post-import
//! finalising. The library sweep's `WHERE trickplay_generated = 0`
//! query naturally skips us; startup-reconciliation resets 3→0 to
//! recover from process crashes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::events::AppEvent;
use crate::playback::trickplay::{self, Params};
use crate::state::AppState;

/// Minimum growth (in seconds of covered runtime) before we re-run
/// ffmpeg in Streaming mode. Prevents burning CPU on tiny coverage
/// deltas while still updating the VTT often enough that the user
/// sees new thumbs trickle in.
const MIN_GROWTH_SEC: i64 = 30;

/// Safety factor on "estimated covered runtime" to stay inside the
/// window of data likely downloaded. Progress bytes aren't always
/// contiguous from the start — piece distribution scatter + transcoder
/// contention means a naive `bytes_downloaded / total` overstates how
/// far from byte 0 we can read without hitting a hole. When ffmpeg
/// hits a hole its decoder's error-conceal fills with BLACK frames,
/// which then bake into the sprite sheet. 0.5 is aggressive but the
/// post-import pass regenerates each sheet from the complete
/// filesystem file within seconds, so any early black cells get
/// overwritten quickly.
const COVERAGE_SAFETY: f64 = 0.5;

/// Minimum covered runtime before the first generation attempt.
/// Below this, we'd be producing single-frame nonsense. 20s is
/// enough for two thumbnails at the default 10s interval, and gets
/// the first partial sheet onto disk much faster than the previous
/// 60s gate.
const INITIAL_MIN_SEC: i64 = 20;

/// Outer-loop tick interval. 5s keeps the first sheet's partial regen
/// feeling responsive without pounding ffmpeg for sub-frame growth.
const TICK: Duration = Duration::from_secs(5);

/// Hard cap per `ffmpeg` invocation. A stalled HTTP read (dead peers,
/// I/O wedge) would otherwise pin the task forever. Generous — even
/// a 3-hour file's frames at 10s cadence finishes in under a minute
/// on any reasonable host.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(180);

/// Shared manager of streaming trickplay tasks. Stored on `AppState`.
/// Keyed on (`download_id`, `file_idx`) so a season-pack torrent runs
/// one task per episode file rather than collapsing onto a single
/// download-scoped slot.
#[derive(Debug, Clone, Default)]
pub struct StreamTrickplayManager {
    inner: Arc<Mutex<HashMap<(i64, usize), TaskEntry>>>,
}

#[derive(Debug)]
struct TaskEntry {
    cancel: CancellationToken,
    /// Last successfully generated covered-runtime, in seconds. Read
    /// by the HTTP handler so the UI can show "Generating…" past
    /// this point.
    covered_sec: Arc<AtomicI64>,
}

impl StreamTrickplayManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a task for this download if not already running. Caller
    /// provides the expected total duration — used both to bound
    /// the generator and to cut off the loop when covered ≥ total.
    pub async fn ensure_running(
        &self,
        state: &AppState,
        download_id: i64,
        file_idx: usize,
        total_duration_sec: i64,
    ) {
        let key = (download_id, file_idx);
        let mut tasks = self.inner.lock().await;
        if tasks.contains_key(&key) {
            return;
        }
        let cancel = state.cancel.child_token();
        let covered = Arc::new(AtomicI64::new(0));
        tasks.insert(
            key,
            TaskEntry {
                cancel: cancel.clone(),
                covered_sec: covered.clone(),
            },
        );
        drop(tasks);

        tracing::info!(
            download_id,
            file_idx,
            total_duration_sec,
            "stream trickplay task starting",
        );
        let state = state.clone();
        let manager = self.clone();
        tokio::spawn(async move {
            run_loop(
                state.clone(),
                download_id,
                file_idx,
                total_duration_sec,
                cancel,
                covered,
            )
            .await;
            tracing::info!(download_id, file_idx, "stream trickplay task exited");
            // Remove entry on exit so re-preparation can start a
            // fresh task if the viewer comes back later.
            manager.inner.lock().await.remove(&key);
        });
    }

    /// Stop every running task for this download. A season-pack
    /// torrent may have multiple file-scoped tasks running; cancel
    /// reaches all of them. Idempotent.
    pub async fn stop(&self, download_id: i64) {
        let mut tasks = self.inner.lock().await;
        let to_cancel: Vec<TaskEntry> = tasks
            .extract_if(|(d, _), _| *d == download_id)
            .map(|(_, entry)| entry)
            .collect();
        drop(tasks);
        for entry in to_cancel {
            entry.cancel.cancel();
        }
    }

    /// Current covered-runtime snapshot for the UI's "Generating…"
    /// boundary. None when no task is running for this file.
    #[allow(dead_code)]
    pub async fn covered_sec(&self, download_id: i64, file_idx: usize) -> Option<i64> {
        let tasks = self.inner.lock().await;
        tasks
            .get(&(download_id, file_idx))
            .map(|e| e.covered_sec.load(Ordering::Relaxed))
    }
}

/// Which input ffmpeg is reading from this tick. Streaming mode hits
/// the unified HTTP URL (routed through librqbit's piece-priority
/// stream); post-import mode switches to the concrete library file
/// path so incremental seek works without cues-at-end drama.
enum Input {
    Url(String),
    File(PathBuf),
}

impl Input {
    fn as_str(&self) -> &str {
        match self {
            Self::Url(s) => s.as_str(),
            Self::File(p) => p.to_str().unwrap_or_default(),
        }
    }
}

#[allow(clippy::too_many_lines)] // single linear state machine — splitting hurts readability
async fn run_loop(
    state: AppState,
    download_id: i64,
    file_idx: usize,
    total_duration_sec: i64,
    cancel: CancellationToken,
    covered_sec: Arc<AtomicI64>,
) {
    if total_duration_sec <= 0 {
        tracing::debug!(download_id, "unknown duration, skipping stream trickplay");
        return;
    }
    let Some(torrent) = state.torrent.clone() else {
        return;
    };

    let stream_dir = trickplay::trickplay_stream_dir(&state.data_path, download_id);
    let params = load_params(&state.db).await;
    let api_key = api_key(&state.db).await.unwrap_or_default();

    let entity = lookup_entity_for_download(&state.db, download_id).await;
    let Some((kind_seg, entity_id)) = entity else {
        tracing::debug!(
            download_id,
            "no linked entity for download; skipping stream trickplay",
        );
        return;
    };

    // Each sheet holds `tile² × interval` seconds of runtime. For the
    // defaults (tile=10, interval=10s) that's 1000s per sheet, so a
    // 2-hour movie takes ~8 sheets.
    let sheet_span_sec = i64::from(params.tile_size * params.tile_size * params.interval_secs);
    let total_sheets_needed = (total_duration_sec + sheet_span_sec - 1) / sheet_span_sec;

    let mut output_dir = stream_dir.clone();
    let mut input = Input::Url(unified_play_direct_url(
        state.http_port,
        kind_seg,
        entity_id,
        &api_key,
    ));
    let mut phase_post_import: Option<i64> = None; // Some(media_id) once promoted
    let mut last_covered: i64 = 0;
    let mut last_sealed_sheet: i64 = -1; // -1 = none sealed yet
    let _ = file_idx; // kept for signature compatibility; backend picks file by entity

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(TICK) => {}
        }

        // Phase transition: Streaming → PostImport on import. Promote
        // the stream dir to the library location so the streaming
        // sprites become the library trickplay wholesale — no wasted
        // ffmpeg work. Claim `trickplay_generated = 3` so the library
        // sweep's `= 0` query leaves us alone while we incrementally
        // fill in any gaps past what streaming coverage reached.
        if phase_post_import.is_none() && is_imported(&state.db, download_id).await {
            let Some((media_id, file_path)) = lookup_imported_media(&state.db, download_id).await
            else {
                tracing::debug!(
                    download_id,
                    "no media row for imported download; stopping stream trickplay",
                );
                break;
            };
            let library_dir = trickplay::trickplay_dir(&state.data_path, media_id);
            if let Some(parent) = library_dir.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            // If a previous (crashed) sweep left an empty library dir,
            // rename would EEXIST — clear it first.
            let _ = tokio::fs::remove_dir_all(&library_dir).await;
            if let Err(e) = tokio::fs::rename(&stream_dir, &library_dir).await {
                tracing::warn!(download_id, media_id, error = %e, "failed to promote stream trickplay dir; stopping");
                break;
            }
            let _ = sqlx::query("UPDATE media SET trickplay_generated = 3 WHERE id = ?")
                .bind(media_id)
                .execute(&state.db)
                .await;
            output_dir = library_dir;
            input = Input::File(PathBuf::from(file_path));
            phase_post_import = Some(media_id);
            // In post-import we know the full file is there — force
            // the next tick to run even if the coverage delta gate
            // would otherwise veto it.
            last_covered = 0;
            tracing::info!(
                download_id,
                media_id,
                "stream trickplay promoted to library dir",
            );
        }

        // Coverage: streaming estimates from download %, post-import
        // is always full.
        let coverage = if phase_post_import.is_some() {
            total_duration_sec
        } else if let Some(c) = estimated_covered_sec(
            torrent.as_ref(),
            &state.db,
            download_id,
            file_idx,
            total_duration_sec,
        )
        .await
        {
            c
        } else {
            continue;
        };
        if coverage < INITIAL_MIN_SEC {
            continue;
        }

        // Different work per phase. Streaming: wholesale regen with
        // the "notable growth" gate. PostImport: incremental per-sheet
        // — seal the next un-sealed sheet and keep going until every
        // sheet's runtime range is covered.
        if let Some(media_id) = phase_post_import {
            let next_sheet = last_sealed_sheet + 1;
            if next_sheet >= total_sheets_needed {
                // Fully sealed — flip the flag and exit cleanly. The
                // library sweep will ignore this row from now on.
                let _ = sqlx::query("UPDATE media SET trickplay_generated = 1 WHERE id = ?")
                    .bind(media_id)
                    .execute(&state.db)
                    .await;
                covered_sec.store(total_duration_sec, Ordering::Relaxed);
                state.emit(AppEvent::TrickplayStreamUpdated {
                    download_id,
                    covered_sec: total_duration_sec,
                });
                tracing::info!(download_id, media_id, "post-import trickplay finalised");
                break;
            }
            let start = next_sheet.saturating_mul(sheet_span_sec);
            let span = (total_duration_sec - start).min(sheet_span_sec);
            #[allow(clippy::cast_precision_loss)]
            let fut = trickplay::generate_sheet(
                input.as_str(),
                u32::try_from(next_sheet).unwrap_or(0),
                start as f64,
                span as f64,
                &output_dir,
                &params,
            );
            let Ok(result) = tokio::time::timeout(FFMPEG_TIMEOUT, fut).await else {
                tracing::warn!(
                    download_id,
                    sheet = next_sheet,
                    timeout_sec = FFMPEG_TIMEOUT.as_secs(),
                    "post-import sheet ffmpeg timed out; will retry on next tick",
                );
                continue;
            };
            match result {
                Ok(dims) => {
                    last_sealed_sheet = next_sheet;
                    #[allow(
                        clippy::cast_precision_loss,
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss
                    )]
                    let covered_after = (last_sealed_sheet + 1)
                        .saturating_mul(sheet_span_sec)
                        .min(total_duration_sec);
                    #[allow(clippy::cast_precision_loss)]
                    let _ = trickplay::write_vtt(
                        &output_dir,
                        covered_after as f64,
                        &params,
                        u32::try_from(last_sealed_sheet + 1).unwrap_or(0),
                        dims.thumb_w,
                        dims.thumb_h,
                    )
                    .await;
                    covered_sec.store(covered_after, Ordering::Relaxed);
                    state.emit(AppEvent::TrickplayStreamUpdated {
                        download_id,
                        covered_sec: covered_after,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        download_id,
                        sheet = next_sheet,
                        error = %e,
                        "post-import sheet generation failed; will retry on next tick",
                    );
                }
            }
            continue;
        }

        // Streaming mode (pre-import): wholesale regen. We CAN'T do
        // per-sheet incremental here because ffmpeg input-seek on a
        // partial MKV needs the cue index which lives at the end of
        // the file. `-ss 0 -t coverage` sidesteps that by always
        // reading from the start.
        if coverage < last_covered + MIN_GROWTH_SEC && coverage < total_duration_sec {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let duration = coverage.min(total_duration_sec) as f64;
        let fut = trickplay::generate_partial(input.as_str(), duration, &output_dir, &params);
        let Ok(result) = tokio::time::timeout(FFMPEG_TIMEOUT, fut).await else {
            tracing::warn!(
                download_id,
                coverage,
                timeout_sec = FFMPEG_TIMEOUT.as_secs(),
                "stream trickplay ffmpeg timed out; will retry on next tick",
            );
            continue;
        };
        match result {
            Ok(_) => {
                last_covered = coverage;
                covered_sec.store(coverage, Ordering::Relaxed);
                state.emit(AppEvent::TrickplayStreamUpdated {
                    download_id,
                    covered_sec: coverage,
                });
            }
            Err(e) => {
                tracing::warn!(
                    download_id,
                    coverage,
                    error = %e,
                    "stream trickplay generation failed; will retry on next tick",
                );
            }
        }
    }

    // Cleanup on cancel. In Streaming phase we wipe the stream dir
    // (never made it to library). In PostImport phase with an
    // un-finalised claim we reset the flag so a later library sweep
    // can pick up the work.
    if cancel.is_cancelled() {
        if let Some(media_id) = phase_post_import {
            let _ = sqlx::query(
                "UPDATE media SET trickplay_generated = 0 WHERE id = ? AND trickplay_generated = 3",
            )
            .bind(media_id)
            .execute(&state.db)
            .await;
        } else {
            let _ = tokio::fs::remove_dir_all(&stream_dir).await;
        }
    }
}

/// How much runtime we *guess* is safe to sample — the fraction of
/// total bytes downloaded, scaled by duration, minus a safety
/// margin to stay clear of not-yet-downloaded pieces.
async fn estimated_covered_sec(
    torrent: &dyn crate::download::TorrentSession,
    db: &SqlitePool,
    download_id: i64,
    file_idx: usize,
    total_duration_sec: i64,
) -> Option<i64> {
    let hash: Option<String> = sqlx::query_scalar("SELECT torrent_hash FROM download WHERE id = ?")
        .bind(download_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()?;
    let hash = hash?;
    let file_bytes = torrent.file_progress(&hash, file_idx)?;
    let file_total = torrent
        .files(&hash)?
        .into_iter()
        .find(|(idx, _, _)| *idx == file_idx)
        .map(|(_, _, len)| len)?;
    if file_total == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let estimated = (file_bytes as f64 / file_total as f64
        * total_duration_sec as f64
        * COVERAGE_SAFETY) as i64;
    Some(estimated.clamp(0, total_duration_sec))
}

async fn is_imported(db: &SqlitePool, download_id: i64) -> bool {
    // Mirrors the query in stream_info — UNION of movie-linked and
    // episode-linked media rows.
    sqlx::query_scalar::<_, i64>(
        "SELECT m.id FROM media m
         JOIN download_content dc ON m.movie_id = dc.movie_id
         WHERE dc.download_id = ? AND dc.movie_id IS NOT NULL
         UNION ALL
         SELECT m.id FROM media m
         JOIN media_episode me ON me.media_id = m.id
         JOIN download_content dc ON me.episode_id = dc.episode_id
         WHERE dc.download_id = ? AND dc.episode_id IS NOT NULL
         LIMIT 1",
    )
    .bind(download_id)
    .bind(download_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// Internal URL that ffmpeg reads from for the growing partial
/// file. Points at the unified `/direct` endpoint so librqbit's
/// piece-prioritised file stream stays in the loop.
fn unified_play_direct_url(port: u16, kind: &'static str, entity_id: i64, api_key: &str) -> String {
    format!("http://127.0.0.1:{port}/api/v1/play/{kind}/{entity_id}/direct?api_key={api_key}")
}

/// Look up the imported `(media_id, file_path)` for a download, if
/// any. Returns None before import. Used at the Streaming → `PostImport`
/// transition to locate the library file for incremental ffmpeg
/// seeking and to target the right trickplay dir.
async fn lookup_imported_media(db: &SqlitePool, download_id: i64) -> Option<(i64, String)> {
    // Same UNION as `is_imported` — movie-linked OR episode-linked
    // media row. We take the first match; in practice a download
    // imports to exactly one media row.
    sqlx::query_as::<_, (i64, String)>(
        "SELECT m.id, m.file_path FROM media m
         JOIN download_content dc ON m.movie_id = dc.movie_id
         WHERE dc.download_id = ? AND dc.movie_id IS NOT NULL
         UNION ALL
         SELECT m.id, m.file_path FROM media m
         JOIN media_episode me ON me.media_id = m.id
         JOIN download_content dc ON me.episode_id = dc.episode_id
         WHERE dc.download_id = ? AND dc.episode_id IS NOT NULL
         LIMIT 1",
    )
    .bind(download_id)
    .bind(download_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

/// Look up the `(kind, entity_id)` that a download is linked to.
/// Used by the stream-trickplay task to build the unified playback
/// URL for ffmpeg. Returns None when the `download_content` row is
/// missing (e.g. ad-hoc torrents added outside a watch flow).
async fn lookup_entity_for_download(
    db: &SqlitePool,
    download_id: i64,
) -> Option<(&'static str, i64)> {
    let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT movie_id, episode_id FROM download_content WHERE download_id = ? LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let (movie_id, episode_id) = row?;
    if let Some(m) = movie_id {
        return Some(("movie", m));
    }
    episode_id.map(|e| ("episode", e))
}

async fn load_params(db: &SqlitePool) -> Params {
    let ffmpeg: Option<String> = sqlx::query_scalar("SELECT ffmpeg_path FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    Params {
        ffmpeg_path: ffmpeg.clone().unwrap_or_else(|| "ffmpeg".into()),
        ffprobe_path: ffmpeg
            .as_ref()
            .map_or_else(|| "ffprobe".into(), |p| p.replace("ffmpeg", "ffprobe")),
        ..Params::default()
    }
}

async fn api_key(db: &SqlitePool) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// Output directory helper — re-exported so handlers can serve files
/// without depending on `playback::trickplay` directly.
pub fn output_dir(data_path: &Path, download_id: i64) -> PathBuf {
    trickplay::trickplay_stream_dir(data_path, download_id)
}
