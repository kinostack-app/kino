#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]

pub struct Media {
    pub id: i64,
    pub movie_id: Option<i64>,
    pub file_path: String,
    pub relative_path: String,
    pub size: i64,
    pub container: Option<String>,
    pub resolution: Option<i64>,
    pub source: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hdr_format: Option<String>,
    pub is_remux: bool,
    pub is_proper: bool,
    pub is_repack: bool,
    pub scene_name: Option<String>,
    pub release_group: Option<String>,
    pub release_hash: Option<String>,
    pub runtime_ticks: Option<i64>,
    pub date_added: String,
    pub original_file_path: Option<String>,
    pub indexer_flags: Option<String>,
    pub trickplay_generated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct MediaEpisode {
    pub id: i64,
    pub media_id: i64,
    pub episode_id: i64,
}
