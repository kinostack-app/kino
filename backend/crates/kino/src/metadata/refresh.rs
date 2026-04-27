//! Metadata refresh service — periodically re-fetches TMDB data for
//! monitored movies and shows, updates DB fields in place, and detects
//! new episodes.
//!
//! Called from the scheduler's `metadata_refresh` task (see
//! `scheduler::execute_task`). Uses two staleness tiers so active
//! content (airing shows, recently-released movies) refreshes often
//! enough to catch new episodes / release-date shifts while stable
//! content (ended shows, older movies) polls rarely. The shared TMDB
//! client handles rate limiting and retry across all refresh calls.
//!
//! Logos are NOT fetched here — they're lazy-loaded on first view via
//! the `/api/v1/images/{type}/{id}/logo` endpoint, same pattern as
//! posters. See `metadata::image_handlers::serve_logo` for the cache flow.

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::events::AppEvent;
use crate::time::Timestamp;
use crate::tmdb::TmdbClient;

/// Staleness tiers. See `docs/subsystems/01-metadata.md` § Refresh
/// cadence for the rationale. Hot = likely-to-change rows (airing
/// shows, in-release-window movies); cold = stable catalogue.
const HOT_STALE_HOURS: i64 = 1;
const COLD_STALE_HOURS: i64 = 72;
/// A movie is "in its release window" for 60 days post-release, or
/// any time before its theatrical release (date may still shift).
const MOVIE_HOT_WINDOW_DAYS: i64 = 60;

/// Sweep over monitored movies + shows with a stale
/// `last_metadata_refresh` and re-fetch from TMDB. Returns the number
/// of entities refreshed. Per-row tier selection happens in SQL so
/// we scan each table once.
#[allow(clippy::too_many_lines)]
pub async fn refresh_sweep(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    tmdb: &TmdbClient,
) -> anyhow::Result<u64> {
    let hot_cutoff = Timestamp::now_minus(chrono::Duration::hours(HOT_STALE_HOURS)).to_rfc3339();
    let cold_cutoff = Timestamp::now_minus(chrono::Duration::hours(COLD_STALE_HOURS)).to_rfc3339();
    let movie_hot_window_start = (chrono::Utc::now()
        - chrono::Duration::days(MOVIE_HOT_WINDOW_DAYS))
    .date_naive()
    .to_string();

    tracing::debug!(
        hot_cutoff = %hot_cutoff,
        cold_cutoff = %cold_cutoff,
        movie_hot_window_start = %movie_hot_window_start,
        "metadata refresh sweep starting"
    );

    let mut total = 0u64;
    let mut hot_refreshed = 0u64;
    let mut cold_refreshed = 0u64;
    let mut failed = 0u64;

    // Movies: hot when `release_date` is NULL, in the future, or
    // within the last 60 days; cold otherwise.
    let stale_movies: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, tmdb_id,
                CASE
                  WHEN release_date IS NULL OR release_date >= ?
                    THEN 'hot'
                  ELSE 'cold'
                END AS tier
         FROM movie
         WHERE monitored = 1
           AND (
             last_metadata_refresh IS NULL
             OR (
               CASE
                 WHEN release_date IS NULL OR datetime(release_date) >= datetime(?)
                   THEN datetime(last_metadata_refresh) < datetime(?)
                 ELSE datetime(last_metadata_refresh) < datetime(?)
               END
             )
           )",
    )
    .bind(&movie_hot_window_start)
    .bind(&movie_hot_window_start)
    .bind(&hot_cutoff)
    .bind(&cold_cutoff)
    .fetch_all(pool)
    .await?;

    for (id, tmdb_id, tier) in stale_movies {
        if let Err(e) = refresh_movie(pool, tmdb, id, tmdb_id).await {
            tracing::warn!(movie_id = id, tier = %tier, error = %e, "movie refresh failed");
            failed += 1;
            continue;
        }
        if tier == "hot" {
            hot_refreshed += 1;
        } else {
            cold_refreshed += 1;
        }
        total += 1;
    }

    // Shows: hot when status is anything other than 'Ended' /
    // 'Canceled' (case-insensitive). NULL status stays hot so a newly
    // added row doesn't sit cold before its first successful refresh
    // populates the status.
    let stale_shows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, tmdb_id,
                CASE
                  WHEN status IS NULL OR LOWER(status) NOT IN ('ended', 'canceled', 'cancelled')
                    THEN 'hot'
                  ELSE 'cold'
                END AS tier
         FROM show
         WHERE monitored = 1
           AND (
             last_metadata_refresh IS NULL
             OR (
               CASE
                 WHEN status IS NULL OR LOWER(status) NOT IN ('ended', 'canceled', 'cancelled')
                   THEN datetime(last_metadata_refresh) < datetime(?)
                 ELSE datetime(last_metadata_refresh) < datetime(?)
               END
             )
           )",
    )
    .bind(&hot_cutoff)
    .bind(&cold_cutoff)
    .fetch_all(pool)
    .await?;

    for (id, tmdb_id, tier) in stale_shows {
        if let Err(e) = refresh_show(pool, event_tx, tmdb, id, tmdb_id).await {
            tracing::warn!(show_id = id, tier = %tier, error = %e, "show refresh failed");
            failed += 1;
            continue;
        }
        if tier == "hot" {
            hot_refreshed += 1;
        } else {
            cold_refreshed += 1;
        }
        total += 1;
    }

    tracing::info!(
        total,
        hot_refreshed,
        cold_refreshed,
        failed,
        "metadata refresh sweep complete"
    );

    Ok(total)
}

