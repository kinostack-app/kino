//! Integration-test harness. See `docs/roadmap/31-integration-testing.md`
//! for the full design.
//!
//! Owns the test-side helpers — fixtures, fakes, assertions — so
//! individual flow tests stay focused on the business assertion
//! ("after this happens, the user sees that"). Production code does
//! not depend on this module; it's compiled only under `cfg(test)`
//! or with the `test-support` feature enabled (the latter lets
//! crate-external integration tests in `tests/` reach the same
//! primitives).
//!
//! This is the **scaffold-only** initial landing — `TestApp` +
//! `TestAppBuilder` exposing the existing `test_app_with_db()` shape
//! through a builder so subsequent PRs can layer in mock servers,
//! fakes, and the deterministic scheduler trigger without rewriting
//! every call site.

pub mod fake_torrent;
pub mod mock_tmdb;
pub mod mock_torznab;
pub mod mock_trakt;

pub use fake_torrent::FakeTorrentSession;
pub use mock_tmdb::MockTmdbServer;
pub use mock_torznab::MockTorznabServer;
pub use mock_trakt::MockTraktServer;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use sqlx::SqlitePool;
use tower::ServiceExt as _;

use crate::clock::{Clock, MockClock, SystemClock};
use crate::db;
use crate::download::TorrentSession;
use crate::scheduler::Scheduler;
use crate::state::AppState;
use crate::tmdb::TmdbClient;

/// Builder for an isolated test app. Each test calls `build()` to get
/// a fresh router + DB + harness handles. Defaults reproduce today's
/// `test_app_with_db()` so existing tests can migrate gradually
/// without behaviour change.
#[derive(Debug, Default)]
pub struct TestAppBuilder {
    /// When `Some`, the test app installs this clock instead of
    /// `SystemClock`. Cloning the handle before `build()` lets the
    /// test body advance time after construction.
    clock: Option<Arc<dyn Clock>>,
    /// When `Some`, replaces the (default `None`) torrent slot on
    /// `AppState`. Tests that exercise the download pipeline pass a
    /// `FakeTorrentSession` here.
    torrent: Option<Arc<dyn TorrentSession>>,
    /// When `Some`, installs a TMDB client pointed at the given
    /// base URL (typically from a `MockTmdbServer`). The API key is
    /// a fixed test value (`test-tmdb`) — mock servers don't care.
    tmdb_base_url: Option<String>,
}

impl TestAppBuilder {
    /// Default builder: behaves identically to today's
    /// `test_app_with_db()` — empty DB, no TMDB / Trakt / torrent
    /// client, system clock.
    pub fn new() -> Self {
        Self {
            clock: None,
            torrent: None,
            tmdb_base_url: None,
        }
    }

    /// Install a deterministic clock. Tests usually want
    /// `MockClock::at(...)`. Returning the builder by value composes
    /// with chained calls.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Install a torrent session — typically a `FakeTorrentSession`
    /// the test body has cloned a handle to so it can drive
    /// completion / failure / progress explicitly.
    #[must_use]
    pub fn with_torrent(mut self, torrent: Arc<dyn TorrentSession>) -> Self {
        self.torrent = Some(torrent);
        self
    }

    /// Route TMDB requests through the given base URL (usually from
    /// a `MockTmdbServer`). Sets the DB's `tmdb_api_key` to a fixed
    /// test value so `require_tmdb` succeeds — mock servers don't
    /// verify the bearer.
    #[must_use]
    pub fn with_tmdb(mut self, base_url: impl Into<String>) -> Self {
        self.tmdb_base_url = Some(base_url.into());
        self
    }

    /// Spin up the router + DB. Each call creates a fresh
    /// in-memory `SQLite` pool with all migrations applied; no test
    /// shares state with another. Async because the migration
    /// runner is async.
    pub async fn build(self) -> TestApp {
        let pool = db::create_test_pool().await;

        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .expect("ensure_defaults in test");

        let api_key = sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("api key in test config");

        let (log_live, _) = tokio::sync::broadcast::channel(16);
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let (mut state, mut trigger_rx) = AppState::new(
            pool.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            std::path::PathBuf::from("/tmp/kino-test"),
            0,
            log_live,
            event_tx,
            2,
        );

        if let Some(clock) = self.clock {
            state.clock = clock;
        }
        if let Some(torrent) = self.torrent {
            state.torrent = Some(torrent);
        }
        if let Some(base_url) = self.tmdb_base_url {
            // Persist a stub key so the `require_tmdb` gate passes.
            // The value doesn't have to match anything the mock
            // checks — wiremock ignores Authorization by default.
            sqlx::query("UPDATE config SET tmdb_api_key = 'test-tmdb' WHERE id = 1")
                .execute(&pool)
                .await
                .expect("set stub tmdb key");
            state.set_tmdb(Some(TmdbClient::with_base_url(
                "test-tmdb".to_owned(),
                base_url,
            )));
        }

        // Always install a scheduler with the default task set so
        // `TestApp::run_task` can dispatch to `execute_task_with_completion`
        // without surprise "task not registered" failures.
        let scheduler = Scheduler::new(pool.clone());
        scheduler.register_defaults(15).await;
        state.scheduler = Some(scheduler.clone());

        // Drain the scheduler trigger channel in a background task so
        // `state.trigger_tx.send(...)` from handler code (e.g. POST
        // /api/v1/tasks/{name}/run) doesn't fail with a closed
        // receiver. The scheduler's real select loop isn't running in
        // tests, so messages would otherwise back up immediately or
        // bounce off a dead channel; tests that *want* to drive the
        // task call `TestApp::run_task` directly.
        tokio::spawn(async move {
            while trigger_rx.recv().await.is_some() {
                // Discard — the scheduler isn't here to act on it.
            }
        });

        let router = crate::build_router(state.clone());
        TestApp {
            router,
            api_key,
            db: pool,
            state,
            scheduler,
        }
    }
}

