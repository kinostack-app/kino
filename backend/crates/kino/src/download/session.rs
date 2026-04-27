//! `TorrentSession` trait — control-plane abstraction over the
//! `BitTorrent` client so tests can swap a deterministic fake in for
//! `LibrqbitClient` without touching call sites.
//!
//! Trait shape mirrors the methods every existing caller uses today.
//! New methods get added here, not as inherent methods on
//! `LibrqbitClient`, so tests stay in lockstep.
//!
//! See `docs/roadmap/31-integration-testing.md` § "Trait seams"
//! for the broader testability story this is part of.

use std::fmt::Debug;
use std::path::PathBuf;

use async_trait::async_trait;
use librqbit::http_api_types::PeerStatsSnapshot;
use tokio::io::{AsyncRead, AsyncSeek};

use crate::download::torrent_client::TorrentStatus;

/// Per-file streamable view returned by [`TorrentSession::open_file_stream`].
///
/// The trait is `AsyncRead + AsyncSeek + Send + Unpin` plus a `len()`
/// method matching the inherent `len()` librqbit's `FileStream` exposes.
/// Lets handlers serve Range requests and progressive responses
/// without caring whether the bytes come from a real torrent file or
/// a test cursor.
pub trait TorrentFileStream: AsyncRead + AsyncSeek + Send + Unpin {
    /// Total file size in bytes. Used by Range-header math + the
    /// `Content-Length` header.
    fn total_len(&self) -> u64;
}

/// Control-plane interface to the torrent client. Production
/// implementation is [`crate::download::torrent_client::LibrqbitClient`];
/// tests use [`crate::test_support::FakeTorrentSession`].
///
/// Most methods are `fn` (sync) because they look up cached state in
/// the client; the few that hit network or modify session config are
/// `async`. Method names match the existing `LibrqbitClient` inherent
/// methods so the call-site refactor is mechanical.
#[async_trait]
pub trait TorrentSession: Send + Sync + Debug {
    /// Add a torrent from a magnet URL or `.torrent` URL. Returns
    /// `(client_id, info_hash)` matching librqbit's response shape.
    async fn add_torrent(
        &self,
        url: &str,
        only_files: Option<Vec<usize>>,
        paused: bool,
    ) -> anyhow::Result<(usize, String)>;

    /// Snapshot of current state. `None` when the torrent isn't
    /// managed (typo, removed, never added).
    fn get_status(&self, torrent_hash: &str) -> Option<TorrentStatus>;

    async fn pause(&self, torrent_hash: &str) -> anyhow::Result<()>;
    async fn resume(&self, torrent_hash: &str) -> anyhow::Result<()>;

    /// Remove the torrent from the session. `delete_files = true`
    /// asks the underlying client to also delete the on-disk files.
    async fn remove(&self, torrent_hash: &str, delete_files: bool) -> anyhow::Result<()>;

    /// Update the "only-files" selection for a multi-file torrent.
    async fn update_file_selection(
        &self,
        torrent_hash: &str,
        file_indices: Vec<usize>,
    ) -> anyhow::Result<()>;

    /// True once the magnet has resolved into full torrent metadata.
    /// While false, `files` / `torrent_name` / streaming all return
    /// `None` / fail.
    fn metadata_ready(&self, torrent_hash: &str) -> bool;

    /// File list as `(idx, relative_path, size_bytes)`. `None` while
    /// metadata is still resolving.
    fn files(&self, torrent_hash: &str) -> Option<Vec<(usize, PathBuf, u64)>>;

    /// The torrent's own `info.name` — the on-disk subdirectory or
    /// filename librqbit uses under the session's base download
    /// folder.
    fn torrent_name(&self, torrent_hash: &str) -> Option<String>;

    /// Currently-selected file indices (for the Files tab's checked
    /// state). `None` while metadata is still resolving.
    fn selected_files(&self, torrent_hash: &str) -> Option<Vec<usize>>;

    /// Per-piece "have" bitmap (MSB0 packed bytes) plus total piece
    /// count. `None` for unknown hashes / unresolved metadata.
    fn pieces(&self, torrent_hash: &str) -> Option<(Vec<u8>, u32)>;

    /// Snapshot of every peer connection. `None` when the torrent
    /// isn't live (paused, errored, finished).
    fn peer_stats(&self, torrent_hash: &str, include_all: bool) -> Option<PeerStatsSnapshot>;

    /// Bytes downloaded for a specific file in a multi-file torrent.
    /// Used by the streaming-trickplay coverage estimator. `None`
    /// when the torrent isn't managed or the file index is invalid.
    fn file_progress(&self, torrent_hash: &str, file_idx: usize) -> Option<u64>;

    /// Mutate session-level rate limits live. No-op for fakes.
    fn set_speed_limits(&self, download_bps: Option<u32>, upload_bps: Option<u32>);

    /// Open a piece-prioritised reader over a file in the torrent.
    /// Used by watch-now streaming. Boxed so the trait is dyn-safe;
    /// the inner type can be librqbit's `FileStream` in production
    /// or a `Cursor<Vec<u8>>` in tests.
    async fn open_file_stream(
        &self,
        torrent_hash: &str,
        file_idx: usize,
    ) -> anyhow::Result<Box<dyn TorrentFileStream>>;

    /// Enumerate every info hash currently managed by the session
    /// as a lowercase hex string. Used by startup reconciliation
    /// to find "ghost torrents" — librqbit entries with no matching
    /// DB `download` row, typically left over from a mid-grab crash
    /// — so they can be removed before they consume peers, disk,
    /// and bandwidth indefinitely.
    fn list_torrent_hashes(&self) -> Vec<String>;
}
