//! Torrent client abstraction wrapping librqbit.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ListenerMode, ListenerOptions,
    ManagedTorrent, Session, SessionOptions, SessionPersistenceConfig, TorrentStats,
    TorrentStatsState,
    http_api_types::{PeerStatsFilter, PeerStatsSnapshot},
};

/// Simplified status snapshot for our download manager.
#[derive(Debug, Clone)]
pub struct TorrentStatus {
    pub downloaded: i64,
    pub uploaded: i64,
    pub download_speed: i64,
    pub upload_speed: i64,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub eta_seconds: Option<i64>,
    pub finished: bool,
    pub state: TorrentState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentState {
    Initializing,
    Downloading,
    Seeding,
    Paused,
    Error,
}

/// Configuration for the librqbit session.
#[derive(Debug, Clone)]
pub struct TorrentClientConfig {
    pub download_path: PathBuf,
    pub data_path: PathBuf,
    pub bind_interface: Option<String>,
    pub listen_port: u16,
    pub announce_port: Option<u16>,
    pub download_speed_limit: Option<u32>,
    pub upload_speed_limit: Option<u32>,
}

/// Wraps a librqbit Session for torrent management.
pub struct LibrqbitClient {
    session: Arc<Session>,
}

impl std::fmt::Debug for LibrqbitClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibrqbitClient")
            .field("session", &"<librqbit::Session>")
            .finish()
    }
}

impl LibrqbitClient {
    /// Create a new librqbit client with the given configuration.
    pub async fn new(config: TorrentClientConfig) -> anyhow::Result<Self> {
        let mut opts = SessionOptions::default();

        // VPN split tunneling: bind all torrent traffic to VPN interface
        if let Some(ref iface) = config.bind_interface {
            opts.bind_device_name = Some(iface.clone());
        }

        // Persistence for resume across restarts
        let persistence_folder = config.data_path.join("librqbit");
        opts.persistence = Some(SessionPersistenceConfig::Json {
            folder: Some(persistence_folder),
        });
        opts.fastresume = true;

        // Listener config
        opts.listen = Some(ListenerOptions {
            mode: ListenerMode::TcpAndUtp,
            listen_addr: format!("0.0.0.0:{}", config.listen_port)
                .parse()
                .expect("valid listen addr"),
            enable_upnp_port_forwarding: false, // We handle port forwarding ourselves
            announce_port: config.announce_port,
            ..Default::default()
        });

        // Rate limits
        opts.ratelimits = librqbit::limits::LimitsConfig {
            download_bps: config.download_speed_limit.and_then(NonZeroU32::new),
            upload_bps: config.upload_speed_limit.and_then(NonZeroU32::new),
        };

        let session = Session::new_with_opts(config.download_path, opts).await?;

        Ok(Self { session })
    }

    /// Add a torrent from a magnet URL or .torrent URL.
    pub async fn add_torrent(
        &self,
        url: &str,
        only_files: Option<Vec<usize>>,
        paused: bool,
    ) -> anyhow::Result<(usize, String)> {
        // `overwrite: true` is librqbit's resume primitive, not a
        // blind clobber: it checksums any bytes already on disk and
        // treats matching pieces as downloaded. Without it, a torrent
        // can't be re-added if any single file exists — which fires
        // after `just reset` wipes the DB but leaves the downloads
        // dir, or when two torrents share an identical scene file
        // (YG releases bundle `Torrent Downloaded From/*.txt` that
        // collides across every grab from that uploader).
        let add_opts = AddTorrentOptions {
            only_files: only_files.map(|f| f.into_iter().collect()),
            paused,
            overwrite: true,
            ..Default::default()
        };

        let response = self
            .session
            .add_torrent(AddTorrent::from_url(url), Some(add_opts))
            .await?;

        match response {
            AddTorrentResponse::Added(id, handle)
            | AddTorrentResponse::AlreadyManaged(id, handle) => {
                let hash = handle.info_hash().as_string();
                Ok((id, hash))
            }
            AddTorrentResponse::ListOnly(_) => {
                anyhow::bail!("torrent added in list-only mode")
            }
        }
    }

