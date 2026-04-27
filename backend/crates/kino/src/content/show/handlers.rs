use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::content::show::episode::Episode;
use crate::content::show::model::{
    ActiveShowDownload, CreateShow, NextEpisode, Show, ShowListItem,
};
use crate::content::show::series::Series;
use crate::error::{AppError, AppResult};
use crate::models::enums::MonitorNewItems;
use crate::pagination::{Cursor, PaginatedResponse, PaginationParams};
use crate::settings::quality_profile::resolve_quality_profile;
use crate::state::AppState;

/// List shows (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/shows",
    params(PaginationParams),
    responses((
        status = 200,
        body = PaginatedResponse<ShowListItem>,
        description = "Paginated show list"
    )),
    tag = "shows",
    security(("api_key" = []))
)]
pub async fn list_shows(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> AppResult<Json<PaginatedResponse<ShowListItem>>> {
    let limit = params.limit();
    let fetch_limit = limit + 1;
    let cursor = params.cursor.as_deref().and_then(Cursor::decode);

    // Join in per-show rollups in a single query so the Library
    // page doesn't need N+1 round-trips to figure out which shows
    // belong in the Wanted tab. Correlated subqueries are cheap at
    // our scale (SQLite, library in the low thousands).
    // Wanted = monitored + aired + not watched + no media + no active
    // download. The air-date gate matches `wanted_search_sweep`'s
    // eligibility — an episode that hasn't aired yet isn't *actually*
    // being searched, so counting it here makes the Wanted tab look
    // permanently busy for users following shows mid-season.
    let base = "SELECT s.*,
         (SELECT COUNT(*) FROM episode e
          WHERE e.show_id = s.id
            AND e.acquire = 1
            AND e.watched_at IS NULL
            AND (e.air_date_utc IS NULL OR e.air_date_utc <= datetime('now'))
            AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
            AND NOT EXISTS (
              SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
              WHERE dc.episode_id = e.id
                AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
            )
         ) as wanted_episode_count,
         -- Episodes that would be wanted except air date is future.
         -- Separate count lets the UI tag upcoming without implying
         -- search activity (see wanted_episode_count above).
         (SELECT COUNT(*) FROM episode e
          WHERE e.show_id = s.id
            AND e.acquire = 1
            AND e.watched_at IS NULL
            AND e.air_date_utc IS NOT NULL
            AND e.air_date_utc > datetime('now')
            AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
         ) as upcoming_episode_count,
         (SELECT COUNT(*) FROM episode e
          WHERE e.show_id = s.id AND e.watched_at IS NOT NULL) as watched_episode_count,
         (SELECT COUNT(*) FROM episode e
          WHERE e.show_id = s.id AND e.in_scope = 1) as episode_count,
         (SELECT COUNT(DISTINCT me.episode_id) FROM media_episode me
          JOIN episode e ON e.id = me.episode_id
          WHERE e.show_id = s.id) as available_episode_count,
         (SELECT COUNT(*) FROM episode e
          WHERE e.show_id = s.id AND e.season_number >= 1
            AND (e.air_date_utc IS NULL OR e.air_date_utc <= datetime('now'))
         ) as aired_episode_count
         FROM show s";
    // Filter `partial = 0` so a follow that crashed mid-fanout
    // doesn't surface a show with no seasons in the list.
    let mut shows = if let Some(c) = cursor {
        let sql = format!("{base} WHERE s.partial = 0 AND s.id > ? ORDER BY s.id ASC LIMIT ?");
        sqlx::query_as::<_, ShowListItem>(&sql)
            .bind(c.id)
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!("{base} WHERE s.partial = 0 ORDER BY s.id ASC LIMIT ?");
        sqlx::query_as::<_, ShowListItem>(&sql)
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    };

    // Enrich each row with next_episode + active_download. Two cheap
    // batched queries — much easier to read than stuffing this into
    // the rollup select via more CTEs, and the show list rarely
    // exceeds the page size (50). See model comments for the
    // selection rules.
    enrich_with_next_and_active_download(&state, &mut shows).await?;

    Ok(Json(PaginatedResponse::new(shows, limit, |s| Cursor {
        id: s.show.id,
        sort_value: None,
    })))
}

/// Row shapes for the two enrichment queries — one row per show at
/// most (the SQL uses `ROW_NUMBER() ... WHERE rn = 1`).
#[derive(sqlx::FromRow)]
struct NextEpRow {
    show_id: i64,
    episode_id: i64,
    season_number: i64,
    episode_number: i64,
    title: Option<String>,
    available: i64,
}

#[derive(sqlx::FromRow)]
struct ShowActiveCountRow {
    show_id: i64,
    active_count: i64,
}

#[derive(sqlx::FromRow)]
struct ActiveDlRow {
    show_id: i64,
    download_id: i64,
    episode_id: i64,
    season_number: i64,
    episode_number: i64,
    state: String,
    downloaded: i64,
    total_size: Option<i64>,
    download_speed: i64,
}

/// Populate `next_episode` + `active_download` on each `ShowListItem`
/// via two batched queries against the `episode` and `download` tables.
/// Separated out so the main list-shows SQL stays readable.
#[allow(clippy::too_many_lines)] // single linear data pipeline, splitting scatters it
async fn enrich_with_next_and_active_download(
    state: &AppState,
    shows: &mut [ShowListItem],
) -> AppResult<()> {
    if shows.is_empty() {
        return Ok(());
    }
    let show_ids: Vec<i64> = shows.iter().map(|s| s.show.id).collect();
    let placeholders: String = show_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    // Next episode per show. Picks the earliest unwatched *aired*
    // episode, preferring already-imported ones so that Play opens an
    // instant watch when the library has something to offer. Caught-up
    // shows (no unwatched episodes) return no row — the card will
    // render without a Next-up hint (Play still works, falling through
    // to the resolver's tier-3 replay branch).
    let next_sql = format!(
        "WITH ranked AS (
           SELECT e.show_id, e.id AS episode_id, e.season_number, e.episode_number, e.title,
                  CASE WHEN EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
                       THEN 1 ELSE 0 END AS available,
                  ROW_NUMBER() OVER (
                    PARTITION BY e.show_id
                    ORDER BY
                      CASE WHEN EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
                           THEN 0 ELSE 1 END,
                      e.season_number, e.episode_number
                  ) AS rn
           FROM episode e
           WHERE e.show_id IN ({placeholders})
             AND e.season_number >= 1
             AND e.watched_at IS NULL
             AND (e.air_date_utc IS NULL OR e.air_date_utc <= datetime('now'))
         )
         SELECT show_id, episode_id, season_number, episode_number, title, available
         FROM ranked WHERE rn = 1"
    );
    let mut next_q = sqlx::query_as::<_, NextEpRow>(&next_sql);
    for id in &show_ids {
        next_q = next_q.bind(id);
    }
    let next_rows = next_q.fetch_all(&state.db).await?;

    // Active download per show. Rank by state first so the projection
    // reflects what's *happening*: anything actually moving wins over
    // paused, which wins over importing. Without this tier, an earlier-
    // aired paused torrent would hide a later episode that the user
    // individually resumed — the card would show "paused" even though
    // something's downloading. Ties broken by episode order so a batch
    // grab of S01E01+S01E02 still surfaces S01E01 when both are moving.
    let dl_sql = format!(
        "WITH ranked AS (
           SELECT e.show_id, d.id AS download_id, e.id AS episode_id,
                  e.season_number, e.episode_number,
                  d.state, d.downloaded, d.size AS total_size, d.download_speed,
                  ROW_NUMBER() OVER (
                    PARTITION BY e.show_id
                    ORDER BY
                      CASE d.state
                        WHEN 'downloading' THEN 0
                        WHEN 'stalled' THEN 0
                        WHEN 'grabbing' THEN 1
                        WHEN 'queued' THEN 1
                        WHEN 'paused' THEN 2
                        WHEN 'importing' THEN 3
                        ELSE 9
                      END,
                      e.season_number, e.episode_number
                  ) AS rn
           FROM download d
           JOIN download_content dc ON dc.download_id = d.id
           JOIN episode e ON e.id = dc.episode_id
           WHERE e.show_id IN ({placeholders})
             AND d.state IN ('queued','grabbing','downloading','paused','stalled','importing')
         )
         SELECT show_id, download_id, episode_id, season_number, episode_number,
                state, downloaded, total_size, download_speed
         FROM ranked WHERE rn = 1"
    );
    let mut dl_q = sqlx::query_as::<_, ActiveDlRow>(&dl_sql);
    for id in &show_ids {
        dl_q = dl_q.bind(id);
    }
    let dl_rows = dl_q.fetch_all(&state.db).await?;

    // Per-show active download count. Separate query because SQLite
    // doesn't allow `DISTINCT` inside window functions, and a naive
    // `COUNT(*) OVER (...)` would over-count season packs (one
    // `download` ↔ N `download_content` rows).
    let count_sql = format!(
        "SELECT e.show_id, COUNT(DISTINCT d.id) AS active_count
         FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         JOIN episode e ON e.id = dc.episode_id
         WHERE e.show_id IN ({placeholders})
           AND d.state IN ('queued','grabbing','downloading','paused','stalled','importing')
         GROUP BY e.show_id"
    );
    let mut count_q = sqlx::query_as::<_, ShowActiveCountRow>(&count_sql);
    for id in &show_ids {
        count_q = count_q.bind(id);
    }
    let count_rows = count_q.fetch_all(&state.db).await?;

    for show in shows.iter_mut() {
        show.next_episode = next_rows
            .iter()
            .find(|r| r.show_id == show.show.id)
            .map(|r| NextEpisode {
                episode_id: r.episode_id,
                season_number: r.season_number,
                episode_number: r.episode_number,
                title: r.title.clone(),
                available: r.available != 0,
            });
        show.active_download =
            dl_rows
                .iter()
                .find(|r| r.show_id == show.show.id)
                .map(|r| ActiveShowDownload {
                    download_id: r.download_id,
                    episode_id: r.episode_id,
                    season_number: r.season_number,
                    episode_number: r.episode_number,
                    state: r.state.clone(),
                    downloaded: r.downloaded,
                    total_size: r.total_size,
                    download_speed: r.download_speed,
                    active_count: count_rows
                        .iter()
                        .find(|c| c.show_id == show.show.id)
                        .map_or(1, |c| c.active_count),
                });
    }
    Ok(())
}

