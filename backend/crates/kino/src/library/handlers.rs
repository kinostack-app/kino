//! Library-level endpoints: search across movies + shows, calendar, stats.

use std::fmt::Write;

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

// ── Phase-derivation count queries ────────────────────────────
// Replaces the old status='...'-filtered queries. Kept as string
// constants so the predicates aren't duplicated across the stats +
// widget endpoints; matches the same derivation rules as
// content/derived_state.rs.

const WANTED_MOVIE_COUNT: &str = "SELECT COUNT(*) FROM movie mv
    WHERE mv.monitored = 1
      AND mv.watched_at IS NULL
      AND NOT EXISTS (SELECT 1 FROM media m WHERE m.movie_id = mv.id)
      AND NOT EXISTS (
        SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
        WHERE dc.movie_id = mv.id
          AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
      )";

const WANTED_EPISODE_COUNT: &str = "SELECT COUNT(*) FROM episode e
    WHERE e.acquire = 1
      AND e.watched_at IS NULL
      -- Match wanted_search_sweep's eligibility: unaired episodes
      -- aren't being searched, so they don't belong in the wanted
      -- count that drives stats/widgets.
      AND (e.air_date_utc IS NULL OR e.air_date_utc <= datetime('now'))
      AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
      AND NOT EXISTS (
        SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
        WHERE dc.episode_id = e.id
          AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
      )";

const AVAILABLE_MOVIE_COUNT: &str = "SELECT COUNT(*) FROM movie mv
    WHERE mv.watched_at IS NULL
      AND EXISTS (SELECT 1 FROM media m WHERE m.movie_id = mv.id)";

const AVAILABLE_EPISODE_COUNT: &str = "SELECT COUNT(*) FROM episode e
    WHERE e.watched_at IS NULL
      AND EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)";

#[derive(Debug, Deserialize, IntoParams)]
pub struct LibrarySearchQuery {
    /// Substring to search for in title (case-insensitive).
    pub q: String,
    /// Maximum results (default 20, max 100).
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct LibraryHit {
    /// `movie` or `show`.
    pub item_type: String,
    pub id: i64,
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub poster_path: Option<String>,
    /// Only set for movies.
    pub status: Option<String>,
}

/// Search the user's library (movies + shows) by title substring.
#[utoipa::path(
    get, path = "/api/v1/library/search",
    params(LibrarySearchQuery),
    responses((status = 200, body = Vec<LibraryHit>)),
    tag = "library", security(("api_key" = []))
)]
pub async fn library_search(
    State(state): State<AppState>,
    Query(params): Query<LibrarySearchQuery>,
) -> AppResult<Json<Vec<LibraryHit>>> {
    let q = params.q.trim();
    if q.is_empty() {
        return Err(AppError::BadRequest("q parameter is required".into()));
    }
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let pattern = format!("%{q}%");

    // Movies and shows in one UNION so we can cap total results server-side.
    // ORDER first by exact-prefix match, then alphabetically.
    // Movie status is computed via CASE; shows don't have a phase.
    let hits = sqlx::query_as::<_, LibraryHit>(
        "SELECT 'movie' AS item_type, mv.id, mv.tmdb_id, mv.title, mv.year, mv.poster_path,
                CASE
                  WHEN mv.watched_at IS NOT NULL AND mv.watched_at != '' THEN 'watched'
                  WHEN EXISTS(SELECT 1 FROM media m WHERE m.movie_id = mv.id) THEN 'available'
                  WHEN EXISTS(
                    SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
                    WHERE dc.movie_id = mv.id
                      AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  ) THEN 'downloading'
                  ELSE 'wanted'
                END AS status
         FROM movie mv WHERE LOWER(mv.title) LIKE LOWER(?)
         UNION ALL
         SELECT 'show' AS item_type, s.id, s.tmdb_id, s.title, s.year, s.poster_path, NULL AS status
         FROM show s WHERE LOWER(s.title) LIKE LOWER(?) AND s.partial = 0
         ORDER BY title COLLATE NOCASE LIMIT ?",
    )
    .bind(&pattern)
    .bind(&pattern)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(hits))
}

