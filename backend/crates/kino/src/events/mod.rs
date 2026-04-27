//! Typed application events — the central nervous system of kino.
//!
//! Events flow through a broadcast channel. Three always-on listeners
//! consume them: history logger, WebSocket forwarder, webhook dispatcher.

pub mod display;
pub mod listeners;

use serde::Serialize;
use utoipa::ToSchema;

/// Every significant state change in the system.
///
/// Serialized as `{ "event": "<snake_case_variant>", ...fields }`.
/// `#[derive(ToSchema)]` registers the discriminated-union shape in
/// the `OpenAPI` spec so `openapi-ts` can emit a typed TypeScript
/// discriminated union — the frontend's WebSocket + History handlers
/// get exhaustive type narrowing instead of stringly-typed
/// `[key: string]: unknown` access.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AppEvent {
    // ── Content lifecycle ──
    MovieAdded {
        movie_id: i64,
        tmdb_id: i64,
        title: String,
    },
    ShowAdded {
        show_id: i64,
        tmdb_id: i64,
        title: String,
    },

    // ── Search ──
    SearchStarted {
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        title: String,
    },
    ReleaseGrabbed {
        download_id: i64,
        title: String,
        quality: Option<String>,
        indexer: Option<String>,
        /// Bytes — copied from the release row at grab time so the
        /// History UI can show "Grabbed 8.4 GB from `NZBGeek`" without
        /// re-joining the download row later.
        size: Option<i64>,
    },

    // ── Downloads ──
    DownloadStarted {
        download_id: i64,
        title: String,
    },
    /// Per-tick progress snapshot emitted every scheduler tick (1s)
    /// for each active download. The frontend patches its caches
    /// in-place from these fields without refetching — anything
    /// displayed in the downloads table, detail pane, player badge,
    /// or poster overlay belongs here so it stays live at tick
    /// cadence. Omitting a field means the UI falls back to whatever
    /// the last full refetch gave it, which feels like "X doesn't
    /// update until refresh" — don't do that.
    DownloadProgress {
        download_id: i64,
        percent: u8,
        downloaded: i64,
        uploaded: i64,
        /// Alias for `download_speed` — kept short for payload size
        /// since this event fires at 1 Hz per active download.
        speed: i64,
        upload_speed: i64,
        seeders: Option<i64>,
        leechers: Option<i64>,
        eta: Option<i64>,
    },
    DownloadComplete {
        download_id: i64,
        title: String,
        /// Final on-disk size in bytes. None if the torrent client
        /// didn't report a size for some reason.
        size: Option<i64>,
        /// Wall-clock duration from `added_at` to completion. Used
        /// by the History row so the user sees "downloaded 1.2 GB
        /// in 4m 12s".
        duration_ms: Option<i64>,
    },
    DownloadFailed {
        download_id: i64,
        title: String,
        error: String,
    },
    /// User explicitly cancelled the download (cancel button, remove
    /// show, discard episode). Semantically distinct from
    /// `DownloadFailed` so the UI can render intentional stops as a
    /// quiet confirmation instead of the red failure card — failures
    /// want attention, cancels want to disappear.
    DownloadCancelled {
        download_id: i64,
        title: String,
    },
    /// User paused a running download via `POST /downloads/{id}/pause`.
    /// Carries enough context for cross-window UI sync without
    /// waiting for the next `download_progress` tick (which doesn't
    /// carry state). Other tabs invalidate `DOWNLOADS_KEY` on
    /// receipt so poster / downloads-list cards flip instantly.
    DownloadPaused {
        download_id: i64,
        title: String,
    },
    /// User resumed a paused download via `POST /downloads/{id}/resume`.
    /// Companion to `DownloadPaused`.
    DownloadResumed {
        download_id: i64,
        title: String,
    },
    /// librqbit resolved the torrent's info-dict — file list, total
    /// size, and per-file metadata are now available. Emitted once per
    /// torrent, shortly after `DownloadStarted`. Consumers (downloads
    /// detail pane) use it to invalidate file-list caches without
    /// polling librqbit on a timer.
    DownloadMetadataReady {
        download_id: i64,
        torrent_hash: String,
    },

    // ── Import ──
    Imported {
        media_id: i64,
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        /// Parent show id when `episode_id` is set — lets the
        /// frontend render the show's poster in the import hero toast
        /// without an extra round-trip.
        show_id: Option<i64>,
        title: String,
        quality: Option<String>,
    },
    Upgraded {
        media_id: i64,
        movie_id: Option<i64>,
        title: String,
        old_quality: Option<String>,
        new_quality: Option<String>,
    },

    // ── Playback ──
    Watched {
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        title: String,
    },
    /// Inverse of `Watched` — user un-marked an item. Symmetric
    /// payload so cache invalidation can treat both transitions
    /// uniformly; history / notification surfaces distinguish via
    /// the variant name.
    Unwatched {
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        title: String,
    },
    /// Mid-session progress tick — fires every ~10 s while the
    /// user is playing. Drives the Home "Up Next" row + the
    /// `ShowDetail` "next up" progress bar so they reflect the
    /// live position without a manual refresh. NOT emitted on
    /// completion (that's covered by `Watched`, which is a
    /// distinct transition).
    PlaybackProgress {
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        /// Current position in seconds, rounded down.
        position_secs: i64,
        /// 0.0–1.0 progress fraction. Clamped; never exceeds 1.0.
        progress_pct: f64,
    },
    /// Streaming trickplay has generated a new chunk — the VTT now
    /// covers up to `covered_sec`. Frontend listens on this and
    /// refetches the VTT so hover previews light up for the
    /// newly-landed time range.
    TrickplayStreamUpdated {
        download_id: i64,
        covered_sec: i64,
    },
    /// ffprobe has finished inspecting a partial-download file —
    /// the streaming `/prepare` endpoint can now populate video /
    /// audio / subtitle track lists and surface HDR / codec info
    /// the same way the library path does. Frontend listens on
    /// this so the player info chip + decision-engine plan
    /// refresh mid-stream without a page reload.
    StreamProbeReady {
        download_id: i64,
    },

    // ── Metadata ──
    NewEpisode {
        show_id: i64,
        episode_id: i64,
        show_title: String,
        season: i64,
        episode: i64,
        /// Episode-level title (e.g. "Pilot") when TMDB has one.
        /// Lets the history row show the full "S01E04 · Pilot" form
        /// without a second lookup.
        episode_title: Option<String>,
    },

    // ── Health ──
    HealthWarning {
        message: String,
    },
    HealthRecovered {
        message: String,
    },

    // ── FFmpeg bundle download ──
    //
    // Progress / outcome from the user-initiated jellyfin-ffmpeg
    // download (see `playback::ffmpeg_bundle`). Broadcast at ~5 Hz
    // during download, then once on completion / failure. The
    // settings-page modal subscribes; the tracker state (via
    // `GET /api/v1/playback/ffmpeg/download`) is the authoritative
    // source for late-joining clients that missed mid-stream events.
    FfmpegDownloadProgress {
        bytes: u64,
        total: u64,
    },
    FfmpegDownloadCompleted {
        version: String,
        path: String,
    },
    FfmpegDownloadFailed {
        reason: String,
    },

    // ── WebSocket client lag ──
    //
    // Not broadcast on `event_tx` — produced by the WS handler
    // itself when a client's `broadcast::Receiver` falls behind
    // and tokio drops events via `RecvError::Lagged`. Carried
    // through as a real AppEvent variant so the generated TS
    // discriminated union narrows it correctly; the frontend
    // takes the skip count as a signal to invalidate caches and
    // refetch. History / webhooks never see it.
    Lagged {
        /// Number of events tokio dropped before this frame.
        skipped: u64,
    },

    // ── Backup & restore (subsystem 19) ──
    /// New `backup` row landed (manual / scheduled / pre-restore).
    /// Frontend's Settings → Backup page invalidates its list query
    /// on receipt; the toast pipeline surfaces a "Backup created
    /// (4.7 MB)" notification for manual backups only.
    BackupCreated {
        backup_id: i64,
        kind: String,
        size_bytes: i64,
    },
    /// `backup` row + on-disk archive removed.
    BackupDeleted {
        backup_id: i64,
    },
    /// Restore staged on disk. Carries the operator-facing message
    /// the frontend renders alongside per-platform restart commands.
    BackupRestored {
        backup_id: i64,
        message: String,
    },

    // ── Cast sender (subsystem 32) ──
    /// `MEDIA_STATUS` update from a Chromecast receiver, forwarded to
    /// senders via WebSocket. Fired on every state change (PLAYING /
    /// PAUSED / BUFFERING / etc) and on every position-tick the
    /// receiver emits. `status_json` is the serialised
    /// [`rust_cast::channels::media::Status`] payload — the frontend
    /// types are codegened from the receiver's own schema.
    CastStatus {
        session_id: String,
        position_ms: Option<i64>,
        status_json: String,
    },
    /// The Chromecast session ended — either user-initiated stop, the
    /// receiver app exited, or the reconnect ladder gave up.
    CastSessionEnded {
        session_id: String,
        /// Free-form reason ("stopped" | "failed: <detail>") — purely
        /// for display + history. Frontend doesn't branch on it.
        reason: String,
    },

    // ── VPN killswitch (subsystem 33 phase B) ──
    /// The periodic IP-leak self-test observed an external IP that
    /// doesn't match the VPN's expected egress. Carries both IPs as
    /// strings so consumers don't need to parse — they're for
    /// display in notifications + history. Triggered by the
    /// `vpn_killswitch_check` scheduler task; the same task pauses
    /// every active download immediately rather than waiting for the
    /// next health tick.
    IpLeakDetected {
        observed_ip: String,
        expected_ip: String,
    },

    // ── Content removed ──
    ContentRemoved {
        movie_id: Option<i64>,
        show_id: Option<i64>,
        title: String,
    },

    // ── Settings / configuration ──
    //
    // Settings-domain events drive cache invalidation on the frontend.
    // The WebSocket forwarder maps these to query-key invalidations so
    // any client sees settings changes immediately without polling.
    /// An indexer row was created, updated, deleted, or its health
    /// state changed (`escalation_level`, `disabled_until`).
    IndexerChanged {
        indexer_id: i64,
        action: IndexerAction,
    },
    /// App-level config was updated. `scope` narrows which screens
    /// need to re-read (defaults to `All` when we're not sure).
    ConfigChanged {
        scope: ConfigScope,
    },
    /// A quality profile was created, updated, or deleted.
    QualityProfileChanged {
        profile_id: i64,
        action: IndexerAction,
    },
    /// Webhook target CRUD.
    WebhookChanged {
        webhook_id: i64,
        action: IndexerAction,
    },

    // ── Trakt integration ──
    //
    // Emitted by `integrations::trakt`. Frontend listens to invalidate
    // the Trakt status endpoint and (for `Synced`) the Home
    // recommendations + Up Next caches when an import pulled new
    // watched history.
    /// OAuth device-code flow completed; a token is now stored.
    TraktConnected,
    /// Token cleared locally (and revoked server-side on best-effort).
    TraktDisconnected,
    /// A sync run completed. `kind = "initial_import" | "incremental" | "full"`.
    TraktSynced {
        kind: String,
    },

    // ── Lists subsystem ──
    //
    // Notifications for the Lists subsystem (17). Fired from the
    // scheduler's `lists_poll` task and from Trakt auth on connect.
    /// A list poll added more items than `list_bulk_growth_threshold`
    /// in a single sweep — surfaces "List 'X' added N items" so the
    /// user isn't blindsided by a sudden grow.
    ListBulkGrowth {
        list_id: i64,
        title: String,
        added: i64,
    },
    /// A list source has failed 3 consecutive polls. Emitted once on
    /// the transition, not re-fired while still stuck.
    ListUnreachable {
        list_id: i64,
        title: String,
        reason: String,
    },
    /// A system list was auto-created on user action (Trakt connect →
    /// the Trakt-watchlist list). Matches the spec's onboarding note.
    ListAutoAdded {
        list_id: i64,
        title: String,
    },
    /// A list was deleted. Distinct from `ListUnreachable` (which
    /// means "URL poll is failing"): this is user intent to remove
    /// the list from the library entirely.
    ListDeleted {
        list_id: i64,
        title: String,
    },

    // ── Monitor / preference changes ──
    //
    // State changes that aren't content / download transitions but
    // still need cross-tab cache invalidation. Split out rather than
    // overloading `ConfigChanged` so the frontend can be more
    // surgical about which caches refetch.
    /// Monitor/acquire state for a show changed — `update_show_monitor`
    /// handler. Triggers refresh of the show's episode list + watch
    /// state on every tab so per-season acquire bits stay in sync.
    ShowMonitorChanged {
        show_id: i64,
        title: String,
    },
    /// Rating was set / cleared on a movie / show / episode. Frontend
    /// mirrors the value on library rows + clears Trakt recommendations
    /// (a 👍 recalculates the list). `kind` is `"movie" | "show" |
    /// "episode"`; `id` is the library id of that row.
    Rated {
        kind: String,
        id: i64,
        /// `None` when the user cleared the rating.
        value: Option<i64>,
    },
}