#[allow(clippy::similar_names)]
#[tracing::instrument(skip(pool, tmdb), fields(movie_id = id, tmdb_id))]
async fn refresh_movie(
    pool: &SqlitePool,
    tmdb: &TmdbClient,
    id: i64,
    tmdb_id: i64,
) -> anyhow::Result<()> {
    let details = tmdb
        .movie_details(tmdb_id)
        .await
        .map_err(|e| anyhow::anyhow!("tmdb: {e}"))?;

    let year = details
        .release_date
        .as_deref()
        .and_then(|d| d.get(..4)?.parse::<i64>().ok());

    let genres_json = details.genres.as_ref().map_or_else(
        || "[]".into(),
        |g| {
            serde_json::to_string(&g.iter().map(|x| &x.name).collect::<Vec<_>>())
                .unwrap_or_else(|_| "[]".into())
        },
    );

    let imdb_id = details
        .external_ids
        .as_ref()
        .and_then(|e| e.imdb_id.clone())
        .or(details.imdb_id);

    let tvdb_id = details.external_ids.as_ref().and_then(|e| e.tvdb_id);

    let collection_tmdb_id = details.belongs_to_collection.as_ref().map(|c| c.id);
    let collection_name = details
        .belongs_to_collection
        .as_ref()
        .map(|c| c.name.clone());

    let trailer_id = details.videos.as_ref().and_then(|v| {
        v.results
            .iter()
            .find(|vid| vid.site == "YouTube" && vid.video_type == "Trailer")
            .map(|vid| vid.key.clone())
    });

    let now = crate::time::Timestamp::now().to_rfc3339();

    sqlx::query(
        "UPDATE movie SET
             imdb_id = COALESCE(?, imdb_id),
             tvdb_id = COALESCE(?, tvdb_id),
             title = ?, original_title = ?, overview = ?, tagline = ?,
             year = ?, runtime = ?, release_date = ?,
             poster_path = ?, backdrop_path = ?,
             genres = ?, tmdb_rating = ?, tmdb_vote_count = ?, popularity = ?,
             original_language = ?,
             collection_tmdb_id = ?, collection_name = ?,
             youtube_trailer_id = ?,
             last_metadata_refresh = ?
         WHERE id = ?",
    )
    .bind(imdb_id)
    .bind(tvdb_id)
    .bind(&details.title)
    .bind(details.original_title.as_deref())
    .bind(details.overview.as_deref())
    .bind(details.tagline.as_deref())
    .bind(year)
    .bind(details.runtime)
    .bind(details.release_date.as_deref())
    .bind(details.poster_path.as_deref())
    .bind(details.backdrop_path.as_deref())
    .bind(&genres_json)
    .bind(details.vote_average)
    .bind(details.vote_count)
    .bind(details.popularity)
    .bind(details.original_language.as_deref())
    .bind(collection_tmdb_id)
    .bind(collection_name)
    .bind(trailer_id)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

#[allow(clippy::similar_names, clippy::too_many_lines)]
#[tracing::instrument(skip(pool, event_tx, tmdb), fields(show_id = id, tmdb_id))]
async fn refresh_show(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    tmdb: &TmdbClient,
    id: i64,
    tmdb_id: i64,
) -> anyhow::Result<()> {
    let details = tmdb
        .show_details(tmdb_id)
        .await
        .map_err(|e| anyhow::anyhow!("tmdb: {e}"))?;

    // Resolve the show's new-episode monitoring policy once per
    // refresh. Passed into `refresh_season` so the INSERT below
    // gives fresh episodes the right acquire/in_scope defaults
    // instead of the schema's blanket 1/1. `added_at` is the
    // fence used by the `new` mode — an episode's `air_date` is
    // considered "future" relative to when the user chose to
    // follow, not to right-now.
    let show_prefs: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT monitor_new_items, added_at, monitor_specials FROM show WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let (monitor_new_items, show_added_at, monitor_specials) = show_prefs.unwrap_or_else(|| {
        (
            "future".to_string(),
            crate::time::Timestamp::now().to_rfc3339(),
            false,
        )
    });

    let year = details
        .first_air_date
        .as_deref()
        .and_then(|d| d.get(..4)?.parse::<i64>().ok());

    let runtime = details
        .episode_run_time
        .as_ref()
        .and_then(|v| v.first())
        .copied();

    let genres_json = details.genres.as_ref().map_or_else(
        || "[]".into(),
        |g| {
            serde_json::to_string(&g.iter().map(|x| &x.name).collect::<Vec<_>>())
                .unwrap_or_else(|_| "[]".into())
        },
    );

    let network = details
        .networks
        .as_ref()
        .and_then(|n| n.first())
        .map(|n| n.name.clone());

    let imdb_id = details
        .external_ids
        .as_ref()
        .and_then(|e| e.imdb_id.clone());
    let tvdb_id = details.external_ids.as_ref().and_then(|e| e.tvdb_id);

    let trailer_id = details.videos.as_ref().and_then(|v| {
        v.results
            .iter()
            .find(|vid| vid.site == "YouTube" && vid.video_type == "Trailer")
            .map(|vid| vid.key.clone())
    });

    let now = crate::time::Timestamp::now().to_rfc3339();

    sqlx::query(
        "UPDATE show SET
             imdb_id = COALESCE(?, imdb_id),
             tvdb_id = COALESCE(?, tvdb_id),
             title = ?, original_title = ?, overview = ?, tagline = ?,
             year = ?, status = ?, network = ?, runtime = ?,
             poster_path = ?, backdrop_path = ?,
             genres = ?, tmdb_rating = ?, tmdb_vote_count = ?, popularity = ?,
             original_language = ?, youtube_trailer_id = ?,
             first_air_date = ?, last_air_date = ?,
             last_metadata_refresh = ?
         WHERE id = ?",
    )
    .bind(imdb_id)
    .bind(tvdb_id)
    .bind(&details.name)
    .bind(details.original_name.as_deref())
    .bind(details.overview.as_deref())
    .bind(details.tagline.as_deref())
    .bind(year)
    .bind(details.status.as_deref())
    .bind(network)
    .bind(runtime)
    .bind(details.poster_path.as_deref())
    .bind(details.backdrop_path.as_deref())
    .bind(&genres_json)
    .bind(details.vote_average)
    .bind(details.vote_count)
    .bind(details.popularity)
    .bind(details.original_language.as_deref())
    .bind(trailer_id)
    .bind(details.first_air_date.as_deref())
    .bind(details.last_air_date.as_deref())
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;

    // For each TMDB season, upsert series row + fetch episodes + detect new ones.
    if let Some(seasons) = details.seasons {
        for season in seasons {
            refresh_season(
                pool,
                event_tx,
                tmdb,
                id,
                tmdb_id,
                &details.name,
                season.season_number,
                &monitor_new_items,
                &show_added_at,
                monitor_specials,
            )
            .await
            .ok(); // best-effort per season
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::similar_names)]
#[tracing::instrument(
    skip(pool, event_tx, tmdb, show_title, monitor_new_items, show_added_at),
    fields(show_id, show_tmdb_id, season_number)
)]
async fn refresh_season(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    tmdb: &TmdbClient,
    show_id: i64,
    show_tmdb_id: i64,
    show_title: &str,
    season_number: i64,
    monitor_new_items: &str,
    show_added_at: &str,
    monitor_specials: bool,
) -> anyhow::Result<()> {
    let details = tmdb
        .season_details(show_tmdb_id, season_number)
        .await
        .map_err(|e| anyhow::anyhow!("tmdb season: {e}"))?;

    // Upsert series row.
    let episode_count = i64::try_from(details.episodes.as_ref().map_or(0, Vec::len)).unwrap_or(0);
    let series_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO series (show_id, tmdb_id, season_number, title, overview, poster_path, air_date, episode_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(show_id, season_number) DO UPDATE SET
             tmdb_id = excluded.tmdb_id,
             title = excluded.title,
             overview = excluded.overview,
             poster_path = excluded.poster_path,
             air_date = excluded.air_date,
             episode_count = excluded.episode_count
         RETURNING id",
    )
    .bind(show_id)
    .bind(details.id)
    .bind(season_number)
    .bind(details.name.as_deref())
    .bind(details.overview.as_deref())
    .bind(details.poster_path.as_deref())
    .bind(details.air_date.as_deref())
    .bind(episode_count)
    .fetch_one(pool)
    .await?;

    // Upsert each episode; detect & emit event for new ones.
    //
    // Seed acquire/in_scope explicitly on insert based on the show's
    // `monitor_new_items` policy. The `ON CONFLICT` clause deliberately
    // doesn't touch acquire/in_scope — if a user already tweaked them
    // via the Manage dialog or /episodes/{id}/redownload endpoint, a
    // metadata refresh must not clobber that intent.
    if let Some(episodes) = details.episodes {
        for ep in episodes {
            let existed: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM episode WHERE series_id = ? AND episode_number = ?",
            )
            .bind(series_id)
            .bind(ep.episode_number)
            .fetch_optional(pool)
            .await?;

            let (acquire, in_scope) = seed_acquire_in_scope(
                monitor_new_items,
                ep.air_date.as_deref(),
                show_added_at,
                season_number,
                monitor_specials,
            );

            let new_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO episode (series_id, show_id, season_number, tmdb_id, episode_number, title, overview, air_date_utc, runtime, still_path, tmdb_rating, acquire, in_scope)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(series_id, episode_number) DO UPDATE SET
                     tmdb_id = excluded.tmdb_id,
                     title = excluded.title,
                     overview = excluded.overview,
                     air_date_utc = excluded.air_date_utc,
                     runtime = excluded.runtime,
                     still_path = excluded.still_path,
                     tmdb_rating = excluded.tmdb_rating
                 RETURNING id",
            )
            .bind(series_id)
            .bind(show_id)
            .bind(season_number)
            .bind(ep.id)
            .bind(ep.episode_number)
            .bind(ep.name.as_deref())
            .bind(ep.overview.as_deref())
            .bind(ep.air_date.as_deref())
            .bind(ep.runtime)
            .bind(ep.still_path.as_deref())
            .bind(ep.vote_average)
            .bind(acquire)
            .bind(in_scope)
            .fetch_one(pool)
            .await?;

            if existed.is_none() {
                let _ = event_tx.send(AppEvent::NewEpisode {
                    show_id,
                    episode_id: new_id,
                    show_title: show_title.to_string(),
                    season: season_number,
                    episode: ep.episode_number,
                    episode_title: ep.name.clone().filter(|s| !s.is_empty()),
                });
            }
        }
    }

    Ok(())
}

