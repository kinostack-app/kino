#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::models::enums::DownloadState;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Download {
    pub id: i64,
    pub release_id: Option<i64>,
    pub torrent_hash: Option<String>,
    pub title: String,
    /// Stored as `TEXT` in `SQLite` so sqlx reads it as `String`; the
    /// `value_type` override tells utoipa to emit the typed
    /// `DownloadState` enum in the `OpenAPI` schema so the frontend
    /// gets the narrow union instead of a wide `string`.
    #[schema(value_type = DownloadState)]
    pub state: String,
    pub size: Option<i64>,
    pub downloaded: i64,
    pub uploaded: i64,
    pub download_speed: i64,
    pub upload_speed: i64,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub eta: Option<i64>,
    pub added_at: String,
    pub completed_at: Option<String>,
    pub output_path: Option<String>,
    pub magnet_url: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DownloadContent {
    pub id: i64,
    pub download_id: i64,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
}

/// Download with linked content IDs (for list endpoint).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct DownloadWithContent {
    // All Download fields
    pub id: i64,
    pub release_id: Option<i64>,
    pub torrent_hash: Option<String>,
    pub title: String,
    #[schema(value_type = DownloadState)]
    pub state: String,
    pub size: Option<i64>,
    pub downloaded: i64,
    pub uploaded: i64,
    pub download_speed: i64,
    pub upload_speed: i64,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub eta: Option<i64>,
    pub added_at: String,
    pub completed_at: Option<String>,
    pub output_path: Option<String>,
    pub magnet_url: Option<String>,
    pub error_message: Option<String>,
    // Content link
    pub content_movie_id: Option<i64>,
    pub content_episode_id: Option<i64>,
}
