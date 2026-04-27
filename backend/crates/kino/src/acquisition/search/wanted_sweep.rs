//! Wanted + upgrade sweep — the scheduler's periodic
//! `wanted_search` task. Enumerates eligible movies and episodes
//! (tiered backoff per `last_searched_at`) and dispatches each to
//! `super::search_movie` / `super::search_episode`. Whether each
//! item is a first-time search or an upgrade-mode search is decided
//! per-item inside the search functions; the sweep only picks who
//! gets searched.

use crate::state::AppState;

/// Periodic wanted + upgrade search with tiered backoff.
///
/// Eligibility per pass:
///   - Movies never searched (`last_searched_at` IS NULL): always.
///   - Movies `wanted`: backoff by age of first-seen (`added_at)`: newer than
///     7 days → every 24h; 7–30 days → every 7d; older → every 30d.
///   - Movies `available` with auto-upgrade enabled: check every 7 days
///     (upgrade candidates are rarer than first-search candidates).
#[allow(clippy::too_many_lines)]
pub async fn wanted_search_sweep(state: &AppState) -> anyhow::Result<()> {
    let pool = &state.db;

    // No enabled indexers means every search returns zero releases,
    // and we'd stamp `last_searched_at` against that empty result.
    // That locks every wanted movie/episode into the backoff tier
    // until the cooldown expires — even after the user adds an
    // indexer. Bail before touching a thing so the first real sweep
    // after indexer-configure sees pristine `last_searched_at`s.
    let enabled_indexers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM indexer WHERE enabled = 1")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    if enabled_indexers == 0 {
        tracing::debug!("wanted_search_sweep: no enabled indexers, skipping");
        return Ok(());
    }

    // Read auto_upgrade_enabled from config (default on).
    let auto_upgrade: bool =
        sqlx::query_scalar::<_, bool>("SELECT auto_upgrade_enabled FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or(true);

    // Eligibility SQL: combines "never searched" (NULL) + the three
    // wanted-age tiers + upgrade tier. All timestamps are RFC3339.
    // Wanted = monitored + not watched + no media + no active download.
    let wanted_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT mv.id FROM movie mv
         WHERE mv.monitored = 1
           AND mv.watched_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM media m WHERE m.movie_id = mv.id)
           AND NOT EXISTS (
             SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
             WHERE dc.movie_id = mv.id
               AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing'))
           AND (
             mv.last_searched_at IS NULL
             OR (mv.added_at > datetime('now', '-7 days')
                 AND mv.last_searched_at < datetime('now', '-1 day'))
             OR (mv.added_at BETWEEN datetime('now', '-30 days') AND datetime('now', '-7 days')
                 AND mv.last_searched_at < datetime('now', '-7 days'))
             OR (mv.added_at < datetime('now', '-30 days')
                 AND mv.last_searched_at < datetime('now', '-30 days'))
           )",
    )
    .fetch_all(pool)
    .await?;

    // Upgrade candidates: monitored + has media + not watched — the
    // user already has something, we might find something better.
    let upgrade_ids: Vec<i64> = if auto_upgrade {
        sqlx::query_scalar(
            "SELECT mv.id FROM movie mv
             WHERE mv.monitored = 1
               AND mv.watched_at IS NULL
               AND EXISTS (SELECT 1 FROM media m WHERE m.movie_id = mv.id)
               AND (mv.last_searched_at IS NULL OR mv.last_searched_at < datetime('now', '-7 days'))",
        )
        .fetch_all(pool)
        .await?
    } else {
        Vec::new()
    };

    if !wanted_ids.is_empty() || !upgrade_ids.is_empty() {
        tracing::info!(
            wanted = wanted_ids.len(),
            upgrade = upgrade_ids.len(),
            "starting search sweep"
        );
        for id in wanted_ids.into_iter().chain(upgrade_ids) {
            if let Err(e) = super::movie::search_movie(state, id).await {
                tracing::error!(movie_id = id, error = %e, "search failed for movie");
            }
        }
    }
    // NB: no early `return` here — episodes use their own query
    // below, and a library with shows but no movies was previously
    // hanging at "wanted" because this function bailed before ever
    // looking at the episode table.

    // Wanted episodes — same tiered eligibility as movies. Only
    // consider episodes whose air date has passed (or air_date is
    // null, which covers missing TMDB data we still want to search
    // for). We skip future episodes so pre-order-style additions
    // don't hammer indexers for content that doesn't yet exist.
    // Wanted = acquire=1 + aired + not watched + no media + no active
    // download. Replaces the old `status = 'wanted'` gate; status is
    // now derived on read, not persisted.
    // Ordered earliest-first within each show: S01E01 → S01E02 → …
    // so a bulk "all seasons" follow starts at the pilot rather than
    // whatever the DB happens to return. Across shows it's stable but
    // arbitrary — show_id is the tiebreaker.
    let wanted_episodes: Vec<i64> = sqlx::query_scalar(
        "SELECT e.id FROM episode e
         JOIN show s ON s.id = e.show_id
         WHERE e.acquire = 1 AND s.monitored = 1
           AND e.watched_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
           AND NOT EXISTS (
             SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
             WHERE dc.episode_id = e.id
               AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing'))
           AND (e.air_date_utc IS NULL OR e.air_date_utc <= datetime('now'))
           AND (
             e.last_searched_at IS NULL
             OR (e.air_date_utc > datetime('now', '-14 days')
                 AND e.last_searched_at < datetime('now', '-1 day'))
             OR (e.air_date_utc BETWEEN datetime('now', '-60 days') AND datetime('now', '-14 days')
                 AND e.last_searched_at < datetime('now', '-7 days'))
             OR (e.air_date_utc < datetime('now', '-60 days')
                 AND e.last_searched_at < datetime('now', '-30 days'))
           )
         ORDER BY e.show_id ASC, e.season_number ASC, e.episode_number ASC",
    )
    .fetch_all(pool)
    .await?;

    // Episode upgrade candidates: monitored show + episode has media +
    // not watched + last searched > 7 days ago. Mirrors the movie
    // upgrade cadence. Skipped entirely when auto_upgrade is off.
    let upgrade_episodes: Vec<i64> = if auto_upgrade {
        sqlx::query_scalar(
            "SELECT e.id FROM episode e
             JOIN show s ON s.id = e.show_id
             WHERE s.monitored = 1
               AND e.watched_at IS NULL
               AND EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
               AND (e.last_searched_at IS NULL OR e.last_searched_at < datetime('now', '-7 days'))
             ORDER BY e.show_id ASC, e.season_number ASC, e.episode_number ASC",
        )
        .fetch_all(pool)
        .await?
    } else {
        Vec::new()
    };

    if !wanted_episodes.is_empty() || !upgrade_episodes.is_empty() {
        tracing::info!(
            wanted = wanted_episodes.len(),
            upgrade = upgrade_episodes.len(),
            "starting episode search sweep"
        );
        for id in wanted_episodes.into_iter().chain(upgrade_episodes) {
            if let Err(e) = super::episode::search_episode(state, id).await {
                tracing::error!(episode_id = id, error = %e, "search failed for episode");
            }
        }
    }

    Ok(())
}
