//! Watch-now — the "click Play, get pixels" orchestrator. Bridges
//! acquisition (find-and-grab a release) with playback (start
//! streaming bytes) in a single user gesture. The two-phase
//! pattern: phase 1 returns immediately with a placeholder
//! `download_id` so the player can navigate; phase 2 runs search +
//! grab in the background and the player polls download state
//! until bytes arrive.
//!
//! ## Public API
//!
//! - `WatchNowPhase` — typed state for the `download.wn_phase`
//!   column. Acquisition's grab + the download monitor + the
//!   prepare endpoint all branch on it.
//! - `handlers::watch_now` — the single endpoint, registered via
//!   main.rs. The phase-2 background machinery + the
//!   `find_or_create_episode` helper (used by acquire-by-tmdb) are
//!   `pub(crate)` only because `content/show/episode_handlers` calls
//!   the helper directly to avoid duplicating its auto-follow
//!   logic.

pub mod handlers;
pub mod phase;

pub use phase::WatchNowPhase;
