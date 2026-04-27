//! `TranscodeSessionState` — typed lifecycle for a live transcode
//! session. Single source of truth that the watchdog,
//! producer-throttle, and admin-snapshot paths all read from.
//!
//! In-memory only — the session dies with the process, so this enum
//! has no `sqlx::Type` impls. It lives on `TranscodeSession` and is
//! read under the `RwLock` that guards the session map.
//!
//! ## State graph
//!
//! ```text
//!     Active ──producer ahead──▶ Suspended ──SIGCONT──▶ Active
//!       │
//!       ├──HW failure + more rungs──▶ Respawning ──spawn ok──▶ Active
//!       │
//!       ├──HW failure + chain exhausted──▶ Failed
//!       │
//!       ├──clean exit──▶ Exited
//!       │
//!       └──user stop / watchdog kill──▶ Cancelled
//! ```
//!
//! `Active`, `Suspended`, `Respawning` are non-terminal. `Exited`,
//! `Failed`, `Cancelled` are terminal — the session map evicts on
//! transition into any of those.
//!
//! ## Classification methods
//!
//! - [`is_running`](TranscodeSessionState::is_running) — segment
//!   requests can be served.
//! - [`is_terminal`](TranscodeSessionState::is_terminal) — session
//!   map should evict.
//! - [`accepts_throttle_signal`](TranscodeSessionState::accepts_throttle_signal)
//!   — `SIGSTOP`/`SIGCONT` would be meaningful.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// One transcode session's lifecycle position. See the module docs
/// for the full state graph and rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranscodeSessionState {
    /// ffmpeg child is running and producing segments. Default state
    /// after a successful spawn. Producer throttle may transition to
    /// `Suspended`; HW failure may transition to `Respawning`,
    /// `Failed`, or `Exited`.
    Active,

    /// ffmpeg child is `SIGSTOP`-ed by the producer-throttle path
    /// because the encoder got too far ahead of the client. A
    /// `SIGCONT` from the throttle path returns to `Active`. The
    /// child PID still exists; only the throttle path may transition
    /// out.
    Suspended,

    /// Mid-stream HW failure was classified; the previous child has
    /// been reaped and the chain has been advanced; the new child is
    /// in flight. Transitions to `Active` on successful spawn or
    /// `Failed` if the spawn itself errors.
    Respawning,

    /// ffmpeg child exited cleanly (status 0). Terminal — the session
    /// map evicts on entry.
    Exited,

    /// ffmpeg child exited non-zero AND the fallback chain is
    /// exhausted (no more rungs to try). Terminal.
    Failed,

    /// User-initiated stop or watchdog-initiated kill. Distinguished
    /// from `Failed` so the admin UI doesn't surface it as an error.
    /// Terminal.
    Cancelled,
}

impl TranscodeSessionState {
    /// All variants in declaration order.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::Active,
            Self::Suspended,
            Self::Respawning,
            Self::Exited,
            Self::Failed,
            Self::Cancelled,
        ]
        .into_iter()
    }

    /// Wire / log string. Only used for tracing + the admin snapshot
    /// JSON; there is no SQL column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Respawning => "respawning",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse from the wire string. Returns `None` for unknown values.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            "respawning" => Some(Self::Respawning),
            "exited" => Some(Self::Exited),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Can the segment-server route serve from this session? Both
    /// `Active` (currently producing) and `Suspended` (paused but
    /// segments on disk) qualify. `Respawning` does not — the new
    /// child hasn't produced its `init_v{N}.mp4` yet, so a fetch
    /// would 404; the segment route should hold or 503 instead.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Active | Self::Suspended)
    }

    /// Should the session map evict this session on the next sweep?
    /// Terminal states qualify.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Failed | Self::Cancelled)
    }

    /// Would `SIGSTOP` / `SIGCONT` be meaningful for this session?
    /// Only `Active` (can be stopped) and `Suspended` (can be
    /// continued) qualify. Sending a signal to a `Respawning` or
    /// terminal session is a logic error.
    #[must_use]
    pub const fn accepts_throttle_signal(self) -> bool {
        matches!(self, Self::Active | Self::Suspended)
    }

    /// Does the watchdog need to monitor this session for HWA
    /// failures + producer-ahead conditions? `Active` and `Suspended`
    /// qualify; `Respawning` is owned by the respawn path; terminal
    /// states are evict-only.
    #[must_use]
    pub const fn is_watchdog_monitored(self) -> bool {
        matches!(self, Self::Active | Self::Suspended)
    }
}

