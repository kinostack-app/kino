//! User-initiated + scheduler-initiated Cardigann definitions
//! refresh.
//!
//! Mirrors the shape of `playback::ffmpeg_bundle` exactly so the
//! frontend can lift the same progress-modal pattern: state enum,
//! tracker behind an `Arc<Mutex>`, `start_refresh` spawns the
//! task, `AppEvent` broadcasts each step, the `GET .../refresh`
//! endpoint snapshot is the late-joiner-safe authoritative state.
//!
//! One refresh may be in flight at a time. A second `start_refresh`
//! while one is running yields `RefreshError::AlreadyRunning` so
//! the caller (HTTP handler or scheduler) can decide whether to
//! 409 or silently no-op.

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use utoipa::ToSchema;

use crate::events::AppEvent;
use crate::indexers::loader::DefinitionLoader;

// ─── Public state ─────────────────────────────────────────────────

/// Public-facing snapshot of the refresh subsystem. Returned by
/// `GET /api/v1/indexer-definitions/refresh`; emitted via
/// `AppEvent::IndexerDefinitionsRefresh*` variants as the state
/// transitions.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DefinitionsRefreshState {
    /// No refresh active, no previous attempt this session.
    Idle,
    /// Refresh in progress. `fetched` is monotonically increasing;
    /// `total` is the count of YAML files in the upstream listing.
    Running { fetched: u32, total: u32 },
    /// Refresh succeeded. `count` is the number of definitions
    /// written to the cache directory + reloaded into memory.
    Completed { count: u32 },
    /// Refresh failed. `reason` is UI-grade text.
    Failed { reason: String },
}

/// Synchronisation primitive for at-most-one concurrent refresh.
/// Cloned onto `AppState`; every consumer reads the same state.
#[derive(Debug, Clone)]
pub struct DefinitionsRefreshTracker {
    inner: Arc<Mutex<DefinitionsRefreshState>>,
}

impl Default for DefinitionsRefreshTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DefinitionsRefreshTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DefinitionsRefreshState::Idle)),
        }
    }

    /// Current state snapshot for the GET endpoint.
    pub async fn snapshot(&self) -> DefinitionsRefreshState {
        self.inner.lock().await.clone()
    }

    async fn set(&self, state: DefinitionsRefreshState) {
        *self.inner.lock().await = state;
    }
}

// ─── Errors ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("a definitions refresh is already running")]
    AlreadyRunning,
    #[error("indexer definitions loader is not configured")]
    LoaderUnavailable,
}

// ─── Spawn entry point ────────────────────────────────────────────

/// Kick off a refresh task. Returns immediately; progress +
/// completion flow via `AppEvent` broadcasts and the tracker.
///
/// Rejects with `AlreadyRunning` when a prior call's task hasn't
/// reached a terminal state. The HTTP handler maps that to a
/// 409 Conflict; the scheduler treats it as a benign skip.
pub async fn start_refresh(
    tracker: DefinitionsRefreshTracker,
    loader: Option<Arc<DefinitionLoader>>,
    event_tx: broadcast::Sender<AppEvent>,
    db: sqlx::SqlitePool,
) -> Result<(), RefreshError> {
    let loader = loader.ok_or(RefreshError::LoaderUnavailable)?;

    {
        let mut guard = tracker.inner.lock().await;
        if matches!(*guard, DefinitionsRefreshState::Running { .. }) {
            return Err(RefreshError::AlreadyRunning);
        }
        *guard = DefinitionsRefreshState::Running {
            fetched: 0,
            total: 0,
        };
    }

    tokio::spawn(async move {
        run_refresh(&tracker, loader, event_tx, db).await;
    });

    Ok(())
}

async fn run_refresh(
    tracker: &DefinitionsRefreshTracker,
    loader: Arc<DefinitionLoader>,
    event_tx: broadcast::Sender<AppEvent>,
    db: sqlx::SqlitePool,
) {
    let cb_tracker = tracker.clone();
    let cb_event_tx = event_tx.clone();
    let progress: Arc<dyn Fn(u32, u32) + Send + Sync> = Arc::new(move |fetched, total| {
        let t = cb_tracker.clone();
        tokio::spawn(async move {
            t.set(DefinitionsRefreshState::Running { fetched, total })
                .await;
        });
        let _ = cb_event_tx.send(AppEvent::IndexerDefinitionsRefreshProgress { fetched, total });
    });

    match loader.update_from_remote(Some(progress)).await {
        Ok(count) => {
            let count_u32 = u32::try_from(count).unwrap_or(u32::MAX);
            tracker
                .set(DefinitionsRefreshState::Completed { count: count_u32 })
                .await;
            let _ =
                event_tx.send(AppEvent::IndexerDefinitionsRefreshCompleted { count: count_u32 });

            // Persist the success timestamp + flip the user-consent
            // flag to 1. The flag is the gate `refresh_sweep` checks
            // before the daily scheduled run, so the very first
            // successful refresh (necessarily user-triggered, since
            // the gate blocks the scheduler until then) opts the
            // user into the daily-keep-fresh cadence. Idempotent on
            // subsequent (scheduled) refreshes — the WHERE id = 1
            // touch is cheap. ISO-8601 UTC for sqlite-compatible
            // string comparisons.
            let now = Utc::now().to_rfc3339();
            if let Err(e) = sqlx::query(
                "UPDATE config SET definitions_last_refreshed_at = ?, \
                 definitions_auto_refresh_enabled = 1 WHERE id = 1",
            )
            .bind(&now)
            .execute(&db)
            .await
            {
                tracing::warn!(
                    error = %e,
                    "definitions_refresh: failed to persist last-refresh timestamp + consent flag",
                );
            }

            tracing::info!(count, "indexer definitions refresh complete");
        }
        Err(e) => {
            let reason = e.to_string();
            tracker
                .set(DefinitionsRefreshState::Failed {
                    reason: reason.clone(),
                })
                .await;
            let _ = event_tx.send(AppEvent::IndexerDefinitionsRefreshFailed { reason });
            tracing::warn!(error = %e, "indexer definitions refresh failed");
        }
    }
}
