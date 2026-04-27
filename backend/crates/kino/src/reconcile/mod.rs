//! Continuous reconciliation framework.
//!
//! A reconciliation step compares some piece of kino state against
//! an expected condition. It either auto-repairs the drift
//! (whitelisted, idempotent steps) or surfaces it for admin
//! attention. The scheduler ticks [`run_continuous`] on a fixed
//! cadence so drift is caught within minutes rather than at the
//! next user complaint.
//!
//! ## Step policies
//!
//! Each step is classified at compile time as
//! [`StepRepairPolicy::AutoRepair`] or
//! [`StepRepairPolicy::SurfaceOnly`]:
//!
//! - **`AutoRepair`** — the step's repair action is idempotent and
//!   safe to run unattended. Hash normalisation, orphan-row deletion,
//!   stuck-claim resets all qualify.
//! - **`SurfaceOnly`** — the step detects drift but never modifies
//!   state. Operator confirmation required. Used when the corrective
//!   action could destroy user data (deleting a media row whose file
//!   "appears missing" when in fact a mount flickered).
//!
//! The classification is part of the step's identity — flipping a
//! step from `SurfaceOnly` to `AutoRepair` is a deliberate code change
//! visible in review.
//!
//! ## Adding a step
//!
//! 1. Add a variant to [`ReconcileStep`].
//! 2. Wire `name` and `policy` arms.
//! 3. Add a match arm in [`run_continuous`] that does the work and
//!    pushes a [`StepReport`] (`drift_found` / repaired / surfaced).
//!    Steps that produce structured output for downstream surfaces
//!    (admin UI, /status) attach it to [`ReconcileReport`] directly,
//!    as the invariants step does with `invariant_violations`.

use sqlx::SqlitePool;

use crate::invariants::{self, Violation, log_violations};

/// Built-in reconciliation steps. Closed set; adding a step is a
/// compiler-enforced match-arm change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReconcileStep {
    /// Run the invariant suite and surface violations.
    Invariants,
}

/// What action the step takes when drift is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepRepairPolicy {
    /// Step's repair action is idempotent and safe to run
    /// unattended.
    AutoRepair,
    /// Step detects drift but never modifies state. Surfaces to
    /// admin / log only.
    SurfaceOnly,
}

impl ReconcileStep {
    /// All variants in declaration order. The continuous loop runs
    /// them in this order.
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Invariants].into_iter()
    }

    /// Stable name for logs and event payloads.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Invariants => "invariants",
        }
    }

    /// Repair classification. See [`StepRepairPolicy`] for semantics.
    #[must_use]
    pub const fn policy(self) -> StepRepairPolicy {
        match self {
            Self::Invariants => StepRepairPolicy::SurfaceOnly,
        }
    }
}

/// Result of running one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub step: &'static str,
    pub policy: StepRepairPolicy,
    /// Total drift items the step found this tick.
    pub drift_found: u64,
    /// Of those, how many were repaired in place. Always `0` for
    /// `SurfaceOnly` steps.
    pub repaired: u64,
    /// Of those, how many were surfaced (logged, evented) without
    /// modification. Equals `drift_found` for `SurfaceOnly` steps.
    pub surfaced: u64,
}

impl StepReport {
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.drift_found == 0
    }
}

/// Aggregate result of one [`run_continuous`] tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub steps: Vec<StepReport>,
    /// Violations from the invariant step, retained for surfaces
    /// (admin UI, /status warnings) that need to render the
    /// individual offending rows. `log_violations` is the per-tick
    /// emit path; this is the snapshot.
    pub invariant_violations: Vec<Violation>,
}

impl ReconcileReport {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.steps.iter().all(StepReport::ok)
    }

    #[must_use]
    pub fn total_drift(&self) -> u64 {
        self.steps.iter().map(|s| s.drift_found).sum()
    }

    #[must_use]
    pub fn total_repaired(&self) -> u64 {
        self.steps.iter().map(|s| s.repaired).sum()
    }

    #[must_use]
    pub fn total_surfaced(&self) -> u64 {
        self.steps.iter().map(|s| s.surfaced).sum()
    }
}

/// Run every [`ReconcileStep`] in declaration order. Each step's
/// side effects (logs, events, repairs) fire as it runs; the
/// returned report aggregates the counts plus, for the invariants
/// step, the actual violations for downstream surfacing.
pub async fn run_continuous(pool: &SqlitePool) -> sqlx::Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    for step in ReconcileStep::all() {
        match step {
            ReconcileStep::Invariants => {
                let inv_report = invariants::check_all(pool).await?;
                let drift = inv_report.violations.len() as u64;
                log_violations(&inv_report.violations);
                report.steps.push(StepReport {
                    step: step.name(),
                    policy: step.policy(),
                    drift_found: drift,
                    repaired: 0,
                    surfaced: drift,
                });
                report.invariant_violations = inv_report.violations;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn fresh_pool() -> SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .expect("seed defaults");
        pool
    }

    #[test]
    fn step_set_pinned_in_declaration_order() {
        let names: Vec<_> = ReconcileStep::all().map(ReconcileStep::name).collect();
        assert_eq!(names, vec!["invariants"]);
    }

    #[test]
    fn invariants_step_is_surface_only() {
        assert_eq!(
            ReconcileStep::Invariants.policy(),
            StepRepairPolicy::SurfaceOnly
        );
    }

    #[tokio::test]
    async fn run_continuous_on_fresh_db_reports_ok() {
        let pool = fresh_pool().await;
        let report = run_continuous(&pool).await.unwrap();
        assert!(report.ok(), "fresh DB drift: {report:?}");
        assert_eq!(report.total_drift(), 0);
        assert_eq!(report.total_repaired(), 0);
        assert_eq!(report.total_surfaced(), 0);
    }

    #[tokio::test]
    async fn run_continuous_surfaces_planted_invariant_violation() {
        let pool = fresh_pool().await;
        // Plant a show with no series — trips `show_has_seasons`.
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, monitor_new_items, added_at)
             VALUES (?, 'Orphan', 1, 1, 'future', ?)",
        )
        .bind(rand::random::<i32>())
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let report = run_continuous(&pool).await.unwrap();
        assert!(!report.ok());
        assert_eq!(report.total_drift(), 1);
        assert_eq!(report.total_surfaced(), 1);
        assert_eq!(report.total_repaired(), 0);
        let invariants_step = &report.steps[0];
        assert_eq!(invariants_step.step, "invariants");
        assert_eq!(invariants_step.policy, StepRepairPolicy::SurfaceOnly);
        // Violations are attached to the report so downstream surfaces
        // (status warnings, admin UI) can render the per-row detail.
        assert_eq!(report.invariant_violations.len(), 1);
        assert_eq!(report.invariant_violations[0].invariant, "show_has_seasons");
    }

    #[tokio::test]
    async fn surface_only_step_never_repairs() {
        // Plant the violation and confirm the row is still there
        // after the reconcile run — SurfaceOnly must not delete it.
        let pool = fresh_pool().await;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, monitor_new_items, added_at)
             VALUES (?, 'Orphan', 1, 1, 'future', ?)",
        )
        .bind(rand::random::<i32>())
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();
        let _ = run_continuous(&pool).await.unwrap();
        let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM show")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(still_there, 1, "SurfaceOnly step modified state");
    }
}
