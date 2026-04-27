//! Bulk + incremental sync between kino and Trakt. See
//! `docs/subsystems/16-trakt.md` §§ 3-4.
//!
//! Entry points:
//!   - [`dry_run`] — fetch remote state, compare to local, return
//!     counts of what a full import *would* change. No writes.
//!   - [`import_all`] — execute the import the dry-run previewed.
//!   - [`incremental_sweep`] — called by the scheduler every 5 min;
//!     uses `/sync/last_activities` to re-pull only the categories
//!     that changed remotely.
//!   - [`push_watched`] — write a local Watched event back to Trakt
//!     (covers the spec's "history push on watched threshold").
//!   - [`push_rating`] — write a user-set rating to Trakt.
//!
//! All functions short-circuit cleanly when `trakt_sync_*` config
//! toggles are off, so toggling a feature in settings takes effect
//! without any cache invalidation.
//
// Sync logic mirrors Trakt's API surface, which is wide. Each top-
// level sync fn fetches + reconciles one category; combining them is
// long by design (all the schema shapes live together for grep-
// ability). Inline per-fn structs are deliberate — hoisting them to
// module level would scatter 10+ `FooRow`/`FooBody` types for no
// reader benefit.
#![allow(clippy::too_many_lines, clippy::items_after_statements)]

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::client::{TraktClient, TraktError};
use super::reconcile;
use super::types::{
    HistoryEntry, HistoryEpisodeNumber, HistoryPush, HistorySeason, HistoryShow, LastActivities,
    Movie, PlaybackProgress, RatingRow, Show, SyncResult, TraktIds, TrendingMovie, TrendingShow,
    WatchedMovie, WatchedShow, WatchlistRow,
};

/// Counts returned by [`dry_run`]. Frontend renders these in the
/// "Import from Trakt" modal so the user knows what'll change.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DryRunCounts {
    pub watched_movies: i64,
    pub watched_episodes: i64,
    pub rated_movies: i64,
    pub rated_shows: i64,
    pub rated_episodes: i64,
    pub watchlist_movies: i64,
    pub watchlist_shows: i64,
    /// Items in the Trakt data that don't match any local entity —
    /// reassures the user that their unknown-to-kino entries stay put.
    pub unmatched: i64,
}

/// What kind of toggle the caller wants honoured. Used by handlers
/// that want to force a full pull regardless of user preferences
/// (initial import button); scheduled sweeps pass `Config` to respect
/// per-feature toggles.
#[derive(Debug, Clone, Copy)]
pub enum Respect {
    /// Honour `trakt_sync_*` config toggles — used by the periodic sweep.
    Config,
    /// Force everything on — used by the "Import everything from Trakt"
    /// button in the initial-connect UX.
    All,
}

struct Toggles {
    watched: bool,
    ratings: bool,
    watchlist: bool,
}

async fn toggles(db: &SqlitePool, respect: Respect) -> Toggles {
    match respect {
        Respect::All => Toggles {
            watched: true,
            ratings: true,
            watchlist: true,
        },
        Respect::Config => {
            let row: Option<(bool, bool, bool)> = sqlx::query_as(
                "SELECT trakt_sync_watched, trakt_sync_ratings, trakt_sync_watchlist
                 FROM config WHERE id = 1",
            )
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
            // Fallback if the config row is missing entirely — matches
            // the migration's "all on" defaults.
            let (w, r, l) = row.unwrap_or((true, true, true));
            Toggles {
                watched: w,
                ratings: r,
                watchlist: l,
            }
        }
    }
}

/// Count what a full import would change, without writing anything.
pub async fn dry_run(client: &TraktClient) -> Result<DryRunCounts, TraktError> {
    let db = client.db();
    let mut c = DryRunCounts::default();

    let movies: Vec<WatchedMovie> = client.get("/sync/watched/movies").await?;
    for m in &movies {
        if let Some(mid) =
            reconcile::find_movie(db, &m.movie.ids, &m.movie.title, m.movie.year).await
        {
            let watched: Option<String> =
                sqlx::query_scalar("SELECT watched_at FROM movie WHERE id = ?")
                    .bind(mid)
                    .fetch_optional(db)
                    .await?
                    .flatten();
            if watched.is_none() {
                c.watched_movies += 1;
            }
        } else {
            c.unmatched += 1;
        }
    }

    let shows: Vec<WatchedShow> = client.get("/sync/watched/shows").await?;
    for s in &shows {
        let Some(show_id) = reconcile::find_show(db, &s.show.ids, &s.show.title, s.show.year).await
        else {
            c.unmatched += 1;
            continue;
        };
        for season in &s.seasons {
            for ep in &season.episodes {
                if let Some(eid) =
                    reconcile::find_episode(db, show_id, season.number, ep.number).await
                {
                    let watched: Option<String> =
                        sqlx::query_scalar("SELECT watched_at FROM episode WHERE id = ?")
                            .bind(eid)
                            .fetch_optional(db)
                            .await?
                            .flatten();
                    if watched.is_none() {
                        c.watched_episodes += 1;
                    }
                }
            }
        }
    }

    let movie_ratings: Vec<RatingRow> = client.get("/sync/ratings/movies").await?;
    for r in &movie_ratings {
        let Some(ref movie) = r.movie else { continue };
        if reconcile::find_movie(db, &movie.ids, &movie.title, movie.year)
            .await
            .is_some()
        {
            c.rated_movies += 1;
        }
    }

    let show_ratings: Vec<RatingRow> = client.get("/sync/ratings/shows").await?;
    for r in &show_ratings {
        let Some(ref show) = r.show else { continue };
        if reconcile::find_show(db, &show.ids, &show.title, show.year)
            .await
            .is_some()
        {
            c.rated_shows += 1;
        }
    }

    // Episode ratings — reuse the show-resolution cache we already
    // warmed, but the episode resolver needs the show id separately.
    // Episode ratings use the `RatingRow.show` context that Trakt
    // includes alongside the episode body (same shape `import_all`
    // relies on). Previously this branch was a `let _ = ...` stub,
    // so the dry-run preview under-counted; users saw "0 episode
    // ratings to import" then "87 imported" after clicking through.
    let ep_ratings: Vec<RatingRow> = client.get("/sync/ratings/episodes").await?;
    for r in &ep_ratings {
        let Some(ref show) = r.show else { continue };
        let Some(ref ep) = r.episode else { continue };
        let (Some(s_num), Some(e_num)) = (ep.season, ep.number) else {
            continue;
        };
        let Some(show_id) = reconcile::find_show(db, &show.ids, &show.title, show.year).await
        else {
            c.unmatched += 1;
            continue;
        };
        if reconcile::find_episode(db, show_id, s_num, e_num)
            .await
            .is_some()
        {
            c.rated_episodes += 1;
        }
    }

    let wl_movies: Vec<WatchlistRow> = client.get("/sync/watchlist/movies").await?;
    for w in &wl_movies {
        let Some(ref m) = w.movie else { continue };
        if reconcile::find_movie(db, &m.ids, &m.title, m.year)
            .await
            .is_some()
        {
            c.watchlist_movies += 1;
        }
    }

    let wl_shows: Vec<WatchlistRow> = client.get("/sync/watchlist/shows").await?;
    for w in &wl_shows {
        let Some(ref s) = w.show else { continue };
        if reconcile::find_show(db, &s.ids, &s.title, s.year)
            .await
            .is_some()
        {
            c.watchlist_shows += 1;
        }
    }

    Ok(c)
}

