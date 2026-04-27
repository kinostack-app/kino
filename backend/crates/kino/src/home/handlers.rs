//! Home-page-specific endpoints. The Home surface mixes movies and TV
//! episodes in a single "Up Next" row that folds in three signal
//! classes — paused items, up-next-episode-of-a-show, and (when the
//! row would otherwise be short) recently-added unwatched movies as
//! padding. The per-resource /movies and /shows endpoints can't
//! cleanly produce this mix; this module owns the composed query.
//! See `docs/subsystems/18-ui-customisation.md` § Up Next.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::AppResult;
use crate::state::AppState;

/// One entry in the Up Next list. Four signal classes share this
/// shape:
///
/// 1. **In-progress movie** — `kind = "movie"`, `reason = "in_progress"`.
/// 2. **In-progress episode** — `kind = "episode"`, `reason = "in_progress"`.
/// 3. **Up next episode** — `kind = "episode"`, `reason = "up_next"`. The
///    next unwatched available episode of a show where the previous
///    episode is finished and no other episode is mid-play.
/// 4. **Recently added** — `kind = "movie"`, `reason = "recently_added"`.
///    Unwatched newly-imported movies, used as padding so the row
///    isn't empty on fresh installs. Only appears when the three
///    signals above would otherwise yield fewer than `UP_NEXT_MIN`
///    items.
///
/// The `progress_percent` field is pre-computed server-side so the UI
/// can just feed it to the poster card — runtime lives on the `media`
/// table (via join) and mixing units in the frontend is noise.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ContinueItem {
    pub kind: String,
    pub reason: String,
    /// Stable key for React — `kind:library_id`.
    pub id: String,
    /// TMDB id of the movie or parent show — drives the card's link
    /// to `/movie/{tmdb}` or `/show/{tmdb}`.
    pub tmdb_id: i64,
    /// Library id (movie.id for movies, episode.id for episodes).
    /// Used when firing a playback session from the row.
    pub library_id: i64,
    pub title: String,
    pub show_title: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub episode_title: Option<String>,
    pub poster_path: Option<String>,
    pub blurhash_poster: Option<String>,
    pub progress_percent: f32,
    pub last_played_at: Option<String>,
}

/// Threshold below which Up Next pads with recently-added unwatched
/// movies. A fresh install has zero personal signal — rather than
/// show an empty row (which would auto-hide entirely), we surface
/// newly-imported content so Home has something actionable.
const UP_NEXT_MIN: usize = 5;

/// Hard cap on the full row so power users don't drown in items.
const UP_NEXT_CAP: usize = 20;

/// `GET /api/v1/home/up-next` — the composed Up Next list.
/// Mixes paused movies, paused episodes, next-episode picks, and (as
/// padding, when the above signals yield <`UP_NEXT_MIN`) unwatched
/// recently-added movies. Sorted most-recently-relevant-first, capped
/// at `UP_NEXT_CAP`.
///
/// Matches spec §18 § Up Next. The backend composes the row server-
/// side (single endpoint, ordered array) so the frontend doesn't need
/// to merge multiple queries.
#[utoipa::path(
    get, path = "/api/v1/home/up-next",
    responses((status = 200, body = Vec<ContinueItem>)),
    tag = "home", security(("api_key" = []))
)]
pub async fn up_next(State(state): State<AppState>) -> AppResult<Json<Vec<ContinueItem>>> {
    let pool = &state.db;

    let mut items: Vec<ContinueItem> = Vec::new();
    items.extend(in_progress_movies(pool).await);
    items.extend(in_progress_episodes(pool).await);
    items.extend(up_next_episodes(pool).await);

    // Sort before padding so the "organic" items keep their recency
    // ordering. Padding appends at the tail — it's the fallback
    // content, not interleaved with actionable resume items.
    items.sort_by(|a, b| b.last_played_at.cmp(&a.last_played_at));

    if items.len() < UP_NEXT_MIN {
        // Exclude IDs already present (a just-added movie that the
        // user briefly played is in_progress, not padding) so the
        // same card doesn't appear twice.
        let existing: std::collections::HashSet<String> =
            items.iter().map(|i| i.id.clone()).collect();
        for pad in recently_added_movies(pool, UP_NEXT_CAP).await {
            if !existing.contains(&pad.id) {
                items.push(pad);
                if items.len() >= UP_NEXT_CAP {
                    break;
                }
            }
        }
    }

    items.truncate(UP_NEXT_CAP);
    Ok(Json(items))
}

