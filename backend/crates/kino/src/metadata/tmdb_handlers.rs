use axum::Json;
use axum::extract::{Query, State};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tmdb::client::DiscoverFilters;
use crate::tmdb::types::{
    TmdbGenre, TmdbMovieDetails, TmdbPagedDiscoverMovie, TmdbPagedDiscoverShow,
    TmdbPagedSearchResult, TmdbShowDetails,
};

/// Response shape for `GET /api/v1/tmdb/genres` — TMDB segments
/// its genre catalogue by movie vs. TV, and the UI needs both
/// lists to populate filters on the two Browse pages without
/// firing the request twice.
#[derive(Debug, Serialize, ToSchema)]
pub struct GenresResponse {
    pub movie: Vec<TmdbGenre>,
    pub tv: Vec<TmdbGenre>,
}

fn build_filters(q: &DiscoverQuery) -> DiscoverFilters {
    DiscoverFilters {
        page: q.page,
        genre_id: q.genre_id,
        year: q.year,
        year_from: q.year_from,
        year_to: q.year_to,
        sort_by: q.sort_by.clone(),
        vote_average_gte: q.vote_average_gte,
        language: q.language.clone(),
    }
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct SearchQuery {
    pub q: String,
    pub page: Option<i64>,
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct DiscoverQuery {
    pub page: Option<i64>,
    pub genre_id: Option<i64>,
    /// Single-year shortcut — kept for callers that only need an
    /// exact year and don't want to build a range. Prefer
    /// `year_from` / `year_to` for presets like "2010s".
    pub year: Option<i64>,
    /// Inclusive lower bound of the release-date range, as a 4-digit
    /// year. Mapped server-side to TMDB's `primary_release_date.gte`
    /// (movies) / `first_air_date.gte` (shows).
    pub year_from: Option<i64>,
    /// Inclusive upper bound of the release-date range, as a 4-digit
    /// year. Mapped to the `.lte` counterparts.
    pub year_to: Option<i64>,
    pub sort_by: Option<String>,
    pub vote_average_gte: Option<f64>,
    pub language: Option<String>,
}

/// Search TMDB for movies and TV shows.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/search",
    params(SearchQuery),
    responses((status = 200, description = "Search results", body = TmdbPagedSearchResult)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<TmdbPagedSearchResult>> {
    let tmdb = state.require_tmdb()?;
    let results = tmdb
        .search_multi(&q.q, q.page)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(results.into()))
}

/// Get TMDB movie details by TMDB ID.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/movies/{tmdb_id}",
    params(("tmdb_id" = i64, Path, description = "TMDB movie ID")),
    responses((status = 200, description = "Movie details", body = TmdbMovieDetails)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn movie_details(
    State(state): State<AppState>,
    axum::extract::Path(tmdb_id): axum::extract::Path<i64>,
) -> AppResult<Json<TmdbMovieDetails>> {
    let tmdb = state.require_tmdb()?;
    let details = tmdb
        .movie_details(tmdb_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(details))
}

/// Get TMDB show details by TMDB ID.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/shows/{tmdb_id}",
    params(("tmdb_id" = i64, Path, description = "TMDB show ID")),
    responses((status = 200, description = "Show details", body = TmdbShowDetails)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn show_details(
    State(state): State<AppState>,
    axum::extract::Path(tmdb_id): axum::extract::Path<i64>,
) -> AppResult<Json<TmdbShowDetails>> {
    let tmdb = state.require_tmdb()?;
    let details = tmdb
        .show_details(tmdb_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(details))
}

/// Get TMDB season details (episodes) for a show.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/shows/{tmdb_id}/seasons/{season_number}",
    params(
        ("tmdb_id" = i64, Path, description = "TMDB show ID"),
        ("season_number" = i64, Path, description = "Season number"),
    ),
    responses((status = 200, description = "Season details with episodes", body = crate::tmdb::types::TmdbSeasonDetails)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn season_details(
    State(state): State<AppState>,
    axum::extract::Path((tmdb_id, season_number)): axum::extract::Path<(i64, i64)>,
) -> AppResult<Json<crate::tmdb::types::TmdbSeasonDetails>> {
    let tmdb = state.require_tmdb()?;
    let details = tmdb
        .season_details(tmdb_id, season_number)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(details))
}

/// Trending movies this week.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/trending/movies",
    responses((status = 200, description = "Trending movies", body = TmdbPagedDiscoverMovie)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn trending_movies(
    State(state): State<AppState>,
) -> AppResult<Json<TmdbPagedDiscoverMovie>> {
    let tmdb = state.require_tmdb()?;
    let results = tmdb
        .trending_movies()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(results.into()))
}

/// Trending TV shows this week.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/trending/shows",
    responses((status = 200, description = "Trending shows", body = TmdbPagedDiscoverShow)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn trending_shows(
    State(state): State<AppState>,
) -> AppResult<Json<TmdbPagedDiscoverShow>> {
    let tmdb = state.require_tmdb()?;
    let results = tmdb
        .trending_shows()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(results.into()))
}

/// Discover movies (popular, optional genre filter).
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/discover/movies",
    params(DiscoverQuery),
    responses((status = 200, description = "Discover movies", body = TmdbPagedDiscoverMovie)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn discover_movies(
    State(state): State<AppState>,
    Query(q): Query<DiscoverQuery>,
) -> AppResult<Json<TmdbPagedDiscoverMovie>> {
    let tmdb = state.require_tmdb()?;
    let filters = build_filters(&q);
    let results = tmdb
        .discover_movies(&filters)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(results.into()))
}

/// Discover TV shows with filters.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/discover/shows",
    params(DiscoverQuery),
    responses((status = 200, description = "Discover shows", body = TmdbPagedDiscoverShow)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn discover_shows(
    State(state): State<AppState>,
    Query(q): Query<DiscoverQuery>,
) -> AppResult<Json<TmdbPagedDiscoverShow>> {
    let tmdb = state.require_tmdb()?;
    let filters = build_filters(&q);
    let results = tmdb
        .discover_shows(&filters)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(results.into()))
}

/// Get movie and TV genre lists.
#[utoipa::path(
    get,
    path = "/api/v1/tmdb/genres",
    responses((status = 200, description = "Genre lists", body = GenresResponse)),
    tag = "tmdb",
    security(("api_key" = []))
)]
pub async fn genres(State(state): State<AppState>) -> AppResult<Json<GenresResponse>> {
    let tmdb = state.require_tmdb()?;
    let (movies, tv) = tokio::try_join!(tmdb.movie_genres(), tmdb.tv_genres())
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(GenresResponse { movie: movies, tv }))
}
