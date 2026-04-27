//! Backup & restore (subsystem 19).
//!
//! Snapshots the kino DB into a `tar.gz` under
//! `{config.backup_location_path}/`. Three trigger paths:
//!
//! - **Manual** — user clicks "Create backup now" in Settings.
//! - **Scheduled** — daily / weekly / monthly per
//!   `config.backup_schedule`.
//! - **Pre-restore** — auto-fired before any restore so the user
//!   can always undo.
//!
//! ## Public API
//!
//! - [`archive::create`] — write a new archive + insert a `backup`
//!   row. Used by handlers + the scheduler task + the restore
//!   pre-snapshot.
//! - [`archive::stage_restore`] — validate an archive + replace the
//!   live DB on disk. Returns when the swap is committed; the
//!   caller is responsible for surfacing "please restart kino" to
//!   the user (we don't auto-restart in Phase 1).
//! - [`schedule::next_due`] — pure helper used by the scheduler to
//!   decide if it's time to fire.
//! - [`handlers`] — HTTP routes for `/api/v1/backups/*`.
//! - [`model::Backup`] — DB row + API response shape.
//!
//! Restore is a Phase-1 manual flow: we stage the new files in
//! place, log the change, and ask the user to restart. Auto-
//! restart inside the running process means re-initialising
//! `AppState` (DB pool, scheduler, librqbit), which is invasive
//! and best deferred until Phase 2.

pub mod archive;
pub mod handlers;
pub mod model;
pub mod schedule;

pub use model::{Backup, BackupKind};
