use anyhow::Context as _;
use axum::Router;
use axum::middleware;
use clap::Parser;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

pub mod acquisition;
mod api;
mod auth;
pub mod auth_session;
pub mod backup;
pub mod cast_sender;
pub mod cleanup;
pub mod clock;
pub mod content;
pub mod conventions;
mod db;
pub mod download;
mod error;
pub mod events;
#[cfg(test)]
mod flow_tests;
pub mod home;
mod images;
pub mod import;
pub mod indexers;
mod init;
pub mod integrations;
pub mod invariants;
pub mod library;
pub mod mdns;
pub mod metadata;
mod models;
pub mod notification;
pub mod observability;
mod pagination;
pub mod parser;
mod paths;
pub mod playback;
pub mod reconcile;
pub mod scheduler;
mod service_install;
#[cfg(target_os = "windows")]
mod service_runner;
pub mod settings;
mod spa;
pub mod startup;
mod state;
#[cfg(any(test, feature = "harness"))]
pub mod test_support;
pub mod time;
mod tmdb;
mod torznab;
#[cfg(feature = "tray")]
pub mod tray;
pub mod watch_now;

use state::AppState;

#[derive(Parser)]
#[command(
    name = "kino",
    version,
    about = "Media automation and streaming server"
)]
struct Cli {
    /// Port to listen on. Defaults to whatever's stored in
    /// `config.listen_port` (which Settings → General → Port edits).
    /// CLI flag and `KINO_PORT` env both override the DB value at
    /// runtime — useful for one-off debugging or for the
    /// first-run insert (the env value seeds the DB column when no
    /// row exists). 0 = "use the DB value".
    #[arg(short, long, env = "KINO_PORT", default_value_t = 0)]
    port: u16,

    /// Data directory (database, images, trickplay, persistence). When
    /// omitted falls back to the platform-appropriate XDG / native
    /// default — see `paths::default_data_dir`. Native packages set
    /// this explicitly via the systemd unit / launchd plist /
    /// Windows Service descriptor, so service-mode never hits the
    /// fallback.
    #[arg(long, env = "KINO_DATA_PATH")]
    data_path: Option<String>,

