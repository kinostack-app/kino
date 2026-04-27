use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::content::derived_state::movie_status_select;
use crate::content::movie::model::{CreateMovie, Movie};
use crate::error::{AppError, AppResult};
use crate::pagination::{Cursor, PaginatedResponse, PaginationParams};
use crate::settings::quality_profile::resolve_quality_profile;
use crate::state::AppState;

/// List movies (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/movies",
    params(PaginationParams),
    responses((
        status = 200,
        body = PaginatedResponse<Movie>,
        description = "Paginated movie list"
    )),
    tag = "movies",
    security(("api_key" = []))
)]
pub async fn list_movies(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> AppResult<Json<PaginatedResponse<Movie>>> {
    let limit = params.limit();
    let fetch_limit = limit + 1; // Fetch one extra to detect has_more

    let cursor = params.cursor.as_deref().and_then(Cursor::decode);

    let movies = if let Some(c) = cursor {
        let sql = format!(
            "{} WHERE mv.id > ? ORDER BY mv.id ASC LIMIT ?",
            movie_status_select()
        );
        sqlx::query_as::<_, Movie>(&sql)
            .bind(c.id)
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!("{} ORDER BY mv.id ASC LIMIT ?", movie_status_select());
        sqlx::query_as::<_, Movie>(&sql)
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    };

    Ok(Json(PaginatedResponse::new(movies, limit, |m| Cursor {
        id: m.id,
        sort_value: None,
    })))
}

/// Get a movie by ID.
#[utoipa::path(
    get,
    path = "/api/v1/movies/{id}",
    params(("id" = i64, Path, description = "Movie ID")),
    responses(
        (status = 200, description = "Movie details", body = Movie),
        (status = 404, description = "Not found")
    ),
    tag = "movies",
    security(("api_key" = []))
)]
pub async fn get_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Movie>> {
    use crate::content::derived_state::movie_status_select;
    let sql = format!("{} WHERE mv.id = ?", movie_status_select());
    let movie = sqlx::query_as::<_, Movie>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("movie with id {id} not found")))?;
    Ok(Json(movie))
}