    /// Get the status of a torrent by its info hash.
    pub fn get_status(&self, torrent_hash: &str) -> Option<TorrentStatus> {
        let handle = self.find_by_hash(torrent_hash)?;
        let stats = handle.stats();
        Some(convert_stats(&stats))
    }

    /// Pause a torrent.
    pub async fn pause(&self, torrent_hash: &str) -> anyhow::Result<()> {
        let handle = self
            .find_by_hash(torrent_hash)
            .ok_or_else(|| anyhow::anyhow!("torrent not found: {torrent_hash}"))?;
        self.session.pause(&handle).await
    }

    /// Resume a paused torrent.
    pub async fn resume(&self, torrent_hash: &str) -> anyhow::Result<()> {
        let handle = self
            .find_by_hash(torrent_hash)
            .ok_or_else(|| anyhow::anyhow!("torrent not found: {torrent_hash}"))?;
        self.session.unpause(&handle).await
    }

    /// Remove a torrent and optionally delete its files.
    pub async fn remove(&self, torrent_hash: &str, delete_files: bool) -> anyhow::Result<()> {
        let handle = self
            .find_by_hash(torrent_hash)
            .ok_or_else(|| anyhow::anyhow!("torrent not found: {torrent_hash}"))?;
        let id = librqbit::api::TorrentIdOrHash::Hash(handle.info_hash());
        self.session.delete(id, delete_files).await
    }

    /// Update file selection for an active torrent.
    pub async fn update_file_selection(
        &self,
        torrent_hash: &str,
        file_indices: Vec<usize>,
    ) -> anyhow::Result<()> {
        let handle = self
            .find_by_hash(torrent_hash)
            .ok_or_else(|| anyhow::anyhow!("torrent not found: {torrent_hash}"))?;
        let set: HashSet<usize> = file_indices.into_iter().collect();
        self.session.update_only_files(&handle, &set).await
    }

    /// Update rate limits dynamically.
    pub fn set_speed_limits(&self, download_bps: Option<u32>, upload_bps: Option<u32>) {
        self.session
            .ratelimits
            .set_download_bps(download_bps.and_then(NonZeroU32::new));
        self.session
            .ratelimits
            .set_upload_bps(upload_bps.and_then(NonZeroU32::new));
    }

    fn find_by_hash(&self, hash: &str) -> Option<Arc<ManagedTorrent>> {
        let id_or_hash = librqbit::api::TorrentIdOrHash::try_from(hash).ok()?;
        self.session.get(id_or_hash)
    }

    // ── Watch-now streaming support ───────────────────────────────
    //
    // The Watch-now feature needs to reach into librqbit to open a
    // piece-prioritized file stream. We expose the ManagedTorrent
    // handle and a couple of narrow view accessors — the stream
    // handler calls `handle.stream(file_idx).await?` inline because
    // librqbit's `FileStream` type isn't re-exported so we can't name
    // it in a function signature.

    /// Return the raw torrent handle for the given info hash, if the
    /// session is currently managing it. The caller is responsible
    /// for invoking `.stream(file_idx)` — we can't wrap the result
    /// because `FileStream` is not publicly named by librqbit.
    pub fn handle(&self, torrent_hash: &str) -> Option<Arc<ManagedTorrent>> {
        self.find_by_hash(torrent_hash)
    }

    /// True once the magnet has resolved into full torrent metadata
    /// (file list, piece layout). While this returns false, the
    /// client is still fetching the info-dict from peers and the
    /// Watch-now "prepare" endpoint should return 202.
    pub fn metadata_ready(&self, torrent_hash: &str) -> bool {
        let Some(handle) = self.find_by_hash(torrent_hash) else {
            return false;
        };
        handle.with_metadata(|_| ()).is_ok()
    }

