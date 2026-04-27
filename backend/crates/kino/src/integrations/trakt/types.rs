//! Wire-format types for the Trakt v2 API. We model only the fields
//! we use — adding more is cheap thanks to serde's flatten + defaults,
//! and keeping the surface narrow means a benign API addition never
//! breaks our deserialisers.

use serde::{Deserialize, Serialize};

// ── OAuth / device code ───────────────────────────────────────────

/// Response from `POST /oauth/device/code` — kicks off the flow.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Response from `POST /oauth/device/token` on success. On pending,
/// Trakt returns `400`; the poller handles that as a non-error.
#[derive(Debug, Clone, Deserialize)]
pub struct AccessToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub created_at: i64,
    #[serde(default)]
    pub scope: String,
}

// ── Identity helpers ──────────────────────────────────────────────

/// Bag of IDs Trakt attaches to every entity. We try these in order
/// when reconciling against our local library — see `reconcile.rs`.
/// All fields are `Option` because Trakt's data is as messy as TMDB's
/// and the absence of an ID is meaningful.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraktIds {
    #[serde(default)]
    pub trakt: Option<i64>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub imdb: Option<String>,
    #[serde(default)]
    pub tmdb: Option<i64>,
    #[serde(default)]
    pub tvdb: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movie {
    pub title: String,
    #[serde(default)]
    pub year: Option<i64>,
    pub ids: TraktIds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Show {
    pub title: String,
    #[serde(default)]
    pub year: Option<i64>,
    pub ids: TraktIds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    #[serde(default)]
    pub season: Option<i64>,
    #[serde(default)]
    pub number: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    pub ids: TraktIds,
}

// ── User endpoints ────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct UserSettings {
    pub user: User,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub username: String,
    pub ids: UserIds,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserIds {
    pub slug: String,
}

// ── Sync: history / ratings / watchlist / collection ──────────────

/// `/sync/last_activities` — a tree of timestamp buckets. We read only
/// the leaves we care about; everything else is ignored by serde.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LastActivities {
    #[serde(default)]
    pub movies: MovieActivities,
    #[serde(default)]
    pub episodes: EpisodeActivities,
    #[serde(default)]
    pub shows: ShowActivities,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MovieActivities {
    #[serde(default)]
    pub watched_at: Option<String>,
    #[serde(default)]
    pub rated_at: Option<String>,
    #[serde(default)]
    pub watchlisted_at: Option<String>,
    #[serde(default)]
    pub collected_at: Option<String>,
    #[serde(default)]
    pub paused_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EpisodeActivities {
    #[serde(default)]
    pub watched_at: Option<String>,
    #[serde(default)]
    pub rated_at: Option<String>,
    #[serde(default)]
    pub collected_at: Option<String>,
    #[serde(default)]
    pub paused_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShowActivities {
    #[serde(default)]
    pub rated_at: Option<String>,
    #[serde(default)]
    pub watchlisted_at: Option<String>,
}

/// Row of `/sync/watched/movies`.
#[derive(Debug, Clone, Deserialize)]
pub struct WatchedMovie {
    #[serde(default)]
    pub plays: i64,
    #[serde(default)]
    pub last_watched_at: Option<String>,
    pub movie: Movie,
}

/// Row of `/sync/watched/shows`. Each show contains nested season +
/// episode structs; we flatten to per-episode records for reconcile
/// in `sync.rs`.
#[derive(Debug, Clone, Deserialize)]
pub struct WatchedShow {
    pub show: Show,
    #[serde(default)]
    pub seasons: Vec<WatchedSeason>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchedSeason {
    pub number: i64,
    #[serde(default)]
    pub episodes: Vec<WatchedEpisode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchedEpisode {
    pub number: i64,
    #[serde(default)]
    pub plays: i64,
    #[serde(default)]
    pub last_watched_at: Option<String>,
}

/// Row of `/sync/ratings/{type}`.
#[derive(Debug, Clone, Deserialize)]
pub struct RatingRow {
    pub rated_at: String,
    pub rating: i64,
    #[serde(default)]
    pub movie: Option<Movie>,
    #[serde(default)]
    pub show: Option<Show>,
    #[serde(default)]
    pub episode: Option<Episode>,
}

/// Row of `/sync/watchlist/{type}`.
#[derive(Debug, Clone, Deserialize)]
pub struct WatchlistRow {
    pub listed_at: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub movie: Option<Movie>,
    #[serde(default)]
    pub show: Option<Show>,
}

// ── Scrobble ──────────────────────────────────────────────────────

/// Body for `/scrobble/{start|pause|stop}`. Include exactly one of
/// `movie` or `episode`; `progress` is 0.0–100.0.
#[derive(Debug, Clone, Serialize)]
pub struct ScrobbleBody {
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie: Option<Movie>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode: Option<Episode>,
    /// Include when posting an episode so Trakt can disambiguate
    /// cross-show collisions (e.g. two shows with an S01E01 but
    /// different show IDs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show: Option<Show>,
}

/// Response from a successful `/scrobble/*` call. We ignore most
/// fields — the `action` and `id` are useful for diagnostics only.
#[derive(Debug, Clone, Deserialize)]
pub struct ScrobbleAck {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub action: Option<String>,
}

// ── Sync: playback (cross-device resume) ──────────────────────────

/// Row of `/sync/playback/{movies,episodes}`. Trakt holds the most
/// recent paused position per item across all your devices; we read
/// progress (0.0–100.0 %) and `paused_at`, then translate against
/// the local row's runtime to derive ticks.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybackProgress {
    pub progress: f64,
    pub paused_at: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub movie: Option<Movie>,
    #[serde(default)]
    pub episode: Option<Episode>,
    #[serde(default)]
    pub show: Option<Show>,
}

// ── Home rows: trending + recommendations ─────────────────────────

/// Row of `/movies/trending` — note the wrapping shape, it's *not*
/// the same as `/movies/popular` which returns bare movies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendingMovie {
    #[serde(default)]
    pub watchers: i64,
    pub movie: Movie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendingShow {
    #[serde(default)]
    pub watchers: i64,
    pub show: Show,
}

// ── History push ──────────────────────────────────────────────────

/// Body for `POST /sync/history` — back-fill a watched timestamp for
/// a past play. The spec says prefer this over a live scrobble when
/// draining the offline queue with events older than a few minutes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HistoryPush {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub movies: Vec<HistoryEntry<Movie>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub shows: Vec<HistoryShow>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub episodes: Vec<HistoryEntry<Episode>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watched_at: Option<String>,
    #[serde(flatten)]
    pub item: T,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryShow {
    #[serde(flatten)]
    pub show: Show,
    pub seasons: Vec<HistorySeason>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistorySeason {
    pub number: i64,
    pub episodes: Vec<HistoryEntry<HistoryEpisodeNumber>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEpisodeNumber {
    pub number: i64,
}

/// Trakt responds with a summary of what it accepted. We log this on
/// non-zero rejections rather than erroring — the sync is additive.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncResult {
    #[serde(default)]
    pub added: SyncCounts,
    #[serde(default)]
    pub updated: SyncCounts,
    #[serde(default)]
    pub existing: SyncCounts,
    #[serde(default)]
    pub not_found: SyncNotFound,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SyncCounts {
    #[serde(default)]
    pub movies: i64,
    #[serde(default)]
    pub episodes: i64,
    #[serde(default)]
    pub shows: i64,
    #[serde(default)]
    pub seasons: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SyncNotFound {
    #[serde(default)]
    pub movies: serde_json::Value,
    #[serde(default)]
    pub shows: serde_json::Value,
    #[serde(default)]
    pub episodes: serde_json::Value,
}