/// Decide the initial `acquire` / `in_scope` for an episode inserted
/// by the metadata refresh, based on the show's `monitor_new_items`
/// setting, the episode's season number, and the show's specials
/// opt-in. Returned tuple is `(acquire, in_scope)`.
///
/// Season 0 ("Specials") is opt-in via `monitor_specials`. When off
/// (the default), every season-0 episode is seeded at `(0, 0)`
/// regardless of `monitor_new_items`. Covers the common case of
/// weekly short specials for shows the user just wants the main
/// episodes of.
///
/// - **all**: monitor everything (1, 1). Matches the schema default
///   — user wants the full library.
/// - **new**: only if the episode airs after the show was followed.
///   Stops retroactive TMDB adds (a bonus episode dated 2014 added
///   today) from auto-grabbing for a user who just started watching.
/// - **none**: never auto-monitor new episodes. User will tick them
///   manually from the show detail page.
fn seed_acquire_in_scope(
    monitor_new_items: &str,
    ep_air_date: Option<&str>,
    show_added_at: &str,
    season_number: i64,
    monitor_specials: bool,
) -> (i64, i64) {
    if season_number == 0 && !monitor_specials {
        return (0, 0);
    }
    if monitor_new_items == "none" {
        return (0, 0);
    }
    // Everything else (canonical 'future', plus any drift) maps to
    // "monitor only episodes airing from now on". Retroactive TMDB
    // additions (bonus episodes dated before the show was followed)
    // don't auto-grab. Missing air date → treat as future (safer than
    // silently dropping; the wanted-sweep skips null air_date anyway).
    let is_future = match ep_air_date {
        Some(d) if !d.is_empty() => d > show_added_at,
        _ => true,
    };
    if is_future { (1, 1) } else { (0, 0) }
}

