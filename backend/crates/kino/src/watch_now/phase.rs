//! `WatchNowPhase` — typed lifecycle for a watch-now request.
//!
//! Persisted on the `download` row so cancel survives restart and
//! the background alternate-release loop's exit condition is a
//! single-row read.
//!
//! Variants serialise as `snake_case` (`phase_one`, `phase_two`,
//! `settled`, `cancelled`) for natural `sqlite3` REPL inspection.
//!
//! ## State graph
//!
//! ```text
//!     PhaseOne ──pick succeeds──▶ PhaseTwo ──alt loop done──▶ Settled
//!         │                          │
//!         │                          └────user cancel────▶ Cancelled
//!         │
//!         └────user cancel / pick fails terminal────▶ Cancelled
//! ```
//!
//! `PhaseOne` is the initial release-pick window. `PhaseTwo` is the
//! background alternate-release loop after the initial pick streamed.
//! `Settled` is the terminal "loop finished" state — the row remains
//! for history; no background work touches it. `Cancelled` is the
//! terminal user-initiated stop.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sqlx::Sqlite;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use utoipa::ToSchema;

/// One watch-now row's lifecycle position. See the module docs for
/// the full state graph and the rationale for moving this off the
/// in-memory set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WatchNowPhase {
    /// Initial release-pick window. The watch-now operation is
    /// choosing a release, evaluating candidates, and starting the
    /// torrent. Exits to `PhaseTwo` on a successful pick + stream
    /// handoff, or to `Cancelled` on user cancel / terminal failure.
    PhaseOne,

    /// Background alternate-release loop. The initial pick is streaming;
    /// the loop continues to evaluate better releases and may swap if
    /// one promotes ahead. Exits to `Settled` when the loop's exit
    /// condition fires (max attempts, satisfaction threshold, etc) or
    /// to `Cancelled` on user cancel.
    PhaseTwo,

    /// Background loop finished without user cancel. Terminal — the
    /// row stays for history, but no background work touches it.
    Settled,

    /// User-initiated cancel. Terminal. The download row's own phase
    /// transitions independently via `DownloadPhase::Cancelled`.
    Cancelled,
}

impl WatchNowPhase {
    /// All variants in declaration order. Useful for SQL `IN (...)`
    /// clause construction via the predicates below.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::PhaseOne,
            Self::PhaseTwo,
            Self::Settled,
            Self::Cancelled,
        ]
        .into_iter()
    }

    /// Wire / SQL string. The DB column stores these literals.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PhaseOne => "phase_one",
            Self::PhaseTwo => "phase_two",
            Self::Settled => "settled",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse from the wire string. Returns `None` for unknown values
    /// so callers can decide whether to error or coerce.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "phase_one" => Some(Self::PhaseOne),
            "phase_two" => Some(Self::PhaseTwo),
            "settled" => Some(Self::Settled),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Should the background alternate-release loop tick this row?
    /// Only `PhaseTwo` qualifies — `PhaseOne` runs inside the
    /// foreground watch-now operation, terminal phases are done.
    #[must_use]
    pub const fn is_background_loop_active(self) -> bool {
        matches!(self, Self::PhaseTwo)
    }

    /// Is this row still subject to user cancel? Both active phases
    /// accept cancel; terminal phases ignore it.
    #[must_use]
    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::PhaseOne | Self::PhaseTwo)
    }

    /// Terminal — no further transitions. Caller may treat the row
    /// as historical.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Settled | Self::Cancelled)
    }

    /// SQL `IN (...)` clause built from the predicate. Inputs are
    /// static enum values, not user data — safe to interpolate.
    pub fn sql_in_clause(predicate: impl Fn(Self) -> bool) -> String {
        let clause = Self::all()
            .filter(|p| predicate(*p))
            .map(|p| format!("'{}'", p.as_str()))
            .collect::<Vec<_>>()
            .join(",");
        debug_assert!(
            !clause.is_empty(),
            "WatchNowPhase::sql_in_clause produced empty IN list — \
             interpolating it yields `wn_phase IN ()`, which silently \
             matches no rows. Caller passed a predicate matching \
             zero variants."
        );
        clause
    }
}

impl fmt::Display for WatchNowPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WatchNowPhase {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown WatchNowPhase value: {s:?}"))
    }
}