    /// Skip the auto-open of the setup wizard in the user's default
    /// browser on first launch. Service-mode descriptors (systemd
    /// unit, launchd plist, Windows SCM) pass this so the daemon
    /// doesn't try to spawn a browser in a headless context.
    /// Tarball / `cargo install` users can set it via env var if
    /// they prefer the no-auto-open behaviour permanently.
    ///
    /// `BoolishValueParser` accepts the full shell/systemd convention
    /// — `1` / `true` / `yes` / `on` enable it; `0` / `false` /
    /// `no` / `off` disable it. Without that, an `Environment=
    /// KINO_NO_OPEN_BROWSER=1` line in a systemd unit fails parsing
    /// (clap's typed-`bool` only accepts literal `true` / `false`)
    /// and the service crash-loops.
    #[arg(
        long,
        env = "KINO_NO_OPEN_BROWSER",
        value_parser = clap::builder::BoolishValueParser::new(),
        default_value_t = false,
        default_missing_value = "true",
        num_args = 0..=1,
        require_equals = true,
    )]
    no_open_browser: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Delete all data and reset to first-run state
    Reset,
    /// Run the HTTP server in the foreground (default behaviour with
    /// no subcommand on a headless session). What the systemd unit /
    /// launchd plist / Windows Service invokes
    Serve,
    /// Install Kino as a platform-native service (systemd / launchd /
    /// Windows Service Manager). Native packages handle this during
    /// install — this subcommand is the tarball / `cargo install`
    /// fallback. Requires admin/root
    InstallService {
        /// Install as a per-user service where the OS supports it
        /// (systemd user unit / per-user `LaunchAgent`). Defaults to
        /// system-wide
        #[arg(long)]
        user: bool,
    },
    /// Stop the platform service and remove its descriptor
    UninstallService,
    /// Open the Kino web UI in the user's default browser. The
    /// installed `.desktop` / Start Menu / Launchpad entry runs this
    /// so port-aware launching works regardless of how the service
    /// is configured.
    Open,
    /// Grant the kino service user read+write access to a folder
    /// (typically a media library on an external drive that
    /// auto-mounted with the desktop user's permissions).
    /// Run with sudo. Uses POSIX ACLs — non-destructive, reversible
    /// with `sudo setfacl -R -x u:kino <path>`.
    SetupPermissions {
        /// Absolute path to grant kino access to (e.g.
        /// `/media/<user>/MyDrive`).
        path: String,
    },
    /// Add inbound firewall rules so LAN clients can reach kino at
    /// `http://kino.local`. Triggers a graphical privilege prompt
    /// (Polkit / UAC / `osascript`) — no need to re-run with sudo.
    /// Idempotent: rerunning is a no-op once the rule is in place.
    AllowFirewall,
    /// Run the system-tray / menu-bar icon. Talks to the local Kino
    /// server over `http://localhost:{port}`. See subsystem 22
    #[cfg(feature = "tray")]
    Tray,
    /// Write the per-user autostart entry for the tray and start it
    /// now. Native installers handle this during install — this
    /// subcommand is the tarball / `cargo install` fallback
    #[cfg(feature = "tray")]
    InstallTray,
    /// Remove the per-user autostart entry and kill any running tray
    #[cfg(feature = "tray")]
    UninstallTray,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "kino",
        description = "Media automation and streaming server API",
        version = "0.1.0"
    ),
    paths(
        api::status::get_status,
        api::network::lan_probe,
        api::network::mdns_test,
        auth_session::handlers::bootstrap,
        auth_session::handlers::create_session,
        auth_session::handlers::redeem,
        auth_session::handlers::list_sessions,
        auth_session::handlers::revoke_all,
        auth_session::handlers::revoke_session,
        auth_session::handlers::create_cli_token,
        auth_session::handlers::create_bootstrap_token,
        auth_session::handlers::sign_url,
        auth_session::handlers::logout,
        api::health::get_health,
        api::diagnostics::export_bundle,
        settings::config::get_config,
        settings::config::update_config,
        settings::config::rotate_api_key,
        api::fs::test_path,
        api::fs::browse,
        api::fs::mkdir,
        api::fs::mounts,
        api::fs::places,
        metadata::test_handlers::test_tmdb,
        metadata::test_handlers::test_opensubtitles,
        download::vpn::handlers::get_status,
        download::vpn::handlers::test_connection,
        settings::quality_profile::list_quality_profiles,
        settings::quality_profile::get_quality_profile,
        settings::quality_profile::create_quality_profile,
        settings::quality_profile::update_quality_profile,
        settings::quality_profile::delete_quality_profile,
        settings::quality_profile::set_default_quality_profile,
        metadata::tmdb_handlers::search,
        metadata::tmdb_handlers::movie_details,
        metadata::tmdb_handlers::show_details,
        metadata::tmdb_handlers::season_details,
        metadata::tmdb_handlers::trending_movies,
        metadata::tmdb_handlers::trending_shows,
        metadata::tmdb_handlers::discover_movies,
        metadata::tmdb_handlers::discover_shows,
        metadata::tmdb_handlers::genres,
        content::movie::handlers::list_movies,
        content::movie::handlers::get_movie,
        content::movie::handlers::create_movie,
        content::movie::handlers::delete_movie,
        content::show::handlers::list_shows,
        content::show::handlers::show_watch_state,
        content::show::handlers::show_season_episodes_by_tmdb,
        content::show::handlers::update_show_monitor,
        content::show::handlers::pause_show_downloads,
        content::show::handlers::resume_show_downloads,
        content::show::episode_handlers::mark_episode_watched,
        content::show::episode_handlers::unmark_episode_watched,
        content::show::episode_handlers::analyse_season_intro,
        content::show::handlers::monitored_seasons,
        content::show::episode_handlers::redownload_episode,
        content::show::episode_handlers::acquire_episode,
        content::show::episode_handlers::acquire_episode_by_tmdb,
        content::show::episode_handlers::discard_episode,
        content::show::handlers::get_show,
        content::show::handlers::create_show,
        content::show::handlers::list_seasons,
        content::show::handlers::list_episodes,
        content::show::handlers::delete_show,
        metadata::image_handlers::get_image,
        library::handlers::library_search,
        library::handlers::calendar,
        library::handlers::calendar_ics,
        library::handlers::stats,
        library::handlers::widget,
        indexers::handlers::list_indexers,
        indexers::handlers::get_indexer,
        indexers::handlers::create_indexer,
        indexers::handlers::update_indexer,
        indexers::handlers::delete_indexer,
        indexers::handlers::list_definitions,
        indexers::handlers::get_definition,
        indexers::handlers::refresh_definitions,
        indexers::handlers::get_refresh_state,
        indexers::handlers::test_indexer,
        indexers::handlers::retry_indexer,
        acquisition::release::list_releases,
        acquisition::release::episode_releases,
        acquisition::release::movie_releases,
        acquisition::release::grab_release,
        acquisition::release::grab_and_watch,
        playback::handlers::prepare,
        playback::handlers::direct,
        playback::hls::master::hls_master,
        playback::hls::variant::hls_variant,
        playback::hls::segment::hls_segment,
        playback::handlers::stop_transcode,
        playback::handlers::subtitle,
        playback::handlers::trickplay_vtt,
        playback::handlers::trickplay_sprite,
        playback::handlers::play_progress,
        watch_now::handlers::watch_now,
        acquisition::blocklist::list_blocklist,
        acquisition::blocklist::delete_blocklist,
        acquisition::blocklist::get_movie_blocklist,
        acquisition::blocklist::clear_movie_blocklist,
        download::handlers::list_downloads,
        download::handlers::get_download,
        download::handlers::cancel_download,
        download::handlers::pause_download,
        download::handlers::resume_download,
        download::handlers::retry_download,
        download::handlers::blocklist_and_search,
        download::handlers::download_files,
        download::handlers::update_download_files,
        download::handlers::download_peers,
        download::handlers::download_pieces,
        download::handlers::speed_test,
        content::media::handlers::list_media,
        content::media::handlers::get_media,
        content::media::handlers::get_media_streams,
        content::media::handlers::delete_media,
        playback::probe_handlers::probe,
        playback::probe_handlers::transcode_stats,
        playback::probe_handlers::transcode_sessions,
        playback::probe_handlers::stop_transcode_session,
        playback::probe_handlers::start_ffmpeg_download,
        playback::probe_handlers::get_ffmpeg_download,
        playback::probe_handlers::cancel_ffmpeg_download,
        playback::probe_handlers::revert_ffmpeg_to_system,
        home::preferences::get_home_preferences,
        home::preferences::update_home_preferences,
        home::preferences::reset_home_preferences,
        integrations::trakt::handlers::status,
        integrations::trakt::handlers::begin_device,
        integrations::trakt::handlers::poll_device,
        integrations::trakt::handlers::disconnect,
        integrations::trakt::handlers::dry_run,
        integrations::trakt::handlers::import,
        integrations::trakt::handlers::sync_now,
        integrations::trakt::handlers::recommendations,
        integrations::trakt::handlers::trending,
        integrations::trakt::handlers::rate,
        integrations::lists::handlers::list_lists,
        integrations::lists::handlers::create_list,
        integrations::lists::handlers::get_list,
        integrations::lists::handlers::delete_list,
        integrations::lists::handlers::refresh_list,
        integrations::lists::handlers::list_items,
        integrations::lists::handlers::ignore_item,
        playback::probe_handlers::test_transcode,
        playback::cast::issue_cast_token,
        cast_sender::handlers::list_devices,
        cast_sender::handlers::add_device,
        cast_sender::handlers::delete_device,
        cast_sender::handlers::start_session,
        cast_sender::handlers::get_session,
        cast_sender::handlers::stop_session,
        cast_sender::handlers::play,
        cast_sender::handlers::pause,
        cast_sender::handlers::seek,
        backup::handlers::list_backups,
        backup::handlers::create_backup,
        backup::handlers::download_backup,
        backup::handlers::delete_backup,
        backup::handlers::restore_backup,
        backup::handlers::restore_upload,
        playback::watch_state::mark_movie_watched,
        playback::watch_state::unmark_movie_watched,
        scheduler::handlers::list_tasks,
        scheduler::handlers::run_task,
        notification::history::list_history,
        home::handlers::up_next,
        notification::webhook::list_webhooks,
        notification::webhook::create_webhook,
        notification::webhook::update_webhook,
        notification::webhook::delete_webhook,
        notification::webhook::test_webhook,
        observability::handlers::list_logs,
        observability::handlers::export_logs,
        observability::handlers::ingest_client_logs,
        notification::ws_handlers::ws_handler,
    ),
    components(schemas(
        auth_session::handlers::BootstrapReply,
        auth_session::handlers::CreateSessionRequest,
        auth_session::handlers::CreateSessionReply,
        auth_session::handlers::RedeemRequest,
        auth_session::handlers::ListSessionsReply,
        auth_session::handlers::CreateCliTokenRequest,
        auth_session::handlers::CreateCliTokenReply,
        auth_session::handlers::BootstrapTokenReply,
        auth_session::handlers::SignUrlRequest,
        auth_session::handlers::SignUrlReply,
        auth_session::model::SessionView,
        auth_session::model::SessionSource,
        metadata::tmdb_handlers::GenresResponse,
        tmdb::types::TmdbGenre,
        tmdb::types::TmdbSearchResult,
        tmdb::types::TmdbMovieDetails,
        tmdb::types::TmdbShowDetails,
        tmdb::types::TmdbSeasonDetails,
        tmdb::types::TmdbDiscoverMovie,
        tmdb::types::TmdbDiscoverShow,
        tmdb::types::TmdbEpisode,
        tmdb::types::TmdbSeasonSummary,
        playback::AudioTrack,
        playback::chapter_model::Chapter,
        playback::BackendState,
        playback::BackendStatus,
        playback::HwBackend,
        playback::HwCapabilities,
        playback::HwaFailureKind,
        playback::ffmpeg_bundle::FfmpegDownloadState,
        indexers::refresh::DefinitionsRefreshState,
        playback::PlaybackMethod,
        playback::PlaybackPlan,
        playback::SubtitleTrack,
        playback::TranscodeReason,
        playback::TranscodeReasons,
        playback::VideoTrackInfo,
        api::status::StatusResponse,
        api::network::LanProbeReply,
        api::network::MdnsTestReply,
        api::network::MdnsTestRequest,
        mdns::MdnsProvider,
        mdns::ProviderStatus,
        api::status::StatusWarning,
        api::health::HealthResponse,
        api::health::HealthPanels,
        api::health::HealthStatus,
        api::health::StoragePanel,
        api::health::StoragePath,
        api::health::VpnPanel,
        api::health::IndexersPanel,
        api::health::IndexerItem,
        api::health::DownloadsPanel,
        api::health::TranscoderPanel,
        api::health::SchedulerPanel,
        api::health::FailingTask,
        api::health::MetadataPanel,
        home::preferences::HomePreferences,
        home::preferences::HomePreferencesUpdate,
        integrations::trakt::handlers::TraktStatus,
        integrations::trakt::handlers::BeginReply,
        integrations::trakt::handlers::PollReq,
        integrations::trakt::handlers::PollReply,
        integrations::trakt::handlers::HomeRow,
        integrations::trakt::handlers::HomeItem,
        integrations::trakt::handlers::RateReq,
        integrations::lists::handlers::CreateListResponse,
        integrations::lists::handlers::IgnoreItemRequest,
        integrations::lists::model::List,
        integrations::lists::model::ListView,
        integrations::lists::model::ListItem,
        integrations::lists::model::ListItemView,
        integrations::lists::model::ListPreview,
        integrations::lists::model::CreateListRequest,
        integrations::trakt::sync::DryRunCounts,
        acquisition::release::GrabAndWatchReply,
        acquisition::release::ReleaseWithStatus,
        download::handlers::DownloadFileEntry,
        download::handlers::DownloadFilesReply,
        download::handlers::UpdateFileSelection,
        download::handlers::DownloadPeer,
        download::handlers::DownloadPeersReply,
        download::handlers::DownloadPiecesReply,
        playback::handlers::PlayPrepareReply,
        playback::PlayKind,
        playback::handlers::PlayState,
        playback::handlers::PlayProgressBody,
        watch_now::handlers::WatchNowRequest,
        playback::cast::CastTokenRequest,
        playback::cast::CastTokenReply,
        cast_sender::CastDevice,
        cast_sender::CastSession,
        cast_sender::CastSessionStatus,
        cast_sender::handlers::AddDeviceRequest,
        cast_sender::handlers::StartSessionRequest,
        cast_sender::handlers::SeekRequest,
        backup::Backup,
        backup::BackupKind,
        backup::handlers::RestoreReply,
        home::handlers::ContinueItem,
        settings::config::ConfigResponse,
        settings::config::ConfigUpdate,
        settings::quality_profile::QualityProfile,
        settings::quality_profile::QualityProfileWithUsage,
        settings::quality_profile::CreateQualityProfile,
        settings::quality_profile::UpdateQualityProfile,
        settings::quality_profile::QualityTier,
        content::movie::model::Movie,
        content::movie::model::CreateMovie,
        content::show::model::Show,
        content::show::model::ShowListItem,
        content::show::model::NextEpisode,
        content::show::model::ActiveShowDownload,
        content::show::model::CreateShow,
        content::show::handlers::ShowWatchState,
        content::show::handlers::ShowNextUpEpisode,
        content::show::handlers::SeasonAcquireState,
        content::show::episode_handlers::AcquireEpisodeByTmdb,
        content::show::series::Series,
        content::show::episode::Episode,
        indexers::model::Indexer,
        indexers::model::CreateIndexer,
        indexers::model::UpdateIndexer,
        indexers::loader::DefinitionSummary,
        indexers::loader::IndexerDefinitionType,
        indexers::handlers::DefinitionDetail,
        indexers::handlers::DefinitionSettingField,
        indexers::handlers::TestIndexerResult,
        acquisition::release::Release,
        acquisition::blocklist::Blocklist,
        download::model::Download,
        content::media::model::Media,
        playback::stream_model::Stream,
        library::handlers::LibraryHit,
        library::handlers::CalendarEntry,
        library::handlers::LibraryStats,
        library::handlers::WidgetResponse,
        notification::history::History,
        notification::webhook::WebhookTarget,
        notification::webhook::CreateWebhook,
        notification::webhook::UpdateWebhook,
        observability::handlers::LogEntryRow,
        observability::handlers::ClientLogEntry,
        observability::handlers::ClientLogsPayload,
        scheduler::TaskInfo,
        parser::ParsedRelease,
        // Event-stream types: not consumed by any HTTP endpoint,
        // registered here so `openapi-ts` emits TypeScript types the
        // WebSocket + History handlers can use in place of their
        // hand-rolled, stringly-typed interfaces.
        events::AppEvent,
        events::IndexerAction,
        events::ConfigScope,
        // Cross-cutting domain enums the frontend currently mirrors
        // with hardcoded string unions (download state switches,
        // follow-show dialog, etc). Registering them promotes those
        // to typed unions auto-updated on every codegen run.
        models::enums::ContentStatus,
        models::enums::DownloadState,
        models::enums::ReleaseStatus,
        models::enums::MonitorNewItems,
        models::enums::FollowIntent,
        // Release / media quality taxonomy. Surface as named types
        // so downstream frontend code (quality chips, release tables,
        // upgrade ladders) branches on a typed union instead of raw
        // strings. They already sit on parent structs as `String`
        // fields — adding them to components makes the enum explicit.
        models::enums::ShowStatus,
        models::enums::Resolution,
        models::enums::Source,
        models::enums::VideoCodec,
        models::enums::AudioCodec,
        models::enums::HdrFormat,
        // Domain enums the admin UI surfaces directly. All derive
        // ToSchema; registering here so codegen emits typed unions.
        acquisition::RejectReason,
        cleanup::ResourceKind,
        playback::TranscodeSessionState,
    )),
    tags(
        (name = "system", description = "System endpoints"),
        (name = "config", description = "Application configuration"),
        (name = "quality_profiles", description = "Quality profile management"),
        (name = "tmdb", description = "TMDB proxy endpoints"),
        (name = "movies", description = "Movie management"),
        (name = "shows", description = "Show management"),
        (name = "images", description = "Image cache and serving"),
        (name = "library", description = "Library search, calendar, stats"),
        (name = "indexers", description = "Indexer management"),
        (name = "releases", description = "Release search and grab"),
        (name = "blocklist", description = "Blocklist management"),
        (name = "downloads", description = "Download queue management"),
        (name = "media", description = "Media files and streams"),
        (name = "playback", description = "Media playback and streaming"),
        (name = "tasks", description = "Scheduled background tasks"),
        (name = "history", description = "Event history"),
        (name = "webhooks", description = "Webhook notification targets"),
        (name = "websocket", description = "Real-time event WebSocket"),
    ),
    security(
        ("api_key" = [])
    )
)]
struct ApiDoc;

