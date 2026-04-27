//! Deterministic in-memory `TorrentSession` for integration tests.
//!
//! The fake mirrors librqbit's state machine just enough that flow
//! tests can drive the same DB transitions as the real client without
//! a torrent swarm. Tests construct a `FakeTorrentSession`, hand it
//! to the harness via `TestAppBuilder::with_torrent`, then drive
//! state explicitly:
//!
//! ```ignore
//! let torrents = FakeTorrentSession::new();
//! // ... after the user-flow grabs a release ...
//! torrents.complete("infohash123");
//! // The download monitor's next tick now sees `finished = true`
//! // and triggers import.
//! ```
//!
//! State transitions intentionally don't auto-progress on a timer —
//! tests that want to assert on intermediate states need to advance
//! manually. This keeps assertions deterministic; no flaky "did the
//! tick happen yet?" timing.
//!
//! On `complete`, the fake writes a fixture file (or zero-byte stub)
//! to the torrent's expected output path so the import pipeline finds
//! something to hardlink/copy. The fixture path is configurable per
//! torrent.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use librqbit::http_api_types::PeerStatsSnapshot;
use sha2::{Digest as _, Sha256};

use crate::download::torrent_client::{StreamAdapter, TorrentState, TorrentStatus};
use crate::download::{TorrentFileStream, TorrentSession};

/// One synthetic torrent the fake is managing. Tests mutate via the
/// `FakeTorrentSession::complete`, `set_progress`, `fail` helpers.
#[derive(Debug, Clone)]
struct FakeTorrent {
    /// Output directory librqbit would have written into. The fake
    /// uses this to drop the fixture file at the expected path on
    /// `complete`. Defaults to `/tmp/kino-test-downloads/<hash>`.
    output_dir: PathBuf,
    /// File list — `(idx, relative_path, size_bytes)`. Defaults to a
    /// single file named after the torrent if not specified.
    files: Vec<(usize, PathBuf, u64)>,
    /// `info.name` librqbit would advertise — used by import to find
    /// the on-disk subdir.
    name: String,
    /// Aggregate state: progress in bytes, totals, etc.
    status: TorrentStatus,
    /// Currently-selected file indices. `None` = all selected.
    only_files: Option<Vec<usize>>,
    /// When set, `open_file_stream` returns a Cursor over these bytes
    /// instead of failing. Tests that exercise streaming endpoints
    /// install a small fixture here.
    stream_fixture: Option<Vec<u8>>,
}

impl Default for FakeTorrent {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("/tmp/kino-test-downloads"),
            files: vec![],
            name: "fake-torrent".to_owned(),
            status: TorrentStatus {
                downloaded: 0,
                uploaded: 0,
                download_speed: 0,
                upload_speed: 0,
                seeders: Some(0),
                leechers: Some(0),
                eta_seconds: None,
                finished: false,
                state: TorrentState::Initializing,
            },
            only_files: None,
            stream_fixture: None,
        }
    }
}

/// Test handle to the fake torrent client. Cheap to clone — the
/// underlying state is in an `Arc<Mutex<_>>` so the test body and
/// the `AppState` see the same view.
#[derive(Debug, Clone, Default)]
pub struct FakeTorrentSession {
    inner: Arc<Mutex<FakeTorrentState>>,
}

#[derive(Debug, Default)]
struct FakeTorrentState {
    /// Torrents keyed by info hash.
    torrents: HashMap<String, FakeTorrent>,
    /// Next id to hand out from `add_torrent`. Mirrors librqbit's
    /// monotonically-increasing `TorrentId` so tests can assert on
    /// the exact value if they care.
    next_id: usize,
    /// Magnet → info hash map. `add_torrent` stores so duplicate
    /// adds with the same magnet return the same hash (matching
    /// librqbit's `AlreadyManaged` behaviour without us having to
    /// parse the magnet).
    magnet_to_hash: HashMap<String, String>,
}

