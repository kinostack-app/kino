//! Subtitle extraction and conversion.

use std::path::{Path, PathBuf};

use tokio::process::Command;

/// Canonical location for extracted / converted subtitle files
/// belonging to `media_id`. Lives under the configured `data_path`
/// so it co-resides with the rest of kino's state (backup scope,
/// `just reset` scope) and every caller — the subtitle endpoint,
/// the cleanup sweep on media delete, the user-initiated
/// `DELETE /api/v1/media/{id}` handler — references the same
/// function, no format-string drift.
#[must_use]
pub fn cache_dir(data_path: &Path, media_id: i64) -> PathBuf {
    data_path
        .join("cache")
        .join("subs")
        .join(media_id.to_string())
}

/// Stream-source counterpart to `cache_dir` — keyed on
/// `download_id` so in-progress torrents share the same
/// extract-once-then-serve semantics as imported media. Cleared
/// on download completion via the stream-probe's `forget` path.
#[must_use]
pub fn stream_cache_dir(data_path: &Path, download_id: i64) -> PathBuf {
    data_path
        .join("cache")
        .join("subs-stream")
        .join(download_id.to_string())
}

/// Remove the subtitle cache directory for `media_id` if it exists.
/// Missing-dir is not an error (the media may have had no
/// extractable subs). Any other I/O error is surfaced so the
/// caller can log at the right level.
pub async fn clear_cache_dir(data_path: &Path, media_id: i64) -> std::io::Result<()> {
    let dir = cache_dir(data_path, media_id);
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Extract an embedded subtitle track to `WebVTT`.
pub async fn extract_to_vtt(
    input_path: &Path,
    stream_index: i64,
    output_path: &Path,
    ffmpeg_path: &str,
) -> Result<(), SubtitleError> {
    let output = Command::new(ffmpeg_path)
        .args([
            "-i",
            input_path.to_str().ok_or(SubtitleError::InvalidPath)?,
            "-map",
            &format!("0:{stream_index}"),
            "-c:s",
            "webvtt",
            "-y",
            output_path.to_str().ok_or(SubtitleError::InvalidPath)?,
        ])
        .output()
        .await
        .map_err(|e| SubtitleError::Exec(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubtitleError::Failed(stderr.to_string()));
    }

    Ok(())
}

/// Get the path to serve a subtitle, extracting if needed.
///
/// - External text files (.srt, .vtt): serve directly or convert to VTT
/// - Embedded text tracks: extract with ffmpeg to VTT
/// - Image-based (PGS, VOBSUB): return error (must be burned in)
pub async fn get_subtitle_path(
    media_file_path: &Path,
    stream_index: i64,
    stream_codec: &str,
    is_external: bool,
    external_path: Option<&str>,
    temp_dir: &Path,
    ffmpeg_path: &str,
) -> Result<PathBuf, SubtitleError> {
    // External file — serve directly if VTT, otherwise convert
    if is_external {
        let ext_path = external_path.ok_or(SubtitleError::NoPath)?;
        let path = PathBuf::from(ext_path);

        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vtt"))
        {
            return Ok(path);
        }

        // Convert SRT/ASS to VTT
        let vtt_path = temp_dir.join(format!("sub_{stream_index}.vtt"));
        if !vtt_path.exists() {
            extract_to_vtt(&path, 0, &vtt_path, ffmpeg_path).await?;
        }
        return Ok(vtt_path);
    }

    // Embedded subtitle
    match stream_codec {
        // Text-based: extract to VTT
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" => {
            let vtt_path = temp_dir.join(format!("sub_{stream_index}.vtt"));
            if !vtt_path.exists() {
                tokio::fs::create_dir_all(temp_dir)
                    .await
                    .map_err(|e| SubtitleError::Exec(e.to_string()))?;
                extract_to_vtt(media_file_path, stream_index, &vtt_path, ffmpeg_path).await?;
            }
            Ok(vtt_path)
        }
        // Image-based: can't extract as text
        "hdmv_pgs_subtitle" | "pgssub" | "dvd_subtitle" | "dvdsub" | "dvb_subtitle" => {
            Err(SubtitleError::ImageBased(stream_codec.to_owned()))
        }
        _ => Err(SubtitleError::UnsupportedCodec(stream_codec.to_owned())),
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn cache_dir_shape_is_stable() {
        let p = Path::new("/data");
        assert_eq!(
            cache_dir(p, 42),
            PathBuf::from("/data/cache/subs/42"),
            "cache path format is load-bearing for cleanup — any \
             change must be coordinated with cleanup::delete_media_file"
        );
    }

    #[tokio::test]
    async fn clear_cache_dir_missing_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        // Media 7 has never been cached — clearing should not error.
        clear_cache_dir(tmp.path(), 7).await.unwrap();
    }

    #[tokio::test]
    async fn clear_cache_dir_removes_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cache_dir(tmp.path(), 42);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("sub_0.vtt"), "WEBVTT\n")
            .await
            .unwrap();
        clear_cache_dir(tmp.path(), 42).await.unwrap();
        assert!(!dir.exists(), "cache dir should be gone");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SubtitleError {
    #[error("invalid file path")]
    InvalidPath,
    #[error("no external path")]
    NoPath,
    #[error("ffmpeg exec failed: {0}")]
    Exec(String),
    #[error("subtitle extraction failed: {0}")]
    Failed(String),
    #[error("image-based subtitle ({0}) must be burned into video")]
    ImageBased(String),
    #[error("unsupported subtitle codec: {0}")]
    UnsupportedCodec(String),
}

