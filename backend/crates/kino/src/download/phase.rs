//! `DownloadPhase` — typed lifecycle state for the `download` table.
//!
//! Every classification (monitored? streamable? terminal?) is a
//! method on the enum so consumers ask the type rather than writing
//! their own `matches!` chains. Adding a state forces the compiler
//! to surface every exhaustive-match site.
//!
//! Variants serialise as `snake_case` matching the `download.state`
//! TEXT column one-for-one (`Searching` ↔ `"searching"`,
//! `CleanedUp` ↔ `"cleaned_up"`). SQL literals (`state = 'queued'`)
//! and typed binds (`.bind(DownloadPhase::Queued)`) both work.
//!
//! ## Classification methods
//!
//! - [`is_runtime_monitored`](DownloadPhase::is_runtime_monitored)
//!   — the per-tick `download_monitor` polls these.
//! - [`needs_startup_reconcile`](DownloadPhase::needs_startup_reconcile)
//!   — startup re-anchors these against librqbit.
//! - [`is_streamable`](DownloadPhase::is_streamable) — a `<video>`
//!   source can be built from these rows.
//! - [`needs_seed_limit_check`](DownloadPhase::needs_seed_limit_check)
//!   — ratio/time enforcement applies.
//! - [`is_terminal`](DownloadPhase::is_terminal) — no further
//!   transitions; row is safe to delete.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sqlx::Sqlite;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use utoipa::ToSchema;

/// One row's lifecycle position. See the module docs for the full
/// state graph and the classification rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPhase {
    /// Wanted; release pickup hasn't started or is in flight. The
    /// release-search subsystem owns transitions out of this state.
    Searching,

    /// Release picked + persisted; awaiting a librqbit slot. The
    /// download monitor's concurrency cap gates promotion into
    /// `Grabbing`.
    Queued,

    /// Currently being added to librqbit (network call in flight).
    Grabbing,

    /// librqbit reports active download.
    Downloading,

    /// No progress for `stall_timeout` seconds. Still tracked; the
    /// monitor may transition back to `Downloading` if peers
    /// reappear.
    Stalled,

    /// User-initiated pause. Persisted across restart.
    Paused,

    /// librqbit reports complete; awaiting the import handoff.
    /// Distinct from `Imported` so a crash in the import window is
    /// recoverable on startup.
    Completed,

    /// Import is in flight. Distinct from `Completed` so reconciliation
    /// can spot stuck imports.
    Importing,

    /// Import succeeded; media row exists. Torrent may still be
    /// seeding — see `Seeding` for the post-import seed-limit phase.
    Imported,

    /// Post-import seeding phase, distinct from `Imported` so the
    /// seed-limit cleanup can find rows by phase rather than by
    /// joining timestamp arithmetic.
    Seeding,

    /// Torrent removed and source files cleaned. Terminal.
    CleanedUp,

    /// Terminal failure; `error_message` set. Retry creates a fresh
    /// row.
    Failed,

    /// User-initiated cancel. Terminal.
    Cancelled,
}

