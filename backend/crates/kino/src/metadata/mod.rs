//! Metadata enrichment domain — TMDB-driven refreshes, fanart logo
//! resolution, and the per-entity metadata-quality tiering that feeds
//! `metadata_status` on movies and shows.
//!
//! Owns the background sweep that re-fetches stale metadata, the
//! per-image refresh path the UI calls when the user requests a new
//! poster/backdrop/logo, and the logo selection heuristics that pick
//! the cleanest variant per-entity.
//!
//! ## Public API
//!
//! - `refresh::refresh_sweep` — scheduler entry; iterates stale rows
//!   per the tiered freshness ladder and refetches via TMDB
//! - `logos::{ContentType, refresh_entity_logo}` — on-demand logo
//!   refresh, called from `image_handlers` and from the prepare flow
//! - `tmdb_handlers`, `image_handlers`, `test_handlers` — HTTP
//!   surface (search / details / genres / image serve / connectivity
//!   probe), registered via main.rs
//!
//! The TMDB client itself lives at the crate root (`crate::tmdb`)
//! because it predates this domain — moving it here is on the
//! follow-up list per `architecture/crate-layout.md`.

pub mod image_handlers;
pub mod logos;
pub mod refresh;
pub mod test_handlers;
pub mod tmdb_handlers;