impl fmt::Display for TranscodeSessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TranscodeSessionState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown TranscodeSessionState value: {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_parse_roundtrip() {
        for state in TranscodeSessionState::all() {
            let s = state.as_str();
            let back = TranscodeSessionState::parse(s).expect("parses");
            assert_eq!(state, back, "round-trip via {s:?}");
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(TranscodeSessionState::parse("").is_none());
        assert!(TranscodeSessionState::parse("running").is_none());
        assert!(
            TranscodeSessionState::parse("Active").is_none(),
            "case-sensitive"
        );
    }

    #[test]
    fn serde_uses_snake_case_wire_format() {
        assert_eq!(
            serde_json::to_string(&TranscodeSessionState::Respawning).unwrap(),
            "\"respawning\""
        );
        let parsed: TranscodeSessionState = serde_json::from_str("\"suspended\"").unwrap();
        assert_eq!(parsed, TranscodeSessionState::Suspended);
    }

    #[test]
    fn fromstr_matches_parse() {
        for state in TranscodeSessionState::all() {
            let from_str: TranscodeSessionState = state.as_str().parse().unwrap();
            assert_eq!(from_str, state);
        }
        assert!("garbage".parse::<TranscodeSessionState>().is_err());
    }

    #[test]
    fn display_matches_wire_format() {
        for state in TranscodeSessionState::all() {
            assert_eq!(state.to_string(), state.as_str());
        }
    }

    #[test]
    fn running_set_is_pinned() {
        let r: Vec<_> = TranscodeSessionState::all()
            .filter(|s| s.is_running())
            .collect();
        assert_eq!(
            r,
            vec![
                TranscodeSessionState::Active,
                TranscodeSessionState::Suspended,
            ]
        );
    }

    #[test]
    fn terminal_set_is_pinned() {
        let t: Vec<_> = TranscodeSessionState::all()
            .filter(|s| s.is_terminal())
            .collect();
        assert_eq!(
            t,
            vec![
                TranscodeSessionState::Exited,
                TranscodeSessionState::Failed,
                TranscodeSessionState::Cancelled,
            ]
        );
    }

    #[test]
    fn throttle_signal_set_is_pinned() {
        let t: Vec<_> = TranscodeSessionState::all()
            .filter(|s| s.accepts_throttle_signal())
            .collect();
        assert_eq!(
            t,
            vec![
                TranscodeSessionState::Active,
                TranscodeSessionState::Suspended,
            ]
        );
    }

    #[test]
    fn watchdog_monitored_set_is_pinned() {
        let m: Vec<_> = TranscodeSessionState::all()
            .filter(|s| s.is_watchdog_monitored())
            .collect();
        assert_eq!(
            m,
            vec![
                TranscodeSessionState::Active,
                TranscodeSessionState::Suspended,
            ]
        );
    }

    /// Terminal and running are disjoint — a terminal session cannot
    /// be served from. Guards against eviction racing a segment fetch.
    #[test]
    fn terminal_and_running_are_disjoint() {
        for state in TranscodeSessionState::all() {
            assert!(
                !(state.is_terminal() && state.is_running()),
                "{state:?} is both terminal and running"
            );
        }
    }

    /// Throttle-signal targets are a subset of running. Sending
    /// `SIGSTOP` to a terminal/respawning session would either ESRCH
    /// or hit the wrong PID — both bad.
    #[test]
    fn throttle_signal_implies_running() {
        for state in TranscodeSessionState::all() {
            if state.accepts_throttle_signal() {
                assert!(
                    state.is_running(),
                    "{state:?} accepts throttle signal but is not running"
                );
            }
        }
    }

    /// Watchdog-monitored is exactly the running set — if a state is
    /// running, the watchdog must monitor; if the watchdog monitors,
    /// the state must be running. Property-test against the full enum.
    #[test]
    fn watchdog_monitored_equals_running() {
        for state in TranscodeSessionState::all() {
            assert_eq!(
                state.is_watchdog_monitored(),
                state.is_running(),
                "{state:?}: watchdog-monitored ≠ running"
            );
        }
    }
}