impl DownloadPhase {
    /// All variants in declaration order. Useful for SQL `IN (...)`
    /// clause construction via the predicates below.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::Searching,
            Self::Queued,
            Self::Grabbing,
            Self::Downloading,
            Self::Stalled,
            Self::Paused,
            Self::Completed,
            Self::Importing,
            Self::Imported,
            Self::Seeding,
            Self::CleanedUp,
            Self::Failed,
            Self::Cancelled,
        ]
        .into_iter()
    }

    /// Wire / SQL string. Matches the existing `download.state`
    /// TEXT values one-for-one, so SQL literals like
    /// `state = 'queued'` keep working unchanged.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Searching => "searching",
            Self::Queued => "queued",
            Self::Grabbing => "grabbing",
            Self::Downloading => "downloading",
            Self::Stalled => "stalled",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Importing => "importing",
            Self::Imported => "imported",
            Self::Seeding => "seeding",
            Self::CleanedUp => "cleaned_up",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse from the wire string. Returns `None` for unknown values
    /// so callers can decide whether to error or coerce. The sqlx
    /// `Decode` impl bubbles unknowns as decode errors; ad-hoc parse
    /// sites usually want the explicit Option.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "searching" => Some(Self::Searching),
            "queued" => Some(Self::Queued),
            "grabbing" => Some(Self::Grabbing),
            "downloading" => Some(Self::Downloading),
            "stalled" => Some(Self::Stalled),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "importing" => Some(Self::Importing),
            "imported" => Some(Self::Imported),
            "seeding" => Some(Self::Seeding),
            "cleaned_up" => Some(Self::CleanedUp),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Should the per-tick `download_monitor` poll this row?
    /// Includes every state where librqbit might still need
    /// attention OR where an import handoff is pending.
    #[must_use]
    pub const fn is_runtime_monitored(self) -> bool {
        matches!(
            self,
            Self::Searching
                | Self::Queued
                | Self::Grabbing
                | Self::Downloading
                | Self::Stalled
                | Self::Completed
                | Self::Importing
                | Self::Imported
                | Self::Seeding
        )
    }

    /// Should startup reconciliation re-anchor this row against
    /// librqbit's session? Includes `Paused` so a paused row whose
    /// torrent vanished from librqbit is caught and surfaced.
    #[must_use]
    pub const fn needs_startup_reconcile(self) -> bool {
        matches!(
            self,
            Self::Grabbing
                | Self::Downloading
                | Self::Stalled
                | Self::Paused
                | Self::Completed
                | Self::Importing
                | Self::Imported
                | Self::Seeding
        )
    }

    /// Can a `<video>` byte source be served from this row? Both
    /// pre-import (live torrent stream) and post-import (library
    /// file) phases are streamable; terminal phases are not.
    #[must_use]
    pub const fn is_streamable(self) -> bool {
        matches!(
            self,
            Self::Downloading
                | Self::Stalled
                | Self::Completed
                | Self::Importing
                | Self::Imported
                | Self::Seeding
        )
    }

    /// Subject to seed-ratio / seed-time enforcement? Both `Imported`
    /// and the explicit post-import `Seeding` phase qualify — until
    /// the user disables seeding or the limits fire, the torrent is
    /// still active.
    #[must_use]
    pub const fn needs_seed_limit_check(self) -> bool {
        matches!(self, Self::Imported | Self::Seeding)
    }

    /// Holds a librqbit slot — the concurrency cap counts these.
    /// Pre-grab phases (`Searching`, `Queued`) don't yet have a hash;
    /// post-import phases (`Imported`, `Seeding`) seed but don't
    /// count against the cap.
    #[must_use]
    pub const fn consumes_torrent_slot(self) -> bool {
        matches!(self, Self::Grabbing | Self::Downloading | Self::Stalled)
    }

    /// Magnet has resolved into an info-dict — file metadata + total
    /// size are known. Used by the download-monitor metadata-ready
    /// transition that lets the UI drop its initial poll.
    #[must_use]
    pub const fn is_metadata_resolved(self) -> bool {
        matches!(
            self,
            Self::Downloading | Self::Stalled | Self::Completed | Self::Seeding
        )
    }

    /// Pre-import active acquisition — a downloads's lifecycle is in
    /// progress and no media row exists yet. Drives the read-side
    /// `status = 'downloading'` derivation in `content::derived_state` and
    /// the "is anything happening for this content?" filter in
    /// `library::handlers`. Excludes `Completed` deliberately: that's a
    /// brief handoff window between librqbit-complete and the import
    /// commit; the existing classification keeps the pre-`Completed`
    /// shape for consistency with the read-side derivation.
    #[must_use]
    pub const fn is_pre_import_active(self) -> bool {
        matches!(
            self,
            Self::Searching
                | Self::Queued
                | Self::Grabbing
                | Self::Downloading
                | Self::Paused
                | Self::Stalled
                | Self::Importing
        )
    }

    /// Terminal — no further transitions. Caller may delete the row
    /// or treat it as historical.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::CleanedUp | Self::Failed | Self::Cancelled)
    }

    /// SQL `IN (...)` clause built from the predicate. Inputs are
    /// static enum values, not user data — safe to interpolate.
    ///
    /// ```ignore
    /// let sql = format!(
    ///     "SELECT * FROM download WHERE state IN ({})",
    ///     DownloadPhase::sql_in_clause(DownloadPhase::is_runtime_monitored)
    /// );
    /// ```
    pub fn sql_in_clause(predicate: impl Fn(Self) -> bool) -> String {
        let clause = Self::all()
            .filter(|p| predicate(*p))
            .map(|p| format!("'{}'", p.as_str()))
            .collect::<Vec<_>>()
            .join(",");
        debug_assert!(
            !clause.is_empty(),
            "DownloadPhase::sql_in_clause produced empty IN list — \
             interpolating it yields `state IN ()`, which silently \
             matches no rows. Caller passed a predicate matching \
             zero variants."
        );
        clause
    }
}

