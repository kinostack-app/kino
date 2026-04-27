//! `ScrobbleState` — typed lifecycle for a queued Trakt scrobble.
//!
//! Centralises the classification of a `trakt_scrobble_queue` row's
//! progress. The drain path returns a `ScrobbleState`; history
//! events carry `Sent` / `Dropped` so admin UIs don't have to
//! infer outcomes from log lines.
//!
//! Lives as a typed value passed around the drain logic rather than
//! a column on the queue row — scrobble lifecycles are short and the
//! state matters to the drain task + audit trail, not to long-lived
//! UI surfaces. The predicates here drive the in-flight logic
//! whether or not a column ever lands.
//!
//! ## State graph
//!
//! ```text
//!     Pending ─emit attempt─▶ InFlight ─2xx─▶ Sent
//!         ▲                       │
//!         │                       ├─error─▶ Failed (re-queued)
//!         │                       │
//!         └────────retry──────────┘
//!
//!     Pending / Failed ─stale (>5min start, >24h any)─▶ Dropped
//!     Pending ─Trakt disconnected / scrobble disabled─▶ Skipped
//! ```
//!
//! `Pending`, `InFlight`, `Failed` are non-terminal. `Sent`, `Dropped`,
//! `Skipped` are terminal.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// One scrobble attempt's lifecycle position. See the module docs
/// for the full state graph and rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrobbleState {
    /// Queued for emission; no HTTP call yet. Either freshly enqueued
    /// (live emit failed) or selected by the drain task for the next
    /// attempt.
    Pending,

    /// HTTP request to `/scrobble/{action}` is in flight. The drain
    /// task holds the row in this state for the duration of the
    /// request so a parallel sweep won't double-emit.
    InFlight,

    /// 2xx response from Trakt. Terminal — drain deletes the row.
    Sent,

    /// Non-2xx response or transport error. Re-queued; `attempts` is
    /// incremented and the next drain tick will pick it up. Distinct
    /// from `Pending` so admin UIs can show "N items failing to send"
    /// vs "N items queued for first try".
    Failed,

    /// Terminal not-sent. Reasons include: aged out (>24h, or >5min
    /// for `start`/`pause` actions which carry no value when stale),
    /// payload couldn't be reconstructed (movie/episode deleted),
    /// or the row had no resolvable target. The drain emits a WARN
    /// before transitioning. Terminal.
    Dropped,

    /// Terminal not-sent because scrobbling is disabled or Trakt is
    /// disconnected at attempt time. Distinguished from `Dropped` so
    /// admin UIs don't surface "X scrobbles failed" for a user who
    /// has Trakt off on purpose. Terminal.
    Skipped,
}

impl ScrobbleState {
    /// All variants in declaration order.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::Pending,
            Self::InFlight,
            Self::Sent,
            Self::Failed,
            Self::Dropped,
            Self::Skipped,
        ]
        .into_iter()
    }

    /// Wire / log string. Used in tracing fields and admin events.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Dropped => "dropped",
            Self::Skipped => "skipped",
        }
    }

    /// Parse from the wire string. Returns `None` for unknown values.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_flight" => Some(Self::InFlight),
            "sent" => Some(Self::Sent),
            "failed" => Some(Self::Failed),
            "dropped" => Some(Self::Dropped),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }

    /// Is this row eligible for the next drain tick? `Pending` and
    /// `Failed` qualify; `InFlight` is owned by the current tick;
    /// terminal states have no work left.
    #[must_use]
    pub const fn is_drain_eligible(self) -> bool {
        matches!(self, Self::Pending | Self::Failed)
    }

    /// Terminal — drain may delete the row.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Sent | Self::Dropped | Self::Skipped)
    }

    /// Did this scrobble actually reach Trakt? Only `Sent` qualifies.
    /// `Skipped` and `Dropped` are terminal but the user's watch
    /// history was never recorded — separate from "succeeded".
    #[must_use]
    pub const fn succeeded(self) -> bool {
        matches!(self, Self::Sent)
    }

    /// Should the admin UI surface this row as a problem? `Failed`
    /// means we're still trying; `Dropped` is the "we gave up"
    /// signal that warrants user attention. `Skipped` is by design
    /// (Trakt off), not a problem.
    #[must_use]
    pub const fn is_user_visible_problem(self) -> bool {
        matches!(self, Self::Dropped)
    }
}

