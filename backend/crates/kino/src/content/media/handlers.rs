use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::content::media::model::Media;
use crate::error::{AppError, AppResult};
use crate::pagination::{Cursor, PaginatedResponse, PaginationParams};
use crate::playback::stream_model::Stream;
use crate::state::AppState;

/// Query filters for `GET /api/v1/media`. Combined with
/// `PaginationParams` as a flat struct because utoipa's
/// `IntoParams` doesn't compose multiple structs into one
/// endpoint's query string without naming them.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ListMediaQuery {
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    #[param(default = 25, minimum = 1, maximum = 100)]
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl ListMediaQuery {
    fn pagination(&self) -> PaginationParams {
        PaginationParams {
            limit: self.limit,
            cursor: self.cursor.clone(),
            sort: None,
            order: None,
        }
    }
}

/// List media files, optionally filtered by movie or episode.
///
/// Paginated per `docs/subsystems/09-api.md` — the prior
/// implementation silently capped at `LIMIT 100`, which truncated
/// larger libraries without a `has_more` signal so clients
/// couldn't tell they were missing rows. Ordering is `date_added
/// DESC, id DESC` (newest first, stable tiebreak).
///
/// The filtered branches (by `movie_id` or `episode_id`) also
/// paginate — historically those returned every row, but any
/// pathological case of 300+ upgraded copies for the same
/// title would be just as dangerous there.
#[utoipa::path(
    get, path = "/api/v1/media",
    params(ListMediaQuery),
    responses((status = 200, body = PaginatedResponse<Media>)),
    tag = "media", security(("api_key" = []))
)]
pub async fn list_media(
    State(state): State<AppState>,
    Query(query): Query<ListMediaQuery>,
) -> AppResult<Json<PaginatedResponse<Media>>> {
    let pagination = query.pagination();
    let limit = pagination.limit();
    let fetch_limit = limit + 1;
    let cursor = pagination.cursor.as_deref().and_then(Cursor::decode);

    let media = match (query.movie_id, query.episode_id) {
        (Some(movie_id), _) => list_for_movie(&state, movie_id, cursor, fetch_limit).await?,
        (None, Some(episode_id)) => {
            list_for_episode(&state, episode_id, cursor, fetch_limit).await?
        }
        (None, None) => list_all(&state, cursor, fetch_limit).await?,
    };

    Ok(Json(PaginatedResponse::new(media, limit, |m| Cursor {
        id: m.id,
        sort_value: Some(m.date_added.clone()),
    })))
}

async fn list_all(
    state: &AppState,
    cursor: Option<Cursor>,
    fetch_limit: i64,
) -> AppResult<Vec<Media>> {
    if let Some(c) = cursor {
        let added = c.sort_value.unwrap_or_default();
        let rows = sqlx::query_as::<_, Media>(
            "SELECT * FROM media
             WHERE datetime(date_added) < datetime(?) OR (date_added = ? AND id > ?)
             ORDER BY date_added DESC, id ASC LIMIT ?",
        )
        .bind(&added)
        .bind(&added)
        .bind(c.id)
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?;
        Ok(rows)
    } else {
        let rows = sqlx::query_as::<_, Media>(
            "SELECT * FROM media ORDER BY date_added DESC, id ASC LIMIT ?",
        )
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?;
        Ok(rows)
    }
}

async fn list_for_movie(
    state: &AppState,
    movie_id: i64,
    cursor: Option<Cursor>,
    fetch_limit: i64,
) -> AppResult<Vec<Media>> {
    if let Some(c) = cursor {
        let added = c.sort_value.unwrap_or_default();
        let rows = sqlx::query_as::<_, Media>(
            "SELECT * FROM media
             WHERE movie_id = ?
               AND (datetime(date_added) < datetime(?) OR (date_added = ? AND id > ?))
             ORDER BY date_added DESC, id ASC LIMIT ?",
        )
        .bind(movie_id)
        .bind(&added)
        .bind(&added)
        .bind(c.id)
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?;
        Ok(rows)
    } else {
        let rows = sqlx::query_as::<_, Media>(
            "SELECT * FROM media WHERE movie_id = ? ORDER BY date_added DESC, id ASC LIMIT ?",
        )
        .bind(movie_id)
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?;
        Ok(rows)
    }
}

