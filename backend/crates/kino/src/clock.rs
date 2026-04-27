//! Time abstraction so tests can drive clocks deterministically.
//!
//! Production code that needs the wall-clock for **observable
//! behaviour** (scheduler tick eligibility, search backoff, token
//! expiry, stall detection) calls `state.clock.now()` instead of
//! `chrono::Utc::now()`. Code that only uses the clock for log
//! timestamps or display strings keeps `Utc::now()` — drift in a
//! trace timestamp doesn't break tests and the indirection isn't
//! worth the noise.
//!
//! Two implementations:
//! - [`SystemClock`] — wraps `Utc::now()`. The production default.
//! - [`MockClock`] — wall-clock advanced manually via `set` / `advance`.
//!   Tests construct one, mutate it, and assert behaviour at known
//!   instants without sleep-then-poll.
//!
//! See `docs/roadmap/31-integration-testing.md` for the broader
//! testability story this is the first piece of.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};

/// Trait object stored on `AppState`. `Arc<dyn Clock>` lets tests
/// swap implementations without touching every call site that needs
/// a `now()` value.
pub trait Clock: Send + Sync + std::fmt::Debug {
    fn now(&self) -> DateTime<Utc>;
}

/// Production clock — direct passthrough to `chrono::Utc::now()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Test clock — `now()` returns the value held in shared state, never
/// the wall clock. `set` and `advance` mutate that state. Cloning
/// shares the same underlying instant, so a handle returned from the
/// test harness moves the clock for everyone using the same `AppState`.
#[derive(Debug, Clone)]
pub struct MockClock {
    inner: Arc<Mutex<DateTime<Utc>>>,
}

impl MockClock {
    /// Start the clock at a specific instant. Tests usually want a
    /// fixed reference point so assertions on derived timestamps are
    /// stable across runs.
    pub fn at(when: DateTime<Utc>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(when)),
        }
    }

    /// Move the clock to an absolute instant. Used when a test needs
    /// to skip past a long backoff window without the intermediate
    /// ticks mattering.
    pub fn set(&self, when: DateTime<Utc>) {
        let mut guard = self.inner.lock().expect("clock mutex poisoned");
        *guard = when;
    }

    /// Move the clock forward by a delta. Negative deltas would pull
    /// it back — supported, but tests almost always want forward
    /// motion. Mutex poisoning panics; tests that observe a panic
    /// should be fixed to not hold the lock across an assertion.
    pub fn advance(&self, delta: Duration) {
        let mut guard = self.inner.lock().expect("clock mutex poisoned");
        *guard += delta;
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        *self.inner.lock().expect("clock mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_returns_recent_time() {
        let before = Utc::now();
        let observed = SystemClock.now();
        let after = Utc::now();
        assert!(observed >= before && observed <= after);
    }

    #[test]
    fn mock_clock_holds_set_value() {
        let when = DateTime::parse_from_rfc3339("2026-04-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::at(when);
        assert_eq!(clock.now(), when);
        // Wall-clock movement does not affect the mock.
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(clock.now(), when);
    }

    #[test]
    fn mock_clock_advance_moves_forward() {
        let start = DateTime::parse_from_rfc3339("2026-04-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::at(start);
        clock.advance(Duration::seconds(30));
        assert_eq!(clock.now(), start + Duration::seconds(30));
    }

    #[test]
    fn mock_clock_clones_share_state() {
        let start = DateTime::parse_from_rfc3339("2026-04-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let a = MockClock::at(start);
        let b = a.clone();
        a.advance(Duration::minutes(5));
        // The clone observes the same forward motion — critical for
        // the integration harness, which hands separate handles to
        // the test body and to AppState.
        assert_eq!(b.now(), start + Duration::minutes(5));
    }
}
