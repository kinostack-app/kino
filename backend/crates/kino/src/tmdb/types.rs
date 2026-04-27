use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// TMDB multi-search result (movie or TV).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbSearchResult {
    pub id: i64,
    pub media_type: String,
    pub title: Option<String>,
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub release_date: Option<String>,
    pub first_air_date: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub original_language: Option<String>,
    pub genre_ids: Option<Vec<i64>>,
}

/// TMDB paginated response. Generic shape used at the client
/// layer; concrete monomorphisations for each of the three
/// callers below carry the `ToSchema` impl so utoipa can emit
/// honest response bodies on the path macros (generic schemas
/// don't cross into `OpenAPI`).
#[derive(Debug, Serialize, Deserialize)]
pub struct TmdbPagedResponse<T> {
    pub page: i64,
    pub results: Vec<T>,
    pub total_pages: i64,
    pub total_results: i64,
}

/// Paginated `search` response — `TmdbPagedResponse<TmdbSearchResult>`
/// made concrete so utoipa can register a schema for it.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TmdbPagedSearchResult {
    pub page: i64,
    pub results: Vec<TmdbSearchResult>,
    pub total_pages: i64,
    pub total_results: i64,
}

impl From<TmdbPagedResponse<TmdbSearchResult>> for TmdbPagedSearchResult {
    fn from(r: TmdbPagedResponse<TmdbSearchResult>) -> Self {
        Self {
            page: r.page,
            results: r.results,
            total_pages: r.total_pages,
            total_results: r.total_results,
        }
    }
}

/// Paginated trending / discover-movie response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TmdbPagedDiscoverMovie {
    pub page: i64,
    pub results: Vec<TmdbDiscoverMovie>,
    pub total_pages: i64,
    pub total_results: i64,
}

impl From<TmdbPagedResponse<TmdbDiscoverMovie>> for TmdbPagedDiscoverMovie {
    fn from(r: TmdbPagedResponse<TmdbDiscoverMovie>) -> Self {
        Self {
            page: r.page,
            results: r.results,
            total_pages: r.total_pages,
            total_results: r.total_results,
        }
    }
}

/// Paginated trending / discover-show response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TmdbPagedDiscoverShow {
    pub page: i64,
    pub results: Vec<TmdbDiscoverShow>,
    pub total_pages: i64,
    pub total_results: i64,
}

impl From<TmdbPagedResponse<TmdbDiscoverShow>> for TmdbPagedDiscoverShow {
    fn from(r: TmdbPagedResponse<TmdbDiscoverShow>) -> Self {
        Self {
            page: r.page,
            results: r.results,
            total_pages: r.total_pages,
            total_results: r.total_results,
        }
    }
}

/// TMDB movie details (with appended responses).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbMovieDetails {
    pub id: i64,
    pub imdb_id: Option<String>,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub runtime: Option<i64>,
    pub release_date: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub original_language: Option<String>,
    pub genres: Option<Vec<TmdbGenre>>,
    pub belongs_to_collection: Option<TmdbCollection>,
    pub external_ids: Option<TmdbExternalIds>,
    pub release_dates: Option<TmdbReleaseDatesWrapper>,
    pub videos: Option<TmdbVideosWrapper>,
}

/// TMDB TV show details.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbShowDetails {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub first_air_date: Option<String>,
    pub last_air_date: Option<String>,
    pub status: Option<String>,
    pub number_of_seasons: Option<i64>,
    pub number_of_episodes: Option<i64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub original_language: Option<String>,
    pub genres: Option<Vec<TmdbGenre>>,
    pub networks: Option<Vec<TmdbNetwork>>,
    pub episode_run_time: Option<Vec<i64>>,
    pub external_ids: Option<TmdbExternalIds>,
    pub content_ratings: Option<TmdbContentRatingsWrapper>,
    pub videos: Option<TmdbVideosWrapper>,
    pub seasons: Option<Vec<TmdbSeasonSummary>>,
}

/// TMDB season details.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbSeasonDetails {
    pub id: i64,
    pub season_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    pub episodes: Option<Vec<TmdbEpisode>>,
    pub external_ids: Option<TmdbExternalIds>,
}

/// TMDB episode within a season.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbEpisode {
    pub id: i64,
    pub episode_number: i64,
    pub season_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i64>,
    pub still_path: Option<String>,
    pub vote_average: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbGenre {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbCollection {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbNetwork {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbExternalIds {
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbReleaseDatesWrapper {
    pub results: Vec<TmdbReleaseDateCountry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbReleaseDateCountry {
    pub iso_3166_1: String,
    pub release_dates: Vec<TmdbReleaseDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbReleaseDate {
    pub certification: Option<String>,
    #[serde(rename = "type")]
    pub release_type: i64,
    pub release_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbContentRatingsWrapper {
    pub results: Vec<TmdbContentRating>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbContentRating {
    pub iso_3166_1: String,
    pub rating: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbVideosWrapper {
    pub results: Vec<TmdbVideo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbVideo {
    pub key: String,
    pub site: String,
    #[serde(rename = "type")]
    pub video_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TmdbSeasonSummary {
    pub id: i64,
    pub season_number: i64,
    pub name: Option<String>,
    pub episode_count: Option<i64>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
}

/// TMDB discover/trending result (movie format).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbDiscoverMovie {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub release_date: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub genre_ids: Option<Vec<i64>>,
}

/// TMDB discover/trending result (TV format).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TmdbDiscoverShow {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub first_air_date: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub genre_ids: Option<Vec<i64>>,
}

/// Genre list response.
#[derive(Debug, Deserialize)]
pub struct TmdbGenreList {
    pub genres: Vec<TmdbGenre>,
}

/// One entry from TMDB's `/images` response. TMDB nests these under
/// `posters`, `backdrops`, or `logos` depending on the asset kind.
/// Fields we don't currently use are omitted to keep the struct small.
#[derive(Debug, Clone, Deserialize)]
pub struct TmdbImageEntry {
    pub file_path: String,
    pub aspect_ratio: f64,
    pub vote_average: f64,
    pub vote_count: i64,
    pub width: i64,
    pub height: i64,
    /// ISO 639-1 or null (language-agnostic wordmark).
    #[serde(default)]
    pub iso_639_1: Option<String>,
}

/// Response shape for `/movie/{id}/images` and `/tv/{id}/images`.
/// Only `logos` is consumed today — `backdrops` / `posters` already
/// come from the details endpoints.
#[derive(Debug, Deserialize)]
pub struct TmdbImages {
    #[serde(default)]
    pub logos: Vec<TmdbImageEntry>,
}
