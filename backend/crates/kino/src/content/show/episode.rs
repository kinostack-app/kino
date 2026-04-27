#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]

pub struct Episode {
    pub id: i64,
    pub series_id: i64,
    pub show_id: i64,
    pub season_number: i64,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub episode_number: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date_utc: Option<String>,
    pub runtime: Option<i64>,
    pub still_path: Option<String>,
    pub tmdb_rating: Option<f64>,
    /// Derived phase string, populated via a CASE expression in
    /// library-side SELECTs (see `content/derived_state.rs` for the rules).
    /// Not a persisted column — omit from INSERTs.
    #[serde(default)]
    pub status: String,
    /// Scheduler autopilot: 1 = sweep may search + grab releases,
    /// 0 = skip entirely. Writes don't flow through this; it's a
    /// user preference.
    pub acquire: bool,
    /// Watch-scope: 1 = this episode counts for Next Up / progress
    /// / `aired_count`. 0 = ignored from those calculations (e.g. user
    /// excluded older seasons on Follow). Usually equals `acquire`
    /// but can diverge.
    pub in_scope: bool,
    pub playback_position_ticks: i64,
    pub play_count: i64,
    pub last_played_at: Option<String>,
    pub watched_at: Option<String>,
    pub preferred_audio_stream_index: Option<i64>,
    pub preferred_subtitle_stream_index: Option<i64>,
    pub last_searched_at: Option<String>,
    /// Intro-skipper timestamps (subsystem 15). All optional: null
    /// means either not-yet-analysed or analysed-but-not-detected —
    /// `intro_analysis_at` distinguishes the two. Units: milliseconds.
    #[serde(default)]
    pub intro_start_ms: Option<i64>,
    #[serde(default)]
    pub intro_end_ms: Option<i64>,
    #[serde(default)]
    pub credits_start_ms: Option<i64>,
    #[serde(default)]
    pub credits_end_ms: Option<i64>,
    #[serde(default)]
    pub intro_analysis_at: Option<String>,
}
