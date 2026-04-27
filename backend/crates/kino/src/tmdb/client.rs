use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use tokio::sync::Mutex;

use super::types::{
    TmdbDiscoverMovie, TmdbDiscoverShow, TmdbGenre, TmdbGenreList, TmdbImages, TmdbMovieDetails,
    TmdbPagedResponse, TmdbSearchResult, TmdbSeasonDetails, TmdbShowDetails,
};

/// Production TMDB base URL. Used as the default when no override
/// is supplied; tests construct the client with a wiremock URL via
/// [`TmdbClient::with_base_url`].
pub const DEFAULT_TMDB_BASE: &str = "https://api.themoviedb.org/3";

/// Sliding-window rate limit matching TMDB's undocumented ceiling of
/// ~50 requests per 10-second window. We target 40 / 10s to stay
/// comfortably below — bursts up to 40 are fine, but the 41st
/// request in a window blocks until the oldest ages out. Shared
/// across clones of the client via `Arc`.
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_CAPACITY: usize = 40;

/// Retry budget for 429 / 5xx. Zero means "no retries". `3` gives
/// one initial attempt + two retries, spaced by exponential backoff
/// (1s → 2s) unless the server sends `Retry-After`.
const MAX_ATTEMPTS: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_secs(1);
/// Cap the `Retry-After` value we'll honour, to protect against a
/// misbehaving upstream asking us to wait 10 minutes on a refresh
/// sweep. If TMDB really needs >30s we'd rather surface an error
/// and let the scheduler retry on its next tick.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Simple sliding-window rate limiter. On `acquire`, drops any
/// timestamps older than the window, then if we're at capacity
/// sleeps until the oldest entry ages out; otherwise records `now`
/// and returns immediately.
#[derive(Debug)]
struct RateLimiter {
    state: Mutex<VecDeque<Instant>>,
    capacity: usize,
    window: Duration,
}

impl RateLimiter {
    fn new(capacity: usize, window: Duration) -> Self {
        Self {
            state: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            window,
        }
    }

    async fn acquire(&self) {
        loop {
            let sleep_for = {
                let mut q = self.state.lock().await;
                let now = Instant::now();
                while q
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > self.window)
                {
                    q.pop_front();
                }
                if q.len() < self.capacity {
                    q.push_back(now);
                    return;
                }
                // Wait for the oldest to age out. Safe unwrap: we
                // just confirmed len == capacity > 0.
                let oldest = *q.front().expect("queue at capacity cannot be empty");
                (oldest + self.window).saturating_duration_since(now)
            };
            // Log throttling so operators can see limiter pressure in
            // traces — silently sleeping would look like a hung
            // request.
            tracing::debug!(
                wait_ms = u64::try_from(sleep_for.as_millis()).unwrap_or(u64::MAX),
                "tmdb rate limiter throttling"
            );
            tokio::time::sleep(sleep_for).await;
        }
    }
}

