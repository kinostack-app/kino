//! `FFprobe` wrapper for extracting stream metadata from media files.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

/// Probe a media file and return structured stream/format info.
/// `-show_chapters` asks ffprobe to dump the container's
/// chapter atoms alongside streams + format; MKV and MP4 both
/// carry authored chapter marks in their own ways and ffprobe
/// normalises both to the same JSON shape.
pub fn probe(file_path: &Path, ffprobe_path: &str) -> Result<ProbeResult, ProbeError> {
    let output = Command::new(ffprobe_path)
        .args([
            "-i",
            file_path.to_str().ok_or(ProbeError::InvalidPath)?,
            "-v",
            "warning",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            "-show_chapters",
        ])
        .output()
        .map_err(|e| ProbeError::Exec(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProbeError::Failed(stderr.to_string()));
    }

    let result: ProbeResult =
        serde_json::from_slice(&output.stdout).map_err(|e| ProbeError::Parse(e.to_string()))?;

    Ok(result)
}

/// Get the duration in seconds of a media file (quick probe for sample detection).
pub fn get_duration(file_path: &Path, ffprobe_path: &str) -> Result<f64, ProbeError> {
    let result = probe(file_path, ffprobe_path)?;
    result
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .ok_or(ProbeError::NoDuration)
}

/// Detect HDR format from video stream color metadata. ffprobe
/// doesn't always emit `bits_per_raw_sample` (MKV streams on
/// some encoders omit it); fall back to inferring bit depth from
/// `pix_fmt` when the explicit field is absent — `yuv420p10le` /
/// `p010` etc. all tell us it's 10-bit even without the number.
pub fn detect_hdr(stream: &ProbeStream) -> &'static str {
    let bit_depth = stream
        .bits_per_raw_sample
        .as_deref()
        .and_then(|b| b.parse::<u32>().ok())
        .unwrap_or_else(|| bit_depth_from_pix_fmt(stream.pix_fmt.as_deref()));

    if bit_depth < 10 {
        return "sdr";
    }

    // Check side_data for Dolby Vision
    if let Some(ref side_data) = stream.side_data_list {
        for sd in side_data {
            if let Some(ref sd_type) = sd.side_data_type {
                if sd_type.contains("DOVI") || sd_type.contains("Dolby Vision") {
                    return "dolby_vision";
                }
                if sd_type.contains("HDR10+") || sd_type.contains("HDR Dynamic Metadata") {
                    return "hdr10plus";
                }
            }
        }
    }

    let primaries = stream.color_primaries.as_deref().unwrap_or("");
    let transfer = stream.color_transfer.as_deref().unwrap_or("");

    if primaries == "bt2020" && transfer == "smpte2084" {
        return "hdr10";
    }
    if primaries == "bt2020" && transfer == "arib-std-b67" {
        return "hlg";
    }

    "sdr"
}

fn bit_depth_from_pix_fmt(pix_fmt: Option<&str>) -> u32 {
    match pix_fmt.unwrap_or("").to_ascii_lowercase().as_str() {
        s if s.contains("12le") || s.contains("12be") => 12,
        s if s.contains("16le") || s.contains("16be") => 16,
        s if s.contains("10le") || s.contains("10be") || s.contains("p010") => 10,
        _ => 8,
    }
}

/// Detect Dolby Atmos on an audio stream. Atmos rides inside
/// EAC-3 (Joint Object Coding) or `TrueHD` (as an extension);
/// both cases surface a marker in the ffprobe `profile` string:
///
/// * EAC-3-JOC → profile `"Dolby Digital Plus + Dolby Atmos"`
/// * `TrueHD` + Atmos → profile `"TrueHD Atmos"` (ffmpeg ≥ 5) or
///   `"Dolby TrueHD + Dolby Atmos"` in some builds.
///
/// We don't parse the JOC bitstream ourselves — the cost of
/// being wrong for a handful of exotic MKV releases that
/// don't label the profile is tiny compared to wiring a
/// bitstream parser. The few cases that slip past this
/// detection show up without the Atmos label; the underlying
/// EAC-3 / `TrueHD` stream still plays correctly.
///
/// Codec gating matters: `profile` shows up on other stream
/// types too, and we only want this on audio. Caller passes
/// an audio-filtered stream.
#[must_use]
pub fn detect_atmos(stream: &ProbeStream) -> bool {
    let Some(profile) = stream.profile.as_deref() else {
        return false;
    };
    let lc = profile.to_ascii_lowercase();
    // Match "atmos" anywhere in the profile string. Both the
    // EAC-3-JOC and TrueHD-Atmos variants include the word;
    // substring matching keeps the check robust against the
    // minor profile-string differences across ffmpeg versions.
    lc.contains("atmos")
}

