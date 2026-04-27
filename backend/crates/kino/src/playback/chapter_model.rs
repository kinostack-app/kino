#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Container-authored chapter marker for a `media` row.
/// Populated during import from ffprobe's `-show_chapters`
/// output; orthogonal to the intro/credits heuristic skip
/// system.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Chapter {
    pub id: i64,
    pub media_id: i64,
    /// Zero-based position in the authored chapter list.
    /// Stable per-media; the frontend keys React lists on
    /// this without re-ordering jank.
    pub idx: i64,
    pub start_secs: f64,
    pub end_secs: Option<f64>,
    /// Authored title when present; the frontend renders
    /// "Chapter {idx + 1}" as the fallback when absent.
    pub title: Option<String>,
}