/// Rewrite a `WebVTT` file's cue timestamps to account for an
/// HLS transcode that started mid-file via `ffmpeg -ss
/// <offset_secs>`.
///
/// The transcoded output's internal `video.currentTime`
/// starts at 0 even though the content corresponds to
/// source-time `offset_secs`. The source VTT's cues are in
/// source-time — so without rewriting, the browser would
/// wait until `video.currentTime == 900` to show a cue at
/// source-time 00:15:00, which would be the wrong
/// scene (or past the end of the transcoded output
/// entirely). Here we shift every cue by `-offset_secs`;
/// cues that end before zero are dropped (they would
/// never be visible against the trimmed output) and cues
/// straddling the cut are clamped to start at zero.
///
/// Non-cue lines (the `WEBVTT` header, `NOTE` blocks,
/// `STYLE` / `REGION` blocks, blank separators, cue IDs,
/// and cue payload text) pass through untouched.
/// Preserves cue settings after the end timestamp (e.g.
/// `line:90% position:50%`).
#[must_use]
pub fn shift_vtt_timestamps(input: &str, offset_secs: f64) -> String {
    if offset_secs <= 0.0 {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    // A VTT file is a sequence of blocks separated by blank
    // lines. Each cue block is:
    //   optional id line
    //   "HH:MM:SS.mmm --> HH:MM:SS.mmm [settings]"
    //   one-or-more payload lines
    // We walk block-by-block so we can decide to emit or
    // drop a cue based on its timestamps.
    let blocks = input.split("\n\n").peekable();
    let mut first = true;
    for block in blocks {
        let shifted = shift_block(block, offset_secs);
        if let Some(content) = shifted {
            if !first {
                out.push_str("\n\n");
            }
            out.push_str(&content);
            first = false;
        }
    }
    // Preserve a trailing newline if the original had one —
    // some players are picky about terminal newlines.
    if input.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn shift_block(block: &str, offset_secs: f64) -> Option<String> {
    // Find the timestamp line inside this block (if any).
    // A block without one is a header / note / style /
    // region block — pass through unchanged.
    let lines: Vec<&str> = block.lines().collect();
    let ts_idx = lines.iter().position(|l| l.contains("-->"));
    let Some(ts_idx) = ts_idx else {
        return Some(block.to_owned());
    };

    let ts_line = lines[ts_idx];
    let Some(arrow) = ts_line.find("-->") else {
        return Some(block.to_owned());
    };
    let left = ts_line[..arrow].trim();
    let right_raw = ts_line[arrow + 3..].trim_start();
    // Right side may carry cue settings after the end time;
    // split on first whitespace.
    let (right_ts, settings) = match right_raw.find(char::is_whitespace) {
        Some(i) => (&right_raw[..i], &right_raw[i..]),
        None => (right_raw, ""),
    };

    let start = parse_vtt_time(left)?;
    let end = parse_vtt_time(right_ts)?;
    let new_start = start - offset_secs;
    let new_end = end - offset_secs;

    // Entire cue ends before the visible window — drop it.
    if new_end <= 0.0 {
        return None;
    }

    let clamped_start = new_start.max(0.0);
    let shifted_ts = format!(
        "{} --> {}{}",
        format_vtt_time(clamped_start),
        format_vtt_time(new_end),
        settings,
    );

    let mut out = String::with_capacity(block.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if i == ts_idx {
            out.push_str(&shifted_ts);
        } else {
            out.push_str(line);
        }
    }
    Some(out)
}

fn parse_vtt_time(s: &str) -> Option<f64> {
    // Formats: `HH:MM:SS.mmm` or `MM:SS.mmm`.
    // WebVTT allows `.` or `,` as the millisecond
    // separator; we accept both.
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let m: f64 = parts[0].parse().ok()?;
            let s: f64 = parts[1].parse().ok()?;
            Some(m * 60.0 + s)
        }
        3 => {
            let h: f64 = parts[0].parse().ok()?;
            let m: f64 = parts[1].parse().ok()?;
            let s: f64 = parts[2].parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + s)
        }
        _ => None,
    }
}

