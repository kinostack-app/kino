#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::models::enums::ContentStatus;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Movie {
    pub id: i64,
    pub tmdb_id: i64,
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub year: Option<i64>,
    pub runtime: Option<i64>,
    pub release_date: Option<String>,
    pub physical_release_date: Option<String>,
    pub digital_release_date: Option<String>,
    pub certification: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub genres: Option<String>,
    pub tmdb_rating: Option<f64>,
    pub tmdb_vote_count: Option<i64>,
    pub popularity: Option<f64>,
    pub original_language: Option<String>,
    pub collection_tmdb_id: Option<i64>,
    pub collection_name: Option<String>,
    pub youtube_trailer_id: Option<String>,
    pub quality_profile_id: i64,
    /// Derived phase (`wanted` / `downloading` / `available` / `watched`).
    /// Not a persisted column — SELECTs add a `CASE` expression that
    /// computes it from media + active-download + `watched_at`. Callers
    /// building a Movie row by hand can leave this empty; responses
    /// always come through a DB SELECT that populates it.
    #[serde(default)]
    #[schema(value_type = ContentStatus)]
    pub status: String,
    pub monitored: bool,
    pub added_at: String,
    /// Debounce timestamp for the wanted sweep — last time we ran
    /// a search for this movie. Also used as the upgrade-search
    /// cadence timer.
    pub last_searched_at: Option<String>,
    pub blurhash_poster: Option<String>,
    pub blurhash_backdrop: Option<String>,
    pub playback_position_ticks: i64,
    pub play_count: i64,
    pub last_played_at: Option<String>,
    pub watched_at: Option<String>,
    pub preferred_audio_stream_index: Option<i64>,
    pub preferred_subtitle_stream_index: Option<i64>,
    pub last_metadata_refresh: Option<String>,
    // `last_searched_at` is declared above; user_rating tails the
    // list because it was added later.
    /// 1..10 user rating on Trakt's scale. Written by the rate-UI
    /// and mirrored from Trakt on sync; null when unrated.
    pub user_rating: Option<i64>,
    /// Relative path under `data_path/images/` for the cached
    /// clearlogo (`logos/{type}/{tmdb_id}.{svg|png}`). `None` until
    /// the next metadata sweep runs. Subsystem 29.
    pub logo_path: Option<String>,
    /// `'mono'` or `'multi'` — drives the player's retint vs preserve
    /// rendering path. `None` when no logo is stored.
    pub logo_palette: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateMovie {
    pub tmdb_id: i64,
    /// When omitted, the server resolves the current default profile
    /// (exactly one row in `quality_profile` has `is_default = 1`).
    #[serde(default)]
    pub quality_profile_id: Option<i64>,
    pub monitored: Option<bool>,
}