fn main() -> anyhow::Result<()> {
    // Build the log bus before the subscriber so early events are
    // buffered into the channel; the writer task will drain it once the
    // DB pool is ready.
    let (log_bus, log_rx) = observability::new_bus();
    setup_tracing(&log_bus);

    let cli = Cli::parse();

    // Resolve data path once, before either the sync dispatch or the
    // async server bootstrap looks at it. Priority: explicit
    // `--data-path` flag > `KINO_DATA_PATH` env var > per-OS default
    // from `paths::default_data_dir()` (XDG on Linux, ~/Library on
    // macOS, %LOCALAPPDATA% on Windows). Native packages bypass the
    // fallback entirely by passing `--data-path` in the systemd /
    // launchd / Windows Service descriptor.
    let data_path = cli
        .data_path
        .clone()
        .unwrap_or_else(|| paths::default_data_dir().to_string_lossy().into_owned());

    // Handle non-server subcommands first. `Serve` (and `None`) fall
    // through to the server boot path below; everything else exits
    // before we touch the database / port. Sync subcommands run on
    // the calling thread so GUI event loops (`kino tray`) can own
    // the main thread — required by AppKit on macOS and assumed by
    // `tao` + `tray-icon`. Tray-feature-gated variants are matched
    // separately so the binary still compiles with
    // `--no-default-features` (headless / Pi / Docker builds).
    match cli.command {
        Some(Command::Reset) => return reset_data_sync(&data_path),
        Some(Command::InstallService { user }) => return service_install::install(user),
        Some(Command::UninstallService) => return service_install::uninstall(),
        Some(Command::Open) => return open_browser_at_port(),
        Some(Command::SetupPermissions { path }) => return setup_permissions(&path),
        Some(Command::AllowFirewall) => return allow_firewall(),
        #[cfg(feature = "tray")]
        Some(Command::Tray) => return tray::run(),
        #[cfg(feature = "tray")]
        Some(Command::InstallTray) => return tray::install(),
        #[cfg(feature = "tray")]
        Some(Command::UninstallTray) => return tray::uninstall(),
        Some(Command::Serve) | None => {}
    }

    // Server path needs an async runtime. Built manually rather than
    // via `#[tokio::main]` so the sync subcommand dispatch above
    // runs before any runtime is up — `kino tray` would otherwise
    // race the runtime for the main thread.
    //
    // On Windows we try the SCM dispatcher first. If we're invoked
    // by services.msc / `sc start kino`, SCM owns the main thread
    // for the lifetime of the service and hands our worker closure
    // off to a background thread. If we're invoked from a console
    // (`kino serve` from cmd.exe), the dispatcher returns
    // `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT` immediately and we
    // fall through to the interactive path. See
    // `docs/architecture/service-install.md`.
    #[cfg(target_os = "windows")]
    {
        let runner = move |cancel: tokio_util::sync::CancellationToken| {
            run_server_blocking_with_cancel(cli, data_path, log_bus, log_rx, Some(cancel))
        };
        match service_runner::try_run_under_scm(runner)? {
            service_runner::ScmOutcome::Claimed => Ok(()),
            service_runner::ScmOutcome::NotUnderScm(runner) => {
                let exit = runner(tokio_util::sync::CancellationToken::new());
                if exit == 0 {
                    Ok(())
                } else {
                    anyhow::bail!("server exited with status {exit}")
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let exit = run_server_blocking_with_cancel(cli, data_path, log_bus, log_rx, None);
        if exit == 0 {
            Ok(())
        } else {
            anyhow::bail!("server exited with status {exit}")
        }
    }
}

/// Build a tokio runtime, run `server_main`, and return a process
/// exit code (0 = clean, 1 = error). Shared by the interactive code
/// path and the Windows SCM dispatcher's worker thread.
///
/// `external_cancel` is plumbed through to `server_main` as a
/// secondary shutdown trigger; on Windows the SCM dispatcher signals
/// it when it receives `SERVICE_CONTROL_STOP`. On Unix it's `None`
/// and shutdown comes from the existing `tokio::signal::ctrl_c()`
/// handler inside `shutdown_signal()`.
fn run_server_blocking_with_cancel(
    cli: Cli,
    data_path: String,
    log_bus: observability::LogBus,
    log_rx: tokio::sync::mpsc::Receiver<observability::LogRecord>,
    external_cancel: Option<tokio_util::sync::CancellationToken>,
) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to build tokio runtime: {e}");
            return 1;
        }
    };
    match runtime.block_on(server_main(
        cli,
        data_path,
        log_bus,
        log_rx,
        external_cancel,
    )) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("server exited with error: {e}");
            1
        }
    }
}

fn setup_tracing(log_bus: &observability::LogBus) {
    use tracing_subscriber::prelude::*;

    // Two filters — stderr respects `RUST_LOG` (default: info-level) so
    // `just logs` stays readable, while the SQLite sink captures DEBUG
    // from our own code so the persisted log in the DB has the detail
    // we need when reproducing rare bugs. Noisy third-party crates
    // (hyper / sqlx / librqbit / rustls / h2 / tokio) stay at info so
    // the firehose doesn't drown out our own traces.
    //
    // DEBUG (not TRACE) is deliberate: TRACE is reserved for hot-path
    // per-poll / per-frame events that would overwhelm the mpsc buffer
    // between the layer and the SQLite writer (we've seen ~400k events
    // dropped in minutes at trace level). Promote a specific target
    // with `KINO_DB_LOG=debug,kino::api::stream=trace` when you need
    // that granularity for a reproduction session.
    let stderr_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // Default `info` for everything except a few noisy libraries.
        // `html5ever` emits per-page warnings about quirks the parser
        // doesn't fully implement ("foster parenting not implemented")
        // every time we scrape a Cardigann indexer page; not actionable
        // and clutters `journalctl -u kino`.
        EnvFilter::new("info,html5ever=error,markup5ever=error")
    });
    let sqlite_filter = EnvFilter::try_from_env("KINO_DB_LOG").unwrap_or_else(|_| {
        EnvFilter::new(
            "debug,hyper=info,tower=info,tower_http=info,reqwest=info,\
             sqlx=info,h2=info,librqbit=info,rqbit=info,rustls=info,\
             tokio=info,tokio_util=info,watchexec=info,\
             html5ever=error,markup5ever=error",
        )
    });

    // systemd sets `JOURNAL_STREAM` when stderr is wired to journald
    // (man systemd.exec). Drop our own timestamp + ANSI colour codes
    // when present — journald adds its own timestamp and `journalctl`
    // colourises on the read side, so we'd otherwise double-print
    // both. Keeps `journalctl -u kino` output clean. Falls back to
    // the human-friendly default when running in a terminal.
    let under_journald = std::env::var_os("JOURNAL_STREAM").is_some();
    let stderr_layer = if under_journald {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .without_time()
            .with_ansi(false)
            .with_filter(stderr_filter)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(stderr_filter)
            .boxed()
    };
    let sqlite_layer =
        observability::layer::SqliteLogLayer::from_bus(log_bus).with_filter(sqlite_filter);
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(sqlite_layer)
        .init();
}