    /// List files in the torrent as (idx, `relative_path`, `size_bytes`).
    /// Returns `None` while metadata is still resolving — pair with
    /// `metadata_ready` for a consistent view.
    pub fn files(&self, torrent_hash: &str) -> Option<Vec<(usize, PathBuf, u64)>> {
        let handle = self.find_by_hash(torrent_hash)?;
        handle
            .with_metadata(|m| {
                m.file_infos
                    .iter()
                    .enumerate()
                    .map(|(i, fi)| (i, fi.relative_filename.clone(), fi.len))
                    .collect::<Vec<_>>()
            })
            .ok()
    }

    /// The torrent's own `info.name` — what librqbit uses as the
    /// subdirectory name (multi-file torrents) or filename (single-
    /// file torrents) under the session's base download folder.
    /// Safer for import than guessing from our stored `download.title`
    /// (which is often the cleaned release title, not the on-disk
    /// name that includes site prefixes etc.).
    pub fn torrent_name(&self, torrent_hash: &str) -> Option<String> {
        let handle = self.find_by_hash(torrent_hash)?;
        handle.name()
    }

    /// Snapshot of every peer currently connected (or recently
    /// disconnected) for this torrent. Returns `None` when the
    /// torrent is paused / not live (no peer connections to report).
    /// The Detail pane's Peers tab renders this list.
    pub fn peer_stats(&self, torrent_hash: &str, include_all: bool) -> Option<PeerStatsSnapshot> {
        let handle = self.find_by_hash(torrent_hash)?;
        let live = handle.live()?;
        let filter = if include_all {
            // Accept "all" via serde to bypass needing PeerStatsFilterState
            // in scope — the default is "live" only.
            serde_json::from_str(r#"{"state":"all"}"#).unwrap_or_default()
        } else {
            PeerStatsFilter::default()
        };
        Some(live.per_peer_stats_snapshot(filter))
    }

    /// Per-piece "have" bitmap — one bit per piece in MSB0 order,
    /// packed into bytes. Paired with `total_pieces` so the UI can
    /// render a progress canvas even when `bitmap.len() * 8` is
    /// greater than `total_pieces` (the trailing bits are padding).
    /// Returns `None` while metadata is still resolving or for
    /// unknown hashes.
    pub fn pieces(&self, torrent_hash: &str) -> Option<(Vec<u8>, u32)> {
        let handle = self.find_by_hash(torrent_hash)?;
        let info_hash = handle.info_hash();
        // Api is a thin wrapper (session + optional log sender), cheap
        // to construct per-call. Lets us reach `api_dump_haves` which
        // is the only public surface that exposes the chunk-tracker's
        // bitmap — `ManagedTorrent::with_chunk_tracker` is pub(crate).
        let api = librqbit::Api::new(Arc::clone(&self.session), None);
        let (bf, total) = api
            .api_dump_haves(librqbit::api::TorrentIdOrHash::Hash(info_hash))
            .ok()?;
        // BitBox<u8, Msb0> → raw bytes via as_raw_slice(). Clone since
        // the bit-box owns the allocation and we need a Send response.
        Some((bf.as_raw_slice().to_vec(), total))
    }

    /// Currently-selected file indices for a multi-file torrent. Used
    /// to render checked state in the Files tab — the user can toggle
    /// which files to download via `update_file_selection`.
    pub fn selected_files(&self, torrent_hash: &str) -> Option<Vec<usize>> {
        let handle = self.find_by_hash(torrent_hash)?;
        // `only_files` returns `None` when all files are selected (the
        // torrent was added without a filter). Normalize that to "all
        // indices" so the UI always has a concrete set.
        if let Some(set) = handle.only_files() {
            let mut v: Vec<usize> = set.into_iter().collect();
            v.sort_unstable();
            Some(v)
        } else {
            let total = handle.with_metadata(|m| m.file_infos.len()).ok()?;
            Some((0..total).collect())
        }
    }

    /// Bytes downloaded for a specific file in a multi-file torrent.
    /// Used by the streaming-trickplay coverage estimator. Returns
    /// `None` when the torrent isn't managed or the file index is
    /// out of range.
    pub fn file_progress(&self, torrent_hash: &str, file_idx: usize) -> Option<u64> {
        let handle = self.find_by_hash(torrent_hash)?;
        handle.stats().file_progress.get(file_idx).copied()
    }

    /// Enumerate every currently-managed torrent's info hash as a
    /// lowercase hex string. Used by startup reconciliation to
    /// identify "ghost torrents" — ones librqbit restored from its
    /// persistence but that have no matching DB `download` row.
    pub fn list_torrent_hashes(&self) -> Vec<String> {
        self.session
            .with_torrents(|iter| iter.map(|(_, t)| t.info_hash().as_string()).collect())
    }
}

// ── TorrentSession trait impl ─────────────────────────────────────
//
// Production code now reaches the torrent client through the
// `TorrentSession` trait so tests can substitute `FakeTorrentSession`.
// Each method here is a thin pass-through to the inherent method
// already defined on `LibrqbitClient` — the trait shape was designed
// to match those signatures.

#[async_trait::async_trait]
impl crate::download::TorrentSession for LibrqbitClient {
    async fn add_torrent(
        &self,
        url: &str,
        only_files: Option<Vec<usize>>,
        paused: bool,
    ) -> anyhow::Result<(usize, String)> {
        LibrqbitClient::add_torrent(self, url, only_files, paused).await
    }