/// Pull everything the user asked for and apply it to the local DB.
/// Returns the same shape as `dry_run` but with the actual applied
/// counts (useful as a success toast in the UI).
pub async fn import_all(
    client: &TraktClient,
    respect: Respect,
) -> Result<DryRunCounts, TraktError> {
    let db = client.db();
    let t = toggles(db, respect).await;
    let mut c = DryRunCounts::default();

    if t.watched {
        let movies: Vec<WatchedMovie> = client.get("/sync/watched/movies").await?;
        for m in movies {
            let Some(mid) =
                reconcile::find_movie(db, &m.movie.ids, &m.movie.title, m.movie.year).await
            else {
                c.unmatched += 1;
                continue;
            };
            // Only write when we'd actually change state — otherwise
            // we'd bump `last_played_at` over a local fresher one.
            let current: Option<String> =
                sqlx::query_scalar("SELECT watched_at FROM movie WHERE id = ?")
                    .bind(mid)
                    .fetch_optional(db)
                    .await?
                    .flatten();
            if current.is_none() {
                sqlx::query(
                    "UPDATE movie SET
                        watched_at = ?,
                        play_count = MAX(play_count, ?),
                        last_played_at = COALESCE(last_played_at, ?)
                     WHERE id = ?",
                )
                .bind(&m.last_watched_at)
                .bind(m.plays.max(1))
                .bind(&m.last_watched_at)
                .bind(mid)
                .execute(db)
                .await?;
                c.watched_movies += 1;
            }
        }

        let shows: Vec<WatchedShow> = client.get("/sync/watched/shows").await?;
        for s in shows {
            let Some(show_id) =
                reconcile::find_show(db, &s.show.ids, &s.show.title, s.show.year).await
            else {
                c.unmatched += 1;
                continue;
            };
            for season in s.seasons {
                for ep in season.episodes {
                    let Some(eid) =
                        reconcile::find_episode(db, show_id, season.number, ep.number).await
                    else {
                        continue;
                    };
                    let current: Option<String> =
                        sqlx::query_scalar("SELECT watched_at FROM episode WHERE id = ?")
                            .bind(eid)
                            .fetch_optional(db)
                            .await?
                            .flatten();
                    if current.is_none() {
                        sqlx::query(
                            "UPDATE episode SET
                                watched_at = ?,
                                play_count = MAX(play_count, ?),
                                last_played_at = COALESCE(last_played_at, ?)
                             WHERE id = ?",
                        )
                        .bind(&ep.last_watched_at)
                        .bind(ep.plays.max(1))
                        .bind(&ep.last_watched_at)
                        .bind(eid)
                        .execute(db)
                        .await?;
                        c.watched_episodes += 1;
                    }
                }
            }
        }
    }

    if t.ratings {
        let movie_ratings: Vec<RatingRow> = client.get("/sync/ratings/movies").await?;
        for r in movie_ratings {
            let Some(ref m) = r.movie else { continue };
            if let Some(mid) = reconcile::find_movie(db, &m.ids, &m.title, m.year).await {
                sqlx::query("UPDATE movie SET user_rating = ? WHERE id = ?")
                    .bind(r.rating)
                    .bind(mid)
                    .execute(db)
                    .await?;
                c.rated_movies += 1;
            }
        }

        let show_ratings: Vec<RatingRow> = client.get("/sync/ratings/shows").await?;
        for r in show_ratings {
            let Some(ref s) = r.show else { continue };
            if let Some(sid) = reconcile::find_show(db, &s.ids, &s.title, s.year).await {
                sqlx::query("UPDATE show SET user_rating = ? WHERE id = ?")
                    .bind(r.rating)
                    .bind(sid)
                    .execute(db)
                    .await?;
                c.rated_shows += 1;
            }
        }

        // Episode ratings — the row's `show` field carries the parent
        // (Trakt's response wraps each ep in its show). We need both
        // to find the local episode row.
        let ep_ratings: Vec<RatingRow> = client.get("/sync/ratings/episodes").await?;
        for r in ep_ratings {
            let Some(ref s) = r.show else { continue };
            let Some(ref ep) = r.episode else { continue };
            let (Some(season), Some(number)) = (ep.season, ep.number) else {
                continue;
            };
            let Some(show_id) = reconcile::find_show(db, &s.ids, &s.title, s.year).await else {
                continue;
            };
            if let Some(eid) = reconcile::find_episode(db, show_id, season, number).await {
                sqlx::query("UPDATE episode SET user_rating = ? WHERE id = ?")
                    .bind(r.rating)
                    .bind(eid)
                    .execute(db)
                    .await?;
                c.rated_episodes += 1;
            }
        }
    }

    if t.watchlist {
        // Watchlist entries mark items as monitored so the scheduler
        // starts acquiring them. We don't auto-create movies the user
        // doesn't have in their library — that'd pull in hundreds of
        // TMDB requests on first import. Non-library watchlist items
        // are surfaced separately by the Lists subsystem (17), which
        // owns the inverse direction (poll watchlist as a list).
        let wl_movies: Vec<WatchlistRow> = client.get("/sync/watchlist/movies").await?;
        for w in wl_movies {
            let Some(ref m) = w.movie else { continue };
            if let Some(mid) = reconcile::find_movie(db, &m.ids, &m.title, m.year).await {
                sqlx::query("UPDATE movie SET monitored = 1 WHERE id = ?")
                    .bind(mid)
                    .execute(db)
                    .await?;
                c.watchlist_movies += 1;
            }
        }

        let wl_shows: Vec<WatchlistRow> = client.get("/sync/watchlist/shows").await?;
        for w in wl_shows {
            let Some(ref s) = w.show else { continue };
            if let Some(sid) = reconcile::find_show(db, &s.ids, &s.title, s.year).await {
                sqlx::query("UPDATE show SET monitored = 1 WHERE id = ?")
                    .bind(sid)
                    .execute(db)
                    .await?;
                // Re-enable acquire/in_scope on episodes that were
                // previously quieted (e.g. by a Play-auto-follow
                // that set every episode to acquire=0). Without this,
                // flipping `show.monitored = 1` alone leaves the
                // scheduler gated by `episode.acquire = 1` and
                // nothing actually starts downloading. Also nudge
                // `last_searched_at` clear for those episodes so the
                // next sweep picks them up immediately.
                sqlx::query(
                    "UPDATE episode SET
                        acquire = 1,
                        in_scope = 1,
                        last_searched_at = CASE WHEN acquire = 0 THEN NULL ELSE last_searched_at END
                     WHERE show_id = ? AND acquire = 0",
                )
                .bind(sid)
                .execute(db)
                .await?;
                c.watchlist_shows += 1;
            }
        }
    }

    // Stamp import-done so the scheduled incremental sweep knows to
    // diff from here on rather than reimport every hour.
    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO trakt_sync_state (id, initial_import_done, last_full_sync_at)
         VALUES (1, 1, ?)
         ON CONFLICT(id) DO UPDATE SET
            initial_import_done = 1,
            last_full_sync_at = excluded.last_full_sync_at",
    )
    .bind(&now)
    .execute(db)
    .await?;

    Ok(c)
}

