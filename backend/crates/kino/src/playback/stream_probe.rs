//! Probe cache for in-progress torrent downloads.
//!
//! The library path runs a full ffprobe at import time and writes
//! the result into the DB's `stream` table. The streaming path
//! (watch-while-downloading) had no equivalent — the decision
//! engine planned with empty `SourceInfo`, so it had no idea
//! Fellowship was HEVC + 10-bit + HDR10 + DV + `TrueHD` + Atmos.
//! The downstream effects were visible:
//!
//! * The info chip showed "No probe data yet" with multi-GB of the
//!   file on disk (MKV headers fit in the first few MB).
//! * The decision engine couldn't flag `VideoRangeTypeNotSupported`,
//!   so no tonemap stage was emitted and HDR sources rendered with
//!   washed-out colours.
//! * The track picker was empty — user couldn't select an audio
//!   language or subtitle track mid-stream.
//!
//! This cache closes the gap by running ffprobe lazily against
//! the partial file once enough bytes have landed. The result is
//! held in memory keyed on `download_id` and fired out as an
//! `AppEvent::StreamProbeReady` so the frontend's `/prepare`
//! query refreshes + the info chip lights up.
//!
//! # State machine (per `download_id`)
//!
//! `Pending` → `Ready(Arc<ProbeResult>)` on successful probe
//! `Pending` → `Failed { cooldown }` on probe error — e.g. not
//!    enough bytes yet, ffprobe crash, etc. After the cooldown
//!    the entry can re-probe.
//!
//! Concurrent callers share the probe run via a per-entry Mutex —
//! first caller runs, subsequent callers wait then see `Ready`.
//! No double-probing.
//!
//! # Size gate
//!
//! Probing before the MKV header has fully landed gives partial
//! or incorrect output (sometimes ffprobe reads past the edge and
//! stalls). We gate on `downloaded_bytes >= 5 MB` — MKV headers
//! typically fit in <1 MB, 5 MB is comfortable headroom.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock, broadcast};

use crate::events::AppEvent;
use crate::import::ffprobe::{ProbeError, ProbeResult, probe};

/// Minimum bytes downloaded before we attempt a probe. MKV
/// headers usually fit in the first ~1 MB; 5 MB is a
/// comfortable margin that avoids "ffprobe can't find EOF"
/// retries on a just-started download.
const MIN_BYTES_FOR_PROBE: i64 = 5_000_000;

/// Cooldown after a failed probe before we try again. Re-probes
/// at the end of a 30 s window so if the failure was "not enough
/// bytes yet", we pick up again once more data has landed.
const FAILURE_COOLDOWN: Duration = Duration::from_secs(30);

/// Cache map keyed on (`download_id`, `file_idx`).
type EntriesMap = Arc<RwLock<HashMap<(i64, usize), Arc<Entry>>>>;

#[derive(Debug, Clone)]
pub struct StreamProbeCache {
    /// One entry per (`download_id`, `file_idx`). A season-pack
    /// torrent has multiple files (one per episode), each needing
    /// its own probe — keying on `download_id` alone would have one
    /// file's probe shadow every other. Entries stay in memory
    /// until `forget_download` is called (download completed /
    /// cancelled / imported).
    entries: EntriesMap,
    /// ffprobe binary path — read once at spawn time. Held in
    /// an Arc so a future "ffmpeg bundle download" pathway that
    /// also updates ffprobe stays coherent; for now the field is
    /// static for a given cache instance.
    ffprobe_path: Arc<String>,
    /// Fan-out channel for `AppEvent::StreamProbeReady` so other
    /// tabs / the frontend invalidate their `/prepare` cache on
    /// probe completion. None in tests.
    event_tx: Option<broadcast::Sender<AppEvent>>,
}

#[derive(Debug)]
struct Entry {
    /// Per-entry lock. First caller to acquire this runs the
    /// probe; concurrent callers wait here and observe `Ready`
    /// when the mutex releases.
    lock: Mutex<EntryState>,
}

#[derive(Debug, Clone)]
enum EntryState {
    /// No probe attempted yet, or last one failed past the
    /// cooldown. The next caller to hit the entry runs one.
    Pending,
    /// Probe completed successfully. `Arc` so concurrent readers
    /// share the payload without cloning the full tree.
    Ready(Arc<ProbeResult>),
    /// Probe failed; eligible for retry after `at + FAILURE_COOLDOWN`.
    /// `error` kept for the `Debug` impl so traces show *why* the
    /// last attempt failed, but we don't surface it to callers —
    /// the cooldown is the only user-visible effect.
    Failed {
        #[allow(dead_code)]
        error: String,
        at: Instant,
    },
}

