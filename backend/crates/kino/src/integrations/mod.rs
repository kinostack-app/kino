//! Third-party integrations kino speaks to. Each integration sits in
//! its own submodule and exposes a trait-free public API (functions +
//! types) rather than a cross-cutting abstraction — we only have a
//! handful of these and premature abstraction would obscure what each
//! one's actually doing.
//!
//! ## Public API
//!
//! Each integration's submodule documents its own surface:
//! - `lists` — `MDBList` / TMDB / Trakt list import + sync. Read by
//!   the scheduler's list-refresh task and by the lists handlers.
//! - `opensubtitles` — used by import to fetch subtitles per the
//!   user's accepted-languages profile.
//! - `trakt` — OAuth + scrobble + bulk/incremental sync + push
//!   collection. Consumed by content/show + content/movie watched
//!   handlers, by import for `Imported` push, by the events
//!   listener for `Watched`/`Unwatched`/`Rated` push.

pub mod lists;
pub mod opensubtitles;
pub mod trakt;