#[allow(clippy::too_many_lines)]
async fn server_main(
    cli: Cli,
    data_path: String,
    log_bus: observability::LogBus,
    log_rx: tokio::sync::mpsc::Receiver<observability::LogRecord>,
    external_cancel: Option<tokio_util::sync::CancellationToken>,
) -> anyhow::Result<()> {
    tracing::info!(
        cli_port = cli.port,
        data_path = %data_path,
        "kino starting (cli_port=0 means: use config.listen_port)"
    );

    // Single-instance lock — refuse to start a second `kino serve`
    // pointing at the same data directory. Catches the common
    // confusion of running `kino` from a terminal while the systemd
    // service is already up: the second process would race the
    // first on SQLite migrations, corrupt the WAL, and double-bind
    // mDNS announcements. fs4 is cross-platform (flock on
    // Unix, LockFileEx on Windows). Bound to a `_lock` binding so
    // its `Drop` releases the OS lock when serve exits cleanly;
    // a hard kill releases it via kernel cleanup.
    //
    // Limited to serve mode. Admin subcommands (`kino reset`,
    // `kino tray`, `kino install-tray`, `kino setup-permissions`,
    // `kino open`) do NOT take this lock — they're meant to coexist
    // with a running service.
    let _serve_lock = acquire_serve_lock(&data_path)?;
    tracing::info!("serve lock acquired");

    // Database
    let pool = db::create_pool(&data_path).await?;
    db::run_migrations(&pool).await?;
    tracing::info!("database ready");

    // Log writer task — drains buffered records and writes to SQLite.
    // Started after migrations so the log_entry table exists.
    {
        let pool = pool.clone();
        let drops = log_bus.drops.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        tokio::spawn(observability::writer::run(pool, log_rx, drops, cancel));
    }

    // First-run initialization. The returned bool flags whether
    // this invocation just inserted the default config row; we use
    // it below to decide whether to auto-open the setup wizard in
    // the user's browser.
    let was_first_run = init::ensure_defaults(&pool, &data_path).await?;

    // Startup reconciliation runs AFTER VPN + librqbit are up so the
    // download-state phase can verify rows against the live torrent
    // client, and AFTER we've loaded the configured
    // `media_library_path` so filesystem verification isn't silently
    // gated off by an empty path. See the call site below for the
    // full sequence.

    // TMDB client (if API key configured)
    let tmdb = init::create_tmdb_client(&pool).await;

    // Image cache
    let image_cache = images::ImageCache::new(&data_path);

    // VPN: bring the tunnel up before librqbit. Three outcomes:
    //   Ok(Some(_)) — tunnel up, bind torrents to it
    //   Ok(None)    — VPN disabled in config, bind to default route
    //   Err(_)      — VPN *was* enabled but failed to start. Fail
    //                 closed: refuse to start the torrent client so
    //                 peer connections can't leak the user's real IP.
    //                 The UI still boots so the user can fix config.
    let (vpn_manager, vpn_interface, vpn_required_but_failed) =
        match init::maybe_start_vpn(&pool).await {
            Ok(Some(m)) => {
                let iface = m.interface_name().to_owned();
                (Some(m), Some(iface), false)
            }
            Ok(None) => (None, None, false),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "VPN failed to start — torrent client disabled to prevent IP leak"
                );
                (None, None, true)
            }
        };

    // Torrent client. Resolves the configured download_path with a
    // sensible default when the row carries `Some("")` (empty
    // string from the first-run insert) or `None`. The empty-string
    // case used to slip past `unwrap_or_else` and reach librqbit
    // verbatim, which then bound the session's default output to
    // "" and let every torrent write to CWD — under
    // `ProtectHome=true` that hits the kino service's empty
    // namespace mount and add_torrent fails with EACCES on the
    // first piece. The trim+filter is what makes the fallback
    // fire reliably.
    let download_path = {
        let path: Option<String> =
            sqlx::query_scalar("SELECT download_path FROM config WHERE id = 1")
                .fetch_optional(&pool)
                .await?
                .flatten()
                .filter(|s: &String| !s.trim().is_empty());
        path.unwrap_or_else(|| format!("{data_path}/downloads"))
    };
    // Belt-and-braces: if the resolved path doesn't exist yet
    // (fresh install where postinst didn't pre-create it, custom
    // path the user just typed), create it before librqbit tries
    // to open files there. Symmetric with the equivalent
    // create_dir_all in init.rs for the config-row paths.
    if let Err(e) = std::fs::create_dir_all(&download_path) {
        tracing::warn!(
            path = %download_path,
            error = %e,
            "couldn't create download_path — librqbit add_torrent will likely fail"
        );
    }
    let torrent_client = if vpn_required_but_failed {
        tracing::warn!(
            "torrent client not started — VPN is required but its tunnel failed. \
             Fix the VPN config (Settings → VPN) and restart kino to re-enable downloads."
        );
        None
    } else {
        match download::torrent_client::LibrqbitClient::new(
            download::torrent_client::TorrentClientConfig {
                download_path: std::path::PathBuf::from(&download_path),
                data_path: std::path::PathBuf::from(&data_path),
                bind_interface: vpn_interface,
                listen_port: 6881,
                announce_port: vpn_manager
                    .as_ref()
                    .and_then(download::vpn::VpnManager::forwarded_port),
                download_speed_limit: None,
                upload_speed_limit: None,
            },
        )
        .await
        {
            Ok(client) => {
                tracing::info!(path = %download_path, "torrent client ready");
                Some(client)
            }
            Err(e) => {
                tracing::warn!(error = %e, "torrent client failed to start — downloads disabled");
                None
            }
        }
    };

    // Startup reconciliation — now that VPN + librqbit are up and
    // we know the configured library path, walk the reconciliation
    // phases. Reads `media_library_path` fresh so phase 5 can
    // verify files on disk (previously hardcoded to `""` which
    // short-circuited the check).
    let media_library_path: String =
        sqlx::query_scalar("SELECT media_library_path FROM config WHERE id = 1")
            .fetch_optional(&pool)
            .await?
            .flatten()
            .unwrap_or_default();
    let reconcile_result = startup::reconcile(
        &pool,
        &media_library_path,
        torrent_client
            .as_ref()
            .map(|c| c as &dyn download::TorrentSession),
    )
    .await?;
    tracing::info!(
        orphans = reconcile_result.orphans_cleaned,
        ghost_torrents = reconcile_result.ghost_torrents_removed,
        downloads = reconcile_result.downloads_reconciled,
        entities = reconcile_result.entities_fixed,
        files_verified = reconcile_result.files_verified,
        files_verified_lazily = reconcile_result.files_verified_lazily,
        library_path = %media_library_path,
        "startup reconciliation complete"
    );

    // Transcode manager
    let transcode_temp = std::path::PathBuf::from(&data_path).join("transcode-temp");
    // Clean up stale transcode files from previous run. First call
    // normally fails with NotFound on a fresh boot; warn on anything
    // else. Create must succeed.
    if let Err(e) = std::fs::remove_dir_all(&transcode_temp)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %transcode_temp.display(), error = %e, "failed to clean transcode temp dir");
    }
    if let Err(e) = std::fs::create_dir_all(&transcode_temp) {
        tracing::error!(path = %transcode_temp.display(), error = %e, "failed to create transcode temp dir");
    }
    // Read ffmpeg binary path + HWA preference from config so the
    // settings UI, probe cache, and live transcoder all stay in
    // lockstep. Empty/missing values fall through to sensible
    // defaults: bare "ffmpeg" on `PATH` for the binary, software
    // encoding for the backend.
    let (ffmpeg_path, hw_method): (String, String) = {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT ffmpeg_path, hw_acceleration FROM config WHERE id = 1")
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();
        let (ff, hw) = row.unwrap_or((None, None));
        (
            ff.filter(|s| !s.is_empty())
                .unwrap_or_else(|| "ffmpeg".to_string()),
            hw.unwrap_or_else(|| "none".to_string()),
        )
    };
    let hwaccel = playback::transcode::HwAccel::from_config(&hw_method);
    tracing::info!(
        ffmpeg = %ffmpeg_path,
        hw_acceleration = %hw_method,
        "transcode manager configured"
    );
    // Shared broadcast channel — owned by AppState, threaded into
    // the transcode manager so the per-session HWA-failure
    // watchdog can emit `HealthWarning` without reaching into
    // state. Constructed here so both consumers get the same
    // sender (distinct channels would mean the watchdog's events
    // never reach the WS fan-out).
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel::<events::AppEvent>(512);
    let transcode_manager = playback::transcode::TranscodeManager::new(
        transcode_temp,
        &ffmpeg_path,
        hwaccel,
        Some(event_tx.clone()),
    );

    // Indexer definitions — load whatever's already on disk, but
    // NEVER block startup on a remote fetch. First-run installs
    // come up with an empty loader; the setup wizard's Library
    // Sources step calls `POST /api/v1/indexer-definitions/refresh`
    // to download the catalogue with progress UI. The daily
    // `definitions_refresh` scheduler task keeps it current
    // afterwards. Both paths route through `indexers::refresh::
    // start_refresh` so the WS event stream + the persisted
    // `definitions_last_refreshed_at` timestamp work uniformly.
    //
    // Pre-2026-04-28 behaviour (inline blocking fetch on first
    // run) delayed the HTTP listener bind by ~68s while pulling
    // 547 YAMLs from GitHub — users hit a 60+ second window of
    // "site can't be reached" before the wizard appeared. Don't
    // re-introduce.
    let definitions = {
        let defs_dir = std::path::PathBuf::from(&data_path).join("definitions");
        let loader = indexers::loader::DefinitionLoader::new(defs_dir);
        if let Err(e) = loader.load_all() {
            tracing::warn!(error = %e, "failed to load indexer definitions");
        }
        let n = loader.count();
        if n > 0 {
            tracing::info!(count = n, "indexer definitions loaded from disk");
        } else {
            tracing::info!(
                "no indexer definitions on disk yet — wizard will trigger fetch on demand"
            );
        }
        Some(loader)
    };

    // Scheduler — `auto_search_interval` drives the wanted-search
    // task. The metadata-refresh tick is fixed at 30 min; per-row
    // tiering lives in `metadata::refresh::refresh_sweep`, so there's
    // no single user-facing interval to honour any more. Falls back
    // to a sensible default if the config row is missing (first boot,
    // mid-migration, etc.).
    let sched = scheduler::Scheduler::new(pool.clone());
    let search_min: i64 =
        sqlx::query_scalar("SELECT auto_search_interval FROM config WHERE id = 1")
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(15);
    sched
        .register_defaults(u64::try_from(search_min.max(1)).unwrap_or(15))
        .await;

    // Cloudflare solver (TLS fingerprint + Camoufox browser fallback)
    let cf_solver =
        indexers::cloudflare::CloudflareSolver::new(std::path::PathBuf::from(&data_path));

    let intro_concurrency: i64 =
        sqlx::query_scalar("SELECT max_concurrent_intro_analyses FROM config WHERE id = 1")
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(2);
    // Resolve + bind the HTTP port BEFORE constructing AppState so
    // `state.http_port` reflects the actual bound port. Ordering
    // matters: the transcode HLS path builds an internal URL like
    // `http://127.0.0.1:{state.http_port}/api/v1/play/...` for ffmpeg
    // to fetch. If `http_port` is `0` (the CLI default sentinel
    // before this resolution), ffmpeg gets `127.0.0.1:0` and dies
    // with "Port missing in uri" — which presents as a "long delay
    // then plays after full download" because the frontend retries
    // until the file is complete and library lookup takes over.
    //
    // CLI / KINO_PORT explicit override wins (`cli.port != 0`).
    // Otherwise read from `config.listen_port` — that's what
    // Settings → Port writes, and what the user expects to see
    // honoured on restart. The DB column was just seeded by
    // `ensure_defaults` from the KINO_PORT env (if any) on first
    // run.
    let requested_port: u16 = if cli.port != 0 {
        cli.port
    } else {
        let from_db: Option<i64> =
            sqlx::query_scalar("SELECT listen_port FROM config WHERE id = 1")
                .fetch_optional(&pool)
                .await?;
        u16::try_from(from_db.unwrap_or(80)).unwrap_or(80)
    };
    let (listener, effective_port) = bind_with_fallback(requested_port).await?;
    let bound = listener.local_addr()?;
    tracing::info!(addr = %bound, "HTTP listener bound, accepting requests");
    write_runtime_port_file(bound.port());
    // The URL file gets written below, AFTER mDNS publishes —
    // we need the resolved hostname (which may differ from
    // config.mdns_hostname when collision detection bumped a
    // suffix on top of it).

    let (state, trigger_rx) = AppState::new(
        pool.clone(),
        tmdb,
        Some(image_cache),
        Some(sched.clone()),
        torrent_client,
        Some(transcode_manager),
        definitions,
        Some(cf_solver),
        vpn_manager,
        std::path::PathBuf::from(&data_path),
        effective_port,
        log_bus.live.clone(),
        event_tx,
        u32::try_from(intro_concurrency.max(1)).unwrap_or(2),
    );
    let cancel = state.cancel.clone();

    // Bridge the optional external cancellation source (Windows SCM
    // dispatcher's `SERVICE_CONTROL_STOP` handler today; leave room
    // for other callers later) into the AppState's cancel token, so
    // shutdown_signal() and every task spawned with `cancel.clone()`
    // observe it identically to a Unix SIGTERM / ctrl+c.
    if let Some(ext) = external_cancel {
        let inner = cancel.clone();
        tokio::spawn(async move {
            ext.cancelled().await;
            inner.cancel();
        });
    }

    // Spawn background tasks
    let tracker = tokio_util::task::TaskTracker::new();

    // Event listeners (history, WebSocket, webhooks, post-import hooks)
    tracker.spawn(events::listeners::run_event_listeners(
        state.clone(),
        cancel.clone(),
    ));

    // Scheduler execution loop. Pass the tracker in so every task
    // the scheduler spawns for a due tick or a manual trigger is
    // registered on the same shutdown tracker as the scheduler
    // loop itself — the 10-second graceful-shutdown window covers
    // in-flight DB writes instead of aborting them mid-transaction.
    {
        let sched = state.scheduler.clone().expect("scheduler initialized");
        let sched_state = state.clone();
        let sched_tracker = tracker.clone();
        tracker.spawn(async move {
            sched.run(sched_state, trigger_rx, sched_tracker).await;
        });
    }

    // Warm the HW-accel probe cache so the status banner has
    // something to read on the first /status call. Runs in the
    // background — startup doesn't wait for ffmpeg trial encodes.
    // Reuses the `ffmpeg_path` resolved above so probe + transcoder
    // can never disagree about which binary they're talking to.
    {
        let ffmpeg = ffmpeg_path.clone();
        tracker.spawn(async move {
            crate::playback::hw_probe_cache::detect_and_cache(&ffmpeg).await;
        });
    }

    tracing::info!("background tasks started");

    // HTTP server with graceful shutdown.
    //
    // Bind explicitly with a grep-friendly success line. Bind errors
    // (`Address already in use` when running alongside another kino
    // on the same port) propagate via `?` and terminate the binary —
    // systemd's `Restart=on-failure` will then visibly crash-loop
    // rather than letting the daemon limp along with no HTTP
    // listener. Wrap with `with_context` so the journal entry names
    // the port, not just the bare OS error.
    // Listener was bound earlier (before AppState::new) so
    // state.http_port reflects reality. The rest of this function
    // uses `effective_port` (the actually-bound port) rather than
    // `cli.port` because we may have fallen back from 80 → 8080.
    let app = build_router(state);
    let port = effective_port;

    // Auto-open the setup wizard in the user's default browser on
    // first run. Gated on (a) a fresh DB (no config row pre-existed),
    // (b) a desktop session being available (DISPLAY / WAYLAND_DISPLAY
    // on Linux; assume yes on macOS / Windows), and (c) the user not
    // having opted out via `--no-open-browser` / `KINO_NO_OPEN_BROWSER`.
    // Native packages set the opt-out in the systemd unit / launchd
    // plist / Windows Service descriptor so headless service-mode
    // never tries to spawn a browser.
    if was_first_run && !cli.no_open_browser && has_desktop_session() {
        let url = format!("http://localhost:{port}");
        // Brief delay so the listener has a chance to actually accept
        // before the browser hits it. 500ms is well under the time
        // it takes a browser to launch.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match webbrowser::open(&url) {
                Ok(()) => tracing::info!(url = %url, "opened setup wizard in default browser"),
                Err(e) => tracing::warn!(
                    error = %e,
                    url = %url,
                    "first-run browser open failed; visit the URL manually"
                ),
            }
        });
    }

    // mDNS responder. Held in a binding so its `Drop` fires on
    // process exit and sends the unregister goodbye; otherwise
    // neighbours' caches keep our name until the TTL elapses.
    let mdns_settings = mdns::load_settings(&pool).await;
    let mdns_handle_keepalive = match mdns::start(&mdns_settings, port) {
        Ok(handle) => handle,
        Err(e) => {
            tracing::warn!(error = %e, "mDNS responder failed to start; continuing without it");
            None
        }
    };
    // Now write the runtime URL file with the actually-published
    // hostname (post collision detection) so the tray + `kino open`
    // pick up the right name.
    let resolved_hostname = mdns_handle_keepalive
        .as_ref()
        .map(|h| h.resolved_hostname.clone());
    write_runtime_url_file(port, resolved_hostname.as_deref(), mdns_settings.enabled);
    // Hold the handle so its `Drop` fires the unregister goodbye on
    // process exit. Renamed away from the leading-underscore because
    // we read `.resolved_hostname` above (clippy warns on underscore-
    // prefixed bindings that get accessed).
    let _mdns_handle = mdns_handle_keepalive;

    // Cast device discovery — long-running mDNS browser populating
    // `cast_device`. Runs alongside the mDNS responder above; the
    // two don't fight because the responder *announces* and the
    // browser *queries* on the same multicast address.
    if let Err(e) = cast_sender::discovery::spawn(pool.clone()) {
        tracing::warn!(
            error = %e,
            "cast: mDNS browse failed to start; manual device-add still works",
        );
    }

    // `into_make_service_with_connect_info` exposes the peer's
    // SocketAddr to handlers via `ConnectInfo<SocketAddr>` — used by
    // the sessions API to log the originating IP on session creates,
    // QR-code redemptions, and brute-force-rejected attempts.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(cancel.clone()))
    .await?;

    // Server stopped — shut down background tasks
    tracing::info!("shutting down...");
    cancel.cancel();
    tracker.close();

    tokio::select! {
        () = tracker.wait() => tracing::info!("all background tasks stopped"),
        () = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
            tracing::warn!("shutdown timeout, some tasks did not complete");
        }
    }

    Ok(())
}

