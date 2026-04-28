//! Scheduler — interval-based task runner. Each registered task has
//! a name + interval; on every tick, due tasks fire on a worker
//! pool. Tasks self-report duration + outcome via `mark_done`; the
//! task table feeds the admin dashboard so operators can see when
//! a sweep last ran.
//!
//! ## Public API
//!
//! - `Scheduler` — the cheap-to-clone handle other domains use to
//!   register tasks at boot
//! - `TaskTrigger` — the mpsc-channel signal that lets handlers fire
//!   a task immediately rather than waiting for the next tick
//!   (e.g., `MovieAdded` triggers `wanted_search` so a fresh follow
//!   doesn't sit queued for 15 minutes)
//! - `TaskInfo`, `TaskDef` — admin view + registration shape
//! - `model` — the `scheduler_task` row model surfaced via handlers
//! - `handlers` — `/tasks` HTTP, registered via main.rs

pub mod handlers;
pub mod model;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::sync::RwLock;

/// Definition of a scheduled task.
#[derive(Debug, Clone)]
pub struct TaskDef {
    pub name: String,
    pub interval: Duration,
    pub last_run_at: Option<DateTime<Utc>>,
    pub running: bool,
    /// Wall-clock duration of the most recent completed run. Kept in
    /// memory only — diagnostic, not cadence-critical, so we don't
    /// persist it across restarts.
    pub last_duration_ms: Option<u64>,
    /// Error string from the most recent run, or `None` if it
    /// succeeded. Cleared on the next successful run so the UI
    /// doesn't stick on a resolved failure.
    pub last_error: Option<String>,
}

/// Payload sent on the scheduler's trigger channel. Most callers
/// fire-and-forget (`TaskTrigger::fire("wanted_search")`); tests that
/// need to block until the task finishes use `with_completion` and
/// await the returned oneshot receiver.
pub struct TaskTrigger {
    pub name: String,
    /// When `Some`, the scheduler sends the task's `Result` here
    /// after `execute_task` returns — including any error string
    /// from the task body. Callers that set this are guaranteed to
    /// receive one value before the oneshot is dropped, even if the
    /// task panicked (the panic surfaces as `Err("task panicked")`).
    pub completion: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
}

impl TaskTrigger {
    /// Fire-and-forget trigger — production call sites use this.
    #[must_use]
    pub fn fire(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            completion: None,
        }
    }

    /// Trigger plus a one-shot receiver for the task's result. Used
    /// by integration tests to drive a task to completion
    /// synchronously instead of polling `/tasks` for `running=false`.
    #[must_use]
    pub fn with_completion(
        name: impl Into<String>,
    ) -> (Self, tokio::sync::oneshot::Receiver<Result<(), String>>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            Self {
                name: name.into(),
                completion: Some(tx),
            },
            rx,
        )
    }
}

impl std::fmt::Debug for TaskTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskTrigger")
            .field("name", &self.name)
            .field("completion", &self.completion.is_some())
            .finish()
    }
}

/// Serializable task info for the API.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct TaskInfo {
    pub name: String,
    pub interval_seconds: u64,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub running: bool,
    pub last_duration_ms: Option<u64>,
    pub last_error: Option<String>,
}

/// The scheduler manages periodic background tasks.
#[derive(Debug, Clone)]
pub struct Scheduler {
    tasks: Arc<RwLock<HashMap<String, TaskDef>>>,
    db: SqlitePool,
}

/// Task health state — "last tick was OK" vs "last tick was an
/// error". Used to gate `HealthWarning` / `HealthRecovered` on
/// transitions only. Without this, a flapping task (indexer DNS
/// failure, TMDB rate-limit) would fire a warning every interval
/// for the same root cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskHealth {
    Ok,
    Err,
}

/// Process-wide map of `task_name → last known health`. Bound by
/// the fixed set of registered tasks (~20 entries), so no
/// eviction needed. Mirrors the `DISK_STATE` pattern in
/// `cleanup::mod`.
static TASK_HEALTH: std::sync::LazyLock<std::sync::Mutex<HashMap<String, TaskHealth>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn emit_task_warning_if_transition(
    event_tx: &tokio::sync::broadcast::Sender<crate::events::AppEvent>,
    name: &str,
    err: &anyhow::Error,
) {
    let was_ok = {
        let Ok(mut map) = TASK_HEALTH.lock() else {
            return;
        };
        let prior = map.get(name).copied();
        map.insert(name.to_owned(), TaskHealth::Err);
        prior != Some(TaskHealth::Err)
    };
    if was_ok {
        let _ = event_tx.send(crate::events::AppEvent::HealthWarning {
            message: format!("Task '{name}' failed: {err}"),
        });
    }
}