fn format_vtt_time(secs: f64) -> String {
    // Negative inputs are nonsense here (callers clamp to
    // zero first) — but guard against NaN / weird floats
    // so we always emit a valid VTT timestamp.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let millis = (secs.max(0.0) * 1000.0).round() as u64;
    let ms = millis % 1000;
    let total_sec = millis / 1000;
    let sec = total_sec % 60;
    let total_min = total_sec / 60;
    let min = total_min % 60;
    let hour = total_min / 60;
    format!("{hour:02}:{min:02}:{sec:02}.{ms:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_offset_passes_through() {
        let vtt = "WEBVTT\n\n00:00:10.000 --> 00:00:12.000\nHello\n";
        assert_eq!(shift_vtt_timestamps(vtt, 0.0), vtt);
    }

    #[test]
    fn shifts_cues_back() {
        let vtt = "WEBVTT\n\n00:00:10.000 --> 00:00:12.000\nHello\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(
            shifted.contains("00:00:05.000 --> 00:00:07.000"),
            "got {shifted}"
        );
    }

    #[test]
    fn drops_cues_ending_before_offset() {
        let vtt = "WEBVTT\n\n\
            00:00:01.000 --> 00:00:02.000\nGone\n\n\
            00:00:10.000 --> 00:00:12.000\nKept\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(!shifted.contains("Gone"), "got {shifted}");
        assert!(shifted.contains("Kept"));
    }

    #[test]
    fn clamps_straddling_cue_to_zero() {
        // Cue runs 00:04–00:06; offset 5 → new range -1 to 1.
        // Should clamp to 0 → 1.
        let vtt = "WEBVTT\n\n00:00:04.000 --> 00:00:06.000\nStraddle\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(
            shifted.contains("00:00:00.000 --> 00:00:01.000"),
            "got {shifted}"
        );
    }

    #[test]
    fn preserves_cue_settings() {
        let vtt = "WEBVTT\n\n00:00:10.000 --> 00:00:12.000 line:90% position:50%\nHello\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(shifted.contains("line:90% position:50%"), "got {shifted}");
    }

    #[test]
    fn preserves_note_and_style_blocks() {
        let vtt = "WEBVTT\n\n\
            NOTE This is a note\n\n\
            STYLE\n::cue { color: red; }\n\n\
            00:00:10.000 --> 00:00:12.000\nHello\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(shifted.contains("NOTE This is a note"));
        assert!(shifted.contains("::cue { color: red; }"));
    }

    #[test]
    fn handles_comma_decimal_separator() {
        // WebVTT spec allows `.`, but SRT-sourced files
        // sometimes carry through commas. We accept either
        // to be forgiving.
        let vtt = "WEBVTT\n\n00:00:10,500 --> 00:00:12,500\nHello\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(
            shifted.contains("00:00:05.500 --> 00:00:07.500"),
            "got {shifted}"
        );
    }

    #[test]
    fn preserves_cue_ids() {
        let vtt = "WEBVTT\n\ncue-42\n00:00:10.000 --> 00:00:12.000\nHello\n";
        let shifted = shift_vtt_timestamps(vtt, 5.0);
        assert!(shifted.contains("cue-42"));
        assert!(shifted.contains("00:00:05.000 --> 00:00:07.000"));
    }
}