#[cfg(test)]
mod seed_tests {
    use super::seed_acquire_in_scope;

    // Regular-season episodes (`specials_on = false` is irrelevant when
    // season > 0). Keep tests explicit about season + specials flag so
    // the matrix is readable.

    #[test]
    fn none_never_monitors() {
        assert_eq!(
            seed_acquire_in_scope("none", Some("2019-07-25"), "2026-04-18", 1, false),
            (0, 0)
        );
        assert_eq!(
            seed_acquire_in_scope("none", None, "2026-04-18", 1, false),
            (0, 0)
        );
    }

    #[test]
    fn future_monitors_only_future_air_dates() {
        // Retroactive TMDB add (aired before follow date): don't monitor.
        assert_eq!(
            seed_acquire_in_scope("future", Some("2019-07-25"), "2026-04-18", 1, false),
            (0, 0)
        );
        // Future air date: monitor.
        assert_eq!(
            seed_acquire_in_scope("future", Some("2026-06-01"), "2026-04-18", 1, false),
            (1, 1)
        );
        // Unknown air date: monitor (safer than silent drop).
        assert_eq!(
            seed_acquire_in_scope("future", None, "2026-04-18", 1, false),
            (1, 1)
        );
        // Empty string: monitor (treated as unknown).
        assert_eq!(
            seed_acquire_in_scope("future", Some(""), "2026-04-18", 1, false),
            (1, 1)
        );
    }