// ── sqlx ──────────────────────────────────────────────────────────

impl sqlx::Type<Sqlite> for WatchNowPhase {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for WatchNowPhase {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        let s = self.as_str().to_owned();
        <String as sqlx::Encode<'q, Sqlite>>::encode(s, buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for WatchNowPhase {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s: String = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Self::parse(&s).ok_or_else(|| format!("unknown WatchNowPhase from sqlx: {s:?}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_parse_roundtrip() {
        for phase in WatchNowPhase::all() {
            let s = phase.as_str();
            let back = WatchNowPhase::parse(s).expect("parses");
            assert_eq!(phase, back, "round-trip via {s:?}");
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(WatchNowPhase::parse("").is_none());
        assert!(WatchNowPhase::parse("not_a_phase").is_none());
        assert!(WatchNowPhase::parse("PhaseOne").is_none(), "case-sensitive");
    }

    #[test]
    fn serde_uses_snake_case_wire_format() {
        assert_eq!(
            serde_json::to_string(&WatchNowPhase::PhaseOne).unwrap(),
            "\"phase_one\""
        );
        assert_eq!(
            serde_json::to_string(&WatchNowPhase::PhaseTwo).unwrap(),
            "\"phase_two\""
        );
        let parsed: WatchNowPhase = serde_json::from_str("\"settled\"").unwrap();
        assert_eq!(parsed, WatchNowPhase::Settled);
    }

    #[test]
    fn fromstr_matches_parse() {
        for phase in WatchNowPhase::all() {
            let from_str: WatchNowPhase = phase.as_str().parse().unwrap();
            assert_eq!(from_str, phase);
        }
        assert!("garbage".parse::<WatchNowPhase>().is_err());
    }

    #[test]
    fn display_matches_wire_format() {
        for phase in WatchNowPhase::all() {
            assert_eq!(phase.to_string(), phase.as_str());
        }
    }

    /// Pin the background-loop set. Expanding it must be a deliberate
    /// update — letting the loop tick a terminal row would mean
    /// background work continues against a request the user cancelled.
    #[test]
    fn background_loop_set_is_pinned() {
        let active: Vec<_> = WatchNowPhase::all()
            .filter(|p| p.is_background_loop_active())
            .collect();
        assert_eq!(active, vec![WatchNowPhase::PhaseTwo]);
    }

    #[test]
    fn cancellable_set_is_pinned() {
        let c: Vec<_> = WatchNowPhase::all()
            .filter(|p| p.is_cancellable())
            .collect();
        assert_eq!(c, vec![WatchNowPhase::PhaseOne, WatchNowPhase::PhaseTwo]);
    }

    #[test]
    fn terminal_set_is_pinned() {
        let t: Vec<_> = WatchNowPhase::all().filter(|p| p.is_terminal()).collect();
        assert_eq!(t, vec![WatchNowPhase::Settled, WatchNowPhase::Cancelled]);
    }

    /// Terminal and background-loop-active are disjoint by construction.
    /// If they ever overlap, the loop would tick a row the cleanup
    /// paths consider gone (re-introducing the watch-now cancel race).
    #[test]
    fn terminal_and_background_loop_are_disjoint() {
        for phase in WatchNowPhase::all() {
            assert!(
                !(phase.is_terminal() && phase.is_background_loop_active()),
                "{phase:?} is both terminal and background-loop-active"
            );
        }
    }

    /// Cancellable and terminal are disjoint by construction. A
    /// terminal row can't be cancelled — cancel from terminal must
    /// be a no-op at the operation layer, but the predicate must
    /// reject it before that.
    #[test]
    fn terminal_and_cancellable_are_disjoint() {
        for phase in WatchNowPhase::all() {
            assert!(
                !(phase.is_terminal() && phase.is_cancellable()),
                "{phase:?} is both terminal and cancellable"
            );
        }
    }

    #[test]
    fn sql_in_clause_renders_quoted_csv() {
        let clause = WatchNowPhase::sql_in_clause(WatchNowPhase::is_terminal);
        assert_eq!(clause, "'settled','cancelled'");
    }

    #[test]
    #[should_panic(expected = "matching zero variants")]
    fn sql_in_clause_panics_on_empty_predicate() {
        let _ = WatchNowPhase::sql_in_clause(|_| false);
    }
}