/// Constructed test app: hand off to the test body for arrange-act-assert.
///
/// Holds the router for HTTP-level interactions, the DB pool for
/// direct query assertions, the full `AppState` so test helpers can
/// reach background-service state, and the `Scheduler` so
/// `run_task` can dispatch to `execute_task_with_completion`
/// without going through the `mpsc` channel.
#[derive(Debug)]
pub struct TestApp {
    pub router: Router,
    pub api_key: String,
    pub db: SqlitePool,
    pub state: AppState,
    pub scheduler: Scheduler,
}

impl TestApp {
    /// Authenticated GET. Returns the raw response so the caller can
    /// assert on status, headers, or body shape as needed.
    pub async fn get(&self, path: &str) -> Response {
        let req = Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {}", self.api_key))
            .body(Body::empty())
            .expect("valid request");
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("router response")
    }

    /// Authenticated POST with a JSON body. `body` is serialised on
    /// the way out so tests don't need to construct strings by hand.
    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Response {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid request");
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("router response")
    }

    /// Authenticated PUT with a JSON body. Used by endpoints that
    /// replace-in-place (webhooks, indexers).
    pub async fn put(&self, path: &str, body: &serde_json::Value) -> Response {
        let req = Request::builder()
            .method("PUT")
            .uri(path)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid request");
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("router response")
    }

    /// Authenticated PATCH with a JSON body. Used by endpoints that
    /// do partial updates (preferences, show monitor toggles).
    pub async fn patch(&self, path: &str, body: &serde_json::Value) -> Response {
        let req = Request::builder()
            .method("PATCH")
            .uri(path)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid request");
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("router response")
    }

    /// Authenticated DELETE. Used by tests that exercise the cancel /
    /// unfollow paths.
    pub async fn delete(&self, path: &str) -> Response {
        let req = Request::builder()
            .method("DELETE")
            .uri(path)
            .header("authorization", format!("Bearer {}", self.api_key))
            .body(Body::empty())
            .expect("valid request");
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("router response")
    }

    /// Synchronously run a scheduler task and return its result.
    /// Bypasses the trigger mpsc + tick loop so a single test call
    /// doesn't need the scheduler's select-loop spawned. On error
    /// the returned string matches what would be persisted in
    /// `task.last_error`.
    pub async fn run_task(&self, name: &str) -> Result<(), String> {
        let (_trigger, rx) = crate::scheduler::TaskTrigger::with_completion(name);
        // Call execute_task_with_completion directly — the scheduler
        // select loop isn't running in tests, so the channel would
        // otherwise block forever.
        let sched = self.scheduler.clone();
        let state = self.state.clone();
        let name = name.to_owned();
        let (tx, rx_local) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            sched
                .execute_task_with_completion(&state, &name, Some(tx))
                .await;
        });
        // Drop the TaskTrigger-bound rx — we're using rx_local from
        // the directly-spawned future. `rx` is here purely for
        // TaskTrigger symmetry in case callers want the message.
        drop(rx);
        rx_local.await.expect("scheduler task completion")
    }
}

/// Response-shape helpers shared across tests. Pulled out of `TestApp`
/// so they're usable on any `axum::response::Response` (e.g. when a
/// test holds a raw response from a handler call instead of going
/// through the router).
pub async fn json_body(resp: Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("response body");
    serde_json::from_slice(&body).expect("response body is JSON")
}

pub fn assert_status(resp: &Response, expected: StatusCode) {
    assert_eq!(
        resp.status(),
        expected,
        "expected {expected}, got {}",
        resp.status()
    );
}

/// Convenience constructor for a system clock — saves callers from
/// importing both the trait and the type when they just want the
/// production default explicitly.
#[must_use]
pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

/// Convenience constructor for a fixed-instant mock clock.
#[must_use]
pub fn mock_clock_at(when: chrono::DateTime<chrono::Utc>) -> Arc<MockClock> {
    Arc::new(MockClock::at(when))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    #[tokio::test]
    async fn builder_default_serves_status() {
        let app = TestAppBuilder::new().build().await;
        let resp = app.get("/api/v1/status").await;
        assert_status(&resp, StatusCode::OK);
    }

    #[tokio::test]
    async fn builder_installs_mock_clock() {
        let when = Utc.with_ymd_and_hms(2026, 4, 19, 12, 0, 0).unwrap();
        let clock = mock_clock_at(when);
        let app = TestAppBuilder::new()
            .with_clock(clock.clone())
            .build()
            .await;

        // Sanity: the harness still serves through the router.
        let resp = app.get("/api/v1/status").await;
        assert_status(&resp, StatusCode::OK);

        // Sanity: the mock clock is the one installed and movable.
        clock.advance(Duration::hours(2));
        assert_eq!(clock.now(), when + Duration::hours(2));
    }
}