/// Check `/sync/last_activities` against our stored watermarks;
/// re-fetch any bucket whose remote timestamp has moved. Called by
/// the `trakt_sync_incremental` scheduler task every ~5 minutes.
pub async fn incremental_sweep(client: &TraktClient) -> Result<(), TraktError> {
    if !super::is_connected(client.db()).await {
        return Ok(());
    }
    let remote: LastActivities = client.get("/sync/last_activities").await?;

    // Fetch stored watermarks in one query.
    #[derive(sqlx::FromRow, Default)]
    struct Local {
        last_watched_movies_at: Option<String>,
        last_watched_episodes_at: Option<String>,
        last_rated_movies_at: Option<String>,
        last_rated_shows_at: Option<String>,
        last_rated_episodes_at: Option<String>,
        last_watchlist_movies_at: Option<String>,
        last_watchlist_shows_at: Option<String>,
        last_playback_at: Option<String>,
    }
    let local: Local = sqlx::query_as(
        "SELECT last_watched_movies_at, last_watched_episodes_at,
                last_rated_movies_at, last_rated_shows_at, last_rated_episodes_at,
                last_watchlist_movies_at, last_watchlist_shows_at,
                last_playback_at
         FROM trakt_sync_state WHERE id = 1",
    )
    .fetch_optional(client.db())
    .await?
    .unwrap_or_default();

    // Simple string compare is correct: Trakt returns ISO 8601 UTC
    // with lexicographic ordering identical to chronological.
    let moved = |a: &Option<String>, b: &Option<String>| -> bool {
        match (a, b) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(ours), Some(theirs)) => theirs > ours,
        }
    };

    // Re-run import_all targeted at the moved buckets. For MVP we do
    // the simple thing: if any bucket moved, re-pull that section of
    // the full import. Smarter diff (only items newer than local
    // watermark) is a future optimisation.
    let t = toggles(client.db(), Respect::Config).await;
    if t.watched
        && (moved(&local.last_watched_movies_at, &remote.movies.watched_at)
            || moved(&local.last_watched_episodes_at, &remote.episodes.watched_at))
    {
        tracing::info!("trakt incremental: watched history moved, re-pulling");
        let _ = import_watched_only(client).await?;
    }
    if t.ratings
        && (moved(&local.last_rated_movies_at, &remote.movies.rated_at)
            || moved(&local.last_rated_shows_at, &remote.shows.rated_at)
            || moved(&local.last_rated_episodes_at, &remote.episodes.rated_at))
    {
        tracing::info!("trakt incremental: ratings moved, re-pulling");
        let _ = import_ratings_only(client).await?;
    }
    if t.watchlist
        && (moved(
            &local.last_watchlist_movies_at,
            &remote.movies.watchlisted_at,
        ) || moved(&local.last_watchlist_shows_at, &remote.shows.watchlisted_at))
    {
        tracing::info!("trakt incremental: watchlist moved, re-pulling");
        let _ = import_watchlist_only(client).await?;
        // Also refresh the Trakt-watchlist system list (subsystem 17).
        // The resolver pulls /sync/watchlist itself; one extra HTTP
        // call per watchlist-moved sweep. Best-effort.
        if let Err(e) = sync_watchlist_system_list(client.db()).await {
            tracing::warn!(error = %e, "trakt watchlist system-list sync failed");
        }
    }
    // Resume points: paused_at moves whenever any device reports a
    // pause. We pull /sync/playback (cheap; only paused items in
    // flight) and update local resume positions when Trakt's is newer.
    // Gated on `trakt_resume_sync_enabled` — users who play on
    // several devices but want to keep kino's resume positions
    // authoritative (e.g. local seek memory during an evening) can
    // disable this bucket without losing other sync buckets.
    let resume_enabled: bool =
        sqlx::query_scalar("SELECT trakt_resume_sync_enabled FROM config WHERE id = 1")
            .fetch_optional(client.db())
            .await
            .ok()
            .flatten()
            .unwrap_or(true);
    if resume_enabled
        && (moved(&local.last_playback_at, &remote.movies.paused_at)
            || moved(&local.last_playback_at, &remote.episodes.paused_at))
    {
        tracing::info!("trakt incremental: playback moved, pulling resume points");
        let _ = pull_playback(client).await;
    } else if !resume_enabled {
        tracing::debug!("trakt resume sync disabled — skipping /sync/playback pull this sweep");
    }

    // Persist the new watermarks + timestamp so we don't re-fetch
    // until the next Trakt change. paused_at is tracked as a single
    // watermark across movies/episodes — picking the later of the two
    // keeps the next sweep idempotent.
    let paused_at = match (&remote.movies.paused_at, &remote.episodes.paused_at) {
        (Some(a), Some(b)) => Some(if a >= b { a.clone() } else { b.clone() }),
        (Some(a), None) | (None, Some(a)) => Some(a.clone()),
        (None, None) => None,
    };
    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "UPDATE trakt_sync_state SET
            last_watched_movies_at   = COALESCE(?, last_watched_movies_at),
            last_watched_episodes_at = COALESCE(?, last_watched_episodes_at),
            last_rated_movies_at     = COALESCE(?, last_rated_movies_at),
            last_rated_shows_at      = COALESCE(?, last_rated_shows_at),
            last_rated_episodes_at   = COALESCE(?, last_rated_episodes_at),
            last_watchlist_movies_at = COALESCE(?, last_watchlist_movies_at),
            last_watchlist_shows_at  = COALESCE(?, last_watchlist_shows_at),
            last_playback_at         = COALESCE(?, last_playback_at),
            last_incremental_sync_at = ?
         WHERE id = 1",
    )
    .bind(&remote.movies.watched_at)
    .bind(&remote.episodes.watched_at)
    .bind(&remote.movies.rated_at)
    .bind(&remote.shows.rated_at)
    .bind(&remote.episodes.rated_at)
    .bind(&remote.movies.watchlisted_at)
    .bind(&remote.shows.watchlisted_at)
    .bind(&paused_at)
    .bind(&now)
    .execute(client.db())
    .await?;
    Ok(())
}

