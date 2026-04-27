#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;

use crate::models::enums::MonitorNewItems;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Show {
    pub id: i64,
    pub tmdb_id: i64,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub year: Option<i64>,
    pub status: Option<String>,
    pub network: Option<String>,
    pub runtime: Option<i64>,
    pub certification: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub genres: Option<String>,
    pub tmdb_rating: Option<f64>,
    pub tmdb_vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub original_language: Option<String>,
    pub youtube_trailer_id: Option<String>,
    pub quality_profile_id: i64,
    pub monitored: bool,
    #[schema(value_type = MonitorNewItems)]
    pub monitor_new_items: String,
    /// Season 0 ("Specials") opt-in. Defaults to `false` — many shows
    /// drop weekly specials that would otherwise clutter Next Up and
    /// the calendar. Users toggle it from the Follow / Manage dialog.
    #[serde(default)]
    pub monitor_specials: bool,
    /// See `FollowIntent` enum — `'explicit'` or `'adhoc'`. Stored
    /// as `String` so sqlx maps cleanly; `value_type` override gives
    /// the frontend the narrow union.
    #[schema(value_type = crate::models::enums::FollowIntent)]
    pub follow_intent: String,
    pub added_at: String,
    pub blurhash_poster: Option<String>,
    pub blurhash_backdrop: Option<String>,
    pub first_air_date: Option<String>,
    pub last_air_date: Option<String>,
    pub last_metadata_refresh: Option<String>,
    /// 1..10 user rating on Trakt's scale. Null when unrated.
    pub user_rating: Option<i64>,
    /// Per-show intro-skipper toggle (subsystem 15). When false, the
    /// player never surfaces the Skip Intro button for any episode of
    /// this show — covers shows the user likes the theme song of.
    #[serde(default = "default_skip_intros")]
    pub skip_intros: bool,
    /// Cached clearlogo path (see subsystem 29). `None` until the
    /// metadata sweep fetches it.
    pub logo_path: Option<String>,
    /// `'mono'` or `'multi'` palette classification; `None` when no
    /// logo is stored.
    pub logo_palette: Option<String>,
    /// True when the follow flow is mid-fanout (show row inserted,
    /// season + episode loop still in progress or crashed). Reads
    /// at the API boundary filter `partial = 0` so partial shows
    /// stay invisible until the reconcile loop completes the fanout
    /// (or the user retries).
    #[serde(default)]
    pub partial: bool,
}