fn reset_data_sync(data_path: &str) -> anyhow::Result<()> {
    let data_dir = std::path::Path::new(data_path);

    for ext in ["kino.db", "kino.db-wal", "kino.db-shm"] {
        let path = data_dir.join(ext);
        if path.exists() {
            std::fs::remove_file(&path)?;
            tracing::info!(path = %path.display(), "deleted");
        }
    }

    // Wipe every directory keyed by an autoincrement id (`media_id`,
    // `download_id`, HLS session). The DB reset rolls AUTOINCREMENT
    // back to 1, so leaving these in place lets a fresh import inherit
    // a previous run's sprites / extracted subs / transcode segments
    // and serve them under the new id. Image cache stays — keyed by
    // `tmdb_id`, stable across resets, expensive to re-fetch.
    //
    // `definitions` is the Cardigann YAML catalogue downloaded from
    // the third-party Prowlarr/Indexers repo — it's a pure cache
    // (the manual refresh button rebuilds it on demand). Including
    // it in reset means `just reset` produces a true first-run
    // state where the consent gate is honoured: no catalogue on
    // disk, no auto-refresh, wizard surfaces the Download CTA.
    for sub in [
        "librqbit",
        "trickplay",
        "trickplay-stream",
        "cache",
        "transcode-temp",
        "definitions",
    ] {
        let path = data_dir.join(sub);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
            tracing::info!(path = %path.display(), "cleared");
        }
    }

    tracing::info!("reset complete — restart kino to begin fresh setup");
    Ok(())
}

pub(crate) const RUNTIME_PORT_FILE_LINUX: &str = "/run/kino/port";

/// Resolve the port the live kino server is bound on. Used by
/// `kino tray` and `kino open` (both run in the user's session,
/// outside the systemd service's env). Order:
///
/// 1. `/run/kino/port` (Linux) — authoritative; the running
///    server writes the bound port here AFTER the listener is up,
///    including the 80 → 8080 fallback case. If this exists, trust
///    it absolutely.
/// 2. `$KINO_PORT` env — useful for macOS / Windows where we don't
///    write a runtime file yet, and as a manual override.
/// 3. None — caller defaults to 80 (matches the schema default
///    so a stale tray clicks don't surprise the user).
pub(crate) fn discovered_port() -> Option<u16> {
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string(RUNTIME_PORT_FILE_LINUX)
        && let Ok(p) = s.trim().parse::<u16>()
    {
        return Some(p);
    }
    if let Ok(s) = std::env::var("KINO_PORT")
        && let Ok(p) = s.trim().parse::<u16>()
        && p != 0
    {
        return Some(p);
    }
    None
}

/// Single-instance gate for `kino serve`. Cross-platform via fs4
/// (flock on Unix, `LockFileEx` on Windows). Lock file lives in the
/// data directory so a second invocation pointing at a different
/// `KINO_DATA_PATH` is allowed (multi-tenant homelab use case) but
/// two processes against the same data dir refuse to coexist.
///
/// Returns the `File` handle as the lock guard — the OS lock is
/// held for the lifetime of the file. Dropping it (process exit,
/// crash, kill) releases the lock via kernel cleanup.
fn acquire_serve_lock(data_path: &str) -> anyhow::Result<std::fs::File> {
    use fs4::fs_std::FileExt as _;
    let dir = std::path::PathBuf::from(data_path);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating data dir {}", dir.display()))?;
    let path = dir.join("kino-serve.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening serve lock file at {}", path.display()))?;
    file.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "another `kino serve` is already running against {}. \
             Stop it first (`sudo systemctl stop kino` for the system \
             service, or `pkill kino`) before starting a second instance.",
            dir.display()
        )
    })?;
    Ok(file)
}

/// Bind the HTTP listener with a graceful fallback. If the user
/// asked for port 80 (the systemd-service default) and the bind
/// fails with `EACCES` (no `CAP_NET_BIND_SERVICE`) or `EADDRINUSE`
/// (something else holds it — nginx, an old kino, …), we drop down
/// to 8080 and emit a warning. This keeps the service alive on
/// hosts where 80 is unavailable instead of crash-looping, and the
/// degraded state surfaces in `/api/v1/status` warnings so the UI
/// can surface "we wanted :80 but had to use :8080" guidance. For
/// any other port the user explicitly asked for, no fallback —
/// failing fast is the right behaviour.
async fn bind_with_fallback(requested: u16) -> anyhow::Result<(tokio::net::TcpListener, u16)> {
    let bind_addr = format!("0.0.0.0:{requested}");
    match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => Ok((l, requested)),
        Err(e) if requested == 80 => {
            tracing::warn!(
                error = %e,
                requested,
                fallback = 8080,
                "couldn't bind privileged port; falling back to 8080. \
                 Other devices on the LAN will need to use http://kino.local:8080 \
                 instead of bare http://kino.local. To fix permanently: \
                 grant CAP_NET_BIND_SERVICE on Linux (already set in our systemd \
                 unit), free up port 80 (stop nginx / Apache), or set \
                 KINO_PORT to a different port."
            );
            let fallback = "0.0.0.0:8080";
            let l = tokio::net::TcpListener::bind(fallback)
                .await
                .with_context(|| {
                    format!("HTTP listener failed to bind {bind_addr} AND fallback {fallback}")
                })?;
            Ok((l, 8080))
        }
        Err(e) => {
            Err(anyhow::Error::from(e).context(format!("HTTP listener failed to bind {bind_addr}")))
        }
    }
}

fn open_browser_at_port() -> anyhow::Result<()> {
    let url = discovered_url();
    webbrowser::open(&url).map_err(|e| anyhow::anyhow!("failed to open browser at {url}: {e}"))?;

    // Best-effort: also start the tray indicator if it isn't already
    // running. `kino open` is the user's first interactive entry point
    // — fired from the .desktop launcher in the app menu — so it
    // executes inside the user's desktop session naturally (no PAM /
    // D-Bus / postinst gymnastics needed). The tray binary's
    // single-instance lock makes a duplicate spawn a clean no-op,
    // so we don't need to probe first; let the lock arbitrate. On
    // headless or non-tray builds this branch is compiled out.
    spawn_tray_detached();
    Ok(())
}

/// Spawn `kino tray` as a detached child so the parent (the `open`
/// invocation) can exit immediately. The tray reparents to init /
/// the user's systemd manager and continues running independently.
/// All errors are swallowed — a failed tray spawn must not break
/// the headline "open the browser" UX.
#[cfg(feature = "tray")]
fn spawn_tray_detached() {
    use std::process::Stdio;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg("tray")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(feature = "tray"))]
const fn spawn_tray_detached() {}

#[cfg(target_os = "linux")]
fn write_runtime_port_file(port: u16) {
    let path = std::path::Path::new(RUNTIME_PORT_FILE_LINUX);
    let Some(parent) = path.parent() else {
        return;
    };
    if !parent.exists() {
        return;
    }
    if let Err(e) = std::fs::write(path, port.to_string()) {
        tracing::debug!(error = %e, path = %path.display(), "couldn't write runtime port file");
    }
}

#[cfg(not(target_os = "linux"))]
fn write_runtime_port_file(_port: u16) {}

#[cfg(target_os = "linux")]
pub(crate) const RUNTIME_URL_FILE_LINUX: &str = "/run/kino/url";