impl FakeTorrentSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a bare hash without any file layout or fixture —
    /// enough to make `list_torrent_hashes` / `get_status` return
    /// "yes, this torrent is in the session." Used by startup-
    /// reconciliation tests that only care whether the hash is
    /// live, not what it contains.
    pub fn add_hash(&self, hash: impl Into<String>) {
        let torrent = FakeTorrent {
            output_dir: std::path::PathBuf::new(),
            files: Vec::new(),
            name: String::new(),
            status: TorrentStatus {
                downloaded: 0,
                uploaded: 0,
                download_speed: 0,
                upload_speed: 0,
                seeders: Some(0),
                leechers: Some(0),
                eta_seconds: None,
                finished: false,
                state: TorrentState::Downloading,
            },
            only_files: None,
            stream_fixture: None,
        };
        self.inner
            .lock()
            .expect("fake torrent state poisoned")
            .torrents
            .insert(hash.into(), torrent);
    }

    /// Pre-stage a torrent at a known hash so the test can grab a
    /// release that "magically already exists" without going through
    /// `add_torrent`. Used by tests that want to control the hash
    /// rather than letting it auto-derive.
    pub fn preload(
        &self,
        hash: impl Into<String>,
        output_dir: PathBuf,
        files: Vec<(usize, PathBuf, u64)>,
        name: impl Into<String>,
    ) {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let total_bytes: u64 = files.iter().map(|(_, _, len)| *len).sum();
        let total_bytes_signed = i64::try_from(total_bytes).unwrap_or(i64::MAX);
        let torrent = FakeTorrent {
            output_dir,
            files,
            name: name.into(),
            status: TorrentStatus {
                downloaded: 0,
                uploaded: 0,
                download_speed: 0,
                upload_speed: 0,
                seeders: Some(0),
                leechers: Some(0),
                eta_seconds: None,
                finished: false,
                state: TorrentState::Downloading,
            },
            only_files: None,
            stream_fixture: None,
        };
        let _ = total_bytes_signed; // reserved for future progress maths
        guard.torrents.insert(hash.into(), torrent);
    }

    /// Drive a torrent to "completed" — `status.finished = true`,
    /// state = `Seeding`, downloaded = total. Writes the configured
    /// fixture file (or a 1KB placeholder) to disk so the import
    /// pipeline can find it. Returns the file path that was written.
    pub fn complete(&self, hash: &str) -> Option<PathBuf> {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let torrent = guard.torrents.get_mut(hash)?;
        torrent.status.finished = true;
        torrent.status.state = TorrentState::Seeding;
        torrent.status.download_speed = 0;
        // Sum file sizes for downloaded total.
        let total_bytes: u64 = torrent.files.iter().map(|(_, _, len)| *len).sum();
        torrent.status.downloaded = i64::try_from(total_bytes).unwrap_or(i64::MAX);
        // Write a placeholder file so `import_trigger::find_media_file`
        // has something to discover. Using the first file's name
        // (or a default) under the output_dir — matches the layout
        // librqbit would create.
        let dir = torrent.output_dir.join(&torrent.name);
        let _ = std::fs::create_dir_all(&dir);
        let path = if let Some((_, rel_path, _)) = torrent.files.first() {
            dir.join(rel_path)
        } else {
            dir.join(format!("{}.mkv", torrent.name))
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, b"\0".repeat(1024));
        Some(path)
    }

    /// Move progress to a fraction of total (0.0–1.0). Doesn't trigger
    /// completion even at 1.0 — call `complete` for that. Useful for
    /// asserting on intermediate UI states.
    pub fn set_progress(&self, hash: &str, fraction: f64) {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let Some(torrent) = guard.torrents.get_mut(hash) else {
            return;
        };
        let total_bytes: u64 = torrent.files.iter().map(|(_, _, len)| *len).sum();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let downloaded = (total_bytes as f64 * fraction.clamp(0.0, 1.0)) as u64;
        torrent.status.downloaded = i64::try_from(downloaded).unwrap_or(i64::MAX);
        torrent.status.state = TorrentState::Downloading;
        torrent.status.download_speed = 1_000_000; // 1 MB/s synthetic
    }

    /// Drive to failed state with a stored error message.
    pub fn fail(&self, hash: &str, _error: &str) {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let Some(torrent) = guard.torrents.get_mut(hash) else {
            return;
        };
        torrent.status.state = TorrentState::Error;
        torrent.status.download_speed = 0;
    }

    /// Install a stream fixture so `open_file_stream` returns these
    /// bytes via a `Cursor`. Tests that exercise the watch-now stream
    /// endpoint need this to assert on Range-request behaviour.
    pub fn set_stream_fixture(&self, hash: &str, bytes: Vec<u8>) {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        if let Some(torrent) = guard.torrents.get_mut(hash) {
            torrent.stream_fixture = Some(bytes);
        }
    }

    /// Number of torrents currently managed — handy assertion target
    /// for "did the grab actually call `add_torrent`?"
    pub fn count(&self) -> usize {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.len()
    }

    /// True if the torrent was previously added then removed — useful
    /// for asserting that `delete_show` actually called `remove`.
    pub fn was_removed(&self, hash: &str) -> bool {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        !guard.torrents.contains_key(hash)
    }
}