// ── Calendar ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct CalendarQuery {
    /// Inclusive start date (YYYY-MM-DD). Defaults to 7 days ago.
    pub start: Option<String>,
    /// Inclusive end date (YYYY-MM-DD). Defaults to 30 days ahead.
    pub end: Option<String>,
}

#[derive(sqlx::FromRow)]
struct EpisodeRow {
    episode_id: i64,
    air_date_utc: String,
    season_number: i64,
    episode_number: i64,
    is_finale: bool,
    episode_title: Option<String>,
    show_id: i64,
    show_title: String,
    tmdb_id: i64,
    poster_path: Option<String>,
    media_id: Option<i64>,
    download_id: Option<i64>,
    download_percent: Option<i64>,
    status: String,
}

#[derive(sqlx::FromRow)]
struct MovieRow {
    movie_id: i64,
    title: String,
    tmdb_id: i64,
    poster_path: Option<String>,
    release_date: Option<String>,
    physical_release_date: Option<String>,
    digital_release_date: Option<String>,
    media_id: Option<i64>,
    download_id: Option<i64>,
    download_percent: Option<i64>,
    status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CalendarEntry {
    /// `episode` or `movie`.
    pub item_type: String,
    /// Air or release date (YYYY-MM-DD).
    pub date: String,
    pub title: String,
    /// Season/episode number when `item_type` is episode.
    pub season_number: Option<i64>,
    pub episode_number: Option<i64>,
    /// True if this is the last episode of its season.
    pub is_finale: Option<bool>,
    /// True if this is S?E01 — the UI labels these as premieres.
    pub is_premiere: Option<bool>,
    /// Episode-specific title (for shows).
    pub episode_title: Option<String>,
    pub show_id: Option<i64>,
    pub show_title: Option<String>,
    pub movie_id: Option<i64>,
    /// Internal episode id; needed for mark-watched / redownload actions.
    pub episode_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub poster_path: Option<String>,
    pub status: Option<String>,
    /// Library media id when `status = 'available'` — powers the
    /// Play button directly from the calendar cell.
    pub media_id: Option<i64>,
    /// Active download id when `status = 'downloading'` — lets the
    /// card join `download_progress` WS events for live %.
    pub download_id: Option<i64>,
    /// 0–100 snapshot so cells render immediately without waiting for
    /// the downloads cache to hydrate. Live-updated via the WS patch.
    pub download_percent: Option<i64>,
}

/// List upcoming episodes (from monitored shows) + movie releases in a date
/// range. No grouping or pagination — this is a small list for a calendar UI.
#[utoipa::path(
    get, path = "/api/v1/calendar",
    params(CalendarQuery),
    responses((status = 200, body = Vec<CalendarEntry>)),
    tag = "library", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn calendar(
    State(state): State<AppState>,
    Query(params): Query<CalendarQuery>,
) -> AppResult<Json<Vec<CalendarEntry>>> {
    let today = chrono::Utc::now().date_naive();
    let start = params
        .start
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today - chrono::Duration::days(7));
    let end = params
        .end
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today + chrono::Duration::days(30));
    if end < start {
        return Err(AppError::BadRequest("end must be >= start".into()));
    }
    let start_s = start.to_string();
    let end_s = end.to_string();

    let episodes = sqlx::query_as::<_, EpisodeRow>(
        "SELECT e.id AS episode_id, e.air_date_utc, e.season_number, e.episode_number, e.title AS episode_title,
                -- Finale detection: is this the max episode_number for its season?
                (e.episode_number = (SELECT MAX(e2.episode_number) FROM episode e2
                                      WHERE e2.show_id = e.show_id AND e2.season_number = e.season_number)) AS is_finale,
                s.id AS show_id, s.title AS show_title, s.tmdb_id, s.poster_path,
                (SELECT me.media_id FROM media_episode me WHERE me.episode_id = e.id LIMIT 1) AS media_id,
                (SELECT d.id FROM download_content dc JOIN download d ON d.id = dc.download_id
                  WHERE dc.episode_id = e.id
                    AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  LIMIT 1) AS download_id,
                (SELECT
                   CASE WHEN d.size IS NULL OR d.size = 0 THEN 0
                        ELSE CAST(d.downloaded * 100 / d.size AS INTEGER) END
                 FROM download_content dc JOIN download d ON d.id = dc.download_id
                  WHERE dc.episode_id = e.id
                    AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  LIMIT 1) AS download_percent,
                CASE
                  WHEN e.watched_at IS NOT NULL AND e.watched_at != '' THEN 'watched'
                  WHEN EXISTS(SELECT 1 FROM media_episode me WHERE me.episode_id = e.id) THEN 'available'
                  WHEN EXISTS(
                    SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
                    WHERE dc.episode_id = e.id
                      AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  ) THEN 'downloading'
                  -- Episodes in a monitored show but with acquire=0
                  -- aren't going to be grabbed by the scheduler. Flag
                  -- them as `unmonitored` so the UI doesn't show them
                  -- with the same 'wanted' styling as things that
                  -- actually are queued up to fetch.
                  WHEN e.acquire = 0 THEN 'unmonitored'
                  ELSE 'wanted'
                END AS status
         FROM episode e JOIN show s ON e.show_id = s.id
         WHERE s.monitored = 1
           AND s.partial = 0
           AND e.in_scope = 1
           AND e.air_date_utc IS NOT NULL
           AND substr(e.air_date_utc, 1, 10) BETWEEN ? AND ?
         ORDER BY e.air_date_utc",
    )
    .bind(&start_s)
    .bind(&end_s)
    .fetch_all(&state.db)
    .await?;

    let movies = sqlx::query_as::<_, MovieRow>(
        "SELECT mv.id AS movie_id, mv.title, mv.tmdb_id, mv.poster_path,
                mv.release_date, mv.physical_release_date, mv.digital_release_date,
                (SELECT m.id FROM media m WHERE m.movie_id = mv.id LIMIT 1) AS media_id,
                (SELECT d.id FROM download_content dc JOIN download d ON d.id = dc.download_id
                  WHERE dc.movie_id = mv.id
                    AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  LIMIT 1) AS download_id,
                (SELECT
                   CASE WHEN d.size IS NULL OR d.size = 0 THEN 0
                        ELSE CAST(d.downloaded * 100 / d.size AS INTEGER) END
                 FROM download_content dc JOIN download d ON d.id = dc.download_id
                  WHERE dc.movie_id = mv.id
                    AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  LIMIT 1) AS download_percent,
                CASE
                  WHEN mv.watched_at IS NOT NULL AND mv.watched_at != '' THEN 'watched'
                  WHEN EXISTS(SELECT 1 FROM media m WHERE m.movie_id = mv.id) THEN 'available'
                  WHEN EXISTS(
                    SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
                    WHERE dc.movie_id = mv.id
                      AND d.state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
                  ) THEN 'downloading'
                  ELSE 'wanted'
                END AS status
         FROM movie mv
         WHERE mv.monitored = 1
           AND (
                 (mv.release_date IS NOT NULL AND mv.release_date BETWEEN ? AND ?)
              OR (mv.physical_release_date IS NOT NULL AND mv.physical_release_date BETWEEN ? AND ?)
              OR (mv.digital_release_date IS NOT NULL AND mv.digital_release_date BETWEEN ? AND ?)
           )",
    )
    .bind(&start_s)
    .bind(&end_s)
    .bind(&start_s)
    .bind(&end_s)
    .bind(&start_s)
    .bind(&end_s)
    .fetch_all(&state.db)
    .await?;

    let mut out: Vec<CalendarEntry> = Vec::with_capacity(episodes.len() + movies.len());

    for e in episodes {
        let is_premiere = e.episode_number == 1;
        out.push(CalendarEntry {
            item_type: "episode".into(),
            date: e
                .air_date_utc
                .get(..10)
                .unwrap_or(&e.air_date_utc)
                .to_string(),
            title: e
                .episode_title
                .clone()
                .unwrap_or_else(|| format!("S{:02}E{:02}", e.season_number, e.episode_number)),
            season_number: Some(e.season_number),
            episode_number: Some(e.episode_number),
            is_finale: Some(e.is_finale),
            is_premiere: Some(is_premiere),
            episode_title: e.episode_title,
            show_id: Some(e.show_id),
            show_title: Some(e.show_title),
            movie_id: None,
            episode_id: Some(e.episode_id),
            tmdb_id: Some(e.tmdb_id),
            poster_path: e.poster_path,
            status: Some(e.status),
            media_id: e.media_id,
            download_id: e.download_id,
            download_percent: e.download_percent,
        });
    }

    for m in movies {
        // Pick the earliest release date in the window to display.
        let candidates = [
            m.release_date.as_deref(),
            m.physical_release_date.as_deref(),
            m.digital_release_date.as_deref(),
        ];
        let best = candidates
            .iter()
            .flatten()
            .copied()
            .filter(|d: &&str| *d >= start_s.as_str() && *d <= end_s.as_str())
            .min()
            .map_or_else(
                || m.release_date.clone().unwrap_or_default(),
                std::string::ToString::to_string,
            );

        out.push(CalendarEntry {
            item_type: "movie".into(),
            date: best,
            title: m.title,
            season_number: None,
            episode_number: None,
            is_finale: None,
            is_premiere: None,
            episode_title: None,
            show_id: None,
            show_title: None,
            movie_id: Some(m.movie_id),
            episode_id: None,
            tmdb_id: Some(m.tmdb_id),
            poster_path: m.poster_path,
            status: Some(m.status),
            media_id: m.media_id,
            download_id: m.download_id,
            download_percent: m.download_percent,
        });
    }

    out.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(Json(out))
}

