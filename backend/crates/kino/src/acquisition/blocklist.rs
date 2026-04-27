//! Blocklist — releases the user (or the system) refused. Acquisition
//! consults this on every grab attempt; the trait `BlocklistEntry` in
//! `release_target` is the matcher used at decision time. This module
//! owns the row model + the HTTP CRUD that lets the user inspect /
//! clear entries.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::pagination::{Cursor, PaginatedResponse, PaginationParams};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Blocklist {
    pub id: i64,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub source_title: String,
    pub torrent_info_hash: Option<String>,
    pub indexer_id: Option<i64>,
    pub size: Option<i64>,
    pub resolution: Option<i64>,
    pub source: Option<String>,
    pub video_codec: Option<String>,
    pub message: Option<String>,
    pub date: String,
}

/// List blocklist entries (paginated).
#[utoipa::path(
    get, path = "/api/v1/blocklist",
    params(PaginationParams),
    responses((status = 200, description = "Paginated blocklist")),
    tag = "blocklist", security(("api_key" = []))
)]
pub async fn list_blocklist(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> AppResult<Json<PaginatedResponse<Blocklist>>> {
    let limit = params.limit();
    let fetch_limit = limit + 1;
    let cursor = params.cursor.as_deref().and_then(Cursor::decode);

    let items = if let Some(c) = cursor {
        sqlx::query_as::<_, Blocklist>(
            "SELECT * FROM blocklist WHERE id > ? ORDER BY id ASC LIMIT ?",
        )
        .bind(c.id)
        .bind(fetch_limit)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, Blocklist>("SELECT * FROM blocklist ORDER BY id ASC LIMIT ?")
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    };

    Ok(Json(PaginatedResponse::new(items, limit, |b| Cursor {
        id: b.id,
        sort_value: None,
    })))
}

/// Delete a blocklist entry.
#[utoipa::path(
    delete, path = "/api/v1/blocklist/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "blocklist", security(("api_key" = []))
)]
pub async fn delete_blocklist(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let result = sqlx::query("DELETE FROM blocklist WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "blocklist entry {id} not found"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Clear all blocklist entries for a movie.
#[utoipa::path(
    delete, path = "/api/v1/blocklist/movie/{movie_id}",
    params(("movie_id" = i64, Path)),
    responses((status = 200, description = "Number of entries removed")),
    tag = "blocklist", security(("api_key" = []))
)]
pub async fn clear_movie_blocklist(
    State(state): State<AppState>,
    Path(movie_id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let result = sqlx::query("DELETE FROM blocklist WHERE movie_id = ?")
        .bind(movie_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({
        "removed": result.rows_affected()
    })))
}

/// Count blocklist entries for a movie.
#[utoipa::path(
    get, path = "/api/v1/blocklist/movie/{movie_id}",
    params(("movie_id" = i64, Path)),
    responses((status = 200)),
    tag = "blocklist", security(("api_key" = []))
)]
pub async fn get_movie_blocklist(
    State(state): State<AppState>,
    Path(movie_id): Path<i64>,
) -> AppResult<Json<Vec<Blocklist>>> {
    let items = sqlx::query_as::<_, Blocklist>(
        "SELECT * FROM blocklist WHERE movie_id = ? ORDER BY date DESC",
    )
    .bind(movie_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(items))
}

// ─── Blocklist-on-grab-failure operations ───────────────────────────

#[derive(sqlx::FromRow)]
struct BlocklistDlRow {
    release_id: Option<i64>,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct BlocklistReleaseRow {
    title: String,
    info_hash: Option<String>,
    indexer_id: Option<i64>,
    size: Option<i64>,
    resolution: Option<i64>,
    source: Option<String>,
    video_codec: Option<String>,
}

/// Pure blocklist write: add the release tied to `download_id` to
/// the blocklist and clear the content's `last_searched_at` so the
/// next search tier picks it up immediately. Does NOT kick the
/// scheduler — callers that want the full "blocklist then retry"
/// behaviour go through [`blocklist_and_retry`] instead; callers
/// running their own retry loop (watch-now phase 2) use this variant
/// so they aren't stepping on a parallel scheduler-triggered grab.
///
/// No-op if the download has no `release_id`, the release can't be
/// loaded, or the download has no content link (orphan row).
pub async fn blocklist_release_for_download(state: &AppState, download_id: i64, reason: &str) {
    let pool = &state.db;

    let row: Option<BlocklistDlRow> = match sqlx::query_as(
        "SELECT d.release_id,
                dc.movie_id   AS movie_id,
                dc.episode_id AS episode_id
         FROM download d
         LEFT JOIN download_content dc ON dc.download_id = d.id
         WHERE d.id = ?
         LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(download_id, error = %e, "blocklist: DB fetch failed");
            return;
        }
    };

    let Some(row) = row else { return };
    let Some(release_id) = row.release_id else {
        return;
    };

    // Pull release details for the blocklist entry.
    let rel: Option<BlocklistReleaseRow> = match sqlx::query_as(
        "SELECT title, info_hash, indexer_id, size, resolution, source, video_codec
         FROM release WHERE id = ?",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(release_id, error = %e, "blocklist: release fetch failed");
            return;
        }
    };
    let Some(rel) = rel else { return };

    let now = crate::time::Timestamp::now().to_rfc3339();
    // Hash stored lowercase so the matches_release helper +
    // blocklist_hashes_normalized invariant agree on canonical form.
    let normalized_hash = rel.info_hash.as_ref().map(|h| h.to_lowercase());
    let insert_result = sqlx::query(
        "INSERT INTO blocklist (
            movie_id, episode_id, source_title, torrent_info_hash,
            indexer_id, size, resolution, source, video_codec, message, date
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(row.movie_id)
    .bind(row.episode_id)
    .bind(&rel.title)
    .bind(normalized_hash.as_deref())
    .bind(rel.indexer_id)
    .bind(rel.size)
    .bind(rel.resolution)
    .bind(rel.source.as_deref())
    .bind(rel.video_codec.as_deref())
    .bind(reason)
    .bind(&now)
    .execute(pool)
    .await;
    if let Err(e) = insert_result {
        tracing::warn!(release_id, error = %e, "blocklist insert failed");
        return;
    }

    // Clear last_searched_at on the content so the next sweep picks
    // it up immediately instead of waiting out the backoff tier.
    if let Some(mid) = row.movie_id {
        let _ = sqlx::query("UPDATE movie SET last_searched_at = NULL WHERE id = ?")
            .bind(mid)
            .execute(pool)
            .await;
    }
    if let Some(eid) = row.episode_id {
        let _ = sqlx::query("UPDATE episode SET last_searched_at = NULL WHERE id = ?")
            .bind(eid)
            .execute(pool)
            .await;
    }

    tracing::info!(
        download_id,
        release_id,
        movie_id = ?row.movie_id,
        episode_id = ?row.episode_id,
        %reason,
        "release blocklisted",
    );
}

/// Blocklist the release tied to a failed download and re-trigger a
/// search for its content so the next-best release gets grabbed. This
/// turns download failures (dead torrent, broken file, peers gone)
/// from a dead-end into self-healing retries.
///
/// Only call from *automatic* failure paths (dead timeout, import
/// failure) — user-initiated cancels should not loop back into search.
pub async fn blocklist_and_retry(state: &AppState, download_id: i64, reason: &str) {
    blocklist_release_for_download(state, download_id, reason).await;

    // Kick the scheduler to re-run wanted_search. Non-fatal if the
    // channel is full (another trigger is pending; it'll cover this).
    let _ = state
        .trigger_tx
        .try_send(crate::scheduler::TaskTrigger::fire("wanted_search"));
}