/// TMDB API client with rate limiting + retry.
#[derive(Debug, Clone)]
pub struct TmdbClient {
    http: Client,
    api_key: String,
    /// Base URL (no trailing slash). Configurable so integration
    /// tests can point at a wiremock server without env-var games.
    base_url: String,
    limiter: Arc<RateLimiter>,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, DEFAULT_TMDB_BASE.to_owned())
    }

    /// Constructor for tests + future env-driven config: routes all
    /// requests at the given base URL. No trailing slash.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            http: Client::new(),
            api_key,
            base_url,
            limiter: Arc::new(RateLimiter::new(RATE_CAPACITY, RATE_WINDOW)),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Compose a request URL against `self.base_url`. Centralising the
    /// prefix here lets the constructor alone control where requests
    /// go — individual endpoint methods don't carry the prefix inline.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, TmdbError> {
        // Retry on 429 / 5xx / transport errors. Each attempt takes
        // a rate-limit slot so we never blow through TMDB's ceiling
        // by retrying blindly.
        let path = url.strip_prefix(&self.base_url[..]).unwrap_or(url);
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            self.limiter.acquire().await;
            match self.get_once::<T>(url, path).await {
                Ok(v) => return Ok(v),
                Err(e) if attempt < MAX_ATTEMPTS && e.is_transient() => {
                    let wait = e.retry_delay(attempt);
                    tracing::info!(
                        path,
                        attempt,
                        delay_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                        error = %e,
                        "tmdb transient error — retrying"
                    );
                    tokio::time::sleep(wait).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn get_once<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        path: &str,
    ) -> Result<T, TmdbError> {
        let start = std::time::Instant::now();
        tracing::debug!(path, "tmdb GET");

        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(path, error = %e, "tmdb request failed");
                TmdbError::Network(e.to_string())
            })?;

        let status = resp.status();
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        if status == reqwest::StatusCode::NOT_FOUND {
            tracing::debug!(path, status = 404, duration_ms, "tmdb not-found");
            return Err(TmdbError::NotFound);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // TMDB emits `Retry-After` in seconds. Bound it so a
            // misbehaving proxy can't park us for 10 minutes.
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs)
                .map(|d| d.min(MAX_RETRY_AFTER));
            tracing::warn!(
                path,
                duration_ms,
                retry_after_s = retry_after.map(|d| d.as_secs()),
                "tmdb rate-limited"
            );
            return Err(TmdbError::RateLimited { retry_after });
        }
        if status.is_server_error() {
            tracing::warn!(path, status = status.as_u16(), duration_ms, "tmdb 5xx");
            return Err(TmdbError::Server(status.as_u16()));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(
                path,
                status = status.as_u16(),
                duration_ms,
                body_prefix = %body.chars().take(120).collect::<String>(),
                "tmdb non-2xx",
            );
            return Err(TmdbError::Api(status.as_u16(), body));
        }

        let parsed = resp
            .json()
            .await
            .map_err(|e| TmdbError::Parse(e.to_string()));
        tracing::debug!(
            path,
            status = status.as_u16(),
            duration_ms,
            ok = parsed.is_ok(),
            "tmdb response",
        );
        parsed
    }

    /// Search movies and TV shows.
    pub async fn search_multi(
        &self,
        query: &str,
        page: Option<i64>,
    ) -> Result<TmdbPagedResponse<TmdbSearchResult>, TmdbError> {
        let page = page.unwrap_or(1);
        let url = self.url(&format!(
            "/search/multi?query={}&page={page}",
            urlencoding::encode(query)
        ));
        self.get(&url).await
    }

    /// Get full movie details with external IDs, release dates, and videos.
    pub async fn movie_details(&self, tmdb_id: i64) -> Result<TmdbMovieDetails, TmdbError> {
        let url = self.url(&format!(
            "/movie/{tmdb_id}?append_to_response=external_ids,release_dates,videos"
        ));
        self.get(&url).await
    }

    /// Get full show details with external IDs, content ratings, and videos.
    pub async fn show_details(&self, tmdb_id: i64) -> Result<TmdbShowDetails, TmdbError> {
        let url = self.url(&format!(
            "/tv/{tmdb_id}?append_to_response=external_ids,content_ratings,videos"
        ));
        self.get(&url).await
    }

    /// Get season details with episodes and external IDs.
    pub async fn season_details(
        &self,
        show_tmdb_id: i64,
        season_number: i64,
    ) -> Result<TmdbSeasonDetails, TmdbError> {
        let url = self.url(&format!(
            "/tv/{show_tmdb_id}/season/{season_number}?append_to_response=external_ids"
        ));
        self.get(&url).await
    }

    /// Trending movies this week.
    pub async fn trending_movies(&self) -> Result<TmdbPagedResponse<TmdbDiscoverMovie>, TmdbError> {
        let url = self.url("/trending/movie/week");
        self.get(&url).await
    }

    /// Trending TV shows this week.
    pub async fn trending_shows(&self) -> Result<TmdbPagedResponse<TmdbDiscoverShow>, TmdbError> {
        let url = self.url("/trending/tv/week");
        self.get(&url).await
    }

    /// Discover movies with filters.
    pub async fn discover_movies(
        &self,
        filters: &DiscoverFilters,
    ) -> Result<TmdbPagedResponse<TmdbDiscoverMovie>, TmdbError> {
        let url = self.url(&build_discover_path("movie", filters));
        self.get(&url).await
    }

    /// Discover TV shows with filters.
    pub async fn discover_shows(
        &self,
        filters: &DiscoverFilters,
    ) -> Result<TmdbPagedResponse<TmdbDiscoverShow>, TmdbError> {
        let url = self.url(&build_discover_path("tv", filters));
        self.get(&url).await
    }

    /// Movie genre list.
    pub async fn movie_genres(&self) -> Result<Vec<TmdbGenre>, TmdbError> {
        let url = self.url("/genre/movie/list");
        let resp: TmdbGenreList = self.get(&url).await?;
        Ok(resp.genres)
    }

    /// TV genre list.
    pub async fn tv_genres(&self) -> Result<Vec<TmdbGenre>, TmdbError> {
        let url = self.url("/genre/tv/list");
        let resp: TmdbGenreList = self.get(&url).await?;
        Ok(resp.genres)
    }

    /// Fetch the `/images` response for a movie. Filtered to English
    /// and language-agnostic wordmarks (`include_image_language=en,null`).
    /// Only the `logos` array is consumed today — posters + backdrops
    /// already arrive via the details endpoints. Returns an empty
    /// vector when TMDB has nothing to surface.
    pub async fn movie_logos(&self, tmdb_id: i64) -> Result<TmdbImages, TmdbError> {
        let url = self.url(&format!(
            "/movie/{tmdb_id}/images?include_image_language=en,null"
        ));
        self.get(&url).await
    }

    /// TV counterpart of [`Self::movie_logos`].
    pub async fn show_logos(&self, tmdb_id: i64) -> Result<TmdbImages, TmdbError> {
        let url = self.url(&format!(
            "/tv/{tmdb_id}/images?include_image_language=en,null"
        ));
        self.get(&url).await
    }
}