/// Row shape for the in-progress movie query — we pull exactly the
/// fields the card needs. `runtime_ticks` is joined from `media` so
/// we can compute the progress percentage without a second round-trip
/// per movie.
#[derive(sqlx::FromRow)]
struct MovieRow {
    id: i64,
    tmdb_id: i64,
    title: String,
    poster_path: Option<String>,
    blurhash_poster: Option<String>,
    playback_position_ticks: i64,
    runtime_ticks: Option<i64>,
    last_played_at: Option<String>,
}

async fn in_progress_movies(pool: &sqlx::SqlitePool) -> Vec<ContinueItem> {
    let rows: Vec<MovieRow> = sqlx::query_as(
        r"SELECT mv.id, mv.tmdb_id, mv.title, mv.poster_path, mv.blurhash_poster,
                 mv.playback_position_ticks,
                 m.runtime_ticks,
                 mv.last_played_at
          FROM movie mv
          LEFT JOIN media m ON m.movie_id = mv.id
          WHERE mv.playback_position_ticks > 0
            AND mv.watched_at IS NULL
            AND m.id IS NOT NULL
          ORDER BY mv.last_played_at DESC
          LIMIT 20",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|r| ContinueItem {
            kind: "movie".into(),
            reason: "in_progress".into(),
            id: format!("movie:{}", r.id),
            tmdb_id: r.tmdb_id,
            library_id: r.id,
            title: r.title,
            show_title: None,
            season: None,
            episode: None,
            episode_title: None,
            poster_path: r.poster_path,
            blurhash_poster: r.blurhash_poster,
            progress_percent: progress_pct(r.playback_position_ticks, r.runtime_ticks),
            last_played_at: r.last_played_at,
        })
        .collect()
}

/// Row shape shared by the in-progress and up-next episode queries —
/// both return the same columns (episode fields + parent show fields)
/// so we join once and map once.
#[derive(sqlx::FromRow)]
struct EpisodeRow {
    ep_id: i64,
    show_tmdb_id: i64,
    show_title: String,
    show_poster: Option<String>,
    show_blurhash: Option<String>,
    season_number: i64,
    episode_number: i64,
    episode_title: Option<String>,
    playback_position_ticks: i64,
    runtime_ticks: Option<i64>,
    last_played_at: Option<String>,
}

async fn in_progress_episodes(pool: &sqlx::SqlitePool) -> Vec<ContinueItem> {
    let rows: Vec<EpisodeRow> = sqlx::query_as(
        r"SELECT e.id as ep_id,
                 s.tmdb_id as show_tmdb_id, s.title as show_title,
                 s.poster_path as show_poster, s.blurhash_poster as show_blurhash,
                 e.season_number, e.episode_number, e.title as episode_title,
                 e.playback_position_ticks,
                 m.runtime_ticks,
                 e.last_played_at
          FROM episode e
          JOIN show s ON s.id = e.show_id
          LEFT JOIN media_episode me ON me.episode_id = e.id
          LEFT JOIN media m ON m.id = me.media_id
          WHERE e.playback_position_ticks > 0
            AND e.watched_at IS NULL
            AND m.id IS NOT NULL
            AND s.partial = 0
          ORDER BY e.last_played_at DESC
          LIMIT 20",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().map(episode_row_to_item).collect()
}

async fn up_next_episodes(pool: &sqlx::SqlitePool) -> Vec<ContinueItem> {
    // "Up next" fires for shows where the user has finished at least
    // one episode AND has no episode mid-play. Otherwise the
    // in-progress row already covers them and we'd double-render.
    // The CTE picks the earliest unwatched available episode per show
    // by (season, episode) order, then joins show metadata and the
    // show's most-recent-played timestamp so the row sorts naturally.
    let rows: Vec<EpisodeRow> = sqlx::query_as(
        r"WITH first_unwatched AS (
            SELECT e.*,
                   ROW_NUMBER() OVER (PARTITION BY e.show_id
                                      ORDER BY e.season_number, e.episode_number) AS rn
            FROM episode e
            WHERE e.watched_at IS NULL
              AND EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
          ),
          show_last_play AS (
            SELECT show_id, MAX(last_played_at) AS last_play
            FROM episode
            WHERE last_played_at IS NOT NULL
            GROUP BY show_id
          ),
          shows_with_watched AS (
            SELECT DISTINCT show_id FROM episode WHERE watched_at IS NOT NULL
          ),
          shows_with_in_progress AS (
            SELECT DISTINCT show_id FROM episode
            WHERE playback_position_ticks > 0 AND watched_at IS NULL
          )
          SELECT e.id as ep_id,
                 s.tmdb_id as show_tmdb_id, s.title as show_title,
                 s.poster_path as show_poster, s.blurhash_poster as show_blurhash,
                 e.season_number, e.episode_number, e.title as episode_title,
                 e.playback_position_ticks,
                 m.runtime_ticks,
                 slp.last_play as last_played_at
          FROM first_unwatched e
          JOIN show s ON s.id = e.show_id
          LEFT JOIN show_last_play slp ON slp.show_id = e.show_id
          LEFT JOIN media_episode me ON me.episode_id = e.id
          LEFT JOIN media m ON m.id = me.media_id
          WHERE e.rn = 1
            AND e.show_id IN shows_with_watched
            AND e.show_id NOT IN shows_with_in_progress
            AND s.partial = 0
          ORDER BY slp.last_play DESC
          LIMIT 20",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|r| {
            let mut item = episode_row_to_item(r);
            // Up-next episodes haven't started, so any non-zero
            // position from a previous query would be misleading.
            item.reason = "up_next".into();
            item.progress_percent = 0.0;
            item
        })
        .collect()
}