fn default_skip_intros() -> bool {
    true
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateShow {
    pub tmdb_id: i64,
    /// When omitted, the server resolves the current default profile.
    #[serde(default)]
    pub quality_profile_id: Option<i64>,
    pub monitored: Option<bool>,
    #[schema(value_type = Option<MonitorNewItems>)]
    pub monitor_new_items: Option<String>,
    /// Season numbers to *download* (mark `episode.monitored = 1`
    /// for). Any season not in this list has its episodes created
    /// as `monitored = 0`, so they appear on the show detail page
    /// but don't get searched/downloaded. `None` (default) keeps
    /// every season monitored — the historical behaviour.
    #[serde(default)]
    pub seasons_to_monitor: Option<Vec<i64>>,
    /// Opt into Season 0 ("Specials"). Defaults to `false` when
    /// omitted. Bundled here (rather than piggybacking on
    /// `seasons_to_monitor`) so the dialog can surface a distinct
    /// "Include specials" toggle — clearer intent and a cleaner
    /// default-off.
    #[serde(default)]
    pub monitor_specials: Option<bool>,
    /// Reason the show is being added. `None` defaults to `'explicit'`
    /// (user went through the Follow dialog). Auto-follow paths
    /// (Play, Get, acquire-by-tmdb) pass `Some("adhoc")` so the show
    /// self-removes when its last acquired episode is discarded.
    #[serde(default)]
    #[schema(value_type = Option<crate::models::enums::FollowIntent>)]
    pub follow_intent: Option<String>,
}

/// Show plus per-show rollups used by the library list. Exists so
/// the Library page's Wanted tab can show TV rows without needing a
/// second round-trip per show — the counts are cheap correlated
/// subqueries bundled into the list query.
///
/// `FromRow` is implemented by hand so `next_episode` / `active_download`
/// (which aren't selected from the main list-shows SQL — they're
/// attached in a second pass) don't trip sqlx's "no `Decode` impl for
/// struct" check that `#[sqlx(default)]` can't silence.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ShowListItem {
    #[serde(flatten)]
    pub show: Show,
    /// Episodes that have *already aired* but aren't yet acquired.
    /// These are the ones the scheduler's wanted-sweep will actually
    /// search for. Future-dated episodes are tracked separately in
    /// `upcoming_episode_count` so the Wanted tab doesn't show shows
    /// as "Searching" when they're just waiting for the next air date.
    pub wanted_episode_count: i64,
    /// Episodes that will be wanted once they air (`air_date_utc` in
    /// the future). Purely informational — the UI can tag a show
    /// with "airs Oct 3" without implying search activity.
    pub upcoming_episode_count: i64,
    /// Episodes that have been watched (`watched_at IS NOT NULL`).
    pub watched_episode_count: i64,
    /// Total monitored episodes — denominator for progress UI when
    /// the user has explicitly curated seasons via Follow dialog.
    pub episode_count: i64,
    /// Episodes with an imported file (`media_episode` link exists).
    /// The "X of Y" denominator users care about at a glance:
    /// "how much of the show do I actually have?"
    pub available_episode_count: i64,
    /// Aired regular-season episodes (`season_number` >= 1, `air_date`
    /// in the past). Denominator for play-auto-follow shows where
    /// `episode_count` (monitored-only) would be zero.
    pub aired_episode_count: i64,
    /// The episode that a show-level Play click would pick — matches
    /// the priority order in `watch_now_show_smart`: earliest unwatched
    /// imported episode first, else earliest unwatched aired episode.
    /// `None` for fully-watched shows (caught up) or shows with no
    /// aired episodes yet. Attached post-query in `list_shows`.
    pub next_episode: Option<NextEpisode>,
    /// The single most-relevant active download for this show, if any.
    /// "Most relevant" = lowest (season, episode); matches the episode
    /// the user would typically be watching first. Attached post-query
    /// in `list_shows`.
    pub active_download: Option<ActiveShowDownload>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for ShowListItem {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            show: <Show as sqlx::FromRow<sqlx::sqlite::SqliteRow>>::from_row(row)?,
            wanted_episode_count: row.try_get("wanted_episode_count")?,
            upcoming_episode_count: row.try_get("upcoming_episode_count")?,
            watched_episode_count: row.try_get("watched_episode_count")?,
            episode_count: row.try_get("episode_count")?,
            available_episode_count: row.try_get("available_episode_count")?,
            aired_episode_count: row.try_get("aired_episode_count")?,
            next_episode: None,
            active_download: None,
        })
    }
}

/// Episode that a show-level Play will resolve to.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NextEpisode {
    pub episode_id: i64,
    pub season_number: i64,
    pub episode_number: i64,
    pub title: Option<String>,
    /// True when the episode's file is already imported — Play is an
    /// instant "watch now", no grab needed. False → Play will trigger
    /// a grab + stream-while-downloading flow.
    pub available: bool,
}

/// Active download linked to one of a show's episodes. One per show
/// at most — callers dedupe to the lowest `(season, episode)`. The
/// leader row's progress / state drives the show card's sweep +
/// pill; `active_count` reveals when there's more going on than the
/// single leader surfaces (e.g. a season pack grab with three
/// concurrent episode downloads, so pause-all affects all three).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActiveShowDownload {
    pub download_id: i64,
    pub episode_id: i64,
    pub season_number: i64,
    pub episode_number: i64,
    #[schema(value_type = crate::models::enums::DownloadState)]
    pub state: String,
    pub downloaded: i64,
    pub total_size: Option<i64>,
    pub download_speed: i64,
    /// Total active-state downloads for this show. When `> 1`, the
    /// card renders an "×N" indicator next to the pill so users
    /// know pause-all affects more than one torrent.
    pub active_count: i64,
}