/// Get a show by ID.
#[utoipa::path(
    get,
    path = "/api/v1/shows/{id}",
    params(("id" = i64, Path, description = "Show ID")),
    responses(
        (status = 200, description = "Show details", body = Show),
        (status = 404, description = "Not found")
    ),
    tag = "shows",
    security(("api_key" = []))
)]
pub async fn get_show(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<Show>> {
    // partial = 0: a mid-fanout show 404s for the user even though the
    // row exists. The reconcile loop completes the fanout (or removes
    // it after exhaustion) before it becomes visible.
    let show = sqlx::query_as::<_, Show>("SELECT * FROM show WHERE id = ? AND partial = 0")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("show with id {id} not found")))?;
    Ok(Json(show))
}

/// Internal helper: create a show row + all seasons/episodes + emit
/// `ShowAdded`. Same semantic split as [`crate::content::movie::handlers::create_movie_inner`]:
///
///   - The `ShowAdded` event is a fact about the world — emitted
///     here so History / webhooks / WS library caches always see
///     the add.
///   - The `wanted_search` scheduler trigger is a scheduling
///     decision that belongs on the outer handler; watch-now's
///     `find_or_create_episode` / `watch_now_show_smart` call this
///     helper directly to skip that trigger and avoid racing the
///     scheduler ahead of their own inline search.
#[allow(clippy::too_many_lines)]
pub(crate) async fn create_show_inner(state: &AppState, input: CreateShow) -> AppResult<Show> {
    // Check if show already exists
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM show WHERE tmdb_id = ?")
        .bind(input.tmdb_id)
        .fetch_optional(&state.db)
        .await?;

    if let Some(id) = existing {
        return Err(AppError::Conflict(format!(
            "show with tmdb_id {} already exists (id={id})",
            input.tmdb_id
        )));
    }

    let profile_id = resolve_quality_profile(&state.db, input.quality_profile_id).await?;

    let tmdb = state.require_tmdb()?;
    let details = tmdb
        .show_details(input.tmdb_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let now = crate::time::Timestamp::now().to_rfc3339();
    let year = details
        .first_air_date
        .as_deref()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<i64>().ok());

    let genres = details.genres.as_ref().map(|g| {
        serde_json::to_string(&g.iter().map(|x| &x.name).collect::<Vec<_>>()).unwrap_or_default()
    });

    let network = details
        .networks
        .as_ref()
        .and_then(|n| n.first())
        .map(|n| n.name.clone());

    let certification = details.content_ratings.as_ref().and_then(|cr| {
        cr.results
            .iter()
            .find(|r| r.iso_3166_1 == "US")
            .map(|r| r.rating.clone())
    });

    let runtime = details
        .episode_run_time
        .as_ref()
        .and_then(|r| r.first().copied());

    let trailer = details.videos.as_ref().and_then(|v| {
        v.results
            .iter()
            .find(|t| t.site == "YouTube" && t.video_type == "Trailer")
            .map(|t| t.key.clone())
    });

    let imdb_id = details
        .external_ids
        .as_ref()
        .and_then(|e| e.imdb_id.clone());
    let tvdb_id = details.external_ids.as_ref().and_then(|e| e.tvdb_id);

    let monitored = input.monitored.unwrap_or(true);
    // Policy defaults for `monitor_new_items` when the caller didn't
    // specify one. Kept here (not in the frontend) so CLI / companion
    // apps / tests that omit the field get the same sensible behaviour
    // as the Follow dialog: default to "future" regardless of seasons
    // selection. The old "all" default (used only for bulk Follow with
    // seasons_to_monitor=None) was a footgun — it auto-grabbed any
    // back-dated episodes TMDB later added, which almost always meant
    // surprise-downloads of specials the user never asked for. Callers
    // that genuinely want "all" behaviour can still pass it explicitly;
    // legacy rows already stored as "all" remain valid (see
    // `seed_acquire_in_scope`).
    let monitor_new = input.monitor_new_items.as_deref().unwrap_or("future");
    // Specials (Season 0) are opt-in — most users don't want weekly
    // shorts for shows like The Boys clogging Next Up / calendar.
    // Default false; users tick a checkbox in the Follow dialog to
    // include them.
    let monitor_specials = i64::from(input.monitor_specials.unwrap_or(false));
    // Default intent is `explicit` — this function is called from
    // the Follow dialog path. Auto-follow callers (watch-now,
    // acquire-by-tmdb) pass `Some("adhoc")` so the show is marked
    // for self-removal when its last acquired episode is discarded.
    let follow_intent = input.follow_intent.as_deref().unwrap_or("explicit");

    let show_status = details.status.as_deref().map(|s| {
        let lower = s.to_ascii_lowercase();
        if lower == "ended" {
            "ended"
        } else if lower == "canceled" {
            "cancelled"
        } else if lower == "in production" || lower == "planned" {
            "upcoming"
        } else {
            "returning"
        }
    });

    let show_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO show (tmdb_id, imdb_id, tvdb_id, title, original_title, overview, tagline, year, status, network, runtime, certification, poster_path, backdrop_path, genres, tmdb_rating, tmdb_vote_count, popularity, original_language, youtube_trailer_id, quality_profile_id, monitored, monitor_new_items, monitor_specials, follow_intent, added_at, first_air_date, last_air_date, last_metadata_refresh, partial) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1) RETURNING id",
    )
    .bind(input.tmdb_id)
    .bind(&imdb_id)
    .bind(tvdb_id)
    .bind(&details.name)
    .bind(&details.original_name)
    .bind(&details.overview)
    .bind(&details.tagline)
    .bind(year)
    .bind(show_status)
    .bind(&network)
    .bind(runtime)
    .bind(&certification)
    .bind(&details.poster_path)
    .bind(&details.backdrop_path)
    .bind(&genres)
    .bind(details.vote_average)
    .bind(details.vote_count)
    .bind(details.popularity)
    .bind(&details.original_language)
    .bind(&trailer)
    .bind(profile_id)
    .bind(monitored)
    .bind(monitor_new)
    .bind(monitor_specials)
    .bind(follow_intent)
    .bind(&now)
    .bind(&details.first_air_date)
    .bind(&details.last_air_date)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    // Fetch and create all seasons + episodes
    if let Some(ref seasons) = details.seasons {
        for season_summary in seasons {
            let sn = season_summary.season_number;

            // Fetch season details from TMDB
            let season = tmdb
                .season_details(input.tmdb_id, sn)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;

            let series_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO series (show_id, tmdb_id, season_number, title, overview, poster_path, air_date, episode_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(show_id)
            .bind(season.id)
            .bind(sn)
            .bind(&season.name)
            .bind(&season.overview)
            .bind(&season.poster_path)
            .bind(&season.air_date)
            .bind(season.episodes.as_ref().map(|e| i64::try_from(e.len()).unwrap_or(0)))
            .fetch_one(&state.db)
            .await?;

            if let Some(ref episodes) = season.episodes {
                for ep in episodes {
                    let tvdb_ep_id = season.external_ids.as_ref().and_then(|e| e.tvdb_id);

                    sqlx::query(
                        "INSERT INTO episode (series_id, show_id, season_number, tmdb_id, tvdb_id, episode_number, title, overview, air_date_utc, runtime, still_path, tmdb_rating) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(series_id)
                    .bind(show_id)
                    .bind(sn)
                    .bind(ep.id)
                    .bind(tvdb_ep_id)
                    .bind(ep.episode_number)
                    .bind(&ep.name)
                    .bind(&ep.overview)
                    .bind(&ep.air_date)
                    .bind(ep.runtime)
                    .bind(&ep.still_path)
                    .bind(ep.vote_average)
                    .execute(&state.db)
                    .await?;
                }
            }
        }
    }

    // Per-season download picker. Apply to *both* axes:
    //   acquire = "should the scheduler auto-grab?"
    //   in_scope = "does this count for Next Up / progress?"
    //
    // Typical Follow choices move both together (an excluded season
    // means: don't download + don't clutter Next Up). The split gives
    // us room to diverge later (e.g. a "skip downloading but still
    // count in progress" path for episodes the user has on another
    // server). `None` leaves defaults (both 1 for every episode).
    //
    // When `seasons_to_monitor = Some(vec![])` (adhoc / Play-auto-
    // follow), both axes go to 0 everywhere. The caller will then
    // flip the *specific* episode the user acted on back to (1, 1)
    // via `acquire_episode` / `watch_now_episode`. This keeps Next Up
    // honest — it reflects what the user actually asked for, not the
    // entire aired backlog of a show they tapped "Get S01E04" on.
    if let Some(ref wanted) = input.seasons_to_monitor {
        if wanted.is_empty() {
            // Adhoc / Play-auto-follow: quiet the scheduler *and*
            // keep scope empty until the caller explicitly opts an
            // episode in.
            sqlx::query("UPDATE episode SET acquire = 0, in_scope = 0 WHERE show_id = ?")
                .bind(show_id)
                .execute(&state.db)
                .await?;
        } else {
            // Specific seasons: both axes become 0 outside the list,
            // 1 inside. CASE keeps it to a single UPDATE.
            let placeholders = wanted.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "UPDATE episode SET
                   acquire  = CASE WHEN season_number IN ({placeholders}) THEN 1 ELSE 0 END,
                   in_scope = CASE WHEN season_number IN ({placeholders}) THEN 1 ELSE 0 END
                 WHERE show_id = ?"
            );
            let mut q = sqlx::query(&sql);
            for s in wanted {
                q = q.bind(s);
            }
            for s in wanted {
                q = q.bind(s);
            }
            q.bind(show_id).execute(&state.db).await?;

            // Season 0 rescue: if "Include specials (future)" is on
            // but 0 isn't in the list, the CASE above wiped future-
            // aired specials to (0, 0). Re-apply the seed rule so
            // Season 0 future episodes stay monitored. Mirrors the
            // identical block in `update_show_monitor`.
            if input.monitor_specials.unwrap_or(false) && !wanted.contains(&0) {
                sqlx::query(
                    "UPDATE episode SET acquire = 1, in_scope = 1
                     WHERE show_id = ? AND season_number = 0
                       AND (air_date_utc IS NULL
                            OR datetime(air_date_utc) > datetime((SELECT added_at FROM show WHERE id = ?)))",
                )
                .bind(show_id)
                .bind(show_id)
                .execute(&state.db)
                .await?;
            }
        }
    }

    // Season + episode fanout has fully landed — flip partial off so
    // reads start surfacing the show. Up to this point the row was
    // invisible (every read filters partial = 0).
    sqlx::query("UPDATE show SET partial = 0 WHERE id = ?")
        .bind(show_id)
        .execute(&state.db)
        .await?;

    let show = sqlx::query_as::<_, Show>("SELECT * FROM show WHERE id = ?")
        .bind(show_id)
        .fetch_one(&state.db)
        .await?;

    // Fact about the world — always fires. History / webhooks / WS.
    state.emit(crate::events::AppEvent::ShowAdded {
        show_id,
        tmdb_id: show.tmdb_id,
        title: show.title.clone(),
    });

    Ok(show)
}