/// CRUD-plus-health action on a settings row. Re-used across
/// Indexer/QualityProfile/Webhook events so the frontend can decide
/// between a fresh fetch and a targeted cache update.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexerAction {
    Created,
    Updated,
    Deleted,
    /// Escalation level changed (e.g. indexer auto-disabled after
    /// repeated failures). Not triggered by user actions.
    HealthChanged,
}

/// Which part of the config changed. Frontend uses this to avoid
/// invalidating queries that can't have shifted — e.g. a quality
/// change doesn't need to re-render the notifications screen.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    /// Catch-all when we don't yet partition changes (safe default).
    All,
    /// TMDB API key, metadata language, etc.
    Metadata,
    /// Paths: download, media library.
    Paths,
    /// Quality settings, auto-upgrade flag.
    Quality,
    /// Download client tuning, ratio, bandwidth limits.
    Download,
    /// VPN config.
    Vpn,
    /// Notification toggles / webhook defaults.
    Notifications,
    /// Playback preferences (preferred languages, transcode policy).
    Playback,
}

impl AppEvent {
    /// Event type string for history/webhook filtering.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::MovieAdded { .. } => "movie_added",
            Self::ShowAdded { .. } => "show_added",
            Self::SearchStarted { .. } => "search_started",
            Self::ReleaseGrabbed { .. } => "release_grabbed",
            Self::DownloadStarted { .. } => "download_started",
            Self::DownloadProgress { .. } => "download_progress",
            Self::DownloadComplete { .. } => "download_complete",
            Self::DownloadFailed { .. } => "download_failed",
            Self::DownloadCancelled { .. } => "download_cancelled",
            Self::DownloadPaused { .. } => "download_paused",
            Self::DownloadResumed { .. } => "download_resumed",
            Self::DownloadMetadataReady { .. } => "download_metadata_ready",
            Self::Imported { .. } => "imported",
            Self::Upgraded { .. } => "upgraded",
            Self::Watched { .. } => "watched",
            Self::Unwatched { .. } => "unwatched",
            Self::PlaybackProgress { .. } => "playback_progress",
            Self::TrickplayStreamUpdated { .. } => "trickplay_stream_updated",
            Self::StreamProbeReady { .. } => "stream_probe_ready",
            Self::NewEpisode { .. } => "new_episode",
            Self::HealthWarning { .. } => "health_warning",
            Self::HealthRecovered { .. } => "health_recovered",
            Self::FfmpegDownloadProgress { .. } => "ffmpeg_download_progress",
            Self::FfmpegDownloadCompleted { .. } => "ffmpeg_download_completed",
            Self::FfmpegDownloadFailed { .. } => "ffmpeg_download_failed",
            Self::ContentRemoved { .. } => "content_removed",
            Self::IndexerChanged { .. } => "indexer_changed",
            Self::ConfigChanged { .. } => "config_changed",
            Self::QualityProfileChanged { .. } => "quality_profile_changed",
            Self::WebhookChanged { .. } => "webhook_changed",
            Self::TraktConnected => "trakt_connected",
            Self::TraktDisconnected => "trakt_disconnected",
            Self::TraktSynced { .. } => "trakt_synced",
            Self::ListBulkGrowth { .. } => "list_bulk_growth",
            Self::ListUnreachable { .. } => "list_unreachable",
            Self::ListAutoAdded { .. } => "list_auto_added",
            Self::ListDeleted { .. } => "list_deleted",
            Self::ShowMonitorChanged { .. } => "show_monitor_changed",
            Self::Rated { .. } => "rated",
            Self::Lagged { .. } => "lagged",
            Self::IpLeakDetected { .. } => "ip_leak_detected",
            Self::CastStatus { .. } => "cast_status",
            Self::CastSessionEnded { .. } => "cast_session_ended",
            Self::BackupCreated { .. } => "backup_created",
            Self::BackupDeleted { .. } => "backup_deleted",
            Self::BackupRestored { .. } => "backup_restored",
        }
    }

    /// Title for display/notification purposes.
    pub fn title(&self) -> &str {
        match self {
            Self::MovieAdded { title, .. }
            | Self::ShowAdded { title, .. }
            | Self::SearchStarted { title, .. }
            | Self::ReleaseGrabbed { title, .. }
            | Self::DownloadStarted { title, .. }
            | Self::DownloadComplete { title, .. }
            | Self::DownloadFailed { title, .. }
            | Self::DownloadCancelled { title, .. }
            | Self::DownloadPaused { title, .. }
            | Self::DownloadResumed { title, .. }
            | Self::Imported { title, .. }
            | Self::Upgraded { title, .. }
            | Self::Watched { title, .. }
            | Self::Unwatched { title, .. }
            | Self::ContentRemoved { title, .. }
            | Self::ListBulkGrowth { title, .. }
            | Self::ListUnreachable { title, .. }
            | Self::ListAutoAdded { title, .. }
            | Self::ListDeleted { title, .. }
            | Self::ShowMonitorChanged { title, .. } => title,
            Self::NewEpisode { show_title, .. } => show_title,
            Self::HealthWarning { message } | Self::HealthRecovered { message } => message,
            Self::DownloadProgress { .. }
            | Self::DownloadMetadataReady { .. }
            | Self::TrickplayStreamUpdated { .. }
            | Self::StreamProbeReady { .. }
            | Self::PlaybackProgress { .. }
            | Self::IndexerChanged { .. }
            | Self::ConfigChanged { .. }
            | Self::QualityProfileChanged { .. }
            | Self::WebhookChanged { .. }
            | Self::TraktConnected
            | Self::TraktDisconnected
            | Self::TraktSynced { .. }
            | Self::Rated { .. }
            | Self::Lagged { .. }
            | Self::FfmpegDownloadProgress { .. }
            | Self::FfmpegDownloadCompleted { .. }
            | Self::FfmpegDownloadFailed { .. }
            | Self::IpLeakDetected { .. }
            | Self::CastStatus { .. }
            | Self::CastSessionEnded { .. }
            | Self::BackupCreated { .. }
            | Self::BackupDeleted { .. }
            | Self::BackupRestored { .. } => "",
        }
    }

    /// Quality string to persist on the history row's top-level
    /// `quality` column. Pulled from whichever variant carries one
    /// so the existing quality chip in the UI renders without
    /// having to parse the JSON `data` blob.
    pub fn quality(&self) -> Option<&str> {
        match self {
            Self::ReleaseGrabbed { quality, .. } | Self::Imported { quality, .. } => {
                quality.as_deref()
            }
            Self::Upgraded { new_quality, .. } => new_quality.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structural guard: every `AppEvent` variant's [`event_type`]
    /// string must match the value serde puts in the `"event"`
    /// discriminator field when the variant is serialized. If
    /// someone adds a new variant or renames one and forgets to
    /// update both sides in lockstep, this fails loudly in CI
    /// rather than silently delivering different event names to
    /// the WebSocket (serde) and to History / webhooks
    /// (`event_type`).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn event_type_matches_serde_tag() {
        let samples: &[AppEvent] = &[
            AppEvent::MovieAdded {
                movie_id: 0,
                tmdb_id: 0,
                title: String::new(),
            },
            AppEvent::ShowAdded {
                show_id: 0,
                tmdb_id: 0,
                title: String::new(),
            },
            AppEvent::SearchStarted {
                movie_id: None,
                episode_id: None,
                title: String::new(),
            },
            AppEvent::ReleaseGrabbed {
                download_id: 0,
                title: String::new(),
                quality: None,
                indexer: None,
                size: None,
            },
            AppEvent::DownloadStarted {
                download_id: 0,
                title: String::new(),
            },
            AppEvent::DownloadProgress {
                download_id: 0,
                percent: 0,
                downloaded: 0,
                uploaded: 0,
                speed: 0,
                upload_speed: 0,
                seeders: None,
                leechers: None,
                eta: None,
            },
            AppEvent::DownloadComplete {
                download_id: 0,
                title: String::new(),
                size: None,
                duration_ms: None,
            },
            AppEvent::DownloadFailed {
                download_id: 0,
                title: String::new(),
                error: String::new(),
            },
            AppEvent::DownloadCancelled {
                download_id: 0,
                title: String::new(),
            },
            AppEvent::DownloadPaused {
                download_id: 0,
                title: String::new(),
            },
            AppEvent::DownloadResumed {
                download_id: 0,
                title: String::new(),
            },
            AppEvent::DownloadMetadataReady {
                download_id: 0,
                torrent_hash: String::new(),
            },
            AppEvent::Imported {
                media_id: 0,
                movie_id: None,
                episode_id: None,
                show_id: None,
                title: String::new(),
                quality: None,
            },
            AppEvent::Upgraded {
                media_id: 0,
                movie_id: None,
                title: String::new(),
                old_quality: None,
                new_quality: None,
            },
            AppEvent::Watched {
                movie_id: None,
                episode_id: None,
                title: String::new(),
            },
            AppEvent::Unwatched {
                movie_id: None,
                episode_id: None,
                title: String::new(),
            },
            AppEvent::PlaybackProgress {
                movie_id: None,
                episode_id: None,
                position_secs: 0,
                progress_pct: 0.0,
            },
            AppEvent::TrickplayStreamUpdated {
                download_id: 0,
                covered_sec: 0,
            },
            AppEvent::StreamProbeReady { download_id: 0 },
            AppEvent::NewEpisode {
                show_id: 0,
                episode_id: 0,
                show_title: String::new(),
                season: 0,
                episode: 0,
                episode_title: None,
            },
            AppEvent::HealthWarning {
                message: String::new(),
            },
            AppEvent::HealthRecovered {
                message: String::new(),
            },
            AppEvent::FfmpegDownloadProgress { bytes: 0, total: 0 },
            AppEvent::FfmpegDownloadCompleted {
                version: String::new(),
                path: String::new(),
            },
            AppEvent::FfmpegDownloadFailed {
                reason: String::new(),
            },
            AppEvent::ContentRemoved {
                movie_id: None,
                show_id: None,
                title: String::new(),
            },
            AppEvent::IndexerChanged {
                indexer_id: 0,
                action: IndexerAction::Created,
            },
            AppEvent::ConfigChanged {
                scope: ConfigScope::All,
            },
            AppEvent::QualityProfileChanged {
                profile_id: 0,
                action: IndexerAction::Created,
            },
            AppEvent::WebhookChanged {
                webhook_id: 0,
                action: IndexerAction::Created,
            },
            AppEvent::TraktConnected,
            AppEvent::TraktDisconnected,
            AppEvent::TraktSynced {
                kind: String::new(),
            },
            AppEvent::ListBulkGrowth {
                list_id: 0,
                title: String::new(),
                added: 0,
            },
            AppEvent::ListUnreachable {
                list_id: 0,
                title: String::new(),
                reason: String::new(),
            },
            AppEvent::ListAutoAdded {
                list_id: 0,
                title: String::new(),
            },
            AppEvent::ListDeleted {
                list_id: 0,
                title: String::new(),
            },
            AppEvent::ShowMonitorChanged {
                show_id: 0,
                title: String::new(),
            },
            AppEvent::Rated {
                kind: String::new(),
                id: 0,
                value: None,
            },
            AppEvent::Lagged { skipped: 0 },
            AppEvent::IpLeakDetected {
                observed_ip: String::new(),
                expected_ip: String::new(),
            },
            AppEvent::CastStatus {
                session_id: String::new(),
                position_ms: None,
                status_json: String::new(),
            },
            AppEvent::CastSessionEnded {
                session_id: String::new(),
                reason: String::new(),
            },
            AppEvent::BackupCreated {
                backup_id: 0,
                kind: String::new(),
                size_bytes: 0,
            },
            AppEvent::BackupDeleted { backup_id: 0 },
            AppEvent::BackupRestored {
                backup_id: 0,
                message: String::new(),
            },
        ];
        for ev in samples {
            let json: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(ev).expect("serialize")).unwrap();
            let tag = json
                .get("event")
                .and_then(|v| v.as_str())
                .expect("AppEvent JSON should carry an `event` discriminator");
            assert_eq!(
                tag,
                ev.event_type(),
                "serde tag vs event_type() drift on {ev:?}"
            );
        }
    }
}