async fn import_watched_only(client: &TraktClient) -> Result<DryRunCounts, TraktError> {
    // Reuse import_all but stub the rating/watchlist toggles. A
    // dedicated path would be cleaner; reusing avoids maintaining
    // two near-identical loops.
    let saved: Option<(bool, bool, bool)> = sqlx::query_as(
        "SELECT trakt_sync_watched, trakt_sync_ratings, trakt_sync_watchlist
         FROM config WHERE id = 1",
    )
    .fetch_one(client.db())
    .await
    .ok();
    sqlx::query("UPDATE config SET trakt_sync_ratings = 0, trakt_sync_watchlist = 0 WHERE id = 1")
        .execute(client.db())
        .await?;
    let out = import_all(client, Respect::Config).await;
    if let Some((w, r, l)) = saved {
        sqlx::query(
            "UPDATE config SET trakt_sync_watched = ?, trakt_sync_ratings = ?, trakt_sync_watchlist = ? WHERE id = 1",
        )
        .bind(w)
        .bind(r)
        .bind(l)
        .execute(client.db())
        .await?;
    }
    out
}

async fn import_ratings_only(client: &TraktClient) -> Result<DryRunCounts, TraktError> {
    let saved: Option<(bool, bool, bool)> = sqlx::query_as(
        "SELECT trakt_sync_watched, trakt_sync_ratings, trakt_sync_watchlist
         FROM config WHERE id = 1",
    )
    .fetch_one(client.db())
    .await
    .ok();
    sqlx::query("UPDATE config SET trakt_sync_watched = 0, trakt_sync_watchlist = 0 WHERE id = 1")
        .execute(client.db())
        .await?;
    let out = import_all(client, Respect::Config).await;
    if let Some((w, r, l)) = saved {
        sqlx::query(
            "UPDATE config SET trakt_sync_watched = ?, trakt_sync_ratings = ?, trakt_sync_watchlist = ? WHERE id = 1",
        )
        .bind(w)
        .bind(r)
        .bind(l)
        .execute(client.db())
        .await?;
    }
    out
}

async fn import_watchlist_only(client: &TraktClient) -> Result<DryRunCounts, TraktError> {
    let saved: Option<(bool, bool, bool)> = sqlx::query_as(
        "SELECT trakt_sync_watched, trakt_sync_ratings, trakt_sync_watchlist
         FROM config WHERE id = 1",
    )
    .fetch_one(client.db())
    .await
    .ok();
    sqlx::query("UPDATE config SET trakt_sync_watched = 0, trakt_sync_ratings = 0 WHERE id = 1")
        .execute(client.db())
        .await?;
    let out = import_all(client, Respect::Config).await;
    if let Some((w, r, l)) = saved {
        sqlx::query(
            "UPDATE config SET trakt_sync_watched = ?, trakt_sync_ratings = ?, trakt_sync_watchlist = ? WHERE id = 1",
        )
        .bind(w)
        .bind(r)
        .bind(l)
        .execute(client.db())
        .await?;
    }
    out
}

// ── Outgoing writes ───────────────────────────────────────────────

/// Push a watched event to Trakt's history. Preferred over live
/// scrobble when we're back-filling — Trakt accepts `watched_at`
/// timestamps for anything up to years old. Called from the scrobble
/// queue drain + directly when the user marks something watched.
pub async fn push_watched(
    client: &TraktClient,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
    watched_at: Option<String>,
) -> Result<(), TraktError> {
    let enabled: bool = sqlx::query_scalar("SELECT trakt_sync_watched FROM config WHERE id = 1")
        .fetch_optional(client.db())
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(());
    }

    let mut push = HistoryPush::default();
    if let Some(mid) = movie_id {
        let ids = movie_trakt_ids(client.db(), mid).await?;
        push.movies.push(HistoryEntry {
            watched_at,
            item: Movie {
                title: String::new(),
                year: None,
                ids,
            },
        });
    } else if let Some(eid) = episode_id {
        let (show_ids, season, number) = episode_trakt_refs(client.db(), eid).await?;
        push.shows.push(HistoryShow {
            show: Show {
                title: String::new(),
                year: None,
                ids: show_ids,
            },
            seasons: vec![HistorySeason {
                number: season,
                episodes: vec![HistoryEntry {
                    watched_at,
                    item: HistoryEpisodeNumber { number },
                }],
            }],
        });
    }

    let _: SyncResult = client.post("/sync/history", &push).await?;
    Ok(())
}