/// Follow a show (create from TMDB with all seasons/episodes).
#[utoipa::path(
    post,
    path = "/api/v1/shows",
    request_body = CreateShow,
    responses(
        (status = 201, description = "Show created", body = Show),
        (status = 409, description = "Show already exists")
    ),
    tag = "shows",
    security(("api_key" = []))
)]
pub async fn create_show(
    State(state): State<AppState>,
    Json(input): Json<CreateShow>,
) -> AppResult<(StatusCode, Json<Show>)> {
    let show = create_show_inner(&state, input).await?;

    // Scheduling decision — user added from the Follow dialog and
    // expects wanted episodes to start searching soon. Watch-now's
    // episode / show-smart-play paths skip this by calling
    // `create_show_inner` directly; they run their own search
    // inline and this trigger would race the scheduler ahead of
    // their placeholder-row creation.
    let _ = state
        .trigger_tx
        .try_send(crate::scheduler::TaskTrigger::fire("wanted_search"));

    Ok((StatusCode::CREATED, Json(show)))
}

/// List seasons for a show.
#[utoipa::path(
    get,
    path = "/api/v1/shows/{id}/seasons",
    params(("id" = i64, Path, description = "Show ID")),
    responses((status = 200, description = "List of seasons", body = Vec<Series>)),
    tag = "shows",
    security(("api_key" = []))
)]
pub async fn list_seasons(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Series>>> {
    let seasons = sqlx::query_as::<_, Series>(
        "SELECT * FROM series WHERE show_id = ? ORDER BY season_number",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(seasons))
}

/// Per-season acquire + library stats. Drives the Manage-downloads
/// dialog's tri-state checkboxes: a season with every episode at
/// `acquire = 1` is "fully monitored" (checked), one with *some*
/// episodes acquired-or-imported is "partial" (indeterminate — e.g.
/// after streaming a single episode via Play), and one with neither
/// is "unmonitored" (unchecked).
///
/// Kept as a dedicated endpoint rather than a column on the show row
/// because the data is derived — changing `episode.acquire` elsewhere
/// would otherwise require a mirror write.
#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct SeasonAcquireState {
    pub season_number: i64,
    /// Episodes with `acquire = 1`.
    pub acquiring: i64,
    /// Episodes that already have an imported file (any
    /// `media_episode` row). Lets the UI flag "partial state" even
    /// when nothing is currently scheduled.
    pub in_library: i64,
    /// Total episode rows for the season.
    pub total: i64,
}