async fn list_for_episode(
    state: &AppState,
    episode_id: i64,
    cursor: Option<Cursor>,
    fetch_limit: i64,
) -> AppResult<Vec<Media>> {
    if let Some(c) = cursor {
        let added = c.sort_value.unwrap_or_default();
        let rows = sqlx::query_as::<_, Media>(
            "SELECT m.* FROM media m
             JOIN media_episode me ON m.id = me.media_id
             WHERE me.episode_id = ?
               AND (datetime(m.date_added) < datetime(?) OR (m.date_added = ? AND m.id > ?))
             ORDER BY m.date_added DESC, m.id ASC LIMIT ?",
        )
        .bind(episode_id)
        .bind(&added)
        .bind(&added)
        .bind(c.id)
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?;
        Ok(rows)
    } else {
        let rows = sqlx::query_as::<_, Media>(
            "SELECT m.* FROM media m
             JOIN media_episode me ON m.id = me.media_id
             WHERE me.episode_id = ?
             ORDER BY m.date_added DESC, m.id ASC LIMIT ?",
        )
        .bind(episode_id)
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?;
        Ok(rows)
    }
}

/// Get a media file by ID.
#[utoipa::path(
    get, path = "/api/v1/media/{id}",
    params(("id" = i64, Path)),
    responses((status = 200, body = Media), (status = 404)),
    tag = "media", security(("api_key" = []))
)]
pub async fn get_media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Media>> {
    let media = sqlx::query_as::<_, Media>("SELECT * FROM media WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("media {id} not found")))?;
    Ok(Json(media))
}

/// Get streams for a media file.
#[utoipa::path(
    get, path = "/api/v1/media/{id}/streams",
    params(("id" = i64, Path)),
    responses((status = 200, body = Vec<Stream>), (status = 404)),
    tag = "media", security(("api_key" = []))
)]
pub async fn get_media_streams(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Stream>>> {
    // Verify media exists
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM media WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;

    if exists.is_none() {
        return Err(AppError::NotFound(format!("media {id} not found")));
    }

    let streams = sqlx::query_as::<_, Stream>(
        "SELECT * FROM stream WHERE media_id = ? ORDER BY stream_index",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(streams))
}

/// Delete a media file and its streams.
#[utoipa::path(
    delete, path = "/api/v1/media/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "media", security(("api_key" = []))
)]
pub async fn delete_media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let media = sqlx::query_as::<_, Media>("SELECT * FROM media WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("media {id} not found")))?;

    // Delete the file from disk. If this fails (permission / share
    // locked / file moved) we still DELETE the media row below —
    // leaving a DB row pointing at a ghost file is worse than a
    // dangling file the user can rm manually.
    let path = std::path::Path::new(&media.file_path);
    if path.exists()
        && let Err(e) = tokio::fs::remove_file(path).await
    {
        tracing::warn!(
            media_id = id,
            path = %media.file_path,
            error = %e,
            "failed to remove media file from disk; removing DB row anyway",
        );
    }

    // Drop the extracted-subs cache (same helper the scheduler
    // cleanup uses — one source of truth for where the cache
    // lives and how it's torn down).
    if let Err(e) = crate::playback::subtitle::clear_cache_dir(&state.data_path, id).await {
        tracing::debug!(
            media_id = id,
            error = %e,
            "subtitle cache cleanup failed (continuing)",
        );
    }

    if let Err(e) = crate::playback::trickplay::clear_trickplay_dir(&state.data_path, id).await {
        tracing::debug!(
            media_id = id,
            error = %e,
            "trickplay cache cleanup failed (continuing)",
        );
    }

    // Cascade deletes streams and media_episode via FK
    sqlx::query("DELETE FROM media WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