/// Push a user rating to Trakt. Idempotent on Trakt's side (same
/// rating → no-op, different rating → overwrite).
pub async fn push_rating(
    client: &TraktClient,
    kind: RatingKind,
    id: i64,
    rating: i64,
) -> Result<(), TraktError> {
    let enabled: bool = sqlx::query_scalar("SELECT trakt_sync_ratings FROM config WHERE id = 1")
        .fetch_optional(client.db())
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(());
    }

    #[derive(Serialize)]
    struct RatingItem {
        rating: i64,
        #[serde(flatten)]
        ids: IdsOnly,
    }
    #[derive(Serialize)]
    struct IdsOnly {
        ids: TraktIds,
    }
    #[derive(Serialize, Default)]
    struct Body {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        movies: Vec<RatingItem>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        shows: Vec<RatingItem>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        episodes: Vec<RatingItem>,
    }

    let mut body = Body::default();
    match kind {
        RatingKind::Movie => {
            let ids = movie_trakt_ids(client.db(), id).await?;
            body.movies.push(RatingItem {
                rating,
                ids: IdsOnly { ids },
            });
        }
        RatingKind::Show => {
            let ids = show_trakt_ids(client.db(), id).await?;
            body.shows.push(RatingItem {
                rating,
                ids: IdsOnly { ids },
            });
        }
        RatingKind::Episode => {
            let ids = episode_trakt_ids_direct(client.db(), id).await?;
            body.episodes.push(RatingItem {
                rating,
                ids: IdsOnly { ids },
            });
        }
    }

    let _: SyncResult = client.post("/sync/ratings", &body).await?;
    Ok(())
}

/// Remove a user rating on Trakt. Symmetric to [`push_rating`] but
/// targets `/sync/ratings/remove`. The `ids` body shape doesn't
/// include `rating` (Trakt removes whatever's there).
pub async fn push_unrate(
    client: &TraktClient,
    kind: RatingKind,
    id: i64,
) -> Result<(), TraktError> {
    let enabled: bool = sqlx::query_scalar("SELECT trakt_sync_ratings FROM config WHERE id = 1")
        .fetch_optional(client.db())
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(());
    }
    #[derive(Serialize)]
    struct Item {
        ids: TraktIds,
    }
    #[derive(Serialize, Default)]
    struct Body {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        movies: Vec<Item>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        shows: Vec<Item>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        episodes: Vec<Item>,
    }
    let mut body = Body::default();
    match kind {
        RatingKind::Movie => body.movies.push(Item {
            ids: movie_trakt_ids(client.db(), id).await?,
        }),
        RatingKind::Show => body.shows.push(Item {
            ids: show_trakt_ids(client.db(), id).await?,
        }),
        RatingKind::Episode => body.episodes.push(Item {
            ids: episode_trakt_ids_direct(client.db(), id).await?,
        }),
    }
    let _: SyncResult = client.post("/sync/ratings/remove", &body).await?;
    Ok(())
}

/// Remove a watched event from Trakt's history. Mirror of
/// [`push_watched`]; called when the user un-marks something
/// watched locally. Removes ALL history entries for the item — Trakt's
/// `/sync/history/remove` doesn't take a timestamp filter, and "remove
/// every play of this episode" is what un-mark means anyway.
pub async fn push_unwatch(
    client: &TraktClient,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
) -> Result<(), TraktError> {
    let enabled: bool = sqlx::query_scalar("SELECT trakt_sync_watched FROM config WHERE id = 1")
        .fetch_optional(client.db())
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(());
    }

    #[derive(Serialize, Default)]
    struct Body {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        movies: Vec<Movie>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        shows: Vec<HistoryShow>,
    }
    let mut body = Body::default();
    if let Some(mid) = movie_id {
        body.movies.push(Movie {
            title: String::new(),
            year: None,
            ids: movie_trakt_ids(client.db(), mid).await?,
        });
    } else if let Some(eid) = episode_id {
        let (show_ids, season, number) = episode_trakt_refs(client.db(), eid).await?;
        body.shows.push(HistoryShow {
            show: Show {
                title: String::new(),
                year: None,
                ids: show_ids,
            },
            seasons: vec![HistorySeason {
                number: season,
                episodes: vec![HistoryEntry {
                    watched_at: None,
                    item: HistoryEpisodeNumber { number },
                }],
            }],
        });
    } else {
        return Ok(());
    }
    let _: SyncResult = client.post("/sync/history/remove", &body).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum WatchlistKind {
    Movie,
    Show,
}

/// Add an item to the Trakt watchlist. Called when the user toggles
/// a movie/show to monitored locally.
pub async fn push_watchlist_add(
    client: &TraktClient,
    kind: WatchlistKind,
    id: i64,
) -> Result<(), TraktError> {
    let enabled: bool = sqlx::query_scalar("SELECT trakt_sync_watchlist FROM config WHERE id = 1")
        .fetch_optional(client.db())
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(());
    }
    let body = watchlist_body(client.db(), kind, id).await?;
    let _: SyncResult = client.post("/sync/watchlist", &body).await?;
    Ok(())
}

/// Remove an item from the Trakt watchlist.
pub async fn push_watchlist_remove(
    client: &TraktClient,
    kind: WatchlistKind,
    id: i64,
) -> Result<(), TraktError> {
    let enabled: bool = sqlx::query_scalar("SELECT trakt_sync_watchlist FROM config WHERE id = 1")
        .fetch_optional(client.db())
        .await?
        .unwrap_or(true);
    if !enabled {
        return Ok(());
    }
    let body = watchlist_body(client.db(), kind, id).await?;
    let _: SyncResult = client.post("/sync/watchlist/remove", &body).await?;
    Ok(())
}

async fn watchlist_body(
    db: &SqlitePool,
    kind: WatchlistKind,
    id: i64,
) -> Result<WatchlistBody, TraktError> {
    let mut body = WatchlistBody::default();
    match kind {
        WatchlistKind::Movie => body.movies.push(Movie {
            title: String::new(),
            year: None,
            ids: movie_trakt_ids(db, id).await?,
        }),
        WatchlistKind::Show => body.shows.push(Show {
            title: String::new(),
            year: None,
            ids: show_trakt_ids(db, id).await?,
        }),
    }
    Ok(body)
}

#[derive(Serialize, Default)]
struct WatchlistBody {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    movies: Vec<Movie>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    shows: Vec<Show>,
}

#[derive(Debug, Clone, Copy)]
pub enum RatingKind {
    Movie,
    Show,
    Episode,
}

async fn movie_trakt_ids(db: &SqlitePool, movie_id: i64) -> Result<TraktIds, TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        tmdb_id: Option<i64>,
        imdb_id: Option<String>,
        tvdb_id: Option<i64>,
    }
    let row: Row = sqlx::query_as("SELECT tmdb_id, imdb_id, tvdb_id FROM movie WHERE id = ?")
        .bind(movie_id)
        .fetch_one(db)
        .await?;
    Ok(TraktIds {
        trakt: None,
        slug: None,
        imdb: row.imdb_id,
        tmdb: row.tmdb_id,
        tvdb: row.tvdb_id,
    })
}