#[utoipa::path(
    get, path = "/api/v1/shows/{id}/monitored-seasons",
    params(("id" = i64, Path)),
    responses((status = 200, body = Vec<SeasonAcquireState>), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn monitored_seasons(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<SeasonAcquireState>>> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM show WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("show {id} not found")));
    }
    let rows: Vec<SeasonAcquireState> = sqlx::query_as(
        "SELECT
            e.season_number AS season_number,
            SUM(CASE WHEN e.acquire = 1 THEN 1 ELSE 0 END) AS acquiring,
            SUM(CASE
                  WHEN EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id) THEN 1
                  ELSE 0
                END) AS in_library,
            COUNT(*) AS total
         FROM episode e
         WHERE e.show_id = ?
         GROUP BY e.season_number
         ORDER BY e.season_number",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

/// List episodes for a season.
#[utoipa::path(
    get,
    path = "/api/v1/shows/{id}/seasons/{season_number}/episodes",
    params(
        ("id" = i64, Path, description = "Show ID"),
        ("season_number" = i64, Path, description = "Season number")
    ),
    responses((status = 200, description = "List of episodes", body = Vec<Episode>)),
    tag = "shows",
    security(("api_key" = []))
)]
pub async fn list_episodes(
    State(state): State<AppState>,
    Path((id, season_number)): Path<(i64, i64)>,
) -> AppResult<Json<Vec<Episode>>> {
    let sql = format!(
        "{} WHERE e.show_id = ? AND e.season_number = ? ORDER BY e.episode_number",
        crate::content::derived_state::episode_status_select()
    );
    let episodes = sqlx::query_as::<_, Episode>(&sql)
        .bind(id)
        .bind(season_number)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(episodes))
}

/// `POST /api/v1/shows/{id}/pause-downloads` — pause every active
/// torrent linked to this show's episodes. User-intent surface: "stop
/// what this show is doing right now." Already-paused / terminal
/// rows are skipped. Per-download `DownloadPaused` events fan out so
/// other tabs flip instantly.
///
/// This is a *snapshot* action, not a policy: new downloads started
/// after this call (e.g. by the scheduler finding a fresh episode)
/// run normally. Users who want "stop acquiring for this show" use
/// Manage Downloads to unmonitor seasons.
#[utoipa::path(
    post, path = "/api/v1/shows/{id}/pause-downloads",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn pause_show_downloads(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM show WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("show {id} not found")));
    }
    let download_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT d.id
         FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         JOIN episode e ON e.id = dc.episode_id
         WHERE e.show_id = ? AND d.state IN ('downloading','stalled')",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    for dl_id in download_ids {
        // Best-effort: a single torrent that fails to pause (state
        // changed under us, torrent client hiccup) shouldn't block
        // the rest. The per-download handler logs + errors; we drop
        // the error here so the batch keeps going.
        if let Err(e) =
            crate::download::handlers::pause_download(State(state.clone()), Path(dl_id)).await
        {
            tracing::warn!(show_id = id, download_id = dl_id, error = %e,
                "pause_show_downloads: per-download pause failed");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/shows/{id}/resume-downloads` — inverse: unpause
/// every currently-paused torrent for this show. Doesn't touch
/// downloading / queued rows.
#[utoipa::path(
    post, path = "/api/v1/shows/{id}/resume-downloads",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn resume_show_downloads(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM show WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("show {id} not found")));
    }
    let download_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT d.id
         FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         JOIN episode e ON e.id = dc.episode_id
         WHERE e.show_id = ? AND d.state = 'paused'",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    for dl_id in download_ids {
        if let Err(e) =
            crate::download::handlers::resume_download(State(state.clone()), Path(dl_id)).await
        {
            tracing::warn!(show_id = id, download_id = dl_id, error = %e,
                "resume_show_downloads: per-download resume failed");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Delete a show and all its seasons/episodes.
#[utoipa::path(
    delete,
    path = "/api/v1/shows/{id}",
    params(("id" = i64, Path, description = "Show ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found")
    ),
    tag = "shows",
    security(("api_key" = []))
)]
pub async fn delete_show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Fetch title up front — needed for the `ContentRemoved` event
    // emitted at the end. Doubles as the existence check.
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM show WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let Some(title) = title else {
        return Err(AppError::NotFound(format!("show with id {id} not found")));
    };

    // `download.release_id` has no ON DELETE action, so naive
    // `DELETE FROM show` fails with FK 787 as soon as the show has
    // any grabbed releases: the show-cascade tries to drop `release`
    // rows and blows up on `download.release_id` still pointing at
    // them. Mirror `delete_movie`'s pattern: yank the downloads first,
    // then let the show cascade handle series/episode/release.
    //
    // Also gives us a chance to tell librqbit to stop any still-
    // running torrents so their files stop growing on disk after the
    // library entry is gone.
    //
    // Belt-and-braces: find downloads via BOTH `download_content →
    // episode → show` AND `release.show_id`. Callers like
    // `discard_episode` strip `download_content` before invoking the
    // cascade, which used to hide terminal-state downloads from the
    // JOIN and trip the FK on `release` cleanup. The OR on
    // `release.show_id` closes that gap for any future caller that
    // removes content links early.
    let download_ids: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT d.id, d.torrent_hash
         FROM download d
         LEFT JOIN download_content dc ON dc.download_id = d.id
         LEFT JOIN episode ec ON ec.id = dc.episode_id
         LEFT JOIN release r ON r.id = d.release_id
         WHERE ec.show_id = ? OR r.show_id = ?",
    )
    .bind(id)
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    for (dl_id, hash) in &download_ids {
        // Stop any live streaming state tied to this download — trickplay
        // extractor + HLS transcode session. Without this, deleting a show
        // mid-stream leaves an orphan ffmpeg process and a transcode
        // session consuming CPU until server restart.
        state.stream_trickplay.stop(*dl_id).await;
        if let Some(ref transcode) = state.transcode {
            let session_id = format!("stream-{dl_id}");
            if let Err(e) = transcode.stop_session(&session_id).await {
                tracing::warn!(
                    download_id = dl_id,
                    %session_id,
                    error = %e,
                    "failed to stop stream transcode on show delete",
                );
            }
        }
        if let (Some(client), Some(h)) = (&state.torrent, hash) {
            let outcome = state
                .cleanup_tracker
                .try_remove(crate::cleanup::ResourceKind::Torrent, h, || async {
                    client.remove(h, true).await
                })
                .await?;
            if !outcome.is_removed() {
                tracing::warn!(
                    download_id = dl_id,
                    torrent_hash = %h,
                    ?outcome,
                    "torrent removal queued for retry (show delete)",
                );
            }
        }
        sqlx::query("DELETE FROM download_content WHERE download_id = ?")
            .bind(dl_id)
            .execute(&state.db)
            .await?;
        sqlx::query("DELETE FROM download WHERE id = ?")
            .bind(dl_id)
            .execute(&state.db)
            .await?;
    }

    // Library files for episodes of this show. The schema only
    // cascades show → series → episode → media_episode; the `media`
    // rows themselves and the files on disk would otherwise leak. The
    // earlier code did neither — every deleted show left orphan video
    // files in the library path that filled the disk over time.
    let media_paths: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT m.id, m.file_path
         FROM media m
         JOIN media_episode me ON me.media_id = m.id
         JOIN episode e ON e.id = me.episode_id
         WHERE e.show_id = ?",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let library_root = crate::content::movie::handlers::fetch_library_root(&state.db).await;
    let library_root_path = library_root.as_deref().map(std::path::Path::new);
    for (media_id, path) in &media_paths {
        crate::content::movie::handlers::remove_library_file(*media_id, path, library_root_path)
            .await;
    }
    let media_ids: Vec<i64> = media_paths.iter().map(|(mid, _)| *mid).collect();
    if !media_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", media_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM media WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for mid in &media_ids {
            q = q.bind(mid);
        }
        q.execute(&state.db).await?;
    }

    sqlx::query("DELETE FROM show WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    // Tell every connected client so their caches (library lists,
    // downloads, show-episodes) invalidate. Without this, a stale
    // download row lingers in the UI of any other session until it
    // polls — and downloads have no polling fallback.
    state.emit(crate::events::AppEvent::ContentRemoved {
        movie_id: None,
        show_id: Some(id),
        title,
    });

    Ok(StatusCode::NO_CONTENT)
}