/// Write the canonical "open this URL" hint that `kino tray` and
/// `kino open` consume. Composed from the configured `mdns_hostname`
/// and the actual bound port, with mDNS-aware fallbacks. Lives
/// alongside `/run/kino/port` because they're both runtime-only
/// state the supervisor cleans up via `RuntimeDirectory=kino` in
/// the unit.
///
/// URL composition:
/// - mDNS enabled + port 80 → `http://<host>.local`
/// - mDNS enabled + non-80   → `http://<host>.local:<port>`
/// - mDNS disabled + port 80 → `http://localhost`
/// - mDNS disabled + non-80  → `http://localhost:<port>`
fn write_runtime_url_file(port: u16, resolved_hostname: Option<&str>, mdns_enabled: bool) {
    #[cfg(target_os = "linux")]
    {
        let path = std::path::Path::new(RUNTIME_URL_FILE_LINUX);
        let Some(parent) = path.parent() else {
            return;
        };
        if !parent.exists() {
            return;
        }
        let host = match (mdns_enabled, resolved_hostname) {
            (true, Some(h)) if !h.trim().is_empty() => format!("{}.local", h.trim()),
            _ => "localhost".to_string(),
        };
        let url = if port == 80 {
            format!("http://{host}")
        } else {
            format!("http://{host}:{port}")
        };
        if let Err(e) = std::fs::write(path, &url) {
            tracing::debug!(error = %e, path = %path.display(), "couldn't write runtime url file");
        } else {
            tracing::info!(url = %url, "kino URL: {url}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = port;
        let _ = resolved_hostname;
        let _ = mdns_enabled;
    }
}

/// Read the canonical URL written by `write_runtime_url_file`.
/// Falls back to `http://localhost[:<port>]` based on the runtime
/// port file when the URL file is missing (e.g. on macOS / Windows
/// where we don't write it yet, or before the server has booted).
pub(crate) fn discovered_url() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string(RUNTIME_URL_FILE_LINUX) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    let port = discovered_port().unwrap_or(80);
    if port == 80 {
        "http://localhost".to_owned()
    } else {
        format!("http://localhost:{port}")
    }
}

/// `kino setup-permissions <path>` — grant the kino service user
/// rwx on a folder via POSIX ACLs. Lowest-impact way to share an
/// external drive (mounted under `/media/<user>/...` with the
/// desktop user's perms by default) with the systemd-installed
/// kino service. Reversible: `sudo setfacl -R -x u:kino <path>`.
#[cfg(target_os = "linux")]
fn setup_permissions(path: &str) -> anyhow::Result<()> {
    use std::process::Command;

    let euid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    if euid != "0" {
        eprintln!(
            "kino setup-permissions must run as root.\n\nTry:  sudo kino setup-permissions {path}"
        );
        std::process::exit(1);
    }
    let p = std::path::Path::new(path);
    if !p.is_dir() {
        anyhow::bail!("{path} is not a directory");
    }

    println!("Granting kino user rwx on {path} via ACLs…");
    let recursive = Command::new("setfacl")
        .args(["-R", "-m", "u:kino:rwx", path])
        .status()
        .map_err(|e| {
            anyhow::anyhow!("couldn't exec setfacl ({e}). Install acl: sudo apt install acl")
        })?;
    if !recursive.success() {
        anyhow::bail!(
            "setfacl failed (exit {}). Common causes: filesystem doesn't support ACLs (NTFS often), \
             or the path is on a noexec / read-only mount. Mount with `acl` option, or use `chgrp kino \
             {path} && chmod g+rwxs {path}` instead.",
            recursive.code().unwrap_or(-1)
        );
    }
    let default = Command::new("setfacl")
        .args(["-R", "-d", "-m", "u:kino:rwx", path])
        .status();
    match default {
        Ok(s) if s.success() => {}
        _ => eprintln!(
            "warning: default-ACL pass failed; existing files are accessible but new files \
             created later may not inherit. Mostly fine if kino is doing the file creation."
        ),
    }

    println!("\n✓ Done. Verify by browsing to {path} in kino's path picker.");
    println!("  To revert later: sudo setfacl -R -x u:kino {path}");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn setup_permissions(_path: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "kino setup-permissions is Linux-only. macOS / Windows: grant the kino service account \
         access via the platform's native ACL tools (chmod +a on macOS, icacls on Windows)."
    )
}

/// `kino allow-firewall` — opens inbound TCP 80/8080 + UDP 5353
/// (mDNS) so LAN clients can reach `http://kino.local`. Triggers a
/// graphical privilege prompt on each platform; the user types
/// their password into the OS's native dialog rather than dropping
/// to a terminal `sudo` invocation.
///
/// Idempotent. Re-running once a rule is in place is a no-op
/// (UFW/firewalld/netsh/socketfilterfw all dedupe).
#[cfg(target_os = "linux")]
fn allow_firewall() -> anyhow::Result<()> {
    use std::process::Command;

    eprintln!("kino allow-firewall — opens inbound 80/tcp, 8080/tcp, 5353/udp");

    // Detect which firewall is active. Don't try to mutate anything
    // that isn't on (ufw inactive → user doesn't need a rule; either
    // they have no firewall or use raw nftables which we don't auto-
    // configure). Detection happens unprivileged so we can give the
    // right preview before we ask for a password.
    let ufw_active = Command::new("ufw")
        .arg("status")
        .output()
        .ok()
        .and_then(|o| {
            o.status
                .success()
                .then(|| String::from_utf8_lossy(&o.stdout).contains("Status: active"))
        })
        .unwrap_or(false);
    let firewalld_active = Command::new("firewall-cmd")
        .arg("--state")
        .output()
        .ok()
        .and_then(|o| {
            o.status
                .success()
                .then(|| String::from_utf8_lossy(&o.stdout).contains("running"))
        })
        .unwrap_or(false);

    if !ufw_active && !firewalld_active {
        eprintln!(
            "  No active firewall detected (UFW inactive, firewalld not running).\n  \
             Either you have no firewall blocking inbound traffic, or you're using\n  \
             raw nftables / iptables — kino doesn't auto-configure those (admin convention).\n  \
             If LAN clients still can't reach http://kino.local, run:\n    \
             sudo nft add rule inet filter input tcp dport {{80,8080}} accept\n    \
             sudo nft add rule inet filter input udp dport 5353 accept"
        );
        return Ok(());
    }

    // pkexec triggers the desktop's PolicyKit agent (gnome-shell,
    // kde-polkit, polkit-gnome) to render a graphical password
    // prompt. Falls back to a terminal sudo invocation when the
    // user is on a system without polkit (Arch minimal, headless).
    let pkexec_available = Command::new("which")
        .arg("pkexec")
        .output()
        .ok()
        .is_some_and(|o| o.status.success());

    let runner = if pkexec_available { "pkexec" } else { "sudo" };

    if ufw_active {
        eprintln!("  → UFW active. Adding rules via {runner}...");
        for spec in &[
            ("80", "tcp", "Kino HTTP"),
            ("8080", "tcp", "Kino HTTP fallback"),
            ("5353", "udp", "mDNS for kino.local"),
        ] {
            let (port, proto, comment) = spec;
            let status = Command::new(runner)
                .args([
                    "ufw",
                    "allow",
                    &format!("{port}/{proto}"),
                    "comment",
                    comment,
                ])
                .status();
            match status {
                Ok(s) if s.success() => eprintln!("    ✓ {port}/{proto}"),
                Ok(s) => anyhow::bail!(
                    "ufw allow {port}/{proto} failed (exit {})",
                    s.code().unwrap_or(-1)
                ),
                Err(e) => anyhow::bail!("running {runner} ufw allow {port}/{proto}: {e}"),
            }
        }
    }

    if firewalld_active {
        eprintln!("  → firewalld active. Adding kino service via {runner}...");
        // The .deb / .rpm ships /usr/lib/firewalld/services/kino.xml
        // which bundles all three port rules under one service name.
        let status = Command::new(runner)
            .args(["firewall-cmd", "--permanent", "--add-service=kino"])
            .status();
        match status {
            Ok(s) if s.success() => eprintln!("    ✓ added kino service"),
            Ok(s) => anyhow::bail!(
                "firewall-cmd add-service kino failed (exit {}). \
                 If the kino service is unknown, the package may be older — fall back to:\n  \
                 {runner} firewall-cmd --permanent --add-port=80/tcp \\\n    \
                 --add-port=8080/tcp --add-port=5353/udp",
                s.code().unwrap_or(-1)
            ),
            Err(e) => anyhow::bail!("running {runner} firewall-cmd: {e}"),
        }
        let reload = Command::new(runner)
            .args(["firewall-cmd", "--reload"])
            .status();
        if let Ok(s) = reload
            && s.success()
        {
            eprintln!("    ✓ reloaded firewalld");
        }
    }

    eprintln!("\n✓ Done. Test from a phone / TV: http://kino.local");
    Ok(())
}

#[cfg(target_os = "macos")]
fn allow_firewall() -> anyhow::Result<()> {
    use std::process::Command;

    eprintln!("kino allow-firewall — registering kino with macOS Application Firewall");

    let exe = std::env::current_exe()
        .context("locating current binary path")?
        .to_string_lossy()
        .into_owned();

    // macOS ALF is per-app, not per-port. We add the binary and
    // unblock incoming connections. `osascript ... with administrator
    // privileges` triggers the native Touch ID / password dialog.
    // The two commands are idempotent.
    let script = format!(
        "do shell script \"/usr/libexec/ApplicationFirewall/socketfilterfw --add '{exe}' && \
         /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp '{exe}'\" \
         with administrator privileges"
    );

    let status = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .context("running osascript for the admin prompt")?;
    if !status.success() {
        anyhow::bail!(
            "elevation declined or socketfilterfw failed (osascript exit {}).",
            status.code().unwrap_or(-1)
        );
    }
    eprintln!("\n✓ Registered with macOS firewall. Test from a phone: http://kino.local");
    Ok(())
}

#[cfg(target_os = "windows")]
fn allow_firewall() -> anyhow::Result<()> {
    use std::process::Command;

    eprintln!("kino allow-firewall — adding Windows Defender Firewall rules (UAC prompt incoming)");

    // ShellExecute with the `runas` verb spawns the child elevated
    // via UAC. We pass the netsh command via cmd.exe /c so the rule
    // string survives quoting. Three rules: HTTP TCP 80, fallback
    // TCP 8080, mDNS UDP 5353.
    let netsh_cmd = "netsh advfirewall firewall add rule name=\"Kino HTTP\" dir=in action=allow protocol=TCP localport=80 & \
                     netsh advfirewall firewall add rule name=\"Kino HTTP fallback\" dir=in action=allow protocol=TCP localport=8080 & \
                     netsh advfirewall firewall add rule name=\"Kino mDNS\" dir=in action=allow protocol=UDP localport=5353";

    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("Start-Process cmd -ArgumentList '/c','{netsh_cmd}' -Verb RunAs -Wait"),
        ])
        .status()
        .context("running powershell to elevate netsh")?;
    if !status.success() {
        anyhow::bail!(
            "elevation declined or netsh failed (powershell exit {}).",
            status.code().unwrap_or(-1)
        );
    }
    eprintln!("\n✓ Firewall rules added. Test from a phone: http://kino.local");
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn allow_firewall() -> anyhow::Result<()> {
    anyhow::bail!("kino allow-firewall isn't implemented for this platform")
}

/// Best-effort detection of "is there a desktop session we could
/// pop a browser window into?" Used to gate the auto-open-on-first-
/// run UX so service-mode (systemd, launchd `LaunchDaemon`, Windows
/// Service) doesn't try to spawn a browser into nothing.
///
/// - **Linux**: `DISPLAY` (X11) or `WAYLAND_DISPLAY`. systemd unit
///   doesn't inherit either by default
/// - **macOS / Windows**: assume yes when running interactively.
///   The native packages still pass `--no-open-browser` in their
///   service descriptors as a belt-and-braces opt-out
fn has_desktop_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

async fn shutdown_signal(cancel: tokio_util::sync::CancellationToken) {
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::select! {
        _ = ctrl_c => tracing::info!("received ctrl+c"),
        () = cancel.cancelled() => {},
    }
}