async fn show_trakt_ids(db: &SqlitePool, show_id: i64) -> Result<TraktIds, TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        tmdb_id: Option<i64>,
        imdb_id: Option<String>,
        tvdb_id: Option<i64>,
    }
    let row: Row = sqlx::query_as("SELECT tmdb_id, imdb_id, tvdb_id FROM show WHERE id = ?")
        .bind(show_id)
        .fetch_one(db)
        .await?;
    Ok(TraktIds {
        trakt: None,
        slug: None,
        imdb: row.imdb_id,
        tmdb: row.tmdb_id,
        tvdb: row.tvdb_id,
    })
}

async fn episode_trakt_ids_direct(
    db: &SqlitePool,
    episode_id: i64,
) -> Result<TraktIds, TraktError> {
    // Episode identifiers: we rarely have Trakt-specific episode IDs;
    // only tmdb_id + tvdb_id are reliably populated. The push uses
    // `{show: ids, seasons: [{number, episodes: [{number}]}]}` form
    // instead; this helper is for the rarer case of rating a specific
    // episode without a show context.
    #[derive(sqlx::FromRow)]
    struct Row {
        tmdb_id: Option<i64>,
        tvdb_id: Option<i64>,
    }
    let row: Row = sqlx::query_as("SELECT tmdb_id, tvdb_id FROM episode WHERE id = ?")
        .bind(episode_id)
        .fetch_one(db)
        .await?;
    Ok(TraktIds {
        trakt: None,
        slug: None,
        imdb: None,
        tmdb: row.tmdb_id,
        tvdb: row.tvdb_id,
    })
}

async fn episode_trakt_refs(
    db: &SqlitePool,
    episode_id: i64,
) -> Result<(TraktIds, i64, i64), TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        show_id: i64,
        season_number: i64,
        episode_number: i64,
    }
    let row: Row =
        sqlx::query_as("SELECT show_id, season_number, episode_number FROM episode WHERE id = ?")
            .bind(episode_id)
            .fetch_one(db)
            .await?;
    let show_ids = show_trakt_ids(db, row.show_id).await?;
    Ok((show_ids, row.season_number, row.episode_number))
}

// ── Playback (cross-device resume) ───────────────────────────────

/// Pull resume points from `/sync/playback/{movies,episodes}` and
/// apply them locally when Trakt's `paused_at` is newer than what we
/// already have. Trakt stores progress as a percentage; we convert
/// against the local `runtime` (minutes) to libvlc-style 100ns ticks
/// the rest of the codebase uses.
pub async fn pull_playback(client: &TraktClient) -> Result<(), TraktError> {
    let db = client.db();
    let movies: Vec<PlaybackProgress> = client.get("/sync/playback/movies").await?;
    for p in movies {
        let Some(ref m) = p.movie else { continue };
        let Some(mid) = reconcile::find_movie(db, &m.ids, &m.title, m.year).await else {
            continue;
        };
        // Pull runtime (minutes) and any local paused timestamp so we
        // only overwrite when Trakt is fresher.
        #[derive(sqlx::FromRow)]
        struct Local {
            runtime: Option<i64>,
            last_played_at: Option<String>,
        }
        let local: Option<Local> =
            sqlx::query_as("SELECT runtime, last_played_at FROM movie WHERE id = ?")
                .bind(mid)
                .fetch_optional(db)
                .await?;
        let Some(local) = local else { continue };
        if let Some(ref ours) = local.last_played_at
            && ours.as_str() >= p.paused_at.as_str()
        {
            continue;
        }
        let Some(runtime_min) = local.runtime else {
            continue;
        };
        let ticks = pct_to_ticks(p.progress, runtime_min);
        sqlx::query(
            "UPDATE movie SET playback_position_ticks = ?, last_played_at = ? WHERE id = ?",
        )
        .bind(ticks)
        .bind(&p.paused_at)
        .bind(mid)
        .execute(db)
        .await?;
    }

    let episodes: Vec<PlaybackProgress> = client.get("/sync/playback/episodes").await?;
    for p in episodes {
        let Some(ref s) = p.show else { continue };
        let Some(ref ep) = p.episode else { continue };
        let (Some(season), Some(number)) = (ep.season, ep.number) else {
            continue;
        };
        let Some(show_id) = reconcile::find_show(db, &s.ids, &s.title, s.year).await else {
            continue;
        };
        let Some(eid) = reconcile::find_episode(db, show_id, season, number).await else {
            continue;
        };
        #[derive(sqlx::FromRow)]
        struct Local {
            runtime: Option<i64>,
            last_played_at: Option<String>,
        }
        let local: Option<Local> =
            sqlx::query_as("SELECT runtime, last_played_at FROM episode WHERE id = ?")
                .bind(eid)
                .fetch_optional(db)
                .await?;
        let Some(local) = local else { continue };
        if let Some(ref ours) = local.last_played_at
            && ours.as_str() >= p.paused_at.as_str()
        {
            continue;
        }
        let Some(runtime_min) = local.runtime else {
            continue;
        };
        let ticks = pct_to_ticks(p.progress, runtime_min);
        sqlx::query(
            "UPDATE episode SET playback_position_ticks = ?, last_played_at = ? WHERE id = ?",
        )
        .bind(ticks)
        .bind(&p.paused_at)
        .bind(eid)
        .execute(db)
        .await?;
    }
    Ok(())
}

// Push direction is already covered by the scrobble pipeline:
// `/scrobble/pause` (live) and the queue-drain backfill both update
// Trakt's `/sync/playback`, so a dedicated push here would duplicate
// work. Pull is the only direction this module handles.

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn pct_to_ticks(progress_pct: f64, runtime_min: i64) -> i64 {
    // Runtime is in minutes (typically <300, well within f64 mantissa);
    // ticks are libvlc-style 100ns units (i64 max ≈ 29k years).
    let runtime_sec = (runtime_min as f64) * 60.0;
    let played_sec = (progress_pct / 100.0) * runtime_sec;
    (played_sec * 10_000_000.0) as i64
}

// ── On-add: pull single-item Trakt state ─────────────────────────

