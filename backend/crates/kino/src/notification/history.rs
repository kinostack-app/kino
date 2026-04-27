//! History — durable record of every state change. The history table
//! is unconditionally written by the event listener; the HTTP layer
//! exposes a paginated list view (per-entity + event-type filters)
//! the Library History UI consumes.

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::error::AppResult;
use crate::pagination::{Cursor, PaginatedResponse, PaginationParams};
use crate::state::AppState;

use super::Event;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct History {
    pub id: i64,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub event_type: String,
    pub date: String,
    pub source_title: Option<String>,
    pub quality: Option<String>,
    pub download_id: Option<String>,
    pub data: Option<String>,
}

/// Log an event to the History table.
pub async fn log_event(pool: &SqlitePool, event: &Event) -> anyhow::Result<i64> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let data = serde_json::to_string(event).unwrap_or_default();
    let quality = event.quality.as_deref();

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO history (movie_id, episode_id, event_type, date, source_title, quality, data) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(event.movie_id)
    .bind(event.episode_id)
    .bind(&event.event_type)
    .bind(&now)
    .bind(event.title.as_deref())
    .bind(quality)
    .bind(&data)
    .fetch_one(pool)
    .await?;

    Ok(id)
}

// ─── HTTP handlers ──────────────────────────────────────────────────

/// Query for `GET /api/v1/history`. Accepts the common
/// `PaginationParams` alongside the per-entity + event-type
/// filters. Cursor is opaque base64 per
/// `docs/subsystems/09-api.md` — a legacy integer cursor path
/// was shipped earlier and is gone now (pre-release, no external
/// consumers to preserve).
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct HistoryFilter {
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    /// Single-type filter — `event_type=imported`. Kept for the
    /// per-entity detail views that always narrow to one type.
    pub event_type: Option<String>,
    /// Multi-type filter — comma-separated list, e.g.
    /// `event_types=grabbed,imported,watched`. The Library History
    /// UI uses this to offer a faceted pill row without client-side
    /// re-filtering of paginated results.
    pub event_types: Option<String>,
    #[param(default = 25, minimum = 1, maximum = 100)]
    pub limit: Option<i64>,
    /// Opaque cursor from a previous response. Omit for the first
    /// page.
    pub cursor: Option<String>,
}

impl HistoryFilter {
    fn pagination(&self) -> PaginationParams {
        PaginationParams {
            limit: self.limit,
            cursor: self.cursor.clone(),
            sort: None,
            order: None,
        }
    }
}

/// List history events (paginated, newest first).
#[utoipa::path(
    get, path = "/api/v1/history",
    params(HistoryFilter),
    responses((status = 200, body = PaginatedResponse<History>)),
    tag = "history", security(("api_key" = []))
)]
pub async fn list_history(
    State(state): State<AppState>,
    Query(filter): Query<HistoryFilter>,
) -> AppResult<Json<PaginatedResponse<History>>> {
    let pagination = filter.pagination();
    let limit = pagination.limit();
    let fetch_limit = limit + 1;
    let cursor = pagination.cursor.as_deref().and_then(Cursor::decode);

    let event_types: Vec<String> = filter
        .event_types
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    // Build the WHERE clause dynamically. The `id < ?` cursor
    // clause rides alongside the per-entity + event-type filters
    // so we can cross-paginate without fighting the ORDER BY.
    let mut sql = String::from("SELECT * FROM history");
    let mut where_parts: Vec<String> = Vec::new();
    if filter.movie_id.is_some() {
        where_parts.push("movie_id = ?".into());
    }
    if filter.episode_id.is_some() {
        where_parts.push("episode_id = ?".into());
    }
    if filter.event_type.is_some() {
        where_parts.push("event_type = ?".into());
    }
    if !event_types.is_empty() {
        let placeholders = event_types
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        where_parts.push(format!("event_type IN ({placeholders})"));
    }
    if cursor.is_some() {
        // Newest-first order + `id < cursor` = "older than the
        // last row of the previous page." Monotonic `id` is the
        // right cursor key here: created_at can tie on bursts.
        where_parts.push("id < ?".into());
    }
    if !where_parts.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");

    let mut q = sqlx::query_as::<_, History>(&sql);
    if let Some(movie_id) = filter.movie_id {
        q = q.bind(movie_id);
    }
    if let Some(episode_id) = filter.episode_id {
        q = q.bind(episode_id);
    }
    if let Some(ref event_type) = filter.event_type {
        q = q.bind(event_type);
    }
    for t in &event_types {
        q = q.bind(t);
    }
    if let Some(ref c) = cursor {
        q = q.bind(c.id);
    }
    q = q.bind(fetch_limit);

    let history = q.fetch_all(&state.db).await?;
    Ok(Json(PaginatedResponse::new(history, limit, |h| Cursor {
        id: h.id,
        sort_value: None,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn log_event_creates_history_row() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let event = Event::simple("imported", "The Matrix");
        let id = log_event(&pool, &event).await.unwrap();
        assert!(id > 0);

        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM history WHERE event_type = 'imported'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn log_event_stores_json_data() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let mut event = Event::simple("release_grabbed", "Breaking Bad S05E14");
        event.quality = Some("Bluray-1080p".into());
        event.indexer = Some("My Indexer".into());

        let id = log_event(&pool, &event).await.unwrap();

        let data: String = sqlx::query_scalar("SELECT data FROM history WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(json["quality"], "Bluray-1080p");
        assert_eq!(json["indexer"], "My Indexer");
    }
}
