//! Lists subsystem (17) — fetch + sync external lists into local
//! `list` / `list_item` rows.
//!
//! Each source resolver speaks its own API but returns the same
//! `ListMetadata` + `Vec<RawItem>` shapes so the upstream sync
//! engine ([`sync`]) is source-agnostic.
//!
//! Public surface:
//!   - [`parser::parse_list_url`] — URL → `ParsedList { source_type, source_id }`
//!   - [`fetch_metadata`] / [`fetch_items`] — dispatch on `source_type`
//!   - [`sync::apply_poll`] — diff-and-apply against an existing list
//!   - [`sync::create_list`] — first-add orchestration with soft cap

pub mod handlers;
pub mod mdblist;
pub mod model;
pub mod parser;
pub mod sync;
pub mod tmdb_list;
pub mod trakt_list;

use sqlx::SqlitePool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ListsError {
    #[error("unsupported list URL: {0}")]
    UnsupportedUrl(String),
    #[error("MDBList API key not configured")]
    MissingMdblistKey,
    #[error("Trakt not connected")]
    TraktNotConnected,
    #[error("source unreachable: {0}")]
    Network(String),
    #[error("source returned malformed data: {0}")]
    Parse(String),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Identifies which resolver to dispatch to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Mdblist,
    TmdbList,
    TraktList,
    TraktWatchlist,
}

impl SourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mdblist => "mdblist",
            Self::TmdbList => "tmdb_list",
            Self::TraktList => "trakt_list",
            Self::TraktWatchlist => "trakt_watchlist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mdblist" => Some(Self::Mdblist),
            "tmdb_list" => Some(Self::TmdbList),
            "trakt_list" => Some(Self::TraktList),
            "trakt_watchlist" => Some(Self::TraktWatchlist),
            _ => None,
        }
    }
}

/// Cross-source list-level metadata.
#[derive(Debug, Clone)]
pub struct ListMetadata {
    pub title: String,
    pub description: Option<String>,
    pub item_count: i64,
    /// `movies` / `shows` / `mixed`.
    pub item_type: String,
}

/// One item from a remote list, normalised to TMDB IDs. Items the
/// resolver can't map to a TMDB ID are dropped before they reach the
/// sync engine.
#[derive(Debug, Clone)]
pub struct RawItem {
    pub tmdb_id: i64,
    /// `movie` / `show`.
    pub item_type: String,
    pub title: String,
    pub poster_path: Option<String>,
    pub position: Option<i64>,
    /// When the source added this item to the list (best-effort —
    /// some sources don't publish per-item timestamps; we fall back
    /// to "now" so re-poll diff stays meaningful).
    pub added_at: String,
}

/// Resolved by [`parser::parse_list_url`].
#[derive(Debug, Clone)]
pub struct ParsedList {
    pub source_type: SourceType,
    pub source_id: String,
    /// Canonical URL the resolver will hit (lowercased host, trailing
    /// slashes trimmed). Stored on the `list` row for display.
    pub source_url: String,
}

/// Dispatch metadata fetch to the right resolver.
pub async fn fetch_metadata(
    db: &SqlitePool,
    parsed: &ParsedList,
) -> Result<ListMetadata, ListsError> {
    match parsed.source_type {
        SourceType::Mdblist => mdblist::fetch_metadata(db, &parsed.source_id).await,
        SourceType::TmdbList => tmdb_list::fetch_metadata(db, &parsed.source_id).await,
        SourceType::TraktList => trakt_list::fetch_metadata(db, &parsed.source_id).await,
        SourceType::TraktWatchlist => trakt_list::fetch_watchlist_metadata(db).await,
    }
}

/// Dispatch item fetch to the right resolver.
pub async fn fetch_items(db: &SqlitePool, parsed: &ParsedList) -> Result<Vec<RawItem>, ListsError> {
    match parsed.source_type {
        SourceType::Mdblist => mdblist::fetch_items(db, &parsed.source_id).await,
        SourceType::TmdbList => tmdb_list::fetch_items(db, &parsed.source_id).await,
        SourceType::TraktList => trakt_list::fetch_items(db, &parsed.source_id).await,
        SourceType::TraktWatchlist => trakt_list::fetch_watchlist_items(db).await,
    }
}
