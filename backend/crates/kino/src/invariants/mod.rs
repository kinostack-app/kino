//! Invariants — predicates over kino's state that must always hold.
//!
//! Each invariant is a small async function that returns zero or
//! more [`Violation`]s; an empty `Vec` means the predicate holds.
//! [`StandardInvariant`] enumerates the built-in suite;
//! [`check_all`] runs every variant in declaration order and returns
//! an [`InvariantReport`].
//!
//! ## Where they run
//!
//! - **Tests.** Flow tests assert the suite passes after the
//!   scenario completes; new code that violates an invariant
//!   fails CI.
//! - **Continuous reconciliation.** Scheduler ticks `check_all`
//!   on a fixed cadence and routes violations to the health surface
//!   (and, for whitelisted ones, to the auto-repair path).
//!
//! ## Adding an invariant
//!
//! 1. Add a submodule under `invariants/` exposing
//!    `pub const NAME: &str`, `pub const DESCRIPTION: &str`, and
//!    `pub async fn check(pool) -> sqlx::Result<Vec<Violation>>`.
//! 2. Add a variant to [`StandardInvariant`].
//! 3. Wire `name`, `description`, and `check` arms.
//! 4. Add a fixture-based test (pass + fail) in the submodule.

use sqlx::SqlitePool;

pub mod active_download_has_torrent;
pub mod blocklist_hashes_normalized;
pub mod imported_has_media;
pub mod media_has_owner;
pub mod show_has_seasons;
pub mod stuck_partial_follow;

/// One violated assertion. `invariant` matches the producing
/// invariant's `NAME`; `detail` is a human-readable description
/// including the offending row id(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub invariant: &'static str,
    pub detail: String,
}

/// Built-in invariants. Closed set; adding one requires a new
/// variant + match arms (which the compiler enforces).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StandardInvariant {
    ImportedHasMedia,
    ActiveDownloadHasTorrent,
    BlocklistHashesNormalized,
    ShowHasSeasons,
    MediaHasOwner,
    StuckPartialFollow,
}

impl StandardInvariant {
    /// All variants in declaration order. The check loop runs them
    /// in this order; tests pin the order.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::ImportedHasMedia,
            Self::ActiveDownloadHasTorrent,
            Self::BlocklistHashesNormalized,
            Self::ShowHasSeasons,
            Self::MediaHasOwner,
            Self::StuckPartialFollow,
        ]
        .into_iter()
    }

    /// Stable name (`snake_case`). Used in logs, events, and the
    /// `Violation::invariant` field.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ImportedHasMedia => imported_has_media::NAME,
            Self::ActiveDownloadHasTorrent => active_download_has_torrent::NAME,
            Self::BlocklistHashesNormalized => blocklist_hashes_normalized::NAME,
            Self::ShowHasSeasons => show_has_seasons::NAME,
            Self::MediaHasOwner => media_has_owner::NAME,
            Self::StuckPartialFollow => stuck_partial_follow::NAME,
        }
    }

    /// One-line human-readable description for admin UI.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::ImportedHasMedia => imported_has_media::DESCRIPTION,
            Self::ActiveDownloadHasTorrent => active_download_has_torrent::DESCRIPTION,
            Self::BlocklistHashesNormalized => blocklist_hashes_normalized::DESCRIPTION,
            Self::ShowHasSeasons => show_has_seasons::DESCRIPTION,
            Self::MediaHasOwner => media_has_owner::DESCRIPTION,
            Self::StuckPartialFollow => stuck_partial_follow::DESCRIPTION,
        }
    }

    /// Run the invariant against the live DB. Returns zero or more
    /// violations.
    pub async fn check(self, pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
        match self {
            Self::ImportedHasMedia => imported_has_media::check(pool).await,
            Self::ActiveDownloadHasTorrent => active_download_has_torrent::check(pool).await,
            Self::BlocklistHashesNormalized => blocklist_hashes_normalized::check(pool).await,
            Self::ShowHasSeasons => show_has_seasons::check(pool).await,
            Self::MediaHasOwner => media_has_owner::check(pool).await,
            Self::StuckPartialFollow => stuck_partial_follow::check(pool).await,
        }
    }
}

/// Aggregate result of one [`check_all`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvariantReport {
    /// Number of invariants that produced zero violations.
    pub passed: u64,
    /// All violations from all invariants, in declaration order.
    pub violations: Vec<Violation>,
}

impl InvariantReport {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Run every [`StandardInvariant`] against `pool` and aggregate.
pub async fn check_all(pool: &SqlitePool) -> sqlx::Result<InvariantReport> {
    let mut report = InvariantReport::default();
    for invariant in StandardInvariant::all() {
        let violations = invariant.check(pool).await?;
        if violations.is_empty() {
            report.passed += 1;
        }
        report.violations.extend(violations);
    }
    Ok(report)
}

/// Emit a `tracing::warn!` per violation. Standard reporting hook;
/// callers that want structured emit (events, webhooks) can build
/// on top of [`InvariantReport`] directly.
pub fn log_violations(violations: &[Violation]) {
    for v in violations {
        tracing::warn!(
            invariant = v.invariant,
            detail = %v.detail,
            "invariant violation"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    pub(super) async fn fresh_pool() -> SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .expect("seed defaults");
        pool
    }

    #[test]
    fn all_pinned_in_declaration_order() {
        let names: Vec<_> = StandardInvariant::all()
            .map(StandardInvariant::name)
            .collect();
        assert_eq!(
            names,
            vec![
                "imported_has_media",
                "active_download_has_torrent",
                "blocklist_hashes_normalized",
                "show_has_seasons",
                "media_has_owner",
                "stuck_partial_follow",
            ]
        );
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = StandardInvariant::all()
            .map(StandardInvariant::name)
            .collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate invariant name");
    }

    #[tokio::test]
    async fn check_all_on_fresh_db_passes_every_invariant() {
        let pool = fresh_pool().await;
        let report = check_all(&pool).await.unwrap();
        assert!(report.ok(), "fresh DB violations: {:?}", report.violations);
        assert_eq!(report.passed, 6);
    }
}