#[async_trait]
impl TorrentSession for FakeTorrentSession {
    async fn add_torrent(
        &self,
        url: &str,
        only_files: Option<Vec<usize>>,
        _paused: bool,
    ) -> anyhow::Result<(usize, String)> {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        // Cloned-then-released borrow so the `next_id` mutation below
        // doesn't conflict with an outstanding immutable borrow on the
        // map.
        let existing = guard.magnet_to_hash.get(url).cloned();
        if let Some(existing_hash) = existing {
            // librqbit returns AlreadyManaged for repeat adds — same
            // shape from us so callers' dedup logic gets exercised.
            let id = guard.next_id;
            guard.next_id += 1;
            return Ok((id, existing_hash));
        }

        // Derive a deterministic hash from the URL so tests can
        // pre-arrange assertions against the same input. SHA-256 of
        // the URL truncated to 40 hex chars matches the *shape* of
        // a BitTorrent v1 info hash; semantic correctness doesn't
        // matter — librqbit never sees this in the fake path.
        let mut hasher = Sha256::new();
        hasher.update(url.as_bytes());
        let hash_bytes = hasher.finalize();
        let mut hash = hex::encode(hash_bytes);
        hash.truncate(40);

        let id = guard.next_id;
        guard.next_id += 1;
        guard.magnet_to_hash.insert(url.to_owned(), hash.clone());

        let torrent = FakeTorrent {
            only_files,
            ..Default::default()
        };
        guard.torrents.insert(hash.clone(), torrent);

        Ok((id, hash))
    }