/// Reply for the "what should I play next?" query used on `ShowDetail`.
/// `next_up` is the first unwatched + aired episode by (season, number);
/// null when everything's been watched or nothing has aired yet.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ShowWatchState {
    pub followed: bool,
    pub next_up: Option<ShowNextUpEpisode>,
    pub watched_count: i64,
    /// Episodes with imported media, scoped to `in_scope = 1` to
    /// match `aired_count` — used by the detail-page progress badge
    /// so numerator + denominator share the same filter.
    pub available_count: i64,
    pub aired_count: i64,
    /// Per-season rollups for the season picker UI. Always emitted
    /// (empty vec when the show isn't followed or has no episodes).
    pub season_stats: Vec<SeasonStat>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
pub struct SeasonStat {
    pub season_number: i64,
    /// Total episodes in the season.
    pub total: i64,
    /// Episodes whose air date has passed (or is unknown — those
    /// count as aired so specials without dates still appear).
    pub aired: i64,
    /// Episodes with an imported file.
    pub available: i64,
    /// Episodes with `watched_at` set.
    pub watched: i64,
    /// Episodes with an active download (queued/downloading/etc).
    pub downloading: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ShowNextUpEpisode {
    pub episode_id: i64,
    pub season: i64,
    pub episode: i64,
    pub title: Option<String>,
    /// Playback progress on this episode as a 0.0–1.0 fraction,
    /// or `None` when nothing has been played yet. Lets the show
    /// poster on the detail page render the same resume bar the
    /// Home continue-watching card shows.
    pub progress_percent: Option<f64>,
}

/// `GET /api/v1/shows/by-tmdb/{tmdb_id}/watch-state` — lightweight
/// progress summary so the `ShowDetail` page can render a prominent
/// "Play {S01E03}" CTA when the user is mid-series. 200 with
/// `followed: false` when the show isn't in the library — the UI
/// still shows a generic "Start watching" CTA in that case.
#[utoipa::path(
    get, path = "/api/v1/shows/by-tmdb/{tmdb_id}/watch-state",
    params(("tmdb_id" = i64, Path)),
    responses((status = 200, body = ShowWatchState)),
    tag = "shows", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn show_watch_state(
    State(state): State<AppState>,
    Path(tmdb_id): Path<i64>,
) -> AppResult<axum::Json<ShowWatchState>> {
    #[derive(sqlx::FromRow)]
    struct NextRow {
        id: i64,
        season_number: i64,
        episode_number: i64,
        title: Option<String>,
        /// In-progress playback position, ticks. NULL for episodes
        /// the user has never played. Combined with runtime to give
        /// the 0–1 progress fraction on `ShowNextUpEpisode`.
        playback_position_ticks: Option<i64>,
        /// Runtime in minutes from the TMDB metadata refresh. Used
        /// to normalise playback position. Missing for older shows
        /// that TMDB hasn't refreshed — progress falls through to
        /// `None` in that case.
        runtime: Option<i64>,
    }

    let show_id: Option<i64> = sqlx::query_scalar("SELECT id FROM show WHERE tmdb_id = ?")
        .bind(tmdb_id)
        .fetch_optional(&state.db)
        .await?;
    let Some(show_id) = show_id else {
        return Ok(axum::Json(ShowWatchState {
            followed: false,
            next_up: None,
            watched_count: 0,
            available_count: 0,
            aired_count: 0,
            season_stats: Vec::new(),
        }));
    };

    // Single consistent filter: anything the user has expressed
    // intent to watch (`in_scope = 1`). Play-auto-follow leaves
    // in_scope=1 everywhere (acquire=0), so Next Up keeps working
    // across the series. Explicit "Latest season only" Follow sets
    // in_scope=0 for excluded seasons, and Next Up respects that.
    // No more rule-switching based on "any monitored exists" — the
    // split into acquire + in_scope lets one column consistently
    // answer "is this in scope for progress?"
    let aired_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode
         WHERE show_id = ? AND in_scope = 1
           AND (air_date_utc IS NULL OR air_date_utc <= datetime('now'))",
    )
    .bind(show_id)
    .fetch_one(&state.db)
    .await?;