impl StreamProbeCache {
    #[must_use]
    pub fn new(ffprobe_path: &str, event_tx: Option<broadcast::Sender<AppEvent>>) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            ffprobe_path: Arc::new(ffprobe_path.to_owned()),
            event_tx,
        }
    }

    /// Return the cached probe result for this download, running
    /// a probe if we have enough bytes and no valid cache entry
    /// yet. `None` when the download doesn't have enough bytes
    /// for a reliable probe, or when a recent probe failed and
    /// we're in the cooldown window.
    ///
    /// Concurrent callers with the same `download_id` dedupe on
    /// a per-entry mutex — first one runs the probe, the rest
    /// wait a few hundred ms and see the cached Ready state.
    pub async fn get_or_probe(
        &self,
        download_id: i64,
        file_idx: usize,
        file_path: &Path,
        downloaded_bytes: i64,
    ) -> Option<Arc<ProbeResult>> {
        let entry = self.entry_for(download_id, file_idx).await;
        let mut state = entry.lock.lock().await;

        // Fast paths for already-cached states.
        if let EntryState::Ready(r) = &*state {
            return Some(r.clone());
        }
        if let EntryState::Failed { at, .. } = &*state
            && at.elapsed() < FAILURE_COOLDOWN
        {
            return None;
        }

        // Gate on bytes — before the header lands, ffprobe
        // either hangs or returns nonsense.
        if downloaded_bytes < MIN_BYTES_FOR_PROBE {
            return None;
        }

        // Run the probe on a blocking thread — ffprobe is a
        // synchronous subprocess + stdout read, shouldn't
        // starve the tokio runtime. Typically < 200 ms on a
        // local file.
        let ffprobe_path = self.ffprobe_path.clone();
        let file_path = file_path.to_owned();
        let probe_result: Result<ProbeResult, ProbeError> =
            tokio::task::spawn_blocking(move || probe(&file_path, &ffprobe_path))
                .await
                .map_err(|e| ProbeError::Exec(e.to_string()))
                .and_then(|r| r);

        match probe_result {
            Ok(r) => {
                let arc = Arc::new(r);
                *state = EntryState::Ready(arc.clone());
                drop(state);
                tracing::info!(
                    download_id,
                    streams = arc.streams.as_ref().map_or(0, Vec::len),
                    "stream probe completed",
                );
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(AppEvent::StreamProbeReady { download_id });
                }
                Some(arc)
            }
            Err(e) => {
                tracing::warn!(download_id, error = %e, "stream probe failed; will retry after cooldown");
                *state = EntryState::Failed {
                    error: e.to_string(),
                    at: Instant::now(),
                };
                None
            }
        }
    }

    /// Peek at the cache without triggering a probe. Useful for
    /// the `/prepare` hot path where a probe-miss should return
    /// instantly rather than blocking on ffprobe.
    pub async fn peek(&self, download_id: i64, file_idx: usize) -> Option<Arc<ProbeResult>> {
        let entries = self.entries.read().await;
        let entry = entries.get(&(download_id, file_idx))?;
        let state = entry.lock.lock().await;
        if let EntryState::Ready(r) = &*state {
            Some(r.clone())
        } else {
            None
        }
    }

    /// Drop every cache entry for a download (all files within it).
    /// Called when the torrent finishes importing (library takes
    /// over) or is cancelled (no one will ask about it again).
    pub async fn forget_download(&self, download_id: i64) {
        self.entries
            .write()
            .await
            .retain(|(d, _), _| *d != download_id);
    }

    async fn entry_for(&self, download_id: i64, file_idx: usize) -> Arc<Entry> {
        let key = (download_id, file_idx);
        // Try read-path first — the common case is "entry exists,
        // probably Ready". Upgrade to write-path only when we
        // need to insert.
        {
            let entries = self.entries.read().await;
            if let Some(e) = entries.get(&key) {
                return Arc::clone(e);
            }
        }
        let mut entries = self.entries.write().await;
        Arc::clone(entries.entry(key).or_insert_with(|| {
            Arc::new(Entry {
                lock: Mutex::new(EntryState::Pending),
            })
        }))
    }
}