impl fmt::Display for ScrobbleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ScrobbleState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown ScrobbleState value: {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_parse_roundtrip() {
        for state in ScrobbleState::all() {
            let s = state.as_str();
            let back = ScrobbleState::parse(s).expect("parses");
            assert_eq!(state, back, "round-trip via {s:?}");
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(ScrobbleState::parse("").is_none());
        assert!(ScrobbleState::parse("queued").is_none());
        assert!(ScrobbleState::parse("Pending").is_none(), "case-sensitive");
    }

    #[test]
    fn serde_uses_snake_case_wire_format() {
        assert_eq!(
            serde_json::to_string(&ScrobbleState::InFlight).unwrap(),
            "\"in_flight\""
        );
        let parsed: ScrobbleState = serde_json::from_str("\"sent\"").unwrap();
        assert_eq!(parsed, ScrobbleState::Sent);
    }

    #[test]
    fn fromstr_matches_parse() {
        for state in ScrobbleState::all() {
            let from_str: ScrobbleState = state.as_str().parse().unwrap();
            assert_eq!(from_str, state);
        }
        assert!("garbage".parse::<ScrobbleState>().is_err());
    }

    #[test]
    fn display_matches_wire_format() {
        for state in ScrobbleState::all() {
            assert_eq!(state.to_string(), state.as_str());
        }
    }

    #[test]
    fn drain_eligible_set_is_pinned() {
        let e: Vec<_> = ScrobbleState::all()
            .filter(|s| s.is_drain_eligible())
            .collect();
        assert_eq!(e, vec![ScrobbleState::Pending, ScrobbleState::Failed]);
    }

    #[test]
    fn terminal_set_is_pinned() {
        let t: Vec<_> = ScrobbleState::all().filter(|s| s.is_terminal()).collect();
        assert_eq!(
            t,
            vec![
                ScrobbleState::Sent,
                ScrobbleState::Dropped,
                ScrobbleState::Skipped,
            ]
        );
    }

    #[test]
    fn succeeded_is_only_sent() {
        for state in ScrobbleState::all() {
            assert_eq!(state.succeeded(), state == ScrobbleState::Sent);
        }
    }

    #[test]
    fn user_visible_problem_set_is_pinned() {
        let p: Vec<_> = ScrobbleState::all()
            .filter(|s| s.is_user_visible_problem())
            .collect();
        assert_eq!(p, vec![ScrobbleState::Dropped]);
    }

    /// Drain-eligible and terminal are disjoint. If they ever overlap,
    /// the drain task would re-pick a row that was already terminal —
    /// an `InFlight` from a previous tick stuck for >tick interval
    /// being mis-classified as drainable, etc.
    #[test]
    fn drain_eligible_and_terminal_are_disjoint() {
        for state in ScrobbleState::all() {
            assert!(
                !(state.is_drain_eligible() && state.is_terminal()),
                "{state:?} is both drain-eligible and terminal"
            );
        }
    }

    /// Succeeded implies terminal. A state that "succeeded" but isn't
    /// terminal would mean the drain might re-emit a successful
    /// scrobble — Trakt would dedupe but we'd waste calls + log noise.
    #[test]
    fn succeeded_implies_terminal() {
        for state in ScrobbleState::all() {
            if state.succeeded() {
                assert!(
                    state.is_terminal(),
                    "{state:?} succeeded but is not terminal"
                );
            }
        }
    }
}
