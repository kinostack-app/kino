//! Acquisition — search → policy → grab. The single seam every release
//! lookup, indexer query, wanted-sweep, upgrade-sweep, and manual grab
//! flows through.
//!
//! `policy::AcquisitionPolicy::evaluate` is the single decision
//! function that every grab passes through. `release_target` defines
//! the polymorphic surface (movie + episode) that `policy` operates
//! against. `search` runs the indexer queries and routes hits through
//! the policy gate before grabbing.
//!
//! ## Public API
//!
//! Cross-domain consumers reach acquisition through the re-exports
//! below, never via `acquisition::policy::*` etc:
//! - `AcquisitionPolicy` + `Decision` / `RejectReason` — the gate
//! - `PolicyContext`, `ReleaseCandidate`, `ExistingPick` — its inputs
//! - `ReleaseTarget` trait + `ReleaseTargetKind` — the polymorphic
//!   surface (impls in `content/movie/model.rs`,
//!   `content/show/episode.rs`)
//! - `BlocklistEntry` — the blocklist match shape policy consults
//!
//! The handlers in `release` (HTTP CRUD), `blocklist` (HTTP CRUD +
//! blocklist-on-failure), and `grab` (post-policy side-effects) are
//! consumed by main.rs's router and the watch-now flow only.
//! `search/*` is consumed by the scheduler's wanted-sweep, the
//! events listener, and watch-now.

pub mod blocklist;
pub mod grab;
pub mod policy;
pub mod release;
pub mod release_target;
pub mod search;

pub use policy::{
    AcquisitionPolicy, Decision, ExistingPick, PolicyContext, RejectReason, ReleaseCandidate,
};
pub use release_target::{BlocklistEntry, ReleaseTarget, ReleaseTargetKind};