fn emit_task_recovered_if_transition(
    event_tx: &tokio::sync::broadcast::Sender<crate::events::AppEvent>,
    name: &str,
) {
    let was_err = {
        let Ok(mut map) = TASK_HEALTH.lock() else {
            return;
        };
        let prior = map.get(name).copied();
        map.insert(name.to_owned(), TaskHealth::Ok);
        prior == Some(TaskHealth::Err)
    };
    if was_err {
        let _ = event_tx.send(crate::events::AppEvent::HealthRecovered {
            message: format!("Task '{name}' recovered."),
        });
    }
}

impl Scheduler {
    pub fn new(db: SqlitePool) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    /// Register a task with its interval.
    pub async fn register(&self, name: &str, interval: Duration) {
        // Load last_run_at from persistence
        let last_run: Option<String> =
            sqlx::query_scalar("SELECT last_run_at FROM scheduler_state WHERE task_name = ?")
                .bind(name)
                .fetch_optional(&self.db)
                .await
                .ok()
                .flatten();

        let last_run_at = last_run.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        });

        self.tasks.write().await.insert(
            name.to_owned(),
            TaskDef {
                name: name.to_owned(),
                interval,
                last_run_at,
                running: false,
                last_duration_ms: None,
                last_error: None,
            },
        );
    }

    /// Register all default tasks. The metadata refresh tick is
    /// fixed at 30 minutes — the per-row staleness check (see
    /// `metadata::refresh::refresh_sweep`) handles the tiered
    /// cadence, so the scheduler just needs to tick often enough to
    /// pick up hot-tier (1h-stale) rows promptly.
    pub async fn register_defaults(&self, search_interval_min: u64) {
        self.register(
            "wanted_search",
            Duration::from_secs(search_interval_min * 60),
        )
        .await;
        self.register("metadata_refresh", Duration::from_secs(30 * 60))
            .await;
        self.register("cleanup", Duration::from_secs(3600)).await;
        // Disk-space check cadence matches `vpn_health`: frequent
        // enough that a stuck import doesn't gorge the volume
        // unnoticed, but not so tight the `df` exec overhead shows
        // up in profiles. Emits HealthWarning/HealthRecovered only
        // on transitions (see `cleanup::disk_space_sweep`).
        self.register("disk_space_check", Duration::from_secs(300))
            .await;
        // Weekly orphan scan. Warns on files in the library with
        // no matching `media` row; doesn't auto-delete (too risky
        // without an explicit user action). See
        // `cleanup::orphan_file_scan`.
        self.register("orphan_scan", Duration::from_secs(7 * 24 * 3600))
            .await;
        self.register("indexer_health", Duration::from_secs(1800))
            .await;
        self.register("webhook_retry", Duration::from_secs(900))
            .await;
        self.register("vpn_health", Duration::from_secs(300)).await;
        // Subsystem 33 phase B: 5-minute IP-leak self-test. Same
        // cadence as vpn_health so a wedged tunnel and a leaked
        // packet are both caught within one window.
        self.register("vpn_killswitch_check", Duration::from_secs(300))
            .await;
        // Backup creator (subsystem 19). Ticks every minute so the
        // schedule helper can hit its target HH:MM within a tight
        // window; the helper itself debounces against the most
        // recent scheduled-kind row so we don't fire twice.
        self.register("backup_create", Duration::from_secs(60))
            .await;
        self.register("stale_download_check", Duration::from_secs(1))
            .await;
        // Idle transcode-session sweep — detects the
        // disconnected-client case. `last_activity` is bumped
        // on every segment / playlist / master request, so a
        // session that hasn't seen any request in
        // `TRANSCODE_IDLE_TIMEOUT_SECS` (42s by default,
        // == 2 × segment_length + 30s) is almost certainly a
        // silently-dead client (closed browser tab, crashed
        // page, flaky mobile network). 15s cadence keeps the
        // worst-case leak bounded to ~57s of wasted encoder
        // time instead of the previous 30–90 minutes.
        self.register("transcode_cleanup", Duration::from_secs(15))
            .await;
        // Sliding-window segment sweep — trims per-session HLS
        // segment files below the client's request highwater so
        // a long-running session doesn't accumulate unbounded
        // disk usage. Cadence is frequent (30s) but the sweep
        // is a stat + unlink pass over a handful of files per
        // live session — negligible load.
        self.register("transcode_segment_sweep", Duration::from_secs(30))
            .await;
        self.register("trickplay_generation", Duration::from_secs(300))
            .await;
        self.register("log_retention", Duration::from_secs(3600))
            .await;
        // Trakt integration. Tasks short-circuit when Trakt isn't
        // configured/connected so they're cheap no-ops for users
        // who don't use it.
        self.register("trakt_sync_incremental", Duration::from_secs(300))
            .await;
        self.register("trakt_home_refresh", Duration::from_secs(24 * 3600))
            .await;
        // Lists subsystem (17). One sweep that visits all due lists;
        // per-source-type cadence (6h MDBList/TMDB, 1h Trakt custom)
        // is enforced inside the sweep. Watchlist rides on the Trakt
        // last-activities path so isn't covered here.
        self.register("lists_poll", Duration::from_secs(900)).await;
        // Intro-skipper (subsystem 15). Daily sweep catches seasons
        // that had <2 episodes at first-import time, episodes
        // imported before the feature was enabled, and FFmpeg
        // transient failures worth retrying.
        self.register("intro_catchup", Duration::from_secs(24 * 3600))
            .await;
        self.register("trakt_scrobble_drain", Duration::from_secs(30))
            .await;
        // Cardigann definitions refresh — daily pull from the
        // Prowlarr/Indexers repo so site-behavior fixes land within
        // 24h. A weekly cadence delays compatibility fixes long
        // enough that a tracker stays broken across a whole user's
        // viewing weekend; daily is a cheap one-API-request +
        // mostly-cached-CDN-fetch round trip and Prowlarr is
        // unlikely to ship a regression that takes us with it.
        self.register("definitions_refresh", Duration::from_secs(24 * 3600))
            .await;
        // Daily purge of expired session rows so the table doesn't
        // accumulate every short-lived bootstrap-pending token + every
        // long-expired browser cookie. Cheap delete-by-index.
        self.register("session_purge", Duration::from_secs(24 * 3600))
            .await;
        // Continuous reconciliation: invariant suite + (later) the
        // periodic-safe steps from startup::reconcile. SurfaceOnly
        // by default; auto-repair steps land deliberately as the
        // suite grows. See `reconcile::run_continuous`.
        self.register("reconcile", Duration::from_secs(60)).await;
        // Cleanup retry queue — re-attempts torrent / file / dir
        // removals that previously failed transiently. Cadence
        // matches the tracker's default 5-minute minimum interval
        // between retries on the same row.
        self.register("cleanup_retry", Duration::from_secs(300))
            .await;
    }

    /// Get info for all tasks.
    pub async fn list_tasks(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.read().await;
        let mut result: Vec<TaskInfo> = tasks
            .values()
            .map(|t| {
                let next = t.last_run_at.map(|last| {
                    let next_time =
                        last + chrono::Duration::from_std(t.interval).unwrap_or_default();
                    next_time.to_rfc3339()
                });
                TaskInfo {
                    name: t.name.clone(),
                    interval_seconds: t.interval.as_secs(),
                    last_run_at: t.last_run_at.map(|d| d.to_rfc3339()),
                    next_run_at: next,
                    running: t.running,
                    last_duration_ms: t.last_duration_ms,
                    last_error: t.last_error.clone(),
                }
            })
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Mark a task as running and update `last_run_at`.
    pub async fn mark_running(&self, name: &str) {
        let now = Utc::now();
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(name) {
            task.running = true;
            task.last_run_at = Some(now);
        }

        // Persist `last_run_at` so task cadence survives restart. If
        // the write fails the next tick treats the task as never-run
        // and re-fires — not fatal, but worth surfacing because it
        // hints at DB pressure.
        let now_str = now.to_rfc3339();
        if let Err(e) = sqlx::query(
            "INSERT INTO scheduler_state (task_name, last_run_at) VALUES (?, ?) ON CONFLICT(task_name) DO UPDATE SET last_run_at = excluded.last_run_at",
        )
        .bind(name)
        .bind(&now_str)
        .execute(&self.db)
        .await
        {
            tracing::warn!(task = name, error = %e, "failed to persist scheduler state");
        }
    }

    /// Mark a task as no longer running and record the outcome of
    /// the run. A successful run sets `last_duration_ms` and clears
    /// any previous `last_error` so the UI doesn't pin on a resolved
    /// failure. A failed run records both.
    pub async fn mark_done(&self, name: &str, duration_ms: u64, error: Option<String>) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(name) {
            task.running = false;
            task.last_duration_ms = Some(duration_ms);
            task.last_error = error;
        }
    }

    /// Replace a task's interval in place. The config update endpoint
    /// calls this after saving so the Automation settings page takes
    /// effect without restarting the backend. A no-op if the task
    /// isn't registered (safer than erroring: future boots with an
    /// extra task will still save cleanly).
    pub async fn set_interval(&self, name: &str, interval: Duration) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(name) {
            task.interval = interval;
        }
    }

    /// Check if a task is due to run.
    pub async fn is_due(&self, name: &str) -> bool {
        let tasks = self.tasks.read().await;
        let Some(task) = tasks.get(name) else {
            return false;
        };
        if task.running {
            return false;
        }
        match task.last_run_at {
            None => true, // Never run
            Some(last) => {
                Utc::now() - last >= chrono::Duration::from_std(task.interval).unwrap_or_default()
            }
        }
    }

    /// Run the scheduler loop. Checks due tasks every 1s (matching
    /// librqbit's speed-estimator tick). Spawns each due task as an
    /// independent async task — every spawn goes through the caller-
    /// supplied `TaskTracker` so graceful shutdown waits for them
    /// to complete (or the 10 s hard timeout fires), rather than
    /// the runtime aborting mid-DB-write.
    pub async fn run(
        &self,
        state: crate::state::AppState,
        mut trigger_rx: tokio::sync::mpsc::Receiver<TaskTrigger>,
        tracker: tokio_util::task::TaskTracker,
    ) {
        use tokio::time::{MissedTickBehavior, interval};

        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let cancel = state.cancel.clone();

        tracing::info!("scheduler started");

        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    tracing::info!("scheduler shutting down");
                    break;
                }
                _ = ticker.tick() => {
                    self.run_due_tasks(&state, &tracker).await;
                }
                Some(trigger) = trigger_rx.recv() => {
                    // Manual triggers bypass the due-check but MUST
                    // respect the single-instance invariant. The
                    // claim lives inside `execute_task_with_completion`
                    // so the direct-call test path + the trigger-
                    // channel path share one claim implementation.
                    // If the task is already running the wrapper
                    // bails with a failure on the completion channel.
                    tracing::info!(task = %trigger.name, "manual trigger");
                    let state = state.clone();
                    let sched = self.clone();
                    let TaskTrigger { name, completion } = trigger;
                    tracker.spawn(async move {
                        sched.execute_task_with_completion(&state, &name, completion).await;
                    });
                }
            }
        }
    }

    async fn run_due_tasks(
        &self,
        state: &crate::state::AppState,
        tracker: &tokio_util::task::TaskTracker,
    ) {
        let task_names: Vec<String> = {
            let tasks = self.tasks.read().await;
            tasks.keys().cloned().collect()
        };

        // Stagger is scoped to this single tick — `idx` counts the
        // tasks we've claimed so far in the current loop. A burst of
        // simultaneously-due tasks (common on boot when `last_run_at`
        // is NULL for everything) gets spread across the first ten
        // seconds; a steady-state tick where one task happens to be
        // due gets no artificial delay. Previously the counter was a
        // persistent field so even two coincidentally-due tasks weeks
        // later each got 0–9 s of unwanted lag.
        let mut idx: u64 = 0;
        for name in &task_names {
            // Claim atomically: `is_due + mark_running` fused into
            // one write-locked operation. Previously these were two
            // separate awaits, which meant a second tick could see
            // `running = false` before the first spawn actually
            // reached mark_running — duplicating long-running tasks
            // (notably trickplay_generation) across sweep cycles.
            if !self.try_claim(name, false).await {
                continue;
            }
            let state = state.clone();
            let sched = self.clone();
            let name = name.clone();
            let delay = Duration::from_secs(idx);
            idx = (idx + 1).min(10);

            tracker.spawn(async move {
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
                sched.execute_task(&state, &name).await;
            });
        }
    }

    /// Atomic claim: check `is_due` (unless `force_due` is set) and
    /// flip `running` under the same write lock. Returns true if
    /// the caller now owns the task run (and is responsible for
    /// calling `mark_done`).
    async fn try_claim(&self, name: &str, force_due: bool) -> bool {
        let mut tasks = self.tasks.write().await;
        let Some(task) = tasks.get_mut(name) else {
            return false;
        };
        if task.running {
            return false;
        }
        if !force_due {
            let due = match task.last_run_at {
                None => true,
                Some(last) => {
                    Utc::now() - last
                        >= chrono::Duration::from_std(task.interval).unwrap_or_default()
                }
            };
            if !due {
                return false;
            }
        }
        task.running = true;
        let now = Utc::now();
        task.last_run_at = Some(now);
        // Persist last_run_at so cadence survives restart. Fire-and-
        // forget — the in-memory state is the source of truth for
        // the tick; DB is just so we resume correctly after restart.
        let now_str = now.to_rfc3339();
        let db = self.db.clone();
        let task_name = name.to_owned();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO scheduler_state (task_name, last_run_at) VALUES (?, ?) ON CONFLICT(task_name) DO UPDATE SET last_run_at = excluded.last_run_at",
            )
            .bind(&task_name)
            .bind(&now_str)
            .execute(&db)
            .await;
        });
        true
    }

    /// Manual-trigger claim: bypasses the interval check ("run
    /// right now") but still respects the single-instance guard.
    async fn try_claim_manual(&self, name: &str) -> bool {
        self.try_claim(name, true).await
    }

    // Long on purpose — it's the central dispatch for every scheduled
    // task, and splitting it would fragment the dispatch table readers
    // otherwise have in one place. Each arm is a 1-3 line call into a
    // subsystem module.
    #[allow(clippy::too_many_lines)]
    async fn execute_task(&self, state: &crate::state::AppState, name: &str) {
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;

        let pool = &state.db;
        let event_tx = &state.event_tx;
        // `try_claim` / `try_claim_manual` already set
        // `running=true` + persisted `last_run_at` under the write
        // lock before this task was spawned, so there's no second
        // `mark_running` here. The previous version wrote to the
        // same row twice milliseconds apart — wasted DB traffic
        // and, more subtly, the second write overwrote the claim's
        // `last_run_at` with a later value. `mark_running` is
        // retained for external callers (API manual-trigger /
        // test_support) that exercise `execute_task` directly.
        let start = std::time::Instant::now();
        // TRACE because high-frequency tasks (`stale_download_check`
        // at 1s intervals) emit this every tick. DEBUG was enough to
        // push the default-on SQLite sink into dropping events.
        tracing::trace!(task = name, "scheduler task start");

        // Catch panics from any task body so a panicked task reports
        // as an error, runs `mark_done`, and is rerun next cycle —
        // rather than leaving `running = true` forever (which was
        // the pre-catch behaviour, requiring a restart to recover).
        // `AssertUnwindSafe` is safe here because nothing we pass in
        // is observed again from this function on the unwind path —
        // we drop state on the error branch and let the next
        // scheduler tick re-load fresh.
        let task_body = async {
            match name {
                "wanted_search" => {
                    crate::acquisition::search::wanted_sweep::wanted_search_sweep(state).await
                }
                "stale_download_check" => {
                    crate::download::monitor::monitor_downloads(
                        pool,
                        event_tx,
                        state.torrent.as_deref(),
                        &state.ffprobe_path,
                    )
                    .await
                }
                "cleanup" => {
                    // Read cleanup knobs from config on each run so the
                    // Automation settings page actually takes effect — the
                    // delays and the enabled flag were previously hard-
                    // coded here, which meant turning cleanup off in the
                    // UI didn't actually stop it.
                    //
                    // A transient DB error here used to silently collapse
                    // to `(false, 72, 72)` → cleanup skipped with zero
                    // signal. Log the error explicitly so operators can
                    // see why a sweep no-op'd instead of assuming the
                    // user disabled auto-cleanup.
                    let (enabled, movie_delay, ep_delay): (bool, i64, i64) = match sqlx::query_as(
                    "SELECT auto_cleanup_enabled, auto_cleanup_movie_delay, auto_cleanup_episode_delay FROM config WHERE id = 1",
                )
                .fetch_optional(pool)
                .await
                {
                    Ok(row) => row.unwrap_or((false, 72, 72)),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "cleanup: failed to read config row — skipping this tick"
                        );
                        (false, 72, 72)
                    }
                };
                    crate::cleanup::run_cleanup(
                        pool,
                        Some(event_tx),
                        &state.data_path,
                        movie_delay,
                        ep_delay,
                        enabled,
                    )
                    .await
                    .map(|_| ())
                }
                "metadata_refresh" => {
                    if let Some(tmdb) = state.tmdb_snapshot() {
                        crate::metadata::refresh::refresh_sweep(pool, event_tx, &tmdb)
                            .await
                            .map(|_| ())
                    } else {
                        tracing::debug!("metadata_refresh skipped — no TMDB client");
                        Ok(())
                    }
                }
                "disk_space_check" => crate::cleanup::disk_space_sweep(pool, event_tx).await,
                "orphan_scan" => crate::cleanup::orphan_file_scan(pool).await.map(|_| ()),
                "indexer_health" => crate::indexers::health::health_sweep(state)
                    .await
                    .map(|_| ()),
                "webhook_retry" => crate::notification::webhook_retry::retry_sweep(pool)
                    .await
                    .map(|_| ()),
                "transcode_cleanup" => {
                    if let Some(tr) = state.transcode.as_ref() {
                        // 42s idle threshold: 2 × hls_time (6s)
                        // + 30s grace == 42s. Derived from the
                        // Jellyfin-style ref-counted job model:
                        // when the last outstanding request
                        // completes, arm a timer for roughly the
                        // next-two-segments window; if no new
                        // request arrives the client is gone.
                        tr.sweep(crate::playback::transcode::TRANSCODE_IDLE_TIMEOUT_SECS)
                            .await;
                    }
                    Ok(())
                }
                "transcode_segment_sweep" => {
                    if let Some(tr) = state.transcode.as_ref() {
                        // Keep ~2 min of back-scrub (20 × 6s segments).
                        // Clients that seek further back trigger a
                        // fresh ffmpeg restart at the new `-ss`, so
                        // the lost segments would have been obsolete.
                        tr.sweep_segments(20).await;
                    }
                    Ok(())
                }
                "vpn_health" => {
                    if let Some(vpn) = state.vpn.clone() {
                        crate::download::vpn::health::check_once(
                            pool,
                            vpn,
                            state.torrent.as_deref(),
                            &state.event_tx,
                        )
                        .await
                        .map(|_| ())
                    } else {
                        Ok(())
                    }
                }
                "backup_create" => {
                    if crate::backup::schedule::is_due(pool, chrono::Utc::now())
                        .await
                        .unwrap_or(false)
                        && let Err(e) = crate::backup::archive::create(
                            pool,
                            &state.data_path,
                            crate::backup::BackupKind::Scheduled,
                            &state.event_tx,
                        )
                        .await
                    {
                        tracing::warn!(error = %e, "scheduled backup failed");
                    }
                    Ok(())
                }
                "vpn_killswitch_check" => {
                    if let Some(vpn) = state.vpn.as_ref() {
                        if crate::download::vpn::killswitch::is_enabled(pool).await {
                            crate::download::vpn::leak_check::tick(
                                pool,
                                vpn,
                                state.torrent.as_deref(),
                                &state.event_tx,
                            )
                            .await
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    }
                }
                "log_retention" => crate::observability::log_retention::sweep(pool)
                    .await
                    .map(|_| ()),
                "trickplay_generation" => crate::playback::trickplay_gen::sweep(state)
                    .await
                    .map(|_| ()),
                "trakt_sync_incremental" => {
                    if crate::integrations::trakt::is_connected(pool).await
                        && let Ok(client) = crate::integrations::trakt::client_for(state).await
                    {
                        crate::integrations::trakt::sync::incremental_sweep(&client)
                            .await
                            .map_err(|e| anyhow::anyhow!("{e}"))
                    } else {
                        Ok(())
                    }
                }
                "trakt_home_refresh" => {
                    if crate::integrations::trakt::is_connected(pool).await
                        && let Ok(client) = crate::integrations::trakt::client_for(state).await
                    {
                        crate::integrations::trakt::sync::refresh_home_caches(&client)
                            .await
                            .map_err(|e| anyhow::anyhow!("{e}"))
                    } else {
                        Ok(())
                    }
                }
                "trakt_scrobble_drain" => crate::integrations::trakt::scrobble::drain(pool)
                    .await
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e}")),
                "lists_poll" => crate::integrations::lists::sync::poll_due_lists(pool, event_tx)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}")),
                "intro_catchup" => crate::playback::intro_skipper::catch_up_sweep(state).await,
                "definitions_refresh" => crate::indexers::loader::refresh_sweep(state)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}")),
                "session_purge" => crate::auth_session::purge_expired(pool)
                    .await
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e}")),
                "reconcile" => match crate::reconcile::run_continuous(pool).await {
                    Ok(report) => {
                        *state.last_reconcile.write().await = Some(report);
                        Ok(())
                    }
                    Err(e) => Err(anyhow::anyhow!("{e}")),
                },
                "cleanup_retry" => {
                    let executor = crate::cleanup::AppRemovalExecutor::new(state.torrent.clone());
                    state
                        .cleanup_tracker
                        .retry_failed(&executor)
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("{e}"))
                }
                _ => {
                    tracing::debug!(task = name, "task not yet implemented");
                    Ok(())
                }
            }
        };

        // Convert panic → error string. The `Any` payload is almost
        // always a `String` or `&str` (that's what `panic!()`
        // formatting lands on); fall back to a generic marker for
        // anything exotic so we never lose the signal.
        let result: anyhow::Result<()> = match AssertUnwindSafe(task_body).catch_unwind().await {
            Ok(inner) => inner,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&'static str>()
                            .map(|s| (*s).to_owned())
                    })
                    .unwrap_or_else(|| "task panicked (non-string payload)".to_owned());
                Err(anyhow::anyhow!("task panicked: {msg}"))
            }
        };

        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let error_str = match &result {
            // Fast no-op ticks (stale_download_check with zero active
            // downloads, webhook_retry with nothing queued, etc.) land
            // at TRACE — they fire every 1-3s per task, so at any
            // higher level they dominate the persisted log without
            // carrying information. Anything that actually spent time
            // gets surfaced at INFO. Errors always land at ERROR.
            Ok(()) if duration_ms < 50 => {
                tracing::trace!(task = name, duration_ms, "task completed");
                emit_task_recovered_if_transition(event_tx, name);
                None
            }
            Ok(()) => {
                tracing::info!(task = name, duration_ms, "task completed");
                emit_task_recovered_if_transition(event_tx, name);
                None
            }
            Err(e) => {
                tracing::error!(task = name, duration_ms, error = %e, "task failed");
                // Dedup: a flapping task (indexer DNS failure, TMDB
                // rate-limit) used to page the user once per tick —
                // e.g. wanted_search every minute → 60 HealthWarnings
                // an hour for the same underlying problem. Emit only
                // on the OK→Err transition; subsequent failures stay
                // silent until recovery. Mirrors the disk-space sweep
                // pattern in cleanup::mod.
                emit_task_warning_if_transition(event_tx, name, e);
                Some(format!("{e}"))
            }
        };

        self.mark_done(name, duration_ms, error_str).await;
    }

    /// Wrapper around `execute_task` that forwards the task's result
    /// to a completion sender (used by the manual-trigger path when
    /// the caller wants to `await` completion). The result shape
    /// matches what `mark_done` persisted: `Ok(())` on success,
    /// `Err(error_message)` on failure.
    ///
    /// Public so integration tests can bypass the scheduler's select
    /// loop entirely: `run_task` calls this directly on a
    /// scheduler instance rather than dispatching through
    /// `trigger_tx` (which nobody's consuming in a single-call test).
    pub async fn execute_task_with_completion(
        &self,
        state: &crate::state::AppState,
        name: &str,
        completion: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) {
        // Claim first so this entry point (manual trigger / direct
        // test call) enforces the same "one instance at a time"
        // invariant as the scheduled-tick path. The due-check is
        // bypassed — the whole point of a manual trigger is to run
        // it *now* — but we still reject when the task is already
        // running rather than race.
        if !self.try_claim_manual(name).await {
            tracing::warn!(task = name, "task trigger rejected — already running");
            if let Some(tx) = completion {
                let _ = tx.send(Err(format!("task '{name}' is already running")));
            }
            return;
        }
        // Re-run the inner body and snapshot the result so we can
        // forward it. Doing this after `execute_task` lets us reuse
        // the existing error-emission + mark_done plumbing verbatim.
        self.execute_task(state, name).await;
        if let Some(tx) = completion {
            // Read the last_error we just stored. If the task
            // succeeded, last_error is None → Ok(()). Otherwise Err
            // with the message.
            let err = self
                .tasks
                .read()
                .await
                .get(name)
                .and_then(|t| t.last_error.clone());
            let _ = tx.send(match err {
                Some(msg) => Err(msg),
                None => Ok(()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn register_and_list_tasks() {
        let pool = db::create_test_pool().await;
        let scheduler = Scheduler::new(pool);

        scheduler
            .register("test_task", Duration::from_secs(60))
            .await;
        let tasks = scheduler.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "test_task");
        assert_eq!(tasks[0].interval_seconds, 60);
        assert!(!tasks[0].running);
    }

    #[tokio::test]
    async fn task_due_when_never_run() {
        let pool = db::create_test_pool().await;
        let scheduler = Scheduler::new(pool);
        scheduler.register("test", Duration::from_secs(60)).await;
        assert!(scheduler.is_due("test").await);
    }

    #[tokio::test]
    async fn task_not_due_when_running() {
        let pool = db::create_test_pool().await;
        let scheduler = Scheduler::new(pool);
        scheduler.register("test", Duration::from_secs(60)).await;
        scheduler.mark_running("test").await;
        assert!(!scheduler.is_due("test").await);
    }

    #[tokio::test]
    async fn mark_done_allows_rerun() {
        let pool = db::create_test_pool().await;
        let scheduler = Scheduler::new(pool);
        scheduler.register("test", Duration::from_secs(0)).await; // 0 interval = always due
        scheduler.mark_running("test").await;
        assert!(!scheduler.is_due("test").await);
        scheduler.mark_done("test", 42, None).await;
        // With 0 interval it's immediately due again
        assert!(scheduler.is_due("test").await);
    }

    #[tokio::test]
    async fn last_run_at_persisted() {
        let pool = db::create_test_pool().await;
        let scheduler = Scheduler::new(pool.clone());
        scheduler.register("test", Duration::from_secs(60)).await;
        scheduler.mark_running("test").await;

        // Verify in DB
        let stored: Option<String> =
            sqlx::query_scalar("SELECT last_run_at FROM scheduler_state WHERE task_name = 'test'")
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert!(stored.is_some());
    }

    /// Panic in a task body must translate to an error result +
    /// `mark_done` — otherwise `running=true` sticks forever and
    /// the task never fires again without a process restart. We
    /// can't easily invoke `execute_task` (needs a full `AppState`),
    /// so exercise the same `catch_unwind` pattern directly against
    /// a stand-in async block; the production path is the same
    /// shape.
    #[tokio::test]
    async fn panicking_task_body_yields_error_result() {
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;

        let body = async {
            panic!("simulated task panic");
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let result: anyhow::Result<()> = match AssertUnwindSafe(body).catch_unwind().await {
            Ok(inner) => inner,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&'static str>()
                            .map(|s| (*s).to_owned())
                    })
                    .unwrap_or_else(|| "task panicked (non-string payload)".to_owned());
                Err(anyhow::anyhow!("task panicked: {msg}"))
            }
        };
        let err = result.unwrap_err().to_string();
        assert!(err.contains("task panicked"), "got: {err}");
        assert!(err.contains("simulated task panic"), "got: {err}");
    }
}