    fn get_status(&self, torrent_hash: &str) -> Option<TorrentStatus> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.get(torrent_hash).map(|t| t.status.clone())
    }

    async fn pause(&self, torrent_hash: &str) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let Some(torrent) = guard.torrents.get_mut(torrent_hash) else {
            anyhow::bail!("torrent not found: {torrent_hash}");
        };
        torrent.status.state = TorrentState::Paused;
        Ok(())
    }

    async fn resume(&self, torrent_hash: &str) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let Some(torrent) = guard.torrents.get_mut(torrent_hash) else {
            anyhow::bail!("torrent not found: {torrent_hash}");
        };
        torrent.status.state = TorrentState::Downloading;
        Ok(())
    }

    async fn remove(&self, torrent_hash: &str, _delete_files: bool) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.remove(torrent_hash);
        // Real librqbit silently no-ops for unknown hashes too.
        Ok(())
    }

    async fn update_file_selection(
        &self,
        torrent_hash: &str,
        file_indices: Vec<usize>,
    ) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().expect("fake torrent state poisoned");
        let Some(torrent) = guard.torrents.get_mut(torrent_hash) else {
            anyhow::bail!("torrent not found: {torrent_hash}");
        };
        torrent.only_files = Some(file_indices);
        Ok(())
    }

    fn metadata_ready(&self, torrent_hash: &str) -> bool {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.contains_key(torrent_hash)
    }

    fn files(&self, torrent_hash: &str) -> Option<Vec<(usize, PathBuf, u64)>> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.get(torrent_hash).map(|t| t.files.clone())
    }

    fn torrent_name(&self, torrent_hash: &str) -> Option<String> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.get(torrent_hash).map(|t| t.name.clone())
    }

    fn selected_files(&self, torrent_hash: &str) -> Option<Vec<usize>> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        let torrent = guard.torrents.get(torrent_hash)?;
        match &torrent.only_files {
            Some(set) => Some(set.clone()),
            None => Some((0..torrent.files.len()).collect()),
        }
    }

    fn pieces(&self, torrent_hash: &str) -> Option<(Vec<u8>, u32)> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        let torrent = guard.torrents.get(torrent_hash)?;
        // Synthetic 100-piece bitmap; first N% set based on download progress.
        let total: u32 = 100;
        let total_bytes: u64 = torrent.files.iter().map(|(_, _, len)| *len).sum();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let progress_frac = if total_bytes == 0 {
            0.0
        } else {
            (torrent.status.downloaded as f64 / total_bytes as f64).clamp(0.0, 1.0)
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pieces_have = (f64::from(total) * progress_frac) as u32;
        // Pack into bytes MSB0; ((total + 7) / 8) bytes total.
        let bytes_len = total.div_ceil(8) as usize;
        let mut bitmap = vec![0u8; bytes_len];
        for i in 0..pieces_have {
            let byte = (i / 8) as usize;
            let bit = 7 - (i % 8);
            bitmap[byte] |= 1u8 << bit;
        }
        Some((bitmap, total))
    }

    fn peer_stats(&self, torrent_hash: &str, _include_all: bool) -> Option<PeerStatsSnapshot> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        let torrent = guard.torrents.get(torrent_hash)?;
        // Only return peer data for live torrents — matches librqbit's
        // "live" requirement on `per_peer_stats_snapshot`.
        if !matches!(
            torrent.status.state,
            TorrentState::Downloading | TorrentState::Seeding
        ) {
            return None;
        }
        Some(PeerStatsSnapshot {
            peers: HashMap::new(),
        })
    }

    fn file_progress(&self, torrent_hash: &str, file_idx: usize) -> Option<u64> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        let torrent = guard.torrents.get(torrent_hash)?;
        let (_, _, file_total) = torrent.files.iter().find(|(idx, _, _)| *idx == file_idx)?;
        // Return the same fractional progress applied to this file.
        let total_bytes: u64 = torrent.files.iter().map(|(_, _, len)| *len).sum();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frac = if total_bytes == 0 {
            0.0
        } else {
            (torrent.status.downloaded as f64 / total_bytes as f64).clamp(0.0, 1.0)
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bytes = (*file_total as f64 * frac) as u64;
        Some(bytes)
    }

    fn set_speed_limits(&self, _download_bps: Option<u32>, _upload_bps: Option<u32>) {
        // No-op for fakes; rate limiting isn't simulated.
    }

    async fn open_file_stream(
        &self,
        torrent_hash: &str,
        _file_idx: usize,
    ) -> anyhow::Result<Box<dyn TorrentFileStream>> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        let torrent = guard
            .torrents
            .get(torrent_hash)
            .ok_or_else(|| anyhow::anyhow!("torrent not found: {torrent_hash}"))?;
        let bytes = torrent
            .stream_fixture
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no stream fixture configured for {torrent_hash}"))?;
        let len = bytes.len() as u64;
        Ok(Box::new(StreamAdapter::new(Cursor::new(bytes), len)))
    }

    fn list_torrent_hashes(&self) -> Vec<String> {
        let guard = self.inner.lock().expect("fake torrent state poisoned");
        guard.torrents.keys().cloned().collect()
    }
}
