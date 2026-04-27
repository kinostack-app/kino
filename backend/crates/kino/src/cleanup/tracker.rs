//! `CleanupTracker` — persistent retry queue for resource removals
//! that must succeed but can transiently fail (torrents, files,
//! directories).
//!
//! The pattern: callers wrap a removal call in
//! [`CleanupTracker::try_remove`]. On success the call returns
//! `Removed` and no row exists. On failure the (kind, target) pair
//! is upserted into `cleanup_queue` and the call returns `Queued`.
//! The scheduler ticks [`CleanupTracker::retry_failed`] on a fixed
//! cadence; each eligible row is re-executed via a
//! [`RemovalExecutor`] the caller provides. Successes delete the
//! row; failures bump `attempts`; rows past `max_attempts` are
//! retained as `Exhausted` and surfaced via
//! [`CleanupTracker::pending_exhausted_count`] for admin attention.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use sqlx::{Sqlite, SqlitePool};
use utoipa::ToSchema;

use crate::time::Timestamp;

/// Resource types the tracker handles. Stored as `snake_case` TEXT
/// in `cleanup_queue.resource_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// A torrent in the librqbit session, identified by `info_hash`
    /// (lowercase hex).
    Torrent,

    /// A single file on disk, identified by absolute path.
    File,

    /// A directory on disk, identified by absolute path. The
    /// executor decides between `remove_dir` (must be empty) and
    /// `remove_dir_all` (recursive); the tracker only knows the
    /// target string.
    Directory,
}

impl ResourceKind {
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Torrent, Self::File, Self::Directory].into_iter()
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Torrent => "torrent",
            Self::File => "file",
            Self::Directory => "directory",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "torrent" => Some(Self::Torrent),
            "file" => Some(Self::File),
            "directory" => Some(Self::Directory),
            _ => None,
        }
    }
}

impl sqlx::Type<Sqlite> for ResourceKind {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for ResourceKind {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        let s = self.as_str().to_owned();
        <String as sqlx::Encode<'q, Sqlite>>::encode(s, buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for ResourceKind {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s: String = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Self::parse(&s).ok_or_else(|| format!("unknown ResourceKind: {s:?}").into())
    }
}

/// Outcome of a single [`CleanupTracker::try_remove`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalOutcome {
    /// Removal succeeded immediately. No queue row exists.
    Removed,

    /// Removal failed; the (kind, target) pair is queued (or its
    /// existing row updated) and will retry on the next sweep.
    /// `attempts` is the new total after this call.
    Queued { attempts: i64 },

    /// Row's `attempts` reached `max_attempts`. The retry loop will
    /// no longer pick it up; admin attention required.
    Exhausted { attempts: i64 },
}

impl RemovalOutcome {
    #[must_use]
    pub const fn is_removed(&self) -> bool {
        matches!(self, Self::Removed)
    }
}

