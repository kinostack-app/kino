#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Schema note: `series` also has a `monitored` column with DEFAULT 1.
// It was drafted for a per-season acquire toggle that never shipped —
// per-episode `episode.acquire` turned out to be the right unit of
// control, and nothing in-tree reads the series-level flag. Field
// intentionally omitted from this struct; sqlx ignores unmapped
// columns so SELECT * still works.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Series {
    pub id: i64,
    pub show_id: i64,
    pub tmdb_id: Option<i64>,
    pub season_number: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    pub episode_count: Option<i64>,
}
