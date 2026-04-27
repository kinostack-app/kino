// Several types in this module are scaffolding for the API + sync
// engine commits that follow C1; clippy's dead-code warnings are
// expected until those land.
#![allow(dead_code)]
//! Lists subsystem (17) models.
//!
//! A `List` is any external URL that resolves to a set of TMDB IDs.
//! kino polls it periodically, applies the items as monitored entries,
//! and reflects the current contents in the UI. Source types map to
//! one of four resolvers in `integrations::lists`.
//!
//! `add-never-subtract`: removing an item from a source list never
//! unmonitors it locally. The `list_item` row goes (mirrors current
//! source state) but the underlying `Movie`/`Show` is left alone.
//! Inverse: a user-explicit unmonitor sets `ignored_by_user` so the
//! next poll doesn't auto-re-monitor and start the fight again.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, FromRow)]
pub struct List {
    pub id: i64,
    /// `mdblist` / `tmdb_list` / `trakt_list` / `trakt_watchlist`.
    pub source_type: String,
    pub source_url: String,
    /// Source-side identifier (slug for MDBList/Trakt, numeric for TMDB).
    pub source_id: String,
    pub title: String,
    pub description: Option<String>,
    pub item_count: i64,
    /// `movies` / `shows` / `mixed`.
    pub item_type: String,
    pub last_polled_at: Option<String>,
    /// `ok` or `error: ...`.
    pub last_poll_status: Option<String>,
    pub consecutive_poll_failures: i64,
    /// True for the Trakt watchlist — can't be user-deleted.
    pub is_system: bool,
    pub created_at: String,
}

/// API response shape = `List` + a short strip of TMDB poster paths
/// for the first few items on the list. Lets the /lists grid card
/// show a visual preview of what's in each list without an N+1 fetch
/// from the frontend. Always a separate struct from `List` (the DB
/// row) so sqlx's `FromRow` stays on the flat shape.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ListView {
    #[serde(flatten)]
    pub list: List,
    /// Up to 4 poster paths from the first items in the list, in
    /// source-position order. Empty when the list hasn't been polled
    /// yet or no items have a poster.
    pub preview_posters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, FromRow)]
pub struct ListItem {
    pub id: i64,
    pub list_id: i64,
    pub tmdb_id: i64,
    /// `movie` / `show`.
    pub item_type: String,
    pub title: String,
    pub poster_path: Option<String>,
    pub position: Option<i64>,
    pub added_at: String,
    pub ignored_by_user: bool,
}

/// Body for `POST /api/v1/lists`. Two-phase: clients first call with
/// `confirm=false` (the default) to get a preview, then call again
/// with `confirm=true` to actually create the list.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateListRequest {
    pub url: String,
    /// `false` / absent = preview only. `true` = create the list.
    #[serde(default)]
    pub confirm: bool,
}

/// Response from the preview phase or `GET /lists/{id}` with metadata
/// joined. Frontend renders this so the user can verify the list
/// before committing.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ListPreview {
    pub source_type: String,
    pub source_id: String,
    pub title: String,
    pub description: Option<String>,
    pub item_count: i64,
    pub item_type: String,
}

/// Per-item view for `GET /lists/{id}/items`. Joins live acquisition
/// state from the underlying `Movie`/`Show` so the UI can render
/// status badges without a second roundtrip per item.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ListItemView {
    pub id: i64,
    pub list_id: i64,
    pub tmdb_id: i64,
    pub item_type: String,
    pub title: String,
    pub poster_path: Option<String>,
    pub position: Option<i64>,
    pub added_at: String,
    pub ignored_by_user: bool,
    /// `not_in_library` / `monitoring` / `searching` / `downloading` /
    /// `acquired` / `watched` — derived in the handler from the
    /// joined movie/show row.
    pub state: String,
}