/// `GET /api/v1/calendar.ics` — iCalendar feed for subscribing from
/// Google Calendar, Apple Calendar, Thunderbird, etc. Emits one
/// all-day `VEVENT` per upcoming episode + movie release in a fixed
/// 90-day forward window (plus 30 days back so recent airs don't
/// drop off immediately in the calendar app).
///
/// Auth: supports `?api_key=` in the URL because calendar subscribers
/// can't set headers. The usual auth middleware already handles
/// that query fallback.
#[utoipa::path(
    get, path = "/api/v1/calendar.ics",
    responses((status = 200, description = "iCalendar feed", content_type = "text/calendar")),
    tag = "library", security(("api_key" = []))
)]
pub async fn calendar_ics(State(state): State<AppState>) -> AppResult<axum::response::Response> {
    use axum::response::IntoResponse;

    let today = chrono::Utc::now().date_naive();
    let start_s = (today - chrono::Duration::days(30)).to_string();
    let end_s = (today + chrono::Duration::days(90)).to_string();

    // Reuse the same two queries as the JSON endpoint so the feed
    // mirrors what users see in the calendar page. Small DRY tax.
    let params = CalendarQuery {
        start: Some(start_s),
        end: Some(end_s),
    };
    let entries = calendar(State(state), Query(params)).await?.0;

    let mut body = String::with_capacity(entries.len() * 200 + 256);
    body.push_str("BEGIN:VCALENDAR\r\n");
    body.push_str("VERSION:2.0\r\n");
    body.push_str("PRODID:-//kino//media server//EN\r\n");
    body.push_str("CALSCALE:GREGORIAN\r\n");
    body.push_str("METHOD:PUBLISH\r\n");
    body.push_str("X-WR-CALNAME:kino schedule\r\n");
    body.push_str("X-WR-TIMEZONE:UTC\r\n");

    for e in &entries {
        let compact_date = e.date.replace('-', "");
        // Each event gets a stable UID so updates in the source feed
        // replace the previous occurrence rather than duplicating.
        let uid = match (e.item_type.as_str(), e.episode_id, e.movie_id) {
            ("episode", Some(id), _) => format!("ep-{id}@kino"),
            ("movie", _, Some(id)) => format!("mv-{id}@kino"),
            _ => format!("unknown-{compact_date}@kino"),
        };
        let summary = if e.item_type == "episode" {
            let sn = e.season_number.unwrap_or(0);
            let ep = e.episode_number.unwrap_or(0);
            let show = e.show_title.as_deref().unwrap_or("Episode");
            let title = e.episode_title.as_deref().unwrap_or("");
            if title.is_empty() {
                format!("{show} S{sn:02}E{ep:02}")
            } else {
                format!("{show} S{sn:02}E{ep:02} — {title}")
            }
        } else {
            e.title.clone()
        };

        body.push_str("BEGIN:VEVENT\r\n");
        let _ = writeln!(body, "UID:{uid}\r");
        let _ = writeln!(body, "DTSTAMP:{compact_date}T000000Z\r");
        let _ = writeln!(body, "DTSTART;VALUE=DATE:{compact_date}\r");
        let _ = writeln!(body, "SUMMARY:{}\r", ics_escape(&summary));
        if let Some(status) = e.status.as_deref() {
            let _ = writeln!(body, "DESCRIPTION:Status: {status}\r");
        }
        body.push_str("END:VEVENT\r\n");
    }
    body.push_str("END:VCALENDAR\r\n");

    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/calendar; charset=utf-8"),
        )],
        // Clients poll; don't force them to re-download if nothing
        // changed, but don't cache forever either.
        [(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=900"),
        )],
        body,
    )
        .into_response())
}