#[allow(clippy::too_many_lines)]
fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/api/v1/status",
            axum::routing::get(api::status::get_status),
        )
        .route(
            "/api/v1/network/lan-probe",
            axum::routing::get(api::network::lan_probe),
        )
        .route(
            "/api/v1/network/mdns-test",
            axum::routing::post(api::network::mdns_test),
        )
        .route(
            "/api/v1/health",
            axum::routing::get(api::health::get_health),
        )
        .route(
            "/api/v1/diagnostics/export",
            axum::routing::get(api::diagnostics::export_bundle),
        )
        .route(
            "/api/v1/bootstrap",
            axum::routing::get(auth_session::handlers::bootstrap),
        )
        .route(
            "/api/v1/sessions",
            axum::routing::post(auth_session::handlers::create_session)
                .get(auth_session::handlers::list_sessions)
                .delete(auth_session::handlers::revoke_all),
        )
        .route(
            "/api/v1/sessions/redeem",
            axum::routing::post(auth_session::handlers::redeem),
        )
        .route(
            "/api/v1/sessions/cli",
            axum::routing::post(auth_session::handlers::create_cli_token),
        )
        .route(
            "/api/v1/sessions/bootstrap-token",
            axum::routing::post(auth_session::handlers::create_bootstrap_token),
        )
        .route(
            "/api/v1/sessions/sign-url",
            axum::routing::post(auth_session::handlers::sign_url),
        )
        .route(
            "/api/v1/sessions/{id}",
            axum::routing::delete(auth_session::handlers::revoke_session),
        )
        .route(
            "/api/v1/logout",
            axum::routing::post(auth_session::handlers::logout),
        )
        .route(
            "/api/v1/config",
            axum::routing::get(settings::config::get_config).put(settings::config::update_config),
        )
        .route(
            "/api/v1/config/rotate-api-key",
            axum::routing::post(settings::config::rotate_api_key),
        )
        .route(
            "/api/v1/preferences/home",
            axum::routing::get(home::preferences::get_home_preferences)
                .patch(home::preferences::update_home_preferences),
        )
        .route(
            "/api/v1/preferences/home/reset",
            axum::routing::post(home::preferences::reset_home_preferences),
        )
        // Trakt integration — device-code OAuth + sync + scrobble.
        // See docs/subsystems/16-trakt.md.
        .route(
            "/api/v1/integrations/trakt/status",
            axum::routing::get(integrations::trakt::handlers::status),
        )
        .route(
            "/api/v1/integrations/trakt/device-code",
            axum::routing::post(integrations::trakt::handlers::begin_device),
        )
        .route(
            "/api/v1/integrations/trakt/device-poll",
            axum::routing::post(integrations::trakt::handlers::poll_device),
        )
        .route(
            "/api/v1/integrations/trakt/disconnect",
            axum::routing::post(integrations::trakt::handlers::disconnect),
        )
        .route(
            "/api/v1/integrations/trakt/dry-run",
            axum::routing::get(integrations::trakt::handlers::dry_run),
        )
        .route(
            "/api/v1/integrations/trakt/import",
            axum::routing::post(integrations::trakt::handlers::import),
        )
        .route(
            "/api/v1/integrations/trakt/sync",
            axum::routing::post(integrations::trakt::handlers::sync_now),
        )
        .route(
            "/api/v1/integrations/trakt/recommendations",
            axum::routing::get(integrations::trakt::handlers::recommendations),
        )
        .route(
            "/api/v1/integrations/trakt/trending",
            axum::routing::get(integrations::trakt::handlers::trending),
        )
        .route(
            "/api/v1/integrations/trakt/rate/{kind}/{id}",
            axum::routing::post(integrations::trakt::handlers::rate),
        )
        .route(
            "/api/v1/lists",
            axum::routing::get(integrations::lists::handlers::list_lists)
                .post(integrations::lists::handlers::create_list),
        )
        .route(
            "/api/v1/lists/{id}",
            axum::routing::get(integrations::lists::handlers::get_list)
                .delete(integrations::lists::handlers::delete_list),
        )
        .route(
            "/api/v1/lists/{id}/refresh",
            axum::routing::post(integrations::lists::handlers::refresh_list),
        )
        .route(
            "/api/v1/lists/{id}/items",
            axum::routing::get(integrations::lists::handlers::list_items),
        )
        .route(
            "/api/v1/lists/{id}/items/{item_id}/ignore",
            axum::routing::post(integrations::lists::handlers::ignore_item),
        )
        .route("/api/v1/fs/test", axum::routing::get(api::fs::test_path))
        .route("/api/v1/fs/browse", axum::routing::get(api::fs::browse))
        .route("/api/v1/fs/mkdir", axum::routing::post(api::fs::mkdir))
        .route("/api/v1/fs/mounts", axum::routing::get(api::fs::mounts))
        .route("/api/v1/fs/places", axum::routing::get(api::fs::places))
        .route(
            "/api/v1/metadata/test-tmdb",
            axum::routing::post(metadata::test_handlers::test_tmdb),
        )
        .route(
            "/api/v1/metadata/test-opensubtitles",
            axum::routing::post(metadata::test_handlers::test_opensubtitles),
        )
        .route(
            "/api/v1/vpn/status",
            axum::routing::get(download::vpn::handlers::get_status),
        )
        .route(
            "/api/v1/vpn/test",
            axum::routing::post(download::vpn::handlers::test_connection),
        )
        .route(
            "/api/v1/quality-profiles",
            axum::routing::get(settings::quality_profile::list_quality_profiles)
                .post(settings::quality_profile::create_quality_profile),
        )
        .route(
            "/api/v1/quality-profiles/{id}",
            axum::routing::get(settings::quality_profile::get_quality_profile)
                .put(settings::quality_profile::update_quality_profile)
                .delete(settings::quality_profile::delete_quality_profile),
        )
        .route(
            "/api/v1/quality-profiles/{id}/set-default",
            axum::routing::post(settings::quality_profile::set_default_quality_profile),
        )
        // TMDB proxy
        .route(
            "/api/v1/tmdb/search",
            axum::routing::get(metadata::tmdb_handlers::search),
        )
        .route(
            "/api/v1/tmdb/movies/{tmdb_id}",
            axum::routing::get(metadata::tmdb_handlers::movie_details),
        )
        .route(
            "/api/v1/tmdb/shows/{tmdb_id}",
            axum::routing::get(metadata::tmdb_handlers::show_details),
        )
        .route(
            "/api/v1/tmdb/shows/{tmdb_id}/seasons/{season_number}",
            axum::routing::get(metadata::tmdb_handlers::season_details),
        )
        .route(
            "/api/v1/tmdb/trending/movies",
            axum::routing::get(metadata::tmdb_handlers::trending_movies),
        )
        .route(
            "/api/v1/tmdb/trending/shows",
            axum::routing::get(metadata::tmdb_handlers::trending_shows),
        )
        .route(
            "/api/v1/tmdb/discover/movies",
            axum::routing::get(metadata::tmdb_handlers::discover_movies),
        )
        .route(
            "/api/v1/tmdb/discover/shows",
            axum::routing::get(metadata::tmdb_handlers::discover_shows),
        )
        .route(
            "/api/v1/tmdb/genres",
            axum::routing::get(metadata::tmdb_handlers::genres),
        )
        // Movies
        .route(
            "/api/v1/movies",
            axum::routing::get(content::movie::handlers::list_movies)
                .post(content::movie::handlers::create_movie),
        )
        .route(
            "/api/v1/movies/{id}",
            axum::routing::get(content::movie::handlers::get_movie)
                .delete(content::movie::handlers::delete_movie),
        )
        // Shows
        .route(
            "/api/v1/shows",
            axum::routing::get(content::show::handlers::list_shows)
                .post(content::show::handlers::create_show),
        )
        .route(
            "/api/v1/shows/{id}",
            axum::routing::get(content::show::handlers::get_show)
                .delete(content::show::handlers::delete_show),
        )
        .route(
            "/api/v1/shows/{id}/seasons",
            axum::routing::get(content::show::handlers::list_seasons),
        )
        .route(
            "/api/v1/shows/{id}/monitored-seasons",
            axum::routing::get(content::show::handlers::monitored_seasons),
        )
        .route(
            "/api/v1/shows/{id}/seasons/{season_number}/episodes",
            axum::routing::get(content::show::handlers::list_episodes),
        )
        .route(
            "/api/v1/shows/{id}/seasons/{season_number}/analyse-intro",
            axum::routing::post(content::show::episode_handlers::analyse_season_intro),
        )
        .route(
            "/api/v1/shows/by-tmdb/{tmdb_id}/watch-state",
            axum::routing::get(content::show::handlers::show_watch_state),
        )
        .route(
            "/api/v1/shows/by-tmdb/{tmdb_id}/seasons/{season_number}/episodes",
            axum::routing::get(content::show::handlers::show_season_episodes_by_tmdb),
        )
        .route(
            "/api/v1/episodes/{id}/watched",
            axum::routing::post(content::show::episode_handlers::mark_episode_watched)
                .delete(content::show::episode_handlers::unmark_episode_watched),
        )
        .route(
            "/api/v1/episodes/{id}/redownload",
            axum::routing::post(content::show::episode_handlers::redownload_episode),
        )
        .route(
            "/api/v1/episodes/{id}/acquire",
            axum::routing::post(content::show::episode_handlers::acquire_episode),
        )
        .route(
            "/api/v1/episodes/acquire-by-tmdb",
            axum::routing::post(content::show::episode_handlers::acquire_episode_by_tmdb),
        )
        .route(
            "/api/v1/episodes/{id}/discard",
            axum::routing::post(content::show::episode_handlers::discard_episode),
        )
        .route(
            "/api/v1/shows/{id}/monitor",
            axum::routing::patch(content::show::handlers::update_show_monitor),
        )
        .route(
            "/api/v1/shows/{id}/pause-downloads",
            axum::routing::post(content::show::handlers::pause_show_downloads),
        )
        .route(
            "/api/v1/shows/{id}/resume-downloads",
            axum::routing::post(content::show::handlers::resume_show_downloads),
        )
        // Images
        .route(
            "/api/v1/images/{content_type}/{id}/{image_type}",
            axum::routing::get(metadata::image_handlers::get_image),
        )
        // Library
        .route(
            "/api/v1/library/search",
            axum::routing::get(library::handlers::library_search),
        )
        .route(
            "/api/v1/calendar",
            axum::routing::get(library::handlers::calendar),
        )
        .route(
            "/api/v1/calendar.ics",
            axum::routing::get(library::handlers::calendar_ics),
        )
        .route(
            "/api/v1/stats",
            axum::routing::get(library::handlers::stats),
        )
        .route(
            "/api/v1/widget",
            axum::routing::get(library::handlers::widget),
        )
        // Indexers
        .route(
            "/api/v1/indexers",
            axum::routing::get(indexers::handlers::list_indexers)
                .post(indexers::handlers::create_indexer),
        )
        .route(
            "/api/v1/indexers/{id}",
            axum::routing::get(indexers::handlers::get_indexer)
                .put(indexers::handlers::update_indexer)
                .delete(indexers::handlers::delete_indexer),
        )
        .route(
            "/api/v1/indexers/{id}/test",
            axum::routing::post(indexers::handlers::test_indexer),
        )
        .route(
            "/api/v1/indexers/{id}/retry",
            axum::routing::post(indexers::handlers::retry_indexer),
        )
        .route(
            "/api/v1/indexer-definitions",
            axum::routing::get(indexers::handlers::list_definitions),
        )
        .route(
            "/api/v1/indexer-definitions/refresh",
            axum::routing::post(indexers::handlers::refresh_definitions)
                .get(indexers::handlers::get_refresh_state),
        )
        .route(
            "/api/v1/indexer-definitions/{id}",
            axum::routing::get(indexers::handlers::get_definition),
        )
        // Releases
        .route(
            "/api/v1/releases",
            axum::routing::get(acquisition::release::list_releases),
        )
        .route(
            "/api/v1/episodes/{id}/releases",
            axum::routing::get(acquisition::release::episode_releases),
        )
        .route(
            "/api/v1/movies/{id}/releases",
            axum::routing::get(acquisition::release::movie_releases),
        )
        .route(
            "/api/v1/releases/{id}/grab",
            axum::routing::post(acquisition::release::grab_release),
        )
        .route(
            "/api/v1/releases/{id}/grab-and-watch",
            axum::routing::post(acquisition::release::grab_and_watch),
        )
        .route(
            "/api/v1/watch-now",
            axum::routing::post(watch_now::handlers::watch_now),
        )
        // Unified play API — one URL per entity (movie or episode).
        // Dispatcher in api::play picks between library file + active
        // torrent stream per request. First endpoint: /prepare. Byte-
        // serving endpoints land in follow-up commits.
        .route(
            "/api/v1/play/{kind}/{entity_id}/prepare",
            axum::routing::get(playback::handlers::prepare),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/direct",
            axum::routing::get(playback::handlers::direct),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/master.m3u8",
            axum::routing::get(playback::hls::master::hls_master),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/variant.m3u8",
            axum::routing::get(playback::hls::variant::hls_variant),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/segments/{index}",
            axum::routing::get(playback::hls::segment::hls_segment),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/transcode",
            axum::routing::delete(playback::handlers::stop_transcode),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/subtitles/{stream_index}",
            axum::routing::get(playback::handlers::subtitle),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/trickplay.vtt",
            axum::routing::get(playback::handlers::trickplay_vtt),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/trickplay/{name}",
            axum::routing::get(playback::handlers::trickplay_sprite),
        )
        .route(
            "/api/v1/play/{kind}/{entity_id}/progress",
            axum::routing::post(playback::handlers::play_progress),
        )
        .route(
            "/api/v1/playback/cast-token",
            axum::routing::post(playback::cast::issue_cast_token),
        )
        // Server-side Cast sender (subsystem 32). All `/api/v1/cast/*`
        // endpoints — distinct from the legacy browser-Cast token
        // endpoint above, which stays for the existing chrome.cast.*
        // path.
        .route(
            "/api/v1/cast/devices",
            axum::routing::get(cast_sender::handlers::list_devices)
                .post(cast_sender::handlers::add_device),
        )
        .route(
            "/api/v1/cast/devices/{id}",
            axum::routing::delete(cast_sender::handlers::delete_device),
        )
        .route(
            "/api/v1/cast/sessions",
            axum::routing::post(cast_sender::handlers::start_session),
        )
        .route(
            "/api/v1/cast/sessions/{id}",
            axum::routing::get(cast_sender::handlers::get_session)
                .delete(cast_sender::handlers::stop_session),
        )
        .route(
            "/api/v1/cast/sessions/{id}/play",
            axum::routing::post(cast_sender::handlers::play),
        )
        .route(
            "/api/v1/cast/sessions/{id}/pause",
            axum::routing::post(cast_sender::handlers::pause),
        )
        .route(
            "/api/v1/cast/sessions/{id}/seek",
            axum::routing::post(cast_sender::handlers::seek),
        )
        // Backup & restore (subsystem 19).
        .route(
            "/api/v1/backups",
            axum::routing::get(backup::handlers::list_backups)
                .post(backup::handlers::create_backup),
        )
        .route(
            "/api/v1/backups/{id}",
            axum::routing::delete(backup::handlers::delete_backup),
        )
        .route(
            "/api/v1/backups/{id}/download",
            axum::routing::get(backup::handlers::download_backup),
        )
        .route(
            "/api/v1/backups/{id}/restore",
            axum::routing::post(backup::handlers::restore_backup),
        )
        .route(
            "/api/v1/backups/restore-upload",
            axum::routing::post(backup::handlers::restore_upload)
                .layer(backup::handlers::upload_limit()),
        )
        // Blocklist
        .route(
            "/api/v1/blocklist",
            axum::routing::get(acquisition::blocklist::list_blocklist),
        )
        .route(
            "/api/v1/blocklist/{id}",
            axum::routing::delete(acquisition::blocklist::delete_blocklist),
        )
        .route(
            "/api/v1/blocklist/movie/{movie_id}",
            axum::routing::get(acquisition::blocklist::get_movie_blocklist)
                .delete(acquisition::blocklist::clear_movie_blocklist),
        )
        // Downloads
        .route(
            "/api/v1/downloads",
            axum::routing::get(download::handlers::list_downloads),
        )
        .route(
            "/api/v1/downloads/{id}",
            axum::routing::get(download::handlers::get_download)
                .delete(download::handlers::cancel_download),
        )
        .route(
            "/api/v1/downloads/{id}/pause",
            axum::routing::post(download::handlers::pause_download),
        )
        .route(
            "/api/v1/downloads/{id}/resume",
            axum::routing::post(download::handlers::resume_download),
        )
        .route(
            "/api/v1/downloads/{id}/retry",
            axum::routing::post(download::handlers::retry_download),
        )
        .route(
            "/api/v1/downloads/{id}/blocklist-and-search",
            axum::routing::post(download::handlers::blocklist_and_search),
        )
        .route(
            "/api/v1/downloads/{id}/files",
            axum::routing::get(download::handlers::download_files),
        )
        .route(
            "/api/v1/downloads/{id}/files/select",
            axum::routing::post(download::handlers::update_download_files),
        )
        .route(
            "/api/v1/downloads/{id}/peers",
            axum::routing::get(download::handlers::download_peers),
        )
        .route(
            "/api/v1/downloads/{id}/pieces",
            axum::routing::get(download::handlers::download_pieces),
        )
        .route(
            "/api/v1/downloads/speed-test",
            axum::routing::post(download::handlers::speed_test),
        )
        // Media
        .route(
            "/api/v1/media",
            axum::routing::get(content::media::handlers::list_media),
        )
        .route(
            "/api/v1/media/{id}",
            axum::routing::get(content::media::handlers::get_media)
                .delete(content::media::handlers::delete_media),
        )
        .route(
            "/api/v1/media/{id}/streams",
            axum::routing::get(content::media::handlers::get_media_streams),
        )
        // Playback diagnostics
        .route(
            "/api/v1/playback/probe",
            axum::routing::post(playback::probe_handlers::probe),
        )
        .route(
            "/api/v1/playback/transcode-stats",
            axum::routing::get(playback::probe_handlers::transcode_stats),
        )
        .route(
            "/api/v1/playback/transcode-sessions",
            axum::routing::get(playback::probe_handlers::transcode_sessions),
        )
        .route(
            "/api/v1/playback/transcode-sessions/{session_id}",
            axum::routing::delete(playback::probe_handlers::stop_transcode_session),
        )
        .route(
            "/api/v1/playback/test-transcode",
            axum::routing::post(playback::probe_handlers::test_transcode),
        )
        .route(
            "/api/v1/playback/ffmpeg/download",
            axum::routing::post(playback::probe_handlers::start_ffmpeg_download)
                .get(playback::probe_handlers::get_ffmpeg_download)
                .delete(playback::probe_handlers::cancel_ffmpeg_download),
        )
        .route(
            "/api/v1/playback/ffmpeg/revert",
            axum::routing::post(playback::probe_handlers::revert_ffmpeg_to_system),
        )
        .route(
            "/api/v1/movies/{id}/watched",
            axum::routing::post(playback::watch_state::mark_movie_watched)
                .delete(playback::watch_state::unmark_movie_watched),
        )
        // Tasks
        .route(
            "/api/v1/tasks",
            axum::routing::get(scheduler::handlers::list_tasks),
        )
        .route(
            "/api/v1/tasks/{name}/run",
            axum::routing::post(scheduler::handlers::run_task),
        )
        // History
        .route(
            "/api/v1/history",
            axum::routing::get(notification::history::list_history),
        )
        .route(
            "/api/v1/home/up-next",
            axum::routing::get(home::handlers::up_next),
        )
        // Webhooks
        .route(
            "/api/v1/webhooks",
            axum::routing::get(notification::webhook::list_webhooks)
                .post(notification::webhook::create_webhook),
        )
        .route(
            "/api/v1/webhooks/{id}",
            axum::routing::put(notification::webhook::update_webhook)
                .delete(notification::webhook::delete_webhook),
        )
        .route(
            "/api/v1/webhooks/{id}/test",
            axum::routing::post(notification::webhook::test_webhook),
        )
        // Logs
        .route(
            "/api/v1/logs",
            axum::routing::get(observability::handlers::list_logs),
        )
        .route(
            "/api/v1/logs/export",
            axum::routing::get(observability::handlers::export_logs),
        )
        .route(
            "/api/v1/logs/stream",
            axum::routing::get(observability::handlers::stream_logs),
        )
        .route(
            "/api/v1/client-logs",
            axum::routing::post(observability::handlers::ingest_client_logs),
        )
        // WebSocket
        .route(
            "/api/v1/ws",
            axum::routing::get(notification::ws_handlers::ws_handler),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ))
        // Generates a per-request trace_id span; every `tracing::info!`
        // inside a handler inherits it, so log_entry rows for that
        // request can be found with `?trace_id=…`.
        .layer(middleware::from_fn(observability::trace::trace_layer))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        // CORS is locked down to same-origin by default: the
        // frontend is bundled and served from the same host as the
        // API, so no browser needs to make cross-origin requests
        // against kino. A permissive layer here let any site the
        // user visits make authenticated requests using the API
        // key in browser memory. If you're reverse-proxying the UI
        // from a different origin, expose `KINO_CORS_ORIGIN` as a
        // comma-separated allow-list (e.g.
        // `https://kino.example.com,https://dev.example.com`).
        .layer(build_cors_layer())
        .with_state(state);

    // Compose:
    //   - api: every `/api/v1/*` route (auth-gated, traced, etc.)
    //   - SwaggerUi: `/api/docs/` + `/api-docs/openapi.json`
    //   - SPA fallback: every other path → embedded `frontend/dist/`,
    //     with index.html as the SPA-router fallback for client-side
    //     routes like `/library` or `/play/movie/4`.
    api.merge(SwaggerUi::new("/api/docs/").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .fallback(spa::handler)
}