/// Pull rating + watched + watchlist state for a single just-added
/// movie/show from Trakt. Closes the gap where deleting + re-adding
/// would orphan rows whose remote state hasn't moved since the last
/// incremental watermark. Best-effort: failure is logged, not fatal.
pub async fn pull_one_movie_state(client: &TraktClient, movie_id: i64) -> Result<(), TraktError> {
    let db = client.db();
    let t = toggles(db, Respect::Config).await;
    let Some(target) = movie_target_ids(db, movie_id).await? else {
        return Ok(());
    };

    if t.ratings {
        let rows: Vec<RatingRow> = client.get("/sync/ratings/movies").await?;
        for r in rows {
            let Some(ref m) = r.movie else { continue };
            if ids_match_movie(&m.ids, &target) {
                sqlx::query("UPDATE movie SET user_rating = ? WHERE id = ?")
                    .bind(r.rating)
                    .bind(movie_id)
                    .execute(db)
                    .await?;
                break;
            }
        }
    }
    if t.watched {
        let rows: Vec<WatchedMovie> = client.get("/sync/watched/movies").await?;
        for m in rows {
            if ids_match_movie(&m.movie.ids, &target) {
                sqlx::query(
                    "UPDATE movie SET
                        watched_at     = COALESCE(watched_at, ?),
                        play_count     = MAX(play_count, ?),
                        last_played_at = COALESCE(last_played_at, ?)
                     WHERE id = ?",
                )
                .bind(&m.last_watched_at)
                .bind(m.plays.max(1))
                .bind(&m.last_watched_at)
                .bind(movie_id)
                .execute(db)
                .await?;
                break;
            }
        }
    }
    if t.watchlist {
        let rows: Vec<WatchlistRow> = client.get("/sync/watchlist/movies").await?;
        for w in rows {
            let Some(ref m) = w.movie else { continue };
            if ids_match_movie(&m.ids, &target) {
                sqlx::query("UPDATE movie SET monitored = 1 WHERE id = ?")
                    .bind(movie_id)
                    .execute(db)
                    .await?;
                break;
            }
        }
    }
    Ok(())
}

pub async fn pull_one_show_state(client: &TraktClient, show_id: i64) -> Result<(), TraktError> {
    let db = client.db();
    let t = toggles(db, Respect::Config).await;
    let Some(target) = show_target_ids(db, show_id).await? else {
        return Ok(());
    };

    if t.ratings {
        let rows: Vec<RatingRow> = client.get("/sync/ratings/shows").await?;
        for r in rows {
            let Some(ref s) = r.show else { continue };
            if ids_match_show(&s.ids, &target) {
                sqlx::query("UPDATE show SET user_rating = ? WHERE id = ?")
                    .bind(r.rating)
                    .bind(show_id)
                    .execute(db)
                    .await?;
                break;
            }
        }
        // Episode ratings for this show — fetch the bucket once and
        // filter by show. Cost = one HTTP call per show add.
        let ep_rows: Vec<RatingRow> = client.get("/sync/ratings/episodes").await?;
        for r in ep_rows {
            let Some(ref s) = r.show else { continue };
            if !ids_match_show(&s.ids, &target) {
                continue;
            }
            let Some(ref ep) = r.episode else { continue };
            let (Some(season), Some(number)) = (ep.season, ep.number) else {
                continue;
            };
            if let Some(eid) = reconcile::find_episode(db, show_id, season, number).await {
                sqlx::query("UPDATE episode SET user_rating = ? WHERE id = ?")
                    .bind(r.rating)
                    .bind(eid)
                    .execute(db)
                    .await?;
            }
        }
    }
    if t.watched {
        let rows: Vec<WatchedShow> = client.get("/sync/watched/shows").await?;
        for s in rows {
            if !ids_match_show(&s.show.ids, &target) {
                continue;
            }
            for season in s.seasons {
                for ep in season.episodes {
                    if let Some(eid) =
                        reconcile::find_episode(db, show_id, season.number, ep.number).await
                    {
                        sqlx::query(
                            "UPDATE episode SET
                                watched_at     = COALESCE(watched_at, ?),
                                play_count     = MAX(play_count, ?),
                                last_played_at = COALESCE(last_played_at, ?)
                             WHERE id = ?",
                        )
                        .bind(&ep.last_watched_at)
                        .bind(ep.plays.max(1))
                        .bind(&ep.last_watched_at)
                        .bind(eid)
                        .execute(db)
                        .await?;
                    }
                }
            }
            break;
        }
    }
    if t.watchlist {
        let rows: Vec<WatchlistRow> = client.get("/sync/watchlist/shows").await?;
        for w in rows {
            let Some(ref s) = w.show else { continue };
            if ids_match_show(&s.ids, &target) {
                sqlx::query("UPDATE show SET monitored = 1 WHERE id = ?")
                    .bind(show_id)
                    .execute(db)
                    .await?;
                break;
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct TargetIds {
    tmdb: Option<i64>,
    imdb: Option<String>,
    tvdb: Option<i64>,
}

fn ids_match_movie(remote: &TraktIds, target: &TargetIds) -> bool {
    (target.tmdb.is_some() && remote.tmdb == target.tmdb)
        || (target.imdb.is_some() && remote.imdb == target.imdb)
}

fn ids_match_show(remote: &TraktIds, target: &TargetIds) -> bool {
    (target.tmdb.is_some() && remote.tmdb == target.tmdb)
        || (target.imdb.is_some() && remote.imdb == target.imdb)
        || (target.tvdb.is_some() && remote.tvdb == target.tvdb)
}

async fn movie_target_ids(db: &SqlitePool, movie_id: i64) -> Result<Option<TargetIds>, TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        tmdb_id: Option<i64>,
        imdb_id: Option<String>,
        tvdb_id: Option<i64>,
    }
    let row: Option<Row> =
        sqlx::query_as("SELECT tmdb_id, imdb_id, tvdb_id FROM movie WHERE id = ?")
            .bind(movie_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|r| TargetIds {
        tmdb: r.tmdb_id,
        imdb: r.imdb_id,
        tvdb: r.tvdb_id,
    }))
}

async fn show_target_ids(db: &SqlitePool, show_id: i64) -> Result<Option<TargetIds>, TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        tmdb_id: Option<i64>,
        imdb_id: Option<String>,
        tvdb_id: Option<i64>,
    }
    let row: Option<Row> =
        sqlx::query_as("SELECT tmdb_id, imdb_id, tvdb_id FROM show WHERE id = ?")
            .bind(show_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|r| TargetIds {
        tmdb: r.tmdb_id,
        imdb: r.imdb_id,
        tvdb: r.tvdb_id,
    }))
}