/// Caller-supplied dispatcher that knows how to actually remove a
/// resource by kind. The tracker is purely a queue + retry loop;
/// it doesn't carry references to torrent clients or file APIs.
pub trait RemovalExecutor: Sync {
    /// Execute the removal for `(kind, target)`. Return `Ok(())` on
    /// success (resource gone, *or* already gone — the executor
    /// decides which errors count as already-gone-success). Return
    /// `Err(message)` on transient failure that should retry.
    fn execute(
        &self,
        kind: ResourceKind,
        target: &str,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

/// Summary of one [`CleanupTracker::retry_failed`] sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetryReport {
    /// Rows the sweep attempted to retry this tick.
    pub attempted: u64,
    /// Of those, how many removed cleanly (queue row deleted).
    pub succeeded: u64,
    /// Failed again but still within the retry budget.
    pub still_failing: u64,
    /// Failed and reached `max_attempts`; row retained for admin.
    pub exhausted: u64,
}

/// Default minimum interval between retry attempts on the same row.
pub const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(300);

/// Maximum length of an error message stored in `last_error`.
const MAX_ERROR_MESSAGE_LEN: usize = 4096;

/// Persistent retry queue. See module docs for the contract.
#[derive(Debug, Clone)]
pub struct CleanupTracker {
    pool: SqlitePool,
    retry_interval: Duration,
}

impl CleanupTracker {
    /// Build a tracker with the default 5-minute retry interval.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            retry_interval: DEFAULT_RETRY_INTERVAL,
        }
    }

    /// Build with a custom retry interval. Useful for tests that
    /// drive the sweep with `MockClock`-style time travel.
    #[must_use]
    pub fn with_retry_interval(pool: SqlitePool, retry_interval: Duration) -> Self {
        Self {
            pool,
            retry_interval,
        }
    }

    /// Run `removal` once. On success: return `Removed`, no queue
    /// row. On failure: upsert into `cleanup_queue` (incrementing
    /// `attempts` if a row already existed) and return `Queued` or
    /// `Exhausted` depending on the new attempt count.
    pub async fn try_remove<F, Fut, E>(
        &self,
        kind: ResourceKind,
        target: &str,
        removal: F,
    ) -> sqlx::Result<RemovalOutcome>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), E>>,
        E: ToString,
    {
        match removal().await {
            Ok(()) => {
                // Success path: ensure no stale row sticks around for
                // a previously-failed (kind, target) that the caller
                // has now successfully cleaned up out-of-band.
                self.delete_row(kind, target).await?;
                Ok(RemovalOutcome::Removed)
            }
            Err(e) => self.queue_failure(kind, target, &e.to_string()).await,
        }
    }

    /// Sweep eligible rows (last attempt older than `retry_interval`,
    /// `attempts < max_attempts`) through `executor`. On success:
    /// delete the row. On failure: bump `attempts` and update the
    /// last-attempt timestamp; rows hitting `max_attempts` are left
    /// in place for admin visibility.
    pub async fn retry_failed<E: RemovalExecutor>(
        &self,
        executor: &E,
    ) -> sqlx::Result<RetryReport> {
        let cutoff = Timestamp::now_minus(
            chrono::Duration::from_std(self.retry_interval)
                .unwrap_or_else(|_| chrono::Duration::seconds(300)),
        );
        let rows: Vec<QueueRow> = sqlx::query_as::<_, QueueRow>(
            "SELECT id, resource_kind, target, attempts, max_attempts
             FROM cleanup_queue
             WHERE attempts < max_attempts
               AND (last_attempt_at IS NULL
                    OR datetime(last_attempt_at) <= datetime(?))",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        let mut report = RetryReport::default();
        for row in rows {
            report.attempted += 1;
            match executor.execute(row.resource_kind, &row.target).await {
                Ok(()) => {
                    sqlx::query("DELETE FROM cleanup_queue WHERE id = ?")
                        .bind(row.id)
                        .execute(&self.pool)
                        .await?;
                    report.succeeded += 1;
                }
                Err(msg) => {
                    let new_attempts = row.attempts + 1;
                    sqlx::query(
                        "UPDATE cleanup_queue
                         SET attempts = ?, last_error = ?, last_attempt_at = ?
                         WHERE id = ?",
                    )
                    .bind(new_attempts)
                    .bind(truncate_error(&msg))
                    .bind(Timestamp::now())
                    .bind(row.id)
                    .execute(&self.pool)
                    .await?;
                    if new_attempts >= row.max_attempts {
                        report.exhausted += 1;
                    } else {
                        report.still_failing += 1;
                    }
                }
            }
        }
        Ok(report)
    }

    /// Count of rows currently queued for retry (any state, any
    /// attempt count). For admin / `/status` surfaces.
    pub async fn pending_count(&self) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM cleanup_queue")
            .fetch_one(&self.pool)
            .await
    }

    /// Count of rows that have hit `max_attempts` and are no longer
    /// being retried. These need admin attention.
    pub async fn pending_exhausted_count(&self) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM cleanup_queue WHERE attempts >= max_attempts")
            .fetch_one(&self.pool)
            .await
    }

    // ── internals ─────────────────────────────────────────────────

    async fn delete_row(&self, kind: ResourceKind, target: &str) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM cleanup_queue WHERE resource_kind = ? AND target = ?")
            .bind(kind)
            .bind(target)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn queue_failure(
        &self,
        kind: ResourceKind,
        target: &str,
        error: &str,
    ) -> sqlx::Result<RemovalOutcome> {
        // UPSERT: a second failure on the same target updates the
        // existing row instead of creating a duplicate.
        let now = Timestamp::now();
        sqlx::query(
            "INSERT INTO cleanup_queue
                (resource_kind, target, attempts, last_error, last_attempt_at, created_at)
             VALUES (?, ?, 1, ?, ?, ?)
             ON CONFLICT(resource_kind, target) DO UPDATE SET
                attempts = cleanup_queue.attempts + 1,
                last_error = excluded.last_error,
                last_attempt_at = excluded.last_attempt_at",
        )
        .bind(kind)
        .bind(target)
        .bind(truncate_error(error))
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        let (attempts, max_attempts): (i64, i64) = sqlx::query_as(
            "SELECT attempts, max_attempts FROM cleanup_queue
             WHERE resource_kind = ? AND target = ?",
        )
        .bind(kind)
        .bind(target)
        .fetch_one(&self.pool)
        .await?;

        if attempts >= max_attempts {
            Ok(RemovalOutcome::Exhausted { attempts })
        } else {
            Ok(RemovalOutcome::Queued { attempts })
        }
    }
}