/// Internal helper: create a movie row + emit `MovieAdded`.
///
/// The split with the public [`create_movie`] handler is along the
/// real semantic boundary, not "bits a caller might want":
///
///   - `MovieAdded` is a **fact about the world** — the movie *was*
///     added. History wants it, webhooks want it, WS library caches
///     want it, every client needs to invalidate. No caller
///     meaningfully opts out; the event fires here.
///
///   - The `wanted_search` scheduler trigger is a **scheduling
///     decision** — "search for this soon." That's specific to
///     "user clicked Add and expects a download to start." The
///     watch-now flow does NOT want it (it runs its own inline
///     search via phase-2; the trigger would race the scheduler
///     ahead of watch-now's own placeholder row, producing a
///     duplicate download). That's why the trigger lives on the
///     outer handler and not in here.
#[allow(clippy::too_many_lines)]
pub(crate) async fn create_movie_inner(state: &AppState, input: CreateMovie) -> AppResult<Movie> {
    // Check if movie already exists
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM movie WHERE tmdb_id = ?")
        .bind(input.tmdb_id)
        .fetch_optional(&state.db)
        .await?;

    if let Some(id) = existing {
        return Err(AppError::Conflict(format!(
            "movie with tmdb_id {} already exists (id={id})",
            input.tmdb_id
        )));
    }

    // Resolve the target quality profile: explicit id wins; otherwise
    // fall back to the current default (the one row flagged is_default).
    let profile_id = resolve_quality_profile(&state.db, input.quality_profile_id).await?;

    // Fetch metadata from TMDB
    let tmdb = state.require_tmdb()?;
    let details = tmdb
        .movie_details(input.tmdb_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let now = crate::time::Timestamp::now().to_rfc3339();
    let year = details
        .release_date
        .as_deref()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<i64>().ok());

    let genres = details.genres.as_ref().map(|g| {
        serde_json::to_string(&g.iter().map(|x| &x.name).collect::<Vec<_>>()).unwrap_or_default()
    });

    let certification = details.release_dates.as_ref().and_then(|rd| {
        rd.results
            .iter()
            .find(|c| c.iso_3166_1 == "US")
            .and_then(|c| {
                c.release_dates
                    .iter()
                    .find_map(|r| r.certification.clone())
                    .filter(|s| !s.is_empty())
            })
    });

    let trailer = details.videos.as_ref().and_then(|v| {
        v.results
            .iter()
            .find(|t| t.site == "YouTube" && t.video_type == "Trailer")
            .map(|t| t.key.clone())
    });

    let collection_id = details.belongs_to_collection.as_ref().map(|c| c.id);
    let collection_name = details
        .belongs_to_collection
        .as_ref()
        .map(|c| c.name.clone());

    let imdb_id = details
        .external_ids
        .as_ref()
        .and_then(|e| e.imdb_id.clone());
    let tvdb_id = details.external_ids.as_ref().and_then(|e| e.tvdb_id);

    let monitored = input.monitored.unwrap_or(true);

    // Physical/digital release dates from release_dates (US, type 5=physical, 4=digital)
    let (physical_date, digital_date) = details.release_dates.as_ref().map_or((None, None), |rd| {
        let us = rd.results.iter().find(|c| c.iso_3166_1 == "US");
        let physical = us.and_then(|c| {
            c.release_dates
                .iter()
                .find(|r| r.release_type == 5)
                .and_then(|r| r.release_date.clone())
        });
        let digital = us.and_then(|c| {
            c.release_dates
                .iter()
                .find(|r| r.release_type == 4)
                .and_then(|r| r.release_date.clone())
        });
        (physical, digital)
    });

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO movie (tmdb_id, imdb_id, tvdb_id, title, original_title, overview, tagline, year, runtime, release_date, physical_release_date, digital_release_date, certification, poster_path, backdrop_path, genres, tmdb_rating, tmdb_vote_count, popularity, original_language, collection_tmdb_id, collection_name, youtube_trailer_id, quality_profile_id, monitored, added_at, last_metadata_refresh) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(input.tmdb_id)
    .bind(&imdb_id)
    .bind(tvdb_id)
    .bind(&details.title)
    .bind(&details.original_title)
    .bind(&details.overview)
    .bind(&details.tagline)
    .bind(year)
    .bind(details.runtime)
    .bind(&details.release_date)
    .bind(&physical_date)
    .bind(&digital_date)
    .bind(&certification)
    .bind(&details.poster_path)
    .bind(&details.backdrop_path)
    .bind(&genres)
    .bind(details.vote_average)
    .bind(details.vote_count)
    .bind(details.popularity)
    .bind(&details.original_language)
    .bind(collection_id)
    .bind(&collection_name)
    .bind(&trailer)
    .bind(profile_id)
    .bind(monitored)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    let sql = format!("{} WHERE mv.id = ?", movie_status_select());
    let movie = sqlx::query_as::<_, Movie>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    // Fact about the world — always fires. History row writer,
    // webhook dispatcher, and the WS forwarder all feed from this.
    state.emit(crate::events::AppEvent::MovieAdded {
        movie_id: movie.id,
        tmdb_id: input.tmdb_id,
        title: movie.title.clone(),
    });

    Ok(movie)
}

/// Request a movie (create from TMDB and set as wanted).
#[utoipa::path(
    post,
    path = "/api/v1/movies",
    request_body = CreateMovie,
    responses(
        (status = 201, description = "Movie created", body = Movie),
        (status = 409, description = "Movie already exists")
    ),
    tag = "movies",
    security(("api_key" = []))
)]
pub async fn create_movie(
    State(state): State<AppState>,
    Json(input): Json<CreateMovie>,
) -> AppResult<(StatusCode, Json<Movie>)> {
    let movie = create_movie_inner(&state, input).await?;

    // Scheduling decision — only fires for this "user added from
    // the library" entry point. Watch-now explicitly skips it by
    // calling `create_movie_inner` directly, because it runs its
    // own search inline and firing the trigger here would race the
    // scheduler ahead of watch-now's own placeholder-row creation,
    // producing a duplicate download.
    let _ = state
        .trigger_tx
        .try_send(crate::scheduler::TaskTrigger::fire("wanted_search"));

    Ok((StatusCode::CREATED, Json(movie)))
}

