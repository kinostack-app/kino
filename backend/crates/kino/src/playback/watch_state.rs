//! Movie watched-state handlers. Everything play-related (direct,
//! HLS, subtitles, trickplay, progress, transcode) moved to
//! `api/play.rs` under the unified `/api/v1/play/...` surface.

use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Mark content as watched manually.
#[utoipa::path(
    post, path = "/api/v1/movies/{id}/watched",
    params(("id" = i64, Path)),
    responses((status = 200), (status = 404)),
    tag = "movies", security(("api_key" = []))
)]
pub async fn mark_movie_watched(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE movie SET watched_at = ?, play_count = play_count + 1, playback_position_ticks = 0 WHERE id = ?",
    )
    .bind(&now)
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("movie {id} not found")));
    }
    let title: String = sqlx::query_scalar("SELECT title FROM movie WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    state.emit(crate::events::AppEvent::Watched {
        movie_id: Some(id),
        episode_id: None,
        title,
    });
    push_watched_to_trakt(&state, Some(id), None, Some(now)).await;
    Ok(StatusCode::OK)
}

/// Unmark content as watched (reset to available).
#[utoipa::path(
    delete, path = "/api/v1/movies/{id}/watched",
    params(("id" = i64, Path)),
    responses((status = 200), (status = 404)),
    tag = "movies", security(("api_key" = []))
)]
pub async fn unmark_movie_watched(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let result = sqlx::query(
        "UPDATE movie SET watched_at = NULL, playback_position_ticks = 0 WHERE id = ? AND watched_at IS NOT NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "movie {id} not found or not watched"
        )));
    }
    let title: String = sqlx::query_scalar("SELECT title FROM movie WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    state.emit(crate::events::AppEvent::Unwatched {
        movie_id: Some(id),
        episode_id: None,
        title,
    });
    push_unwatch_to_trakt(&state, Some(id), None).await;
    Ok(StatusCode::OK)
}

/// Best-effort Trakt history push. Failure is logged, never returned —
/// the local update already succeeded and the periodic sweep will
/// reconcile if the connection comes back up.
async fn push_watched_to_trakt(
    state: &AppState,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
    watched_at: Option<String>,
) {
    use crate::integrations::trakt;
    if let Ok(client) = trakt::client_for(state).await
        && let Err(e) = trakt::sync::push_watched(&client, movie_id, episode_id, watched_at).await
    {
        tracing::warn!(error = %e, ?movie_id, ?episode_id, "trakt watched push failed");
    }
}

async fn push_unwatch_to_trakt(state: &AppState, movie_id: Option<i64>, episode_id: Option<i64>) {
    use crate::integrations::trakt;
    if let Ok(client) = trakt::client_for(state).await
        && let Err(e) = trakt::sync::push_unwatch(&client, movie_id, episode_id).await
    {
        tracing::warn!(error = %e, ?movie_id, ?episode_id, "trakt unwatch push failed");
    }
}