fn episode_row_to_item(r: EpisodeRow) -> ContinueItem {
    ContinueItem {
        kind: "episode".into(),
        reason: "in_progress".into(),
        id: format!("episode:{}", r.ep_id),
        tmdb_id: r.show_tmdb_id,
        library_id: r.ep_id,
        title: r.show_title.clone(),
        show_title: Some(r.show_title),
        season: Some(r.season_number),
        episode: Some(r.episode_number),
        episode_title: r.episode_title,
        poster_path: r.show_poster,
        blurhash_poster: r.show_blurhash,
        progress_percent: progress_pct(r.playback_position_ticks, r.runtime_ticks),
        last_played_at: r.last_played_at,
    }
}

/// Padding signal — unwatched movies imported recently. Used only
/// when the actionable signals (paused, up-next-episode) would leave
/// the row under `UP_NEXT_MIN`. Sorts by `media.added_at DESC` so the
/// newest imports surface first; falls back to movie rowid when
/// `added_at` is null on older media rows (pre-backfill).
async fn recently_added_movies(pool: &sqlx::SqlitePool, limit: usize) -> Vec<ContinueItem> {
    let rows: Vec<MovieRow> = sqlx::query_as(
        r"SELECT mv.id, mv.tmdb_id, mv.title, mv.poster_path, mv.blurhash_poster,
                 mv.playback_position_ticks,
                 m.runtime_ticks,
                 m.added_at AS last_played_at
          FROM movie mv
          JOIN media m ON m.movie_id = mv.id
          WHERE mv.watched_at IS NULL
            AND mv.playback_position_ticks = 0
          ORDER BY m.added_at DESC, mv.id DESC
          LIMIT ?",
    )
    .bind(i64::try_from(limit).unwrap_or(20))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|r| ContinueItem {
            kind: "movie".into(),
            reason: "recently_added".into(),
            id: format!("movie:{}", r.id),
            tmdb_id: r.tmdb_id,
            library_id: r.id,
            title: r.title,
            show_title: None,
            season: None,
            episode: None,
            episode_title: None,
            poster_path: r.poster_path,
            blurhash_poster: r.blurhash_poster,
            progress_percent: 0.0,
            // `last_played_at` is aliased from `media.added_at` so the
            // UI has a non-null timestamp to show if it wants to —
            // harmless here because padding items appear after the
            // sort, their timestamp doesn't affect ordering.
            last_played_at: r.last_played_at,
        })
        .collect()
}

fn progress_pct(position: i64, runtime: Option<i64>) -> f32 {
    // We compute in f64 and downcast to f32 for the wire — the
    // precision loss is acceptable since the UI only renders to ~0.5%
    // resolution (progress-bar pixel width), well within f32's ~7
    // significant digits. Cap at 100 so a slightly-over final-scene
    // position doesn't render a >100% bar on the card.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    match runtime {
        Some(total) if total > 0 => {
            let pct = (position as f64 / total as f64) * 100.0;
            pct.clamp(0.0, 100.0) as f32
        }
        _ => 0.0,
    }
}