impl fmt::Display for DownloadPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DownloadPhase {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown DownloadPhase value: {s:?}"))
    }
}

// ── sqlx ──────────────────────────────────────────────────────────

impl sqlx::Type<Sqlite> for DownloadPhase {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for DownloadPhase {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        // `as_str()` returns &'static str; bind as owned String so
        // sqlx's argument lifetime requirements are satisfied.
        let s = self.as_str().to_owned();
        <String as sqlx::Encode<'q, Sqlite>>::encode(s, buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for DownloadPhase {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s: String = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Self::parse(&s).ok_or_else(|| format!("unknown DownloadPhase from sqlx: {s:?}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip every variant through `as_str` / `parse`. Catches
    /// any drift between the wire string and the parser, which would
    /// otherwise manifest as silent data loss.
    #[test]
    fn as_str_parse_roundtrip() {
        for phase in DownloadPhase::all() {
            let s = phase.as_str();
            let back = DownloadPhase::parse(s).expect("parses");
            assert_eq!(phase, back, "round-trip via {s:?}");
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(DownloadPhase::parse("").is_none());
        assert!(DownloadPhase::parse("not_a_phase").is_none());
        assert!(DownloadPhase::parse("Queued").is_none(), "case-sensitive");
    }

    #[test]
    fn serde_uses_snake_case_wire_format() {
        // The wire format MUST match the existing column values, or
        // every SQL literal `state = 'cleaned_up'` breaks.
        assert_eq!(
            serde_json::to_string(&DownloadPhase::CleanedUp).unwrap(),
            "\"cleaned_up\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadPhase::Searching).unwrap(),
            "\"searching\""
        );
        let parsed: DownloadPhase = serde_json::from_str("\"imported\"").unwrap();
        assert_eq!(parsed, DownloadPhase::Imported);
    }

    #[test]
    fn fromstr_matches_parse() {
        for phase in DownloadPhase::all() {
            let from_str: DownloadPhase = phase.as_str().parse().unwrap();
            assert_eq!(from_str, phase);
        }
        assert!("garbage".parse::<DownloadPhase>().is_err());
    }

    #[test]
    fn display_matches_wire_format() {
        for phase in DownloadPhase::all() {
            assert_eq!(phase.to_string(), phase.as_str());
        }
    }

    /// Pin the `is_runtime_monitored` set. Changing it must be a
    /// deliberate update — this test failing is the signal.
    #[test]
    fn runtime_monitored_set_is_pinned() {
        let monitored: Vec<_> = DownloadPhase::all()
            .filter(|p| p.is_runtime_monitored())
            .collect();
        assert_eq!(
            monitored,
            vec![
                DownloadPhase::Searching,
                DownloadPhase::Queued,
                DownloadPhase::Grabbing,
                DownloadPhase::Downloading,
                DownloadPhase::Stalled,
                DownloadPhase::Completed,
                DownloadPhase::Importing,
                DownloadPhase::Imported,
                DownloadPhase::Seeding,
            ]
        );
    }

    #[test]
    fn startup_reconcile_set_is_pinned() {
        let recon: Vec<_> = DownloadPhase::all()
            .filter(|p| p.needs_startup_reconcile())
            .collect();
        assert_eq!(
            recon,
            vec![
                DownloadPhase::Grabbing,
                DownloadPhase::Downloading,
                DownloadPhase::Stalled,
                DownloadPhase::Paused,
                DownloadPhase::Completed,
                DownloadPhase::Importing,
                DownloadPhase::Imported,
                DownloadPhase::Seeding,
            ]
        );
    }

    #[test]
    fn streamable_set_is_pinned() {
        let streamable: Vec<_> = DownloadPhase::all().filter(|p| p.is_streamable()).collect();
        assert_eq!(
            streamable,
            vec![
                DownloadPhase::Downloading,
                DownloadPhase::Stalled,
                DownloadPhase::Completed,
                DownloadPhase::Importing,
                DownloadPhase::Imported,
                DownloadPhase::Seeding,
            ]
        );
    }

    #[test]
    fn seed_limit_set_is_pinned() {
        let s: Vec<_> = DownloadPhase::all()
            .filter(|p| p.needs_seed_limit_check())
            .collect();
        assert_eq!(s, vec![DownloadPhase::Imported, DownloadPhase::Seeding]);
    }

    #[test]
    fn terminal_set_is_pinned() {
        let t: Vec<_> = DownloadPhase::all().filter(|p| p.is_terminal()).collect();
        assert_eq!(
            t,
            vec![
                DownloadPhase::CleanedUp,
                DownloadPhase::Failed,
                DownloadPhase::Cancelled,
            ]
        );
    }

    /// Terminal and runtime-monitored are disjoint by construction.
    /// If they ever overlap, the monitor would re-poll a row that
    /// the cleanup paths consider gone. Property test against the
    /// full enum.
    #[test]
    fn terminal_and_monitored_are_disjoint() {
        for phase in DownloadPhase::all() {
            assert!(
                !(phase.is_terminal() && phase.is_runtime_monitored()),
                "{phase:?} is both terminal and runtime-monitored"
            );
        }
    }

    /// Streamable rows are a subset of runtime-monitored rows. If a
    /// row is streamable but not monitored, the byte source goes
    /// stale because the monitor doesn't refresh it.
    #[test]
    fn streamable_implies_runtime_monitored() {
        for phase in DownloadPhase::all() {
            if phase.is_streamable() {
                assert!(
                    phase.is_runtime_monitored(),
                    "{phase:?} is streamable but not runtime-monitored"
                );
            }
        }
    }

    #[test]
    fn sql_in_clause_renders_quoted_csv() {
        let clause = DownloadPhase::sql_in_clause(DownloadPhase::is_terminal);
        assert_eq!(clause, "'cleaned_up','failed','cancelled'");
    }

    /// An empty IN list silently matches no rows; the `debug_assert`
    /// catches that footgun in test/dev builds.
    #[test]
    #[should_panic(expected = "matching zero variants")]
    fn sql_in_clause_panics_on_empty_predicate() {
        let _ = DownloadPhase::sql_in_clause(|_| false);
    }

    #[test]
    fn consumes_torrent_slot_set_is_pinned() {
        let s: Vec<_> = DownloadPhase::all()
            .filter(|p| p.consumes_torrent_slot())
            .collect();
        assert_eq!(
            s,
            vec![
                DownloadPhase::Grabbing,
                DownloadPhase::Downloading,
                DownloadPhase::Stalled,
            ]
        );
    }

    #[test]
    fn is_metadata_resolved_set_is_pinned() {
        let s: Vec<_> = DownloadPhase::all()
            .filter(|p| p.is_metadata_resolved())
            .collect();
        assert_eq!(
            s,
            vec![
                DownloadPhase::Downloading,
                DownloadPhase::Stalled,
                DownloadPhase::Completed,
                DownloadPhase::Seeding,
            ]
        );
    }

    #[test]
    fn is_pre_import_active_set_is_pinned() {
        let s: Vec<_> = DownloadPhase::all()
            .filter(|p| p.is_pre_import_active())
            .collect();
        assert_eq!(
            s,
            vec![
                DownloadPhase::Searching,
                DownloadPhase::Queued,
                DownloadPhase::Grabbing,
                DownloadPhase::Downloading,
                DownloadPhase::Stalled,
                DownloadPhase::Paused,
                DownloadPhase::Importing,
            ]
        );
    }

    #[test]
    fn consumes_torrent_slot_implies_runtime_monitored() {
        for phase in DownloadPhase::all() {
            if phase.consumes_torrent_slot() {
                assert!(
                    phase.is_runtime_monitored(),
                    "{phase:?} consumes a torrent slot but isn't runtime-monitored"
                );
            }
        }
    }

    #[test]
    fn pre_import_active_and_terminal_are_disjoint() {
        for phase in DownloadPhase::all() {
            assert!(
                !(phase.is_pre_import_active() && phase.is_terminal()),
                "{phase:?} is both pre-import-active and terminal"
            );
        }
    }
}