    fn get_status(&self, torrent_hash: &str) -> Option<TorrentStatus> {
        LibrqbitClient::get_status(self, torrent_hash)
    }

    async fn pause(&self, torrent_hash: &str) -> anyhow::Result<()> {
        LibrqbitClient::pause(self, torrent_hash).await
    }

    async fn resume(&self, torrent_hash: &str) -> anyhow::Result<()> {
        LibrqbitClient::resume(self, torrent_hash).await
    }

    async fn remove(&self, torrent_hash: &str, delete_files: bool) -> anyhow::Result<()> {
        LibrqbitClient::remove(self, torrent_hash, delete_files).await
    }

    async fn update_file_selection(
        &self,
        torrent_hash: &str,
        file_indices: Vec<usize>,
    ) -> anyhow::Result<()> {
        LibrqbitClient::update_file_selection(self, torrent_hash, file_indices).await
    }

    fn metadata_ready(&self, torrent_hash: &str) -> bool {
        LibrqbitClient::metadata_ready(self, torrent_hash)
    }

    fn files(&self, torrent_hash: &str) -> Option<Vec<(usize, std::path::PathBuf, u64)>> {
        LibrqbitClient::files(self, torrent_hash)
    }

    fn torrent_name(&self, torrent_hash: &str) -> Option<String> {
        LibrqbitClient::torrent_name(self, torrent_hash)
    }

    fn selected_files(&self, torrent_hash: &str) -> Option<Vec<usize>> {
        LibrqbitClient::selected_files(self, torrent_hash)
    }

    fn pieces(&self, torrent_hash: &str) -> Option<(Vec<u8>, u32)> {
        LibrqbitClient::pieces(self, torrent_hash)
    }

    fn peer_stats(
        &self,
        torrent_hash: &str,
        include_all: bool,
    ) -> Option<librqbit::http_api_types::PeerStatsSnapshot> {
        LibrqbitClient::peer_stats(self, torrent_hash, include_all)
    }

    fn file_progress(&self, torrent_hash: &str, file_idx: usize) -> Option<u64> {
        LibrqbitClient::file_progress(self, torrent_hash, file_idx)
    }

    fn set_speed_limits(&self, download_bps: Option<u32>, upload_bps: Option<u32>) {
        LibrqbitClient::set_speed_limits(self, download_bps, upload_bps);
    }