#[derive(sqlx::FromRow)]
struct QueueRow {
    id: i64,
    resource_kind: ResourceKind,
    target: String,
    attempts: i64,
    max_attempts: i64,
}

fn truncate_error(s: &str) -> String {
    if s.len() <= MAX_ERROR_MESSAGE_LEN {
        s.to_owned()
    } else {
        let mut out = s[..MAX_ERROR_MESSAGE_LEN].to_owned();
        out.push_str("…[truncated]");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::db;

    async fn fresh_pool() -> SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .expect("seed defaults");
        pool
    }

    /// Counts each call by (kind, target). Returns `Ok` for every
    /// call by default; use `with_failure` to script `Err` outcomes.
    struct CountingExecutor {
        calls: Mutex<Vec<(ResourceKind, String)>>,
        fail_with: Mutex<Option<String>>,
    }

    impl CountingExecutor {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_with: Mutex::new(None),
            }
        }
        fn with_failure(msg: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_with: Mutex::new(Some(msg.to_owned())),
            }
        }
        fn calls(&self) -> Vec<(ResourceKind, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl RemovalExecutor for CountingExecutor {
        async fn execute(&self, kind: ResourceKind, target: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push((kind, target.to_owned()));
            match self.fail_with.lock().unwrap().clone() {
                Some(msg) => Err(msg),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn resource_kind_round_trip() {
        for k in ResourceKind::all() {
            assert_eq!(ResourceKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn truncate_error_caps_long_messages() {
        let s = "x".repeat(MAX_ERROR_MESSAGE_LEN + 100);
        let out = truncate_error(&s);
        assert!(out.len() <= MAX_ERROR_MESSAGE_LEN + "…[truncated]".len());
        assert!(out.ends_with("[truncated]"));
    }

    #[tokio::test]
    async fn try_remove_success_returns_removed_and_no_queue_row() {
        let pool = fresh_pool().await;
        let tracker = CleanupTracker::new(pool.clone());
        let outcome = tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Ok::<_, String>(())
            })
            .await
            .unwrap();
        assert_eq!(outcome, RemovalOutcome::Removed);
        assert_eq!(tracker.pending_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn try_remove_failure_queues_with_attempts_one() {
        let pool = fresh_pool().await;
        let tracker = CleanupTracker::new(pool.clone());
        let outcome = tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Err::<(), _>("librqbit unreachable")
            })
            .await
            .unwrap();
        assert_eq!(outcome, RemovalOutcome::Queued { attempts: 1 });
        assert_eq!(tracker.pending_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn try_remove_twice_increments_existing_row() {
        let pool = fresh_pool().await;
        let tracker = CleanupTracker::new(pool.clone());
        tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Err::<(), _>("first")
            })
            .await
            .unwrap();
        let second = tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Err::<(), _>("second")
            })
            .await
            .unwrap();
        assert_eq!(second, RemovalOutcome::Queued { attempts: 2 });
        assert_eq!(tracker.pending_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn try_remove_after_success_clears_stale_queue_row() {
        let pool = fresh_pool().await;
        let tracker = CleanupTracker::new(pool.clone());
        // First attempt fails → queued.
        tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Err::<(), _>("first")
            })
            .await
            .unwrap();
        assert_eq!(tracker.pending_count().await.unwrap(), 1);
        // Subsequent attempt succeeds → row deleted.
        tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Ok::<_, String>(())
            })
            .await
            .unwrap();
        assert_eq!(tracker.pending_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn retry_failed_runs_executor_for_eligible_rows() {
        let pool = fresh_pool().await;
        // Zero-second interval so freshly-queued rows are immediately eligible.
        let tracker = CleanupTracker::with_retry_interval(pool.clone(), Duration::from_secs(0));
        for i in 0..3 {
            tracker
                .try_remove(ResourceKind::File, &format!("/tmp/x{i}"), || async {
                    Err::<(), _>("disk busy")
                })
                .await
                .unwrap();
        }
        let executor = CountingExecutor::new();
        let report = tracker.retry_failed(&executor).await.unwrap();
        assert_eq!(report.attempted, 3);
        assert_eq!(report.succeeded, 3);
        assert_eq!(executor.calls().len(), 3);
        assert_eq!(tracker.pending_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn retry_failed_skips_recently_attempted_rows() {
        let pool = fresh_pool().await;
        // Long interval; the just-queued row's last_attempt_at = now,
        // so it must NOT be picked up by an immediate sweep.
        let tracker = CleanupTracker::with_retry_interval(pool.clone(), Duration::from_secs(3600));
        tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Err::<(), _>("first")
            })
            .await
            .unwrap();
        let executor = CountingExecutor::new();
        let report = tracker.retry_failed(&executor).await.unwrap();
        assert_eq!(report.attempted, 0);
        assert_eq!(executor.calls().len(), 0);
    }

    #[tokio::test]
    async fn retry_failed_marks_row_exhausted_after_max_attempts() {
        let pool = fresh_pool().await;
        let tracker = CleanupTracker::with_retry_interval(pool.clone(), Duration::from_secs(0));
        tracker
            .try_remove(ResourceKind::Torrent, "abc", || async {
                Err::<(), _>("init")
            })
            .await
            .unwrap();
        // max_attempts default is 5; first try_remove counted 1, so 4 more
        // failed sweeps will exhaust.
        let executor = CountingExecutor::with_failure("still failing");
        for _ in 0..4 {
            let _ = tracker.retry_failed(&executor).await.unwrap();
        }
        // The last sweep should mark exhausted.
        let report_before = tracker.retry_failed(&executor).await.unwrap();
        assert_eq!(report_before.attempted, 0, "exhausted rows are not retried");
        assert_eq!(tracker.pending_exhausted_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn retry_failed_dispatches_only_eligible_rows_to_executor() {
        // Mix of eligible + recently-attempted; executor must only see the
        // eligible ones.
        let pool = fresh_pool().await;
        let long_tracker =
            CleanupTracker::with_retry_interval(pool.clone(), Duration::from_secs(3600));
        // Row A: just queued; not eligible under long interval.
        long_tracker
            .try_remove(ResourceKind::Torrent, "recent", || async {
                Err::<(), _>("e")
            })
            .await
            .unwrap();
        // Row B: queued via a zero-interval tracker, so it has an "old enough" timestamp
        // when the long-interval tracker sweeps with the cutoff - actually we need a
        // different approach. Instead: backdate the queued row directly.
        long_tracker
            .try_remove(ResourceKind::Torrent, "old", || async { Err::<(), _>("e") })
            .await
            .unwrap();
        // Backdate the "old" row's last_attempt_at by an hour.
        sqlx::query(
            "UPDATE cleanup_queue
             SET last_attempt_at = datetime('now', '-2 hours')
             WHERE target = 'old'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let executor = CountingExecutor::new();
        let report = long_tracker.retry_failed(&executor).await.unwrap();
        assert_eq!(report.attempted, 1);
        let calls = executor.calls();
        assert_eq!(calls, vec![(ResourceKind::Torrent, "old".to_owned())]);
    }

    struct AtomicCounter(AtomicUsize);
    impl RemovalExecutor for AtomicCounter {
        async fn execute(&self, _: ResourceKind, _: &str) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Trait bound is `Sync` so the tracker may dispatch concurrently.
    /// The current implementation is sequential; this test guards
    /// the contract for future parallelisation.
    #[tokio::test]
    async fn executor_sync_bound_holds() {
        let pool = fresh_pool().await;
        let tracker = CleanupTracker::with_retry_interval(pool.clone(), Duration::from_secs(0));
        tracker
            .try_remove(ResourceKind::File, "/a", || async { Err::<(), _>("e") })
            .await
            .unwrap();
        let exec = AtomicCounter(AtomicUsize::new(0));
        tracker.retry_failed(&exec).await.unwrap();
        assert_eq!(exec.0.load(Ordering::SeqCst), 1);
    }
}