/// Delete a movie.
#[utoipa::path(
    delete,
    path = "/api/v1/movies/{id}",
    params(("id" = i64, Path, description = "Movie ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found")
    ),
    tag = "movies",
    security(("api_key" = []))
)]
pub async fn delete_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Fetch title up front for the `ContentRemoved` event below;
    // doubles as the existence check.
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM movie WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let Some(title) = title else {
        return Err(AppError::NotFound(format!("movie with id {id} not found")));
    };

    // Clean up associated downloads (must happen before movie delete due to FK)
    let download_ids: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT d.id, d.torrent_hash FROM download d JOIN download_content dc ON d.id = dc.download_id WHERE dc.movie_id = ?",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    for (dl_id, hash) in &download_ids {
        // Stop any live streaming state (trickplay + HLS transcode).
        // Without this, deleting a movie mid-stream leaves orphan
        // ffmpeg + transcode sessions eating CPU.
        state.stream_trickplay.stop(*dl_id).await;
        if let Some(ref transcode) = state.transcode {
            let session_id = format!("stream-{dl_id}");
            if let Err(e) = transcode.stop_session(&session_id).await {
                tracing::warn!(
                    download_id = dl_id,
                    %session_id,
                    error = %e,
                    "failed to stop stream transcode on movie delete",
                );
            }
        }
        if let (Some(client), Some(h)) = (&state.torrent, hash) {
            // Removal can fail if the torrent was already evicted
            // from the session (race with stall-detector). Queue the
            // retry so a real failure doesn't strand the orphan
            // forever.
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
                    "torrent removal queued for retry (movie delete)",
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

    // Clean up releases, media, history (before movie delete due to FK).
    // Library files have to be removed *before* the media DB row is
    // deleted — once the row's gone we've lost the path. The previous
    // code just dropped the row, leaving orphan video files on disk
    // that piled up over time.
    sqlx::query("DELETE FROM release WHERE movie_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    let media_paths: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, file_path FROM media WHERE movie_id = ?")
            .bind(id)
            .fetch_all(&state.db)
            .await?;
    let library_root = fetch_library_root(&state.db).await;
    let library_root_path = library_root.as_deref().map(std::path::Path::new);
    for (media_id, path) in &media_paths {
        remove_library_file(*media_id, path, library_root_path).await;
    }
    sqlx::query("DELETE FROM media WHERE movie_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM history WHERE movie_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    // Now safe to delete the movie
    sqlx::query("DELETE FROM movie WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    // Notify other clients so their caches invalidate (library
    // list, downloads, history).
    state.emit(crate::events::AppEvent::ContentRemoved {
        movie_id: Some(id),
        show_id: None,
        title,
    });

    Ok(StatusCode::NO_CONTENT)
}

/// Remove a library file from disk, logging-not-failing on error so
/// the surrounding DB delete always proceeds. A "ghost" media row
/// pointing at a missing file is benign (startup reconcile cleans it
/// up); a stranded file we can't see is a disk-fill in waiting.
///
/// Pulled into a free function so `delete_movie` here and
/// `delete_show` in shows.rs share one helper instead of duplicating
/// the warn-and-continue pattern.
///
/// `library_root` is the configured `media_library_path`; when
/// provided, we also `rmdir` any newly-empty ancestor directories
/// bounded by that root so deleting the last episode of a season
/// doesn't leave `Season 01/` / `Show Name/` behind forever.
pub(crate) async fn remove_library_file(
    media_id: i64,
    path: &str,
    library_root: Option<&std::path::Path>,
) {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return;
    }
    if let Err(e) = tokio::fs::remove_file(p).await {
        tracing::warn!(
            media_id,
            path,
            error = %e,
            "failed to remove library file on delete; DB row removed anyway",
        );
        return;
    }
    if let Some(parent) = p.parent() {
        crate::cleanup::cleanup_empty_dirs(parent, library_root).await;
    }
}

/// Fetch the configured library root for empty-dir pruning. Returns
/// `None` when unset (setup wizard not run) or blank — callers then
/// skip the dir-prune half of the cleanup and just unlink the file.
pub(crate) async fn fetch_library_root(db: &sqlx::SqlitePool) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT media_library_path FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}