    let watched_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode
         WHERE show_id = ? AND in_scope = 1 AND watched_at IS NOT NULL",
    )
    .bind(show_id)
    .fetch_one(&state.db)
    .await?;

    let available_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode e
         WHERE e.show_id = ? AND e.in_scope = 1
           AND EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)",
    )
    .bind(show_id)
    .fetch_one(&state.db)
    .await?;

    // Next Up priority: if any episode is already imported, surface
    // the earliest unwatched downloaded one first. Falls back to
    // earliest unwatched aired when nothing's downloaded yet. This
    // matches what the library Play card does so the two views agree
    // on "what episode am I up to?"
    let next: Option<NextRow> = sqlx::query_as(
        "SELECT e.id, e.season_number, e.episode_number, e.title,
                e.playback_position_ticks, e.runtime
         FROM episode e
         WHERE e.show_id = ? AND e.in_scope = 1
           AND e.season_number >= 1
           AND e.watched_at IS NULL
           AND EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
         ORDER BY e.season_number, e.episode_number
         LIMIT 1",
    )
    .bind(show_id)
    .fetch_optional(&state.db)
    .await?;
    let next = if next.is_some() {
        next
    } else {
        sqlx::query_as(
            "SELECT id, season_number, episode_number, title,
                    playback_position_ticks, runtime
             FROM episode
             WHERE show_id = ? AND in_scope = 1
               AND season_number >= 1
               AND watched_at IS NULL
               AND (air_date_utc IS NULL OR air_date_utc <= datetime('now'))
             ORDER BY season_number, episode_number
             LIMIT 1",
        )
        .bind(show_id)
        .fetch_optional(&state.db)
        .await?
    };

    // Per-season rollups for the season picker. One query, one row
    // per season. GROUP BY season_number keeps the result compact —
    // 21 rows for Grey's Anatomy is nothing. `in_scope` IS NOT
    // applied here so the picker can colour every season's row even
    // when the user followed with "Latest season only" and the
    // others are out-of-scope (they still exist, they're just
    // greyed out in the picker).
    let season_stats: Vec<SeasonStat> = sqlx::query_as(
        "SELECT
           e.season_number AS season_number,
           COUNT(*) AS total,
           SUM(CASE WHEN e.air_date_utc IS NULL OR e.air_date_utc <= datetime('now') THEN 1 ELSE 0 END) AS aired,
           SUM(CASE WHEN EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id) THEN 1 ELSE 0 END) AS available,
           SUM(CASE WHEN e.watched_at IS NOT NULL THEN 1 ELSE 0 END) AS watched,
           SUM(CASE WHEN EXISTS (
             SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
             WHERE dc.episode_id = e.id
               AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
           ) THEN 1 ELSE 0 END) AS downloading
         FROM episode e
         WHERE e.show_id = ?
         GROUP BY e.season_number
         ORDER BY e.season_number",
    )
    .bind(show_id)
    .fetch_all(&state.db)
    .await?;

    Ok(axum::Json(ShowWatchState {
        followed: true,
        next_up: next.map(|n| {
            // Ticks are 100-nanosecond units (Jellyfin-style).
            // Runtime is minutes → convert to ticks before
            // dividing so the fraction lands in 0.0-1.0.
            let progress_percent = match (n.playback_position_ticks, n.runtime) {
                (Some(pos), Some(rt)) if pos > 0 && rt > 0 => {
                    let total_ticks = rt.saturating_mul(60).saturating_mul(10_000_000);
                    (total_ticks > 0).then(|| {
                        #[allow(clippy::cast_precision_loss)]
                        let p = (pos as f64) / (total_ticks as f64);
                        p.clamp(0.0, 1.0)
                    })
                }
                _ => None,
            };
            ShowNextUpEpisode {
                episode_id: n.id,
                season: n.season_number,
                episode: n.episode_number,
                title: n.title,
                progress_percent,
            }
        }),
        watched_count,
        available_count,
        aired_count,
        season_stats,
    }))
}

/// Unified shape for an episode on the show-detail page: TMDB
/// metadata + (when the show is in the library) local acquisition /
/// watch state. Lets the UI stop branching between "render TMDB
/// episode" and "render library episode" paths — the `EpisodeCard`
/// just reads whatever's populated.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EpisodeView {
    // ── TMDB fields ──
    pub episode_number: i64,
    pub season_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i64>,
    pub still_path: Option<String>,
    pub vote_average: Option<f64>,

    // ── Library fields (null when show isn't followed or episode
    // not yet populated from TMDB details) ──
    pub episode_id: Option<i64>,
    /// Scheduler auto-acquire for this specific episode.
    pub acquire: Option<bool>,
    pub watched_at: Option<String>,
    pub playback_position_ticks: Option<i64>,

    // ── Active download (null when no download touching this
    // episode is in flight) ──
    pub download_id: Option<i64>,
    pub download_state: Option<String>,
    pub download_percent: Option<i64>,

    // ── Imported media (null until import completes) ──
    pub media_id: Option<i64>,
    pub resolution: Option<i64>,
    pub container: Option<String>,

    /// 1..10 user rating on Trakt's scale. Null when unrated or
    /// when the show isn't in the library yet (no episode row).
    pub user_rating: Option<i64>,

    // ── Intro / credits timestamps (subsystem 15). Null when not
    // analysed yet OR analysed but not detected — the Skip button
    // simply doesn't render in either case. ──
    pub intro_start_ms: Option<i64>,
    pub intro_end_ms: Option<i64>,
    pub credits_start_ms: Option<i64>,
    pub credits_end_ms: Option<i64>,
}

/// `GET /api/v1/shows/by-tmdb/{tmdb_id}/seasons/{season}/episodes`
///
/// Returns a list of `EpisodeView` — TMDB season details left-joined
/// with per-episode library state. One canonical endpoint replaces
/// the old branching between the TMDB proxy (for not-followed shows)
/// and the library episodes endpoint (for followed shows). The
/// `EpisodeCard` component can now always render the right thing
/// without asking "is the show in library?"
#[utoipa::path(
    get, path = "/api/v1/shows/by-tmdb/{tmdb_id}/seasons/{season_number}/episodes",
    params(("tmdb_id" = i64, Path), ("season_number" = i64, Path)),
    responses((status = 200, body = Vec<EpisodeView>)),
    tag = "shows", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn show_season_episodes_by_tmdb(
    State(state): State<AppState>,
    Path((tmdb_id, season_number)): Path<(i64, i64)>,
) -> AppResult<axum::Json<Vec<EpisodeView>>> {
    #[derive(sqlx::FromRow)]
    struct LibRow {
        episode_number: i64,
        episode_id: i64,
        acquire: bool,
        watched_at: Option<String>,
        playback_position_ticks: i64,
        download_id: Option<i64>,
        download_state: Option<String>,
        download_percent: Option<i64>,
        media_id: Option<i64>,
        resolution: Option<i64>,
        container: Option<String>,
        user_rating: Option<i64>,
        intro_start_ms: Option<i64>,
        intro_end_ms: Option<i64>,
        credits_start_ms: Option<i64>,
        credits_end_ms: Option<i64>,
    }

    // 1. TMDB is always the source for episode ordering + metadata.
    //    A 404 from TMDB (season doesn't exist) returns an empty
    //    episode list rather than a 500 — the frontend's prefetch
    //    of adjacent seasons can ask for a season number that
    //    doesn't exist on the last iteration (e.g. show has 5
    //    seasons, prefetch asks for 6) and shouldn't hard-fail.
    let tmdb = state.require_tmdb()?;
    let tmdb_season = match tmdb.season_details(tmdb_id, season_number).await {
        Ok(s) => s,
        Err(crate::tmdb::TmdbError::NotFound) => {
            return Ok(axum::Json(Vec::new()));
        }
        Err(e) => return Err(AppError::Internal(e.into())),
    };

    // 2. If the show is in the library, pull its episodes for this
    // season in one query, plus any active downloads and imported
    // media. Key everything by `episode_number` so we can merge
    // against TMDB's list below.
    let show_id: Option<i64> = sqlx::query_scalar("SELECT id FROM show WHERE tmdb_id = ?")
        .bind(tmdb_id)
        .fetch_optional(&state.db)
        .await?;

    let lib_rows: Vec<LibRow> = if let Some(sid) = show_id {
        sqlx::query_as(
            "SELECT
                e.episode_number                 as episode_number,
                e.id                             as episode_id,
                e.acquire                        as acquire,
                e.watched_at                     as watched_at,
                e.playback_position_ticks        as playback_position_ticks,
                d.id                             as download_id,
                d.state                          as download_state,
                CASE
                  WHEN d.size IS NOT NULL AND d.size > 0
                    THEN CAST(d.downloaded * 100 / d.size AS INTEGER)
                  ELSE NULL
                END                              as download_percent,
                m.id                             as media_id,
                m.resolution                     as resolution,
                m.container                      as container,
                e.user_rating                    as user_rating,
                e.intro_start_ms                 as intro_start_ms,
                e.intro_end_ms                   as intro_end_ms,
                e.credits_start_ms               as credits_start_ms,
                e.credits_end_ms                 as credits_end_ms
             FROM episode e
             LEFT JOIN download_content dc ON dc.episode_id = e.id
             LEFT JOIN download d ON d.id = dc.download_id
               AND d.state IN ('searching', 'queued', 'grabbing', 'downloading', 'paused', 'stalled', 'importing')
             LEFT JOIN media_episode me ON me.episode_id = e.id
             LEFT JOIN media m ON m.id = me.media_id
             WHERE e.show_id = ? AND e.season_number = ?",
        )
        .bind(sid)
        .bind(season_number)
        .fetch_all(&state.db)
        .await?
    } else {
        Vec::new()
    };

    let by_ep: std::collections::HashMap<i64, LibRow> = lib_rows
        .into_iter()
        .map(|r| (r.episode_number, r))
        .collect();

    // 3. Merge. TMDB is the ordering authority — episodes it doesn't
    // know about don't appear here (they're orphan library rows that
    // have since been removed from TMDB; a metadata refresh would
    // clean them up).
    let tmdb_eps = tmdb_season.episodes.unwrap_or_default();
    let mut out: Vec<EpisodeView> = Vec::with_capacity(tmdb_eps.len());
    for ep in tmdb_eps {
        let lib = by_ep.get(&ep.episode_number);
        out.push(EpisodeView {
            episode_number: ep.episode_number,
            season_number: ep.season_number,
            name: ep.name,
            overview: ep.overview,
            air_date: ep.air_date,
            runtime: ep.runtime,
            still_path: ep.still_path,
            vote_average: ep.vote_average,
            episode_id: lib.map(|r| r.episode_id),
            acquire: lib.map(|r| r.acquire),
            watched_at: lib.and_then(|r| r.watched_at.clone()),
            playback_position_ticks: lib.map(|r| r.playback_position_ticks),
            download_id: lib.and_then(|r| r.download_id),
            download_state: lib.and_then(|r| r.download_state.clone()),
            download_percent: lib.and_then(|r| r.download_percent),
            media_id: lib.and_then(|r| r.media_id),
            resolution: lib.and_then(|r| r.resolution),
            container: lib.and_then(|r| r.container.clone()),
            user_rating: lib.and_then(|r| r.user_rating),
            intro_start_ms: lib.and_then(|r| r.intro_start_ms),
            intro_end_ms: lib.and_then(|r| r.intro_end_ms),
            credits_start_ms: lib.and_then(|r| r.credits_start_ms),
            credits_end_ms: lib.and_then(|r| r.credits_end_ms),
        });
    }

    Ok(axum::Json(out))
}