/// Pull `/sync/watchlist` and apply the items to the system list
/// row (subsystem 17). Called from `incremental_sweep` when Trakt's
/// `watchlisted_at` watermark advances. No-op when the system list
/// row doesn't exist (Trakt isn't connected, or auto-create hasn't
/// fired yet).
async fn sync_watchlist_system_list(db: &SqlitePool) -> Result<(), super::client::TraktError> {
    let list_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM list WHERE source_type = 'trakt_watchlist' AND is_system = 1 LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    let Some(list_id) = list_id else {
        return Ok(());
    };
    let items = crate::integrations::lists::trakt_list::fetch_watchlist_items(db)
        .await
        .map_err(|e| super::client::TraktError::Other(e.to_string()))?;
    crate::integrations::lists::sync::apply_poll(db, list_id, items)
        .await
        .map_err(|e| super::client::TraktError::Other(e.to_string()))?;
    Ok(())
}

// ── Collection push ──────────────────────────────────────────────

/// Push a freshly-imported movie or episode to Trakt's collection.
/// Called from the event listener on `AppEvent::Imported`. No-op
/// when `trakt_sync_collection` is off or Trakt is disconnected.
pub async fn push_collection_imported(
    client: &TraktClient,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
) -> Result<(), TraktError> {
    if !collection_enabled(client.db()).await {
        return Ok(());
    }
    #[derive(Serialize, Default)]
    struct Body {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        movies: Vec<Movie>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        shows: Vec<HistoryShow>,
    }
    let mut body = Body::default();
    if let Some(mid) = movie_id {
        let ids = movie_trakt_ids(client.db(), mid).await?;
        body.movies.push(Movie {
            title: String::new(),
            year: None,
            ids,
        });
    } else if let Some(eid) = episode_id {
        let (show_ids, season, number) = episode_trakt_refs(client.db(), eid).await?;
        body.shows.push(HistoryShow {
            show: Show {
                title: String::new(),
                year: None,
                ids: show_ids,
            },
            seasons: vec![HistorySeason {
                number: season,
                episodes: vec![HistoryEntry {
                    watched_at: None,
                    item: HistoryEpisodeNumber { number },
                }],
            }],
        });
    } else {
        return Ok(());
    }
    let _: SyncResult = client.post("/sync/collection", &body).await?;
    Ok(())
}

/// Remove a movie or whole show from Trakt's collection on
/// `AppEvent::ContentRemoved`. Whole-show removal cascades to all
/// episodes server-side — Trakt accepts `{shows: [{ids}]}` without
/// per-season detail and treats it as "remove everything we have
/// for this show in collection."
pub async fn push_collection_removed(
    client: &TraktClient,
    movie_id: Option<i64>,
    show_id: Option<i64>,
) -> Result<(), TraktError> {
    if !collection_enabled(client.db()).await {
        return Ok(());
    }
    #[derive(Serialize, Default)]
    struct Body {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        movies: Vec<Movie>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        shows: Vec<Show>,
    }
    let mut body = Body::default();
    if let Some(mid) = movie_id {
        let ids = movie_trakt_ids(client.db(), mid).await?;
        body.movies.push(Movie {
            title: String::new(),
            year: None,
            ids,
        });
    } else if let Some(sid) = show_id {
        let ids = show_trakt_ids(client.db(), sid).await?;
        body.shows.push(Show {
            title: String::new(),
            year: None,
            ids,
        });
    } else {
        return Ok(());
    }
    let _: SyncResult = client.post("/sync/collection/remove", &body).await?;
    Ok(())
}

async fn collection_enabled(db: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT trakt_sync_collection FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
}

// ── Home-row caches ───────────────────────────────────────────────

/// Refresh the recommendations + trending caches. Runs daily.
pub async fn refresh_home_caches(client: &TraktClient) -> Result<(), TraktError> {
    if !super::is_connected(client.db()).await {
        return Ok(());
    }
    // Per-bucket opt-out — users who use Kino as just a scrobble
    // client often don't want Trakt recommendations influencing their
    // Home page. Skip the `/recommendations/*` fetch when disabled;
    // trending (public, non-personalised) still populates so the row
    // isn't empty.
    let recs_enabled: bool =
        sqlx::query_scalar("SELECT trakt_recommendations_enabled FROM config WHERE id = 1")
            .fetch_optional(client.db())
            .await
            .ok()
            .flatten()
            .unwrap_or(true);
    let (recs_movies, recs_shows): (Vec<Movie>, Vec<Show>) = if recs_enabled {
        let movies = client
            .get("/recommendations/movies?limit=20")
            .await
            .unwrap_or_default();
        let shows = client
            .get("/recommendations/shows?limit=20")
            .await
            .unwrap_or_default();
        (movies, shows)
    } else {
        tracing::debug!(
            "trakt recommendations disabled in config — skipping fetch, caching empty lists"
        );
        (Vec::new(), Vec::new())
    };
    let trending_movies: Vec<TrendingMovie> = client
        .get_public("/movies/trending?limit=20")
        .await
        .unwrap_or_default();
    let trending_shows: Vec<TrendingShow> = client
        .get_public("/shows/trending?limit=20")
        .await
        .unwrap_or_default();

    #[derive(Serialize)]
    struct RecsPayload {
        movies: Vec<Movie>,
        shows: Vec<Show>,
    }
    #[derive(Serialize)]
    struct TrendingPayload {
        movies: Vec<TrendingMovie>,
        shows: Vec<TrendingShow>,
    }

    let recs = serde_json::to_string(&RecsPayload {
        movies: recs_movies,
        shows: recs_shows,
    })?;
    let trending = serde_json::to_string(&TrendingPayload {
        movies: trending_movies,
        shows: trending_shows,
    })?;
    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO trakt_sync_state (id, recommendations_cached_at, recommendations_json, trending_cached_at, trending_json)
         VALUES (1, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            recommendations_cached_at = excluded.recommendations_cached_at,
            recommendations_json      = excluded.recommendations_json,
            trending_cached_at        = excluded.trending_cached_at,
            trending_json             = excluded.trending_json",
    )
    .bind(&now)
    .bind(&recs)
    .bind(&now)
    .bind(&trending)
    .execute(client.db())
    .await?;
    Ok(())
}