/// Escape characters that have special meaning in iCal text lines
/// (RFC 5545 §3.3.11). Commas, semicolons, newlines, and backslashes
/// need escaping; everything else passes through.
fn ics_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' | '\r' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

// ── Stats ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct LibraryStats {
    pub movies_total: i64,
    pub movies_wanted: i64,
    pub movies_downloading: i64,
    pub movies_available: i64,
    pub movies_watched: i64,
    pub shows_total: i64,
    pub episodes_total: i64,
    pub episodes_wanted: i64,
    pub episodes_available: i64,
    pub episodes_watched: i64,
    pub media_files: i64,
    pub media_bytes: i64,
    pub downloads_active: i64,
    pub downloads_completed: i64,
    pub downloads_failed: i64,
}

// ── Widget ──────────────────────────────────────────────────────────

/// Flat summary for external dashboards (Homepage, Dashy, Heimdall).
///
/// Shape is deliberately flat + top-level numbers so the Homepage
/// `customapi` widget can map each field to a tile with no traversal:
///
/// ```yaml
/// # homepage config
/// - kino:
///     widget:
///       type: customapi
///       url: http://kino:8080/api/v1/widget
///       headers: { Authorization: "Bearer {api_key}" }
///       mappings:
///         - { field: movies, label: Movies, format: number }
///         - { field: shows, label: Shows, format: number }
///         - { field: wanted, label: Wanted, format: number }
///         - { field: downloading, label: Downloading, format: number }
/// ```
#[derive(Debug, Serialize, ToSchema)]
pub struct WidgetResponse {
    pub movies: i64,
    pub shows: i64,
    pub episodes: i64,
    pub wanted: i64,
    pub downloading: i64,
    pub queued: i64,
    pub available: i64,
    pub watched: i64,
    /// Total size of all imported media on disk, in bytes.
    pub disk_bytes: i64,
}