    #[test]
    fn specials_off_blocks_season_zero() {
        // Even a future-aired special stays (0, 0) when the show has
        // specials opt-in disabled.
        assert_eq!(
            seed_acquire_in_scope("future", Some("2026-06-01"), "2026-04-18", 0, false),
            (0, 0)
        );
        // And a past special stays (0, 0) too (matches the
        // "retroactive add" fallback).
        assert_eq!(
            seed_acquire_in_scope("future", Some("2019-07-25"), "2026-04-18", 0, false),
            (0, 0)
        );
    }

    #[test]
    fn specials_on_treats_season_zero_like_any_other() {
        // Future-aired special: monitor (same as a regular episode).
        assert_eq!(
            seed_acquire_in_scope("future", Some("2026-06-01"), "2026-04-18", 0, true),
            (1, 1)
        );
        // Retroactive special still follows the "new-items" fence.
        assert_eq!(
            seed_acquire_in_scope("future", Some("2019-07-25"), "2026-04-18", 0, true),
            (0, 0)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Stale-filter SQL: returns rows whose `last_metadata_refresh` is NULL
    /// or older than the threshold. Use RFC3339 throughout so lexical
    /// comparison matches chronological order.
    #[tokio::test]
    async fn stale_filter_matches_null_and_old() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let now = chrono::Utc::now();
        let now_s = now.to_rfc3339();
        let two_days_ago = (now - chrono::Duration::days(2)).to_rfc3339();

        sqlx::query(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at)
             VALUES (1, 'Never Refreshed', 1, 1, ?)",
        )
        .bind(&now_s)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at, last_metadata_refresh)
             VALUES (2, 'Fresh', 1, 1, ?, ?)",
        )
        .bind(&now_s)
        .bind(&now_s)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at, last_metadata_refresh)
             VALUES (3, 'Old', 1, 1, ?, ?)",
        )
        .bind(&now_s)
        .bind(&two_days_ago)
        .execute(&pool)
        .await
        .unwrap();

        let threshold = (now - chrono::Duration::hours(12)).to_rfc3339();
        let stale: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT id, tmdb_id FROM movie
             WHERE monitored = 1
               AND (last_metadata_refresh IS NULL OR datetime(last_metadata_refresh) < datetime(?))",
        )
        .bind(&threshold)
        .fetch_all(&pool)
        .await
        .unwrap();

        let ids: Vec<i64> = stale.iter().map(|(_, tmdb)| *tmdb).collect();
        assert!(ids.contains(&1), "NULL refresh should be stale");
        assert!(ids.contains(&3), "old refresh should be stale");
        assert!(!ids.contains(&2), "recent refresh should NOT be stale");
    }

    /// Sweep short-circuits when no rows are stale.
    #[tokio::test]
    async fn sweep_returns_zero_on_empty() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        // No TMDB API calls happen because no stale rows exist.
        let tmdb = TmdbClient::new(String::new());
        let count = refresh_sweep(&pool, &tx, &tmdb).await.unwrap();
        assert_eq!(count, 0);
    }

    /// Show tiering: hot shows (returning, in-production, null status)
    /// must go stale after the 1h hot threshold; cold shows
    /// (ended/canceled) must wait the 72h cold threshold.
    #[tokio::test]
    async fn show_tier_selection_respects_status() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let now = chrono::Utc::now();
        let now_s = now.to_rfc3339();
        let two_h_ago = (now - chrono::Duration::hours(2)).to_rfc3339();
        let four_days_ago = (now - chrono::Duration::days(4)).to_rfc3339();

        // 1: returning show refreshed 2h ago → hot, stale
        // 2: ended show refreshed 2h ago → cold, NOT stale
        // 3: ended show refreshed 4d ago → cold, stale
        // 4: returning show refreshed 2h ago but unmonitored → excluded
        sqlx::query(
            "INSERT INTO show (tmdb_id, title, status, quality_profile_id, monitored, added_at, last_metadata_refresh)
             VALUES (1, 'Returning', 'Returning Series', 1, 1, ?, ?),
                    (2, 'Ended Recently', 'Ended', 1, 1, ?, ?),
                    (3, 'Ended Long Ago', 'Ended', 1, 1, ?, ?),
                    (4, 'Unmonitored', 'Returning Series', 1, 0, ?, ?)",
        )
        .bind(&now_s).bind(&two_h_ago)
        .bind(&now_s).bind(&two_h_ago)
        .bind(&now_s).bind(&four_days_ago)
        .bind(&now_s).bind(&two_h_ago)
        .execute(&pool).await.unwrap();

        let hot_cutoff = (now - chrono::Duration::hours(HOT_STALE_HOURS)).to_rfc3339();
        let cold_cutoff = (now - chrono::Duration::hours(COLD_STALE_HOURS)).to_rfc3339();

        let stale: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT id, tmdb_id,
                    CASE
                      WHEN status IS NULL OR LOWER(status) NOT IN ('ended', 'canceled', 'cancelled')
                        THEN 'hot' ELSE 'cold'
                    END
             FROM show
             WHERE monitored = 1
               AND (
                 last_metadata_refresh IS NULL
                 OR (
                   CASE
                     WHEN status IS NULL OR LOWER(status) NOT IN ('ended', 'canceled', 'cancelled')
                       THEN datetime(last_metadata_refresh) < datetime(?)
                     ELSE datetime(last_metadata_refresh) < datetime(?)
                   END
                 )
               )",
        )
        .bind(&hot_cutoff)
        .bind(&cold_cutoff)
        .fetch_all(&pool)
        .await
        .unwrap();

        let picked: Vec<i64> = stale.iter().map(|(_, tmdb, _)| *tmdb).collect();
        assert!(picked.contains(&1), "hot show stale at 2h must be picked");
        assert!(
            !picked.contains(&2),
            "cold show stale at 2h must NOT be picked"
        );
        assert!(picked.contains(&3), "cold show stale at 4d must be picked");
        assert!(!picked.contains(&4), "unmonitored must be excluded");
    }

    /// Movie tiering: in-release-window movies (future date, or within
    /// the last 60d) follow the hot 1h threshold; older movies follow
    /// the cold 72h threshold.
    #[tokio::test]
    async fn movie_tier_selection_respects_release_window() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let now = chrono::Utc::now();
        let now_s = now.to_rfc3339();
        let two_h_ago = (now - chrono::Duration::hours(2)).to_rfc3339();
        let four_days_ago = (now - chrono::Duration::days(4)).to_rfc3339();

        let recent_release = (now - chrono::Duration::days(10)).date_naive().to_string();
        let old_release = (now - chrono::Duration::days(400)).date_naive().to_string();
        let future_release = (now + chrono::Duration::days(30)).date_naive().to_string();

        // 1: recently released + refreshed 2h ago → hot, stale
        // 2: old release + refreshed 2h ago → cold, NOT stale
        // 3: old release + refreshed 4d ago → cold, stale
        // 4: future release + refreshed 2h ago → hot, stale
        // 5: NULL release + refreshed 2h ago → hot (unknown = treat as active), stale
        sqlx::query(
            "INSERT INTO movie (tmdb_id, title, release_date, quality_profile_id, monitored, added_at, last_metadata_refresh)
             VALUES (1, 'Recent', ?, 1, 1, ?, ?),
                    (2, 'Old Refreshed Today', ?, 1, 1, ?, ?),
                    (3, 'Old Stale', ?, 1, 1, ?, ?),
                    (4, 'Upcoming', ?, 1, 1, ?, ?),
                    (5, 'No Date', NULL, 1, 1, ?, ?)",
        )
        .bind(&recent_release).bind(&now_s).bind(&two_h_ago)
        .bind(&old_release).bind(&now_s).bind(&two_h_ago)
        .bind(&old_release).bind(&now_s).bind(&four_days_ago)
        .bind(&future_release).bind(&now_s).bind(&two_h_ago)
        .bind(&now_s).bind(&two_h_ago)
        .execute(&pool).await.unwrap();

        let hot_cutoff = (now - chrono::Duration::hours(HOT_STALE_HOURS)).to_rfc3339();
        let cold_cutoff = (now - chrono::Duration::hours(COLD_STALE_HOURS)).to_rfc3339();
        let movie_hot_window_start = (now - chrono::Duration::days(MOVIE_HOT_WINDOW_DAYS))
            .date_naive()
            .to_string();

        let stale: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT id, tmdb_id,
                    CASE
                      WHEN release_date IS NULL OR release_date >= ?
                        THEN 'hot' ELSE 'cold'
                    END
             FROM movie
             WHERE monitored = 1
               AND (
                 last_metadata_refresh IS NULL
                 OR (
                   CASE
                     WHEN release_date IS NULL OR datetime(release_date) >= datetime(?)
                       THEN datetime(last_metadata_refresh) < datetime(?)
                     ELSE datetime(last_metadata_refresh) < datetime(?)
                   END
                 )
               )",
        )
        .bind(&movie_hot_window_start)
        .bind(&movie_hot_window_start)
        .bind(&hot_cutoff)
        .bind(&cold_cutoff)
        .fetch_all(&pool)
        .await
        .unwrap();

        let picked: Vec<i64> = stale.iter().map(|(_, tmdb, _)| *tmdb).collect();
        assert!(
            picked.contains(&1),
            "recent release stale at 2h must be picked"
        );
        assert!(
            !picked.contains(&2),
            "old release stale at 2h must NOT be picked"
        );
        assert!(
            picked.contains(&3),
            "old release stale at 4d must be picked"
        );
        assert!(
            picked.contains(&4),
            "future release stale at 2h must be picked"
        );
        assert!(
            picked.contains(&5),
            "null release stale at 2h must be picked"
        );
    }
}
