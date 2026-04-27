//! Library — read-side queries that span multiple domains
//! (content + acquisition + playback). The handlers here join across
//! `movie`, `show`, `episode`, `media`, `download` to produce
//! search results, the calendar view, dashboard stats, and the
//! widget panel for external dashboards.
//!
//! ## Public API
//!
//! `handlers::*` are HTTP route targets, registered via main.rs.
//! Domain-internal: nothing — the cross-domain reads themselves are
//! the value this module surfaces, not any helpers.

pub mod handlers;