/// Flat counter summary for external dashboards (Homepage etc.).
#[utoipa::path(
    get, path = "/api/v1/widget",
    responses((status = 200, body = WidgetResponse)),
    tag = "library", security(("api_key" = []))
)]
pub async fn widget(State(state): State<AppState>) -> AppResult<Json<WidgetResponse>> {
    async fn count(pool: &sqlx::SqlitePool, sql: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await
    }
    let db = &state.db;

    let movies = count(db, "SELECT COUNT(*) FROM movie").await?;
    let shows = count(db, "SELECT COUNT(*) FROM show WHERE partial = 0").await?;
    let episodes = count(db, "SELECT COUNT(*) FROM episode").await?;
    // Phase is derived (see content/derived_state.rs). "Wanted" counts
    // content that's opted in, unwatched, unacquired, and not
    // currently in flight. "Available" is has-media-not-watched.
    // "Watched" is watched_at IS NOT NULL.
    let wanted_movies = count(db, WANTED_MOVIE_COUNT).await?;
    let wanted_eps = count(db, WANTED_EPISODE_COUNT).await?;
    let wanted = wanted_movies + wanted_eps;
    let downloading = count(
        db,
        "SELECT COUNT(*) FROM download WHERE state IN ('grabbing','downloading','stalled','seeding','importing')",
    )
    .await?;
    let queued = count(db, "SELECT COUNT(*) FROM download WHERE state = 'queued'").await?;
    let available_movies = count(db, AVAILABLE_MOVIE_COUNT).await?;
    let available_eps = count(db, AVAILABLE_EPISODE_COUNT).await?;
    let available = available_movies + available_eps;
    let watched_movies = count(
        db,
        "SELECT COUNT(*) FROM movie WHERE watched_at IS NOT NULL",
    )
    .await?;
    let watched_eps = count(
        db,
        "SELECT COUNT(*) FROM episode WHERE watched_at IS NOT NULL",
    )
    .await?;
    let watched = watched_movies + watched_eps;
    let disk_bytes = sqlx::query_scalar::<_, Option<i64>>("SELECT SUM(size) FROM media")
        .fetch_one(db)
        .await?
        .unwrap_or(0);

    Ok(Json(WidgetResponse {
        movies,
        shows,
        episodes,
        wanted,
        downloading,
        queued,
        available,
        watched,
        disk_bytes,
    }))
}