/// TMDB client errors.
/// Filters for TMDB discover endpoints.
#[derive(Debug, Default)]
pub struct DiscoverFilters {
    pub page: Option<i64>,
    pub genre_id: Option<i64>,
    /// Single-year filter — maps to TMDB's `primary_release_year` or
    /// `first_air_date_year`. Superseded by `year_from` / `year_to`
    /// when those are set (range wins over exact year).
    pub year: Option<i64>,
    pub year_from: Option<i64>,
    pub year_to: Option<i64>,
    pub sort_by: Option<String>,
    pub vote_average_gte: Option<f64>,
    pub language: Option<String>,
}

fn build_discover_path(media_type: &str, f: &DiscoverFilters) -> String {
    let page = f.page.unwrap_or(1);
    let sort = f.sort_by.as_deref().unwrap_or("popularity.desc");
    let mut url = format!("/discover/{media_type}?sort_by={sort}&page={page}");

    if let Some(g) = f.genre_id {
        let _ = write!(url, "&with_genres={g}");
    }

    // Range filter wins over exact-year — map to TMDB's date-range
    // params which work correctly with any sort order, unlike
    // `primary_release_year` which narrows to exactly one year.
    // Field names differ by media type: movies use
    // `primary_release_date`, shows use `first_air_date`.
    let (gte_field, lte_field, single_field) = if media_type == "movie" {
        (
            "primary_release_date.gte",
            "primary_release_date.lte",
            "primary_release_year",
        )
    } else {
        (
            "first_air_date.gte",
            "first_air_date.lte",
            "first_air_date_year",
        )
    };
    if f.year_from.is_some() || f.year_to.is_some() {
        if let Some(from) = f.year_from {
            let _ = write!(url, "&{gte_field}={from:04}-01-01");
        }
        if let Some(to) = f.year_to {
            let _ = write!(url, "&{lte_field}={to:04}-12-31");
        }
    } else if let Some(y) = f.year {
        let _ = write!(url, "&{single_field}={y}");
    }

    if let Some(v) = f.vote_average_gte {
        let _ = write!(url, "&vote_average.gte={v}");
    }
    if let Some(ref lang) = f.language {
        let _ = write!(url, "&with_original_language={lang}");
    }
    url
}

#[derive(Debug, thiserror::Error)]
pub enum TmdbError {
    #[error("TMDB resource not found")]
    NotFound,
    #[error("TMDB rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("TMDB server error: {0}")]
    Server(u16),
    #[error("TMDB API error {0}: {1}")]
    Api(u16, String),
    #[error("network error: {0}")]
    Network(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl TmdbError {
    /// Whether this error is worth retrying. Network timeouts, 429s,
    /// and 5xx responses are transient; 404 / 4xx-other / parse
    /// errors are not (retrying won't change the outcome).
    fn is_transient(&self) -> bool {
        matches!(
            self,
            TmdbError::RateLimited { .. } | TmdbError::Server(_) | TmdbError::Network(_)
        )
    }

    /// How long to wait before the next retry. Honours server-side
    /// `Retry-After` when present; otherwise exponential backoff of
    /// `BASE_BACKOFF * 2^(attempt - 1)`.
    fn retry_delay(&self, attempt: u32) -> Duration {
        if let TmdbError::RateLimited {
            retry_after: Some(d),
        } = self
        {
            return *d;
        }
        BASE_BACKOFF * 2u32.pow(attempt.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::{RateLimiter, TmdbError};
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn rate_limiter_allows_burst_then_throttles() {
        let limiter = RateLimiter::new(3, Duration::from_millis(200));
        let start = Instant::now();
        // First 3 should be near-instant.
        for _ in 0..3 {
            limiter.acquire().await;
        }
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "burst slot should be immediate"
        );
        // 4th must wait for the window to slide.
        limiter.acquire().await;
        assert!(
            start.elapsed() >= Duration::from_millis(180),
            "4th call should wait ~window_ms, got {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn transient_errors_classify_correctly() {
        assert!(TmdbError::RateLimited { retry_after: None }.is_transient());
        assert!(TmdbError::Server(503).is_transient());
        assert!(TmdbError::Network("timeout".into()).is_transient());
        assert!(!TmdbError::NotFound.is_transient());
        assert!(!TmdbError::Api(400, "bad".into()).is_transient());
        assert!(!TmdbError::Parse("nope".into()).is_transient());
    }

    #[test]
    fn retry_delay_honours_server_retry_after() {
        let err = TmdbError::RateLimited {
            retry_after: Some(Duration::from_secs(5)),
        };
        assert_eq!(err.retry_delay(1), Duration::from_secs(5));
        assert_eq!(err.retry_delay(2), Duration::from_secs(5));
    }

    #[test]
    fn retry_delay_backs_off_exponentially() {
        let err = TmdbError::Server(503);
        assert_eq!(err.retry_delay(1), Duration::from_secs(1));
        assert_eq!(err.retry_delay(2), Duration::from_secs(2));
        assert_eq!(err.retry_delay(3), Duration::from_secs(4));
    }
}