/// Detect container format from file extension.
pub fn detect_container(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

#[derive(Debug, Deserialize)]
pub struct ProbeResult {
    pub streams: Option<Vec<ProbeStream>>,
    pub format: Option<ProbeFormat>,
    #[serde(default)]
    pub chapters: Option<Vec<ProbeChapter>>,
}

#[derive(Debug, Deserialize)]
pub struct ProbeChapter {
    /// Chapter id — sequential integer, not stable across
    /// re-encodes. We don't preserve it; the import layer
    /// orders chapters by `start_time` and uses that order
    /// as the stable identity.
    pub id: Option<i64>,
    /// Start time as a decimal-seconds string (ffprobe's
    /// normalised form). ffprobe also emits integer
    /// `start` + `time_base`, but `start_time` is already
    /// the computed seconds so we skip the rational math.
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub tags: Option<ProbeChapterTags>,
}

#[derive(Debug, Deserialize)]
pub struct ProbeChapterTags {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProbeStream {
    pub index: i64,
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub profile: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub r_frame_rate: Option<String>,
    pub pix_fmt: Option<String>,
    pub bits_per_raw_sample: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub channels: Option<i64>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<String>,
    pub bit_rate: Option<String>,
    pub tags: Option<StreamTags>,
    pub disposition: Option<StreamDisposition>,
    pub side_data_list: Option<Vec<SideData>>,
}

#[derive(Debug, Deserialize)]
pub struct StreamTags {
    pub language: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StreamDisposition {
    pub default: Option<i64>,
    pub forced: Option<i64>,
    pub hearing_impaired: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SideData {
    pub side_data_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProbeFormat {
    pub duration: Option<String>,
    pub size: Option<String>,
    pub bit_rate: Option<String>,
    pub format_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("invalid file path")]
    InvalidPath,
    #[error("ffprobe exec failed: {0}")]
    Exec(String),
    #[error("ffprobe failed: {0}")]
    Failed(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("no duration found")]
    NoDuration,
}

#[cfg(test)]
mod hdr_tests {
    use super::*;

    fn stream_with_color(
        bits: &str,
        primaries: &str,
        transfer: &str,
        side_data: Option<Vec<SideData>>,
    ) -> ProbeStream {
        ProbeStream {
            index: 0,
            codec_type: Some("video".into()),
            codec_name: Some("hevc".into()),
            profile: None,
            width: Some(3840),
            height: Some(2160),
            r_frame_rate: None,
            pix_fmt: None,
            bits_per_raw_sample: Some(bits.into()),
            color_space: None,
            color_transfer: Some(transfer.into()),
            color_primaries: Some(primaries.into()),
            channels: None,
            channel_layout: None,
            sample_rate: None,
            bit_rate: None,
            tags: None,
            disposition: None,
            side_data_list: side_data,
        }
    }

    #[test]
    fn detect_sdr_low_bit_depth() {
        let s = stream_with_color("8", "bt709", "bt709", None);
        assert_eq!(detect_hdr(&s), "sdr");
    }

    #[test]
    fn detect_hdr10() {
        let s = stream_with_color("10", "bt2020", "smpte2084", None);
        assert_eq!(detect_hdr(&s), "hdr10");
    }

    #[test]
    fn detect_hlg() {
        let s = stream_with_color("10", "bt2020", "arib-std-b67", None);
        assert_eq!(detect_hdr(&s), "hlg");
    }

    #[test]
    fn detect_dolby_vision() {
        let s = stream_with_color(
            "10",
            "bt2020",
            "smpte2084",
            Some(vec![SideData {
                side_data_type: Some("DOVI configuration record".into()),
            }]),
        );
        assert_eq!(detect_hdr(&s), "dolby_vision");
    }

    #[test]
    fn detect_hdr10plus() {
        let s = stream_with_color(
            "10",
            "bt2020",
            "smpte2084",
            Some(vec![SideData {
                side_data_type: Some("HDR Dynamic Metadata SMPTE2094-40".into()),
            }]),
        );
        assert_eq!(detect_hdr(&s), "hdr10plus");
    }

    // ── Atmos detection ─────────────────────────────────────

    fn audio_stream_with_profile(codec: &str, profile: Option<&str>) -> ProbeStream {
        ProbeStream {
            index: 1,
            codec_type: Some("audio".into()),
            codec_name: Some(codec.into()),
            profile: profile.map(str::to_owned),
            width: None,
            height: None,
            r_frame_rate: None,
            pix_fmt: None,
            bits_per_raw_sample: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
            channels: Some(6),
            channel_layout: Some("5.1".into()),
            sample_rate: Some("48000".into()),
            bit_rate: None,
            tags: None,
            disposition: None,
            side_data_list: None,
        }
    }

    #[test]
    fn detect_atmos_in_eac3_joc_profile() {
        let s = audio_stream_with_profile("eac3", Some("Dolby Digital Plus + Dolby Atmos"));
        assert!(detect_atmos(&s));
    }

    #[test]
    fn detect_atmos_in_truehd_profile() {
        let s = audio_stream_with_profile("truehd", Some("TrueHD Atmos"));
        assert!(detect_atmos(&s));
    }

    #[test]
    fn detect_atmos_lowercase_variant() {
        // Some ffmpeg builds emit a lowercase / differently-
        // spelled profile — substring match handles both.
        let s = audio_stream_with_profile("truehd", Some("Dolby TrueHD + Dolby Atmos"));
        assert!(detect_atmos(&s));
    }

    #[test]
    fn no_atmos_on_plain_eac3() {
        let s = audio_stream_with_profile("eac3", Some("Dolby Digital Plus"));
        assert!(!detect_atmos(&s));
    }

    #[test]
    fn no_atmos_when_profile_missing() {
        let s = audio_stream_with_profile("eac3", None);
        assert!(!detect_atmos(&s));
    }

    #[test]
    fn no_atmos_on_aac_even_if_title_mentions_it() {
        // Title metadata isn't consulted — must come from the
        // profile string. A user-named "Atmos Remaster" AAC
        // track shouldn't get the flag.
        let s = audio_stream_with_profile("aac", Some("LC"));
        assert!(!detect_atmos(&s));
    }
}