/// Aggregate stats for the dashboard.
#[utoipa::path(
    get, path = "/api/v1/stats",
    responses((status = 200, body = LibraryStats)),
    tag = "library", security(("api_key" = []))
)]
pub async fn stats(State(state): State<AppState>) -> AppResult<Json<LibraryStats>> {
    // Single round-trip via multiple query_scalar calls; SQLite is local so
    // round-trip cost is negligible and the code is easier to read than a
    // giant CTE.
    async fn count(pool: &sqlx::SqlitePool, sql: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await
    }

    let db = &state.db;
    let movies_total = count(db, "SELECT COUNT(*) FROM movie").await?;
    let movies_wanted = count(db, WANTED_MOVIE_COUNT).await?;
    let movies_downloading = count(
        db,
        "SELECT COUNT(*) FROM download WHERE state IN ('searching','queued','grabbing','downloading','paused','stalled','importing')
         AND id IN (SELECT DISTINCT download_id FROM download_content WHERE movie_id IS NOT NULL)",
    )
    .await?;
    let movies_available = count(db, AVAILABLE_MOVIE_COUNT).await?;
    let movies_watched = count(
        db,
        "SELECT COUNT(*) FROM movie WHERE watched_at IS NOT NULL",
    )
    .await?;
    let shows_total = count(db, "SELECT COUNT(*) FROM show WHERE partial = 0").await?;
    let episodes_total = count(db, "SELECT COUNT(*) FROM episode").await?;
    let episodes_wanted = count(db, WANTED_EPISODE_COUNT).await?;
    let episodes_available = count(db, AVAILABLE_EPISODE_COUNT).await?;
    let episodes_watched = count(
        db,
        "SELECT COUNT(*) FROM episode WHERE watched_at IS NOT NULL",
    )
    .await?;
    let media_files = count(db, "SELECT COUNT(*) FROM media").await?;
    let media_bytes = sqlx::query_scalar::<_, Option<i64>>("SELECT SUM(size) FROM media")
        .fetch_one(db)
        .await?
        .unwrap_or(0);
    let downloads_active = count(
        db,
        "SELECT COUNT(*) FROM download WHERE state IN ('searching','queued','grabbing','downloading','paused','stalled','seeding','importing')",
    )
    .await?;
    let downloads_completed = count(
        db,
        "SELECT COUNT(*) FROM download WHERE state = 'completed'",
    )
    .await?;
    let downloads_failed =
        count(db, "SELECT COUNT(*) FROM download WHERE state = 'failed'").await?;

    Ok(Json(LibraryStats {
        movies_total,
        movies_wanted,
        movies_downloading,
        movies_available,
        movies_watched,
        shows_total,
        episodes_total,
        episodes_wanted,
        episodes_available,
        episodes_watched,
        media_files,
        media_bytes,
        downloads_active,
        downloads_completed,
        downloads_failed,
    }))
}
