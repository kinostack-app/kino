#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]

pub struct Stream {
    pub id: i64,
    pub media_id: i64,
    pub stream_index: i64,
    pub stream_type: String,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_external: bool,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub path: Option<String>,
    pub bitrate: Option<i64>,
    // video fields
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub framerate: Option<f64>,
    pub pixel_format: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub hdr_format: Option<String>,
    // audio fields
    pub channels: Option<i64>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    /// Atmos detected on this audio stream — EAC-3 with JOC or
    /// `TrueHD` with the Atmos extension. Parsed from the
    /// ffprobe `profile` string during import. Orthogonal to
    /// client decode capability; the base codec (EAC-3 / `TrueHD`)
    /// is what the decoder sees.
    pub is_atmos: bool,
    /// Raw ffprobe `profile` string. Orthogonal to `is_atmos`:
    /// the boolean pre-parses the Atmos variants, the raw string
    /// lets downstream code distinguish e.g. `"DTS-HD MA"` from
    /// plain `"DTS"` for HLS `CODECS` emission.
    pub profile: Option<String>,
}