    async fn open_file_stream(
        &self,
        torrent_hash: &str,
        file_idx: usize,
    ) -> anyhow::Result<Box<dyn crate::download::TorrentFileStream>> {
        let handle = self
            .find_by_hash(torrent_hash)
            .ok_or_else(|| anyhow::anyhow!("torrent not managed: {torrent_hash}"))?;
        let file_stream = handle.stream(file_idx).await?;
        let len = file_stream.len();
        // librqbit's `FileStream` type isn't re-exported (see
        // `pub use torrent_state::{...}` in librqbit/lib.rs — it's not
        // in the list). We sidestep having to name the type by using a
        // generic adapter and letting type inference fill in `S`.
        Ok(Box::new(StreamAdapter {
            inner: file_stream,
            len,
        }))
    }

    fn list_torrent_hashes(&self) -> Vec<String> {
        LibrqbitClient::list_torrent_hashes(self)
    }
}

/// Generic adapter wrapping any `AsyncRead + AsyncSeek` source so it
/// satisfies our [`crate::download::TorrentFileStream`] trait. Used
/// here for librqbit's anonymous `FileStream`; tests use the same
/// adapter over `std::io::Cursor` for fixture playback.
#[derive(Debug)]
pub struct StreamAdapter<S> {
    inner: S,
    len: u64,
}

impl<S> StreamAdapter<S> {
    pub fn new(inner: S, len: u64) -> Self {
        Self { inner, len }
    }
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for StreamAdapter<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncSeek + Unpin> tokio::io::AsyncSeek for StreamAdapter<S> {
    fn start_seek(
        mut self: std::pin::Pin<&mut Self>,
        position: std::io::SeekFrom,
    ) -> std::io::Result<()> {
        std::pin::Pin::new(&mut self.inner).start_seek(position)
    }

    fn poll_complete(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<u64>> {
        std::pin::Pin::new(&mut self.inner).poll_complete(cx)
    }
}

impl<S> crate::download::TorrentFileStream for StreamAdapter<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncSeek + Send + Unpin + 'static,
{
    fn total_len(&self) -> u64 {
        self.len
    }
}

#[allow(clippy::cast_possible_wrap)]
fn convert_stats(stats: &TorrentStats) -> TorrentStatus {
    let (download_speed, upload_speed, eta, live_peers, seen_peers) =
        if let Some(ref live) = stats.live {
            let dl = live.download_speed.as_bytes() as i64;
            let ul = live.upload_speed.as_bytes() as i64;
            // Compute ETA from remaining bytes / speed
            let remaining = stats.total_bytes.saturating_sub(stats.progress_bytes);
            let eta = (dl > 0).then(|| (remaining / live.download_speed.as_bytes().max(1)) as i64);
            // Peer counts from librqbit's AggregatePeerStats. `live`
            // is connected peers (swarm activity); `seen` is every
            // peer ever discovered this session. librqbit doesn't
            // split seeder vs leecher at the aggregate level (would
            // need per-peer bitfield inspection), so we expose what
            // it does give us: connected + known.
            (
                dl,
                ul,
                eta,
                Some(i64::from(live.snapshot.peer_stats.live)),
                Some(i64::from(live.snapshot.peer_stats.seen)),
            )
        } else {
            (0, 0, None, None, None)
        };

    let torrent_state = match stats.state {
        TorrentStatsState::Initializing => TorrentState::Initializing,
        TorrentStatsState::Live => {
            if stats.finished {
                TorrentState::Seeding
            } else {
                TorrentState::Downloading
            }
        }
        TorrentStatsState::Paused => TorrentState::Paused,
        TorrentStatsState::Error => TorrentState::Error,
    };

    TorrentStatus {
        downloaded: stats.progress_bytes as i64,
        uploaded: stats.uploaded_bytes as i64,
        download_speed,
        upload_speed,
        // Populate `seeders` with the count of currently-connected
        // peers — it's the field the UI has plumbed through. We
        // carry `seen - live` in `leechers` as "known but not
        // currently connected" so the UI can show both numbers.
        seeders: live_peers,
        leechers: seen_peers
            .zip(live_peers)
            .map(|(seen, live)| (seen - live).max(0)),
        eta_seconds: eta,
        finished: stats.finished,
        state: torrent_state,
    }
}