/// Payload for the Manage-downloads / Update-monitoring flow. Same
/// shape as the per-season fields on `CreateShow`, but applied as a
/// delta to an already-existing show. `seasons_to_monitor` uses the
/// same three-way semantics: None = leave existing monitored state
/// alone; Some([]) = unmonitor every existing episode; Some([n, ...])
/// = only those season numbers monitored, the rest unmonitored.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateShowMonitor {
    #[schema(value_type = Option<MonitorNewItems>)]
    pub monitor_new_items: Option<String>,
    pub seasons_to_monitor: Option<Vec<i64>>,
    /// Opt-in for Season 0 ("Specials"). Written to `show.monitor_
    /// specials` when provided; season-0 episodes are normalised in
    /// the same pass.
    pub monitor_specials: Option<bool>,
}

/// `PATCH /api/v1/shows/{id}/monitor` — re-apply monitor preferences
/// to an existing show (e.g. the Manage downloads flow). Does NOT
/// re-fetch TMDB or reinsert episode rows. Persists
/// `monitor_new_items` + `monitor_specials` on the show row, rewrites
/// per-episode `acquire`/`in_scope` to match the new season list, and
/// cancels in-flight downloads for episodes that just went out of
/// scope.
#[utoipa::path(
    patch, path = "/api/v1/shows/{id}/monitor",
    params(("id" = i64, Path)),
    request_body = UpdateShowMonitor,
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)] // one linear orchestration, splitting scatters it
pub async fn update_show_monitor(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateShowMonitor>,
) -> AppResult<StatusCode> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM show WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("show {id} not found")));
    }

    if let Some(mni) = input.monitor_new_items.as_deref() {
        sqlx::query("UPDATE show SET monitor_new_items = ? WHERE id = ?")
            .bind(mni)
            .bind(id)
            .execute(&state.db)
            .await?;
    }

    if let Some(ms) = input.monitor_specials {
        sqlx::query("UPDATE show SET monitor_specials = ? WHERE id = ?")
            .bind(i64::from(ms))
            .bind(id)
            .execute(&state.db)
            .await?;
    }

    // Submitting the Manage dialog is commitment — flip an adhoc
    // follow to explicit. No-op if the show is already explicit.
    // Effect: the show no longer auto-removes when its last episode
    // is discarded — the user has now deliberately picked settings.
    sqlx::query(
        "UPDATE show SET follow_intent = 'explicit' WHERE id = ? AND follow_intent = 'adhoc'",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    // Mirror create_show's per-season monitor semantics.
    // None = leave episode monitoring alone;
    // Some([]) = drop everything out of scope (both axes zero),
    //           matching create_show_inner's empty-branch — "nothing
    //           to monitor means nothing in scope." The previous
    //           `in_scope = 1` here diverged from create for no
    //           clear reason and left users with ghost progress
    //           counts on shows they'd explicitly dropped.
    // Some([n,…]) = those seasons in scope, everything else zero.
    //
    // Season 0 can now appear in the caller's list — the UI lets the
    // user tick "Include past specials" as either a row in the
    // Specific-seasons checklist or a sub-checkbox under All. When
    // it's there, past-aired specials get (1, 1) like any other
    // listed season. When it isn't, they stay (or become) (0, 0).
    // `monitor_specials` is a *separate* axis for future-facing
    // behavior (seed rule at metadata refresh); the two can vary
    // independently.
    let mut monitoring_expanded = false;
    if let Some(ref wanted) = input.seasons_to_monitor {
        let wanted: Vec<i64> = wanted.clone();
        if wanted.is_empty() {
            sqlx::query("UPDATE episode SET acquire = 0, in_scope = 0 WHERE show_id = ?")
                .bind(id)
                .execute(&state.db)
                .await?;
        } else {
            let placeholders = std::iter::repeat_n("?", wanted.len())
                .collect::<Vec<_>>()
                .join(",");
            // Any episode becoming `acquire = 1` in the new set gets
            // its `last_searched_at` cleared — the old conditional
            // (only when acquire was 0) missed episodes already
            // monitored with stale timestamps. Clearing for the full
            // "now monitored" set is a strict superset and keeps
            // re-monitor semantics "start fresh".
            let sql = format!(
                "UPDATE episode SET
                   acquire  = CASE WHEN season_number IN ({placeholders}) THEN 1 ELSE 0 END,
                   in_scope = CASE WHEN season_number IN ({placeholders}) THEN 1 ELSE 0 END,
                   last_searched_at = CASE
                     WHEN season_number IN ({placeholders}) THEN NULL
                     ELSE last_searched_at
                   END
                 WHERE show_id = ?"
            );
            let mut q = sqlx::query(&sql);
            for sn in &wanted {
                q = q.bind(sn);
            }
            for sn in &wanted {
                q = q.bind(sn);
            }
            for sn in &wanted {
                q = q.bind(sn);
            }
            q.bind(id).execute(&state.db).await?;
            monitoring_expanded = true;

            // If monitoring is being (re-)expanded, make sure the
            // show itself is flagged monitored. The wanted-sweep
            // filters on `show.monitored = 1`, so an earlier
            // "Remove from library" / Trakt-flipped state would
            // otherwise silently gate the scheduler despite a
            // per-season selection being active.
            sqlx::query("UPDATE show SET monitored = 1 WHERE id = ?")
                .bind(id)
                .execute(&state.db)
                .await?;

            // Season 0 rescue: if the user ticked "Include specials"
            // (forward-monitor axis) but didn't include 0 in the
            // download list, the main CASE just flipped every Season
            // 0 episode to (0, 0) — wiping the seed rule's work for
            // future-aired specials. Re-apply the seed rule so new
            // specials still end up monitored. Past-aired specials
            // stay (0, 0) unless 0 was in the list (the "Specials"
            // row on the Specific checklist / the "include past" sub-
            // checkbox under All).
            let monitor_specials: bool =
                sqlx::query_scalar("SELECT monitor_specials FROM show WHERE id = ?")
                    .bind(id)
                    .fetch_one(&state.db)
                    .await?;
            if monitor_specials && !wanted.contains(&0) {
                sqlx::query(
                    "UPDATE episode SET acquire = 1, in_scope = 1, last_searched_at = NULL
                     WHERE show_id = ? AND season_number = 0
                       AND (air_date_utc IS NULL
                            OR datetime(air_date_utc) > datetime((SELECT added_at FROM show WHERE id = ?)))",
                )
                .bind(id)
                .bind(id)
                .execute(&state.db)
                .await?;
            }
        }

        cancel_downloads_for_unmonitored(&state, id).await?;
    }

    // `monitor_new_items` is a forward-looking flag on the show row,
    // but users reasonably expect toggling it to affect episodes
    // *already* in the DB that haven't aired yet — "stop monitoring
    // future episodes" should drop next week's episode from the
    // calendar, not just stop TMDB additions two years from now.
    //
    // Runs after the season-monitor block so the "none" path strictly
    // narrows (any future-aired episode in an otherwise in-scope
    // season drops); the "future" path strictly expands (restores
    // future-aired episodes in seasons that still have any in_scope
    // sibling — the signal for "this season is being tracked"). Both
    // run only when the flag is actually in the patch.
    if let Some(mni) = input.monitor_new_items.as_deref() {
        if mni == "none" {
            sqlx::query(
                "UPDATE episode SET acquire = 0, in_scope = 0
                 WHERE show_id = ?
                   AND air_date_utc IS NOT NULL
                   AND air_date_utc > datetime('now')",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
            cancel_downloads_for_unmonitored(&state, id).await?;
        } else if mni == "future" {
            sqlx::query(
                "UPDATE episode AS target
                 SET acquire = 1, in_scope = 1, last_searched_at = NULL
                 WHERE target.show_id = ?
                   AND target.air_date_utc IS NOT NULL
                   AND target.air_date_utc > datetime('now')
                   AND EXISTS (
                     SELECT 1 FROM episode sib
                     WHERE sib.show_id = target.show_id
                       AND sib.season_number = target.season_number
                       AND sib.in_scope = 1
                   )",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
            monitoring_expanded = true;
        }
    }

    // `monitor_specials` is now purely a forward-looking policy flag
    // — it affects `seed_acquire_in_scope` when the metadata refresh
    // inserts new Season 0 episodes from TMDB. Existing episode
    // state is controlled exclusively by `seasons_to_monitor`
    // (including 0 when the user tickets "Include past specials"
    // in the dialog). No retroactive Season 0 UPDATE here — the
    // previous "specials-only toggle" branch blanket-flipped every
    // past-aired special to acquire=1, causing an indexer search
    // flood for shows with dozens of old shorts.

    // Kick the scheduler if any seasons just became monitored so
    // the user sees downloads start immediately rather than waiting
    // up to `auto_search_interval` minutes for the next tick.
    if monitoring_expanded {
        let _ = state
            .trigger_tx
            .try_send(crate::scheduler::TaskTrigger::fire("wanted_search"));
    }

    let title: String = sqlx::query_scalar("SELECT title FROM show WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    state.emit(crate::events::AppEvent::ShowMonitorChanged { show_id: id, title });

    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct CancelRow {
    id: i64,
    title: String,
    torrent_hash: Option<String>,
}

/// Cancel any in-flight downloads whose every linked episode is now
/// `acquire = 0` for the given show. Called after `update_show_monitor`
/// flips per-season monitoring so that unchecking a partial season
/// also stops the torrent — matching user intent.
///
/// Preserves multi-episode packs that still have at least one
/// monitored episode (NOT EXISTS clause). Terminal downloads
/// (imported/failed/completed) are skipped — cancel is a no-op on
/// them and we don't want to overwrite their state.
async fn cancel_downloads_for_unmonitored(state: &AppState, show_id: i64) -> AppResult<()> {
    let to_cancel: Vec<CancelRow> = sqlx::query_as(
        "SELECT DISTINCT d.id, d.title, d.torrent_hash
         FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         JOIN episode e ON e.id = dc.episode_id
         WHERE e.show_id = ?
           AND d.state NOT IN ('imported', 'failed', 'completed')
           AND NOT EXISTS (
             SELECT 1 FROM download_content dc2
             JOIN episode e2 ON e2.id = dc2.episode_id
             WHERE dc2.download_id = d.id AND e2.acquire = 1
           )",
    )
    .bind(show_id)
    .fetch_all(&state.db)
    .await?;

    for d in to_cancel {
        state.stream_trickplay.stop(d.id).await;
        if let Some(ref transcode) = state.transcode {
            let session_id = format!("stream-{}", d.id);
            if let Err(e) = transcode.stop_session(&session_id).await {
                tracing::warn!(
                    download_id = d.id,
                    %session_id,
                    error = %e,
                    "failed to stop stream transcode on unmonitor-cancel",
                );
            }
        }
        if let (Some(client), Some(hash)) = (&state.torrent, d.torrent_hash.as_ref()) {
            let outcome = state
                .cleanup_tracker
                .try_remove(crate::cleanup::ResourceKind::Torrent, hash, || async {
                    client.remove(hash, true).await
                })
                .await?;
            if !outcome.is_removed() {
                tracing::warn!(
                    download_id = d.id,
                    torrent_hash = %hash,
                    ?outcome,
                    "torrent removal queued for retry (unmonitor-cancel)",
                );
            }
        }
        // Mark as `failed` in the DB (that's the terminal state for
        // a torrent we're no longer tracking) but emit
        // `DownloadCancelled` — this is user-intent via the Manage
        // dialog, not a download failure. Firing `DownloadFailed`
        // surfaces the kino-toast failure card with a "Pick
        // alternate" CTA, which is misleading for a cancel the user
        // themselves triggered. Matches how `cancel_download`
        // already distinguishes the two.
        sqlx::query(
            "UPDATE download SET state = 'failed',
                                 error_message = 'cancelled: season unmonitored'
             WHERE id = ?",
        )
        .bind(d.id)
        .execute(&state.db)
        .await?;

        // Push the state flip to live clients so the downloads pane
        // reflects the cancellation without waiting for the next poll.
        let _ = state
            .event_tx
            .send(crate::events::AppEvent::DownloadCancelled {
                download_id: d.id,
                title: d.title,
            });
    }
    Ok(())
}