/// Build the CORS layer. Reads `KINO_CORS_ORIGIN` at startup — an
/// empty/missing value means "same-origin only" (no `Access-Control-
/// Allow-Origin` header emitted, browsers block cross-origin
/// requests). Any explicit origins in the env var are parsed as
/// comma-separated `HeaderValue`s.
fn build_cors_layer() -> CorsLayer {
    let raw = std::env::var("KINO_CORS_ORIGIN").unwrap_or_default();
    let origins: Vec<reqwest::header::HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if origins.is_empty() {
        // No origins configured → same-origin only. We still need
        // to respond to OPTIONS preflights; the default `CorsLayer`
        // with no `.allow_origin` is a no-op and lets axum's
        // method-not-allowed kick in.
        tracing::info!("CORS: same-origin only (set KINO_CORS_ORIGIN to allow cross-origin)");
        CorsLayer::new()
    } else {
        tracing::info!(
            origins = ?origins.iter().map(|v| v.to_str().unwrap_or("").to_string()).collect::<Vec<_>>(),
            "CORS: allowing configured origins"
        );
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                reqwest::Method::GET,
                reqwest::Method::POST,
                reqwest::Method::PUT,
                reqwest::Method::PATCH,
                reqwest::Method::DELETE,
                reqwest::Method::OPTIONS,
            ])
            .allow_headers([
                reqwest::header::AUTHORIZATION,
                reqwest::header::CONTENT_TYPE,
            ])
            .allow_credentials(true)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cli_tests {
    use super::Cli;
    use clap::Parser;

    /// Service descriptors set `KINO_NO_OPEN_BROWSER=1`; the binary
    /// must accept the full shell/systemd convention or the unit
    /// crash-loops. v0.2.0 shipped with a typed-`bool` clap arg that
    /// only accepted literal `true`/`false`; this test pins the
    /// `BoolishValueParser` semantics so a regression can't reach a
    /// release.
    #[test]
    fn no_open_browser_accepts_boolish_env_values() {
        for truthy in ["1", "true", "yes", "on"] {
            let cli = Cli::try_parse_from(["kino"]).unwrap();
            // Sanity: default is false when env is unset.
            assert!(!cli.no_open_browser);

            // Use the CLI form (`--flag=<value>`) to exercise the same
            // parser path the env var uses, without mutating process
            // env (workspace forbids `unsafe`, which is required for
            // `std::env::set_var` since Rust 2024).
            let cli =
                Cli::try_parse_from(["kino", &format!("--no-open-browser={truthy}")]).unwrap();
            assert!(cli.no_open_browser, "{truthy} should parse as true");
        }
        for falsy in ["0", "false", "no", "off"] {
            let cli = Cli::try_parse_from(["kino", &format!("--no-open-browser={falsy}")]).unwrap();
            assert!(!cli.no_open_browser, "{falsy} should parse as false");
        }
    }

    #[test]
    fn no_open_browser_bare_flag_means_true() {
        let cli = Cli::try_parse_from(["kino", "--no-open-browser"]).unwrap();
        assert!(cli.no_open_browser);
    }

    #[test]
    fn no_open_browser_rejects_garbage() {
        // BoolishValueParser is strict — non-boolish values fail loudly
        // rather than silently coercing.
        assert!(Cli::try_parse_from(["kino", "--no-open-browser=banana"]).is_err());
    }
}
