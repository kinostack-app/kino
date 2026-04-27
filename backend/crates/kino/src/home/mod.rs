//! Home — Up Next list + per-user preferences for the home page
//! layout. Read-side: assembles a Continue Watching feed by querying
//! across `media`, `episode`, `playback_progress`. Write-side: the
//! preferences endpoint persists per-user layout choices (sort
//! order, hidden sections).
//!
//! ## Public API
//!
//! `handlers::*` and `preferences::*` are HTTP route targets,
//! registered via main.rs. Nothing else in this domain is consumed
//! externally — Continue Watching's SQL stays here on purpose so
//! library/handlers (the search side) doesn't grow a parallel
//! implementation.

pub mod handlers;
pub mod preferences;
