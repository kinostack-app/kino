use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Semaphore, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, SystemClock};
use crate::download::TorrentSession;
use crate::download::torrent_client::LibrqbitClient;
use crate::download::vpn::VpnManager;
use crate::events::AppEvent;
use crate::images::ImageCache;
use crate::indexers::cloudflare::CloudflareSolver;
use crate::indexers::loader::DefinitionLoader;
use crate::integrations::trakt::scrobble::ScrobbleManager;
use crate::observability::LogRecord;
use crate::playback::transcode::TranscodeManager;
use crate::playback::trickplay_stream::StreamTrickplayManager;
use crate::scheduler::Scheduler;
use crate::tmdb::TmdbClient;

/// Shared application state available to all handlers and background tasks.
#[derive(Debug, Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub event_tx: broadcast::Sender<AppEvent>,
    /// `TmdbClient` held behind an `Arc<RwLock>` so the cached
    /// instance can be hot-swapped when the user rotates the TMDB
    /// API key in Settings / the wizard. Without this, saving a key
    /// on a fresh install left every TMDB-using endpoint stuck at
    /// "not configured" until the next process restart — homepage
    /// blank, library scans no-op, etc. `parking_lot::RwLock` is
    /// chosen over `tokio::sync::RwLock` because the read path is
    /// hot (every TMDB-backed endpoint takes the lock), reads are
    /// trivial (clone an `Arc`-y `TmdbClient`), and we never hold
    /// the guard across `await`. Updates go through
    /// `AppState::set_tmdb`.
    pub tmdb: Arc<parking_lot::RwLock<Option<TmdbClient>>>,
    pub images: Option<ImageCache>,
    pub scheduler: Option<Scheduler>,
    pub cancel: CancellationToken,
    pub trigger_tx: mpsc::Sender<crate::scheduler::TaskTrigger>,
    pub torrent: Option<Arc<dyn TorrentSession>>,
    pub transcode: Option<TranscodeManager>,
    pub definitions: Option<Arc<DefinitionLoader>>,
    pub cf_solver: Option<Arc<CloudflareSolver>>,
    pub vpn: Option<Arc<VpnManager>>,
    /// Root on-disk data path (DB, images, trickplay, session state).
    /// Needed by ephemeral sub-systems like streaming trickplay that
    /// own their own output directory lifecycle.
    pub data_path: std::path::PathBuf,
    /// HTTP port the server is bound to. Used for internal
    /// self-requests (e.g. `FFmpeg` streaming trickplay runs over the
    /// existing `/stream/{id}/{file_idx}` endpoint so librqbit's
    /// piece-priority logic owns disk reads).
    pub http_port: u16,
    /// Manager for per-download streaming trickplay tasks. Started
    /// when a watch-now session prepares; stopped on import / cancel.
    pub stream_trickplay: StreamTrickplayManager,
    /// Trakt scrobble session manager. Cheap to clone; holds per-media
    /// state for the "emit /scrobble/start on first tick, /stop on
    /// watched threshold" flow. No-op when Trakt isn't connected.
    pub scrobble: ScrobbleManager,
    /// Broadcast channel for live-tail log subscribers. Lossy — slow
    /// consumers lag. The `SQLite` log writer persists the canonical copy.
    pub log_live: broadcast::Sender<LogRecord>,
    /// Shared semaphore capping CPU-heavy FFmpeg-driven background
    /// work (trickplay generation + intro/credits analysis) at a few
    /// concurrent runs. Playback transcoding has its own separate
    /// budget so it always takes priority.
    pub media_processing_sem: Arc<Semaphore>,
    /// Wall-clock abstraction. Production uses `SystemClock`; tests
    /// install a `MockClock` so backoff windows, scheduler-tick
    /// eligibility, and token-expiry checks can be advanced
    /// deterministically without `tokio::time::sleep`. Call sites
    /// that read this for *observable* behaviour go through
    /// `state.clock.now()`; call sites that only need a log
    /// timestamp can keep `chrono::Utc::now()`.
    pub clock: Arc<dyn Clock>,
    /// Serializes the watch-now handler so two fast clicks on the
    /// same Play button don't both pass the `find_active_download`
    /// dedup check (neither seeing the other's uncommitted INSERT)
    /// and both create a download row for the same release. The
    /// critical section is small — a handful of SELECTs + a single
    /// INSERT per call — so holding a process-wide mutex across it
    /// is fine for a single-user app. Not a substitute for a real
    /// DB constraint if we ever go multi-instance.
    pub watch_now_lock: Arc<tokio::sync::Mutex<()>>,
    /// Tracker for the user-initiated jellyfin-ffmpeg download.
    /// One download may be in flight at a time; a second
    /// concurrent call yields `AlreadyRunning`. State flows out
    /// via `FfmpegDownloadProgress` / Completed / Failed
    /// broadcasts on `event_tx`; the `GET
    /// /api/v1/playback/ffmpeg/download` endpoint returns the
    /// snapshot for late-joining clients.
    pub ffmpeg_download: crate::playback::ffmpeg_bundle::FfmpegDownloadTracker,
    /// Tracker for the user-initiated + scheduler-initiated
    /// Cardigann definitions refresh. Mirrors `ffmpeg_download` —
    /// one refresh in flight at a time, state via `IndexerDefinitionsRefresh*`
    /// broadcasts on `event_tx`, snapshot at
    /// `GET /api/v1/indexer-definitions/refresh`.
    pub definitions_refresh: crate::indexers::refresh::DefinitionsRefreshTracker,
    /// In-memory ffprobe cache for in-progress torrent downloads.
    /// Populated lazily on the first `/prepare` that sees enough
    /// bytes on disk — lets the streaming path surface full
    /// track lists + HDR / codec metadata to the decision engine
    /// without waiting for import. See
    /// `playback::stream_probe` for the state machine.
    pub stream_probe: crate::playback::stream_probe::StreamProbeCache,
    /// Latest snapshot from the 60s reconcile task. Surfaces in
    /// `/status` as warnings + drives the admin health banner.
    /// `None` until the first reconcile tick has run.
    pub last_reconcile: Arc<tokio::sync::RwLock<Option<crate::reconcile::ReconcileReport>>>,
    /// Resolved ffprobe binary path. Derived from the transcode
    /// manager's ffmpeg path when present (bundle downloads put both
    /// binaries in the same dir); falls back to `"ffprobe"` from
    /// `PATH`. Read by `stream_probe` and the import probe so a user
    /// who installed bundled jellyfin-ffmpeg gets used everywhere.
    pub ffprobe_path: Arc<String>,
    /// Persistent retry queue for resource removals (torrents, files,
    /// directories) that must succeed but can transiently fail.
    /// Cheap to clone — wraps a pool handle.
    pub cleanup_tracker: crate::cleanup::CleanupTracker,
    /// Per-indexer `IndexerClient` cache, keyed by `indexer.id`.
    /// Long-lived so private trackers (with a login flow) don't
    /// re-authenticate on every search — cookies persist across
    /// the session via the client's shared cookie jar. A 401 (or
    /// any other auth failure) evicts the cached client so the
    /// next call re-logs in cleanly.
    pub indexer_clients: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<i64, Arc<crate::indexers::request::IndexerClient>>,
        >,
    >,
    /// Server-side Cast sender registry (subsystem 32). Owns the
    /// per-session worker threads that bridge `rust_cast`'s blocking
    /// I/O to the tokio runtime. Cheap to clone — wraps an
    /// `Arc<Mutex<HashMap>>`.
    pub cast_sessions: crate::cast_sender::CastSessionManager,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: SqlitePool,
        tmdb: Option<TmdbClient>,
        images: Option<ImageCache>,
        scheduler: Option<Scheduler>,
        torrent: Option<LibrqbitClient>,
        transcode: Option<TranscodeManager>,
        definitions: Option<DefinitionLoader>,
        cf_solver: Option<CloudflareSolver>,
        vpn: Option<VpnManager>,
        data_path: std::path::PathBuf,
        http_port: u16,
        log_live: broadcast::Sender<LogRecord>,
        event_tx: broadcast::Sender<AppEvent>,
        max_concurrent_intro_analyses: u32,
    ) -> (Self, mpsc::Receiver<crate::scheduler::TaskTrigger>) {
        let cancel = CancellationToken::new();
        let (trigger_tx, trigger_rx) = mpsc::channel(32);
        // Derive ffprobe from the transcode manager's ffmpeg path
        // when present — bundle downloads put both binaries in the
        // same directory. Falls back to a PATH-resolved "ffprobe"
        // when transcoding isn't configured (unit tests / setups
        // without ffmpeg).
        let ffprobe_path = transcode.as_ref().map_or_else(
            || "ffprobe".into(),
            |t| t.ffmpeg_path().replace("ffmpeg", "ffprobe"),
        );
        let stream_probe = crate::playback::stream_probe::StreamProbeCache::new(
            &ffprobe_path,
            Some(event_tx.clone()),
        );
        let ffprobe_path = Arc::new(ffprobe_path);
        let cleanup_tracker = crate::cleanup::CleanupTracker::new(db.clone());

        let state = Self {
            db,
            event_tx,
            tmdb: Arc::new(parking_lot::RwLock::new(tmdb)),
            images,
            scheduler,
            cancel,
            trigger_tx,
            torrent: torrent.map(|c| Arc::new(c) as Arc<dyn TorrentSession>),
            transcode,
            definitions: definitions.map(Arc::new),
            cf_solver: cf_solver.map(Arc::new),
            vpn: vpn.map(Arc::new),
            data_path,
            http_port,
            stream_trickplay: StreamTrickplayManager::new(),
            scrobble: ScrobbleManager::new(),
            log_live,
            // Shared budget for intro analysis + trickplay generation
            // (anything else that runs the user's ffmpeg). Sized off
            // `config.max_concurrent_intro_analyses` so the user-facing
            // knob on the settings page is honoured. Clamped to ≥1 so
            // a misconfigured 0 doesn't deadlock the whole pipeline.
            media_processing_sem: Arc::new(Semaphore::new(
                max_concurrent_intro_analyses.max(1) as usize
            )),
            clock: Arc::new(SystemClock),
            watch_now_lock: Arc::new(tokio::sync::Mutex::new(())),
            ffmpeg_download: crate::playback::ffmpeg_bundle::FfmpegDownloadTracker::new(),
            definitions_refresh: crate::indexers::refresh::DefinitionsRefreshTracker::new(),
            stream_probe,
            last_reconcile: Arc::new(tokio::sync::RwLock::new(None)),
            ffprobe_path,
            cleanup_tracker,
            indexer_clients: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            cast_sessions: crate::cast_sender::CastSessionManager::new(),
        };
        (state, trigger_rx)
    }

    /// Fetch or create the cached `IndexerClient` for a given indexer
    /// row. Avoids the re-login storm the freshly-constructed client
    /// caused — private trackers now stay authenticated across an
    /// entire kino run.
    pub async fn indexer_client(
        &self,
        indexer_id: i64,
    ) -> Arc<crate::indexers::request::IndexerClient> {
        let mut map = self.indexer_clients.lock().await;
        if let Some(c) = map.get(&indexer_id) {
            return c.clone();
        }
        let client = Arc::new(crate::indexers::request::IndexerClient::new(
            self.cf_solver.clone(),
        ));
        map.insert(indexer_id, client.clone());
        client
    }

    /// Drop the cached `IndexerClient` for `indexer_id`. Called after
    /// any auth failure so the next search reconstructs a fresh
    /// session (fresh cookie jar, fresh login). Cheap no-op when the
    /// client wasn't cached yet.
    pub async fn invalidate_indexer_client(&self, indexer_id: i64) {
        let mut map = self.indexer_clients.lock().await;
        if map.remove(&indexer_id).is_some() {
            tracing::info!(
                indexer_id,
                "dropped cached indexer client after auth failure"
            );
        }
    }

    /// Replace the clock — only callable from test code. Returns the
    /// updated state by value so it composes with the existing
    /// builder-style construction in `tests::test_app_with_db`.
    #[cfg(test)]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn require_tmdb(&self) -> Result<TmdbClient, crate::error::AppError> {
        self.tmdb
            .read()
            .clone()
            .ok_or_else(|| crate::error::AppError::BadRequest("TMDB API key not configured".into()))
    }

    pub fn tmdb_snapshot(&self) -> Option<TmdbClient> {
        self.tmdb.read().clone()
    }

    pub fn set_tmdb(&self, client: Option<TmdbClient>) {
        *self.tmdb.write() = client;
    }

    pub fn require_images(&self) -> Result<&ImageCache, crate::error::AppError> {
        self.images
            .as_ref()
            .ok_or_else(|| crate::error::AppError::BadRequest("Image cache not configured".into()))
    }

    pub fn require_transcode(&self) -> Result<&TranscodeManager, crate::error::AppError> {
        self.transcode.as_ref().ok_or_else(|| {
            crate::error::AppError::BadRequest("Transcoding not enabled or FFmpeg not found".into())
        })
    }

    pub fn require_definitions(&self) -> Result<&DefinitionLoader, crate::error::AppError> {
        self.definitions.as_deref().ok_or_else(|| {
            crate::error::AppError::BadRequest("Indexer definitions not loaded".into())
        })
    }

    /// Emit an event to all listeners (history, WebSocket, webhooks).
    ///
    /// `broadcast::Sender::send` returns `Err` only when there are zero
    /// subscribers, which is a no-op for us — history listener /
    /// webhook dispatcher / WS forwarder are all always-on. Hence the
    /// deliberate `let _`. Same convention applies to ad-hoc
    /// `event_tx.send(...)` calls throughout the codebase; callers
    /// that want delivery confirmation should use a different channel.
    pub fn emit(&self, event: AppEvent) {
        let _ = self.event_tx.send(event);
    }
}
