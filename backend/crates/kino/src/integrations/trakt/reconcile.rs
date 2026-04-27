//! Match Trakt entities to local kino entities via a multi-id
//! fallback chain. Trakt returns `{tmdb, imdb, tvdb, trakt, slug}`
//! for every item — if the one we prefer (tmdb) has drifted because
//! TMDB merged/retired a record, we fall through to the next.
//!
//! Order is deliberate:
//!   1. `tmdb_id` — our canonical key, matches for ~99% of items
//!   2. `imdb_id` — stable across TMDB's catalogue churn
//!   3. `tvdb_id` — last ID resort (shows/episodes only)
//!   4. title + year — fuzzy fallback for the rare drift case
//!
//! Returns `None` when nothing matches; callers count these as
//! `not_found` and log at DEBUG.

use sqlx::SqlitePool;

use super::types::TraktIds;

/// Resolve a Trakt movie record to a local `movie.id`.
pub async fn find_movie(
    db: &SqlitePool,
    ids: &TraktIds,
    title: &str,
    year: Option<i64>,
) -> Option<i64> {
    if let Some(tmdb) = ids.tmdb
        && let Some(id) = lookup(db, "SELECT id FROM movie WHERE tmdb_id = ?", tmdb).await
    {
        return Some(id);
    }
    if let Some(ref imdb) = ids.imdb
        && let Some(id) = lookup_str(db, "SELECT id FROM movie WHERE imdb_id = ?", imdb).await
    {
        return Some(id);
    }
    if let Some(tvdb) = ids.tvdb
        && let Some(id) = lookup(db, "SELECT id FROM movie WHERE tvdb_id = ?", tvdb).await
    {
        return Some(id);
    }
    // Title + year fuzzy match, last resort. Case-insensitive, exact
    // title; year within ±1 to handle release-year disagreements
    // between TMDB and Trakt (happens on edge cases like films that
    // festival in Dec and release to theatres in Jan).
    if !title.is_empty()
        && let Some(y) = year
    {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM movie
             WHERE lower(title) = lower(?) AND year BETWEEN ? AND ?
             LIMIT 1",
        )
        .bind(title)
        .bind(y - 1)
        .bind(y + 1)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
        if let Some((id,)) = row {
            return Some(id);
        }
    }
    None
}

/// Resolve a Trakt show record to a local `show.id`.
pub async fn find_show(
    db: &SqlitePool,
    ids: &TraktIds,
    title: &str,
    year: Option<i64>,
) -> Option<i64> {
    if let Some(tmdb) = ids.tmdb
        && let Some(id) = lookup(db, "SELECT id FROM show WHERE tmdb_id = ?", tmdb).await
    {
        return Some(id);
    }
    if let Some(ref imdb) = ids.imdb
        && let Some(id) = lookup_str(db, "SELECT id FROM show WHERE imdb_id = ?", imdb).await
    {
        return Some(id);
    }
    if let Some(tvdb) = ids.tvdb
        && let Some(id) = lookup(db, "SELECT id FROM show WHERE tvdb_id = ?", tvdb).await
    {
        return Some(id);
    }
    if !title.is_empty()
        && let Some(y) = year
    {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM show
             WHERE lower(title) = lower(?) AND year BETWEEN ? AND ?
             LIMIT 1",
        )
        .bind(title)
        .bind(y - 1)
        .bind(y + 1)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
        if let Some((id,)) = row {
            return Some(id);
        }
    }
    None
}

/// Resolve a `(show_id, season, episode)` to a local `episode.id`.
/// Preserves callers' already-resolved `show_id` rather than relying
/// on a Trakt episode-id lookup (which we'd need to cache separately).
pub async fn find_episode(db: &SqlitePool, show_id: i64, season: i64, number: i64) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM episode WHERE show_id = ? AND season_number = ? AND episode_number = ?",
    )
    .bind(show_id)
    .bind(season)
    .bind(number)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

async fn lookup(db: &SqlitePool, sql: &str, id: i64) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

async fn lookup_str(db: &SqlitePool, sql: &str, id: &str) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}
