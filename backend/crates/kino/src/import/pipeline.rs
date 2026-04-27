//! Import pipeline — orchestrates the full import flow from completed download to library.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::import::ffprobe;
use crate::import::naming::{self, NamingContext};
use crate::import::transfer;
use crate::parser;

/// Video file extensions we recognize.
const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "avi", "ts", "wmv", "flv", "m4v", "webm"];

/// Directories to skip during file discovery.
const SKIP_DIRS: &[&str] = &[
    "sample",
    "extras",
    "featurettes",
    "behind the scenes",
    "bonus",
];

/// Minimum duration in seconds for a file to not be a sample.
const MIN_DURATION_SECONDS: f64 = 300.0; // 5 minutes

/// Result of a single file import.
#[derive(Debug)]
pub struct ImportedFile {
    pub library_path: PathBuf,
    pub relative_path: String,
    pub size: u64,
    pub container: String,
    pub probe_result: Option<ffprobe::ProbeResult>,
    pub parsed: parser::ParsedRelease,
    pub transfer_method: transfer::TransferMethod,
    pub subtitles: Vec<transfer::SubtitleFile>,
}

/// Discover video files in a directory, filtering out samples and junk.
pub fn discover_video_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    discover_recursive(dir, &mut files);
    files
}

fn discover_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if SKIP_DIRS.iter().any(|s| dir_name.contains(s)) {
                continue;
            }
            discover_recursive(&path, files);
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                files.push(path);
            }
        }
    }
}

/// Filter out sample files using ffprobe duration check.
pub fn filter_samples(files: Vec<PathBuf>, ffprobe_path: &str) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|f| {
            // Check if in a "sample" directory
            let path_str = f.to_string_lossy().to_ascii_lowercase();
            if path_str.contains("/sample/") || path_str.contains("\\sample\\") {
                return false;
            }

            // Check duration
            match ffprobe::get_duration(f, ffprobe_path) {
                Ok(duration) => duration >= MIN_DURATION_SECONDS,
                Err(_) => true, // Can't probe? Include it — better safe than sorry
            }
        })
        .collect()
}

/// Import a single video file into the library.
#[allow(clippy::too_many_arguments)]
pub async fn import_file(
    source_path: &Path,
    library_root: &Path,
    movie_naming_format: &str,
    episode_naming_format: &str,
    season_folder_format: &str,
    use_hardlinks: bool,
    ffprobe_path: &str,
    ctx: &NamingContext,
    is_movie: bool,
    show_name: Option<&str>,
) -> Result<ImportedFile, ImportError> {
    // Probe the file
    let probe_result = ffprobe::probe(source_path, ffprobe_path).ok();

    // Determine library path
    let library_path = if is_movie {
        naming::movie_path(library_root, movie_naming_format, ctx)
    } else {
        naming::episode_path(
            library_root,
            episode_naming_format,
            season_folder_format,
            show_name.unwrap_or(&ctx.title),
            ctx,
        )
    };

    // Transfer to library
    let method = transfer::transfer_file(source_path, &library_path, use_hardlinks)
        .await
        .map_err(|e| ImportError::Transfer(e.to_string()))?;

    // Copy sidecar subtitles
    let source_dir = source_path.parent().unwrap_or(Path::new("."));
    let subtitles = transfer::copy_sidecar_subtitles(source_dir, &library_path)
        .await
        .unwrap_or_default();

    // Get file size
    let size = tokio::fs::metadata(&library_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // Parse the source filename for quality info
    let source_name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let parsed = parser::parse(source_name);

    let relative_path = library_path
        .strip_prefix(library_root)
        .unwrap_or(&library_path)
        .to_string_lossy()
        .to_string();

    Ok(ImportedFile {
        library_path,
        relative_path,
        size,
        container: ctx.container.clone(),
        probe_result,
        parsed,
        transfer_method: method,
        subtitles,
    })
}

/// Create Media and Stream entities from an imported file.
pub async fn create_media_entities(
    pool: &SqlitePool,
    imported: &ImportedFile,
    movie_id: Option<i64>,
    episode_ids: &[i64],
    scene_name: Option<&str>,
) -> Result<i64, ImportError> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let hdr = imported
        .probe_result
        .as_ref()
        .and_then(|p| p.streams.as_ref())
        .and_then(|streams| {
            streams
                .iter()
                .find(|s| s.codec_type.as_deref() == Some("video"))
        })
        .map_or("sdr", ffprobe::detect_hdr);

    #[allow(clippy::cast_possible_wrap)]
    let size = imported.size as i64;

    // Create Media entity
    let media_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO media (movie_id, file_path, relative_path, size, container, resolution, source, video_codec, audio_codec, hdr_format, is_remux, is_proper, is_repack, scene_name, release_group, date_added) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(movie_id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(&imported.relative_path)
    .bind(size)
    .bind(&imported.container)
    .bind(imported.parsed.resolution.as_deref().and_then(|r| r.parse::<i64>().ok()))
    .bind(imported.parsed.source.as_deref())
    .bind(imported.parsed.video_codec.as_deref())
    .bind(imported.parsed.audio_codec.as_deref())
    .bind(hdr)
    .bind(imported.parsed.is_remux)
    .bind(imported.parsed.is_proper)
    .bind(imported.parsed.is_repack)
    .bind(scene_name)
    .bind(imported.parsed.release_group.as_deref())
    .bind(&now)
    .fetch_one(pool)
    .await
    .map_err(|e| ImportError::Database(e.to_string()))?;

    // Create Stream entities from probe result
    if let Some(ref probe) = imported.probe_result
        && let Some(ref streams) = probe.streams
    {
        for stream in streams {
            create_stream_entity(pool, media_id, stream).await?;
        }
    }

    // Persist container-authored chapters. Sorted by
    // start_time so `idx` is a stable zero-based position
    // in presentation order, regardless of ffprobe's
    // internal chapter-id assignment.
    if let Some(ref probe) = imported.probe_result
        && let Some(ref chapters) = probe.chapters
    {
        let mut parsed: Vec<(f64, Option<f64>, Option<String>)> = chapters
            .iter()
            .filter_map(|c| {
                let start = c.start_time.as_deref()?.parse::<f64>().ok()?;
                let end = c.end_time.as_deref().and_then(|s| s.parse::<f64>().ok());
                let title = c.tags.as_ref().and_then(|t| t.title.clone());
                Some((start, end, title))
            })
            .collect();
        parsed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (idx, (start, end, title)) in parsed.into_iter().enumerate() {
            sqlx::query(
                "INSERT INTO chapter (media_id, idx, start_secs, end_secs, title) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(media_id)
            .bind(i64::try_from(idx).unwrap_or(i64::MAX))
            .bind(start)
            .bind(end)
            .bind(title)
            .execute(pool)
            .await
            .map_err(|e| ImportError::Database(e.to_string()))?;
        }
    }

    // Create Stream entities for external subtitles
    let mut sub_index = 1000; // Start external streams at high index
    for sub in &imported.subtitles {
        sqlx::query(
            "INSERT INTO stream (media_id, stream_index, stream_type, codec, language, is_external, is_forced, is_hearing_impaired, path) VALUES (?, ?, 'subtitle', 'srt', ?, 1, ?, ?, ?)",
        )
        .bind(media_id)
        .bind(sub_index)
        .bind(sub.language.as_deref())
        .bind(sub.is_forced)
        .bind(sub.is_hearing_impaired)
        .bind(sub.path.to_string_lossy().as_ref())
        .execute(pool)
        .await
        .map_err(|e| ImportError::Database(e.to_string()))?;
        sub_index += 1;
    }

    // Create MediaEpisode links for TV
    for &ep_id in episode_ids {
        sqlx::query("INSERT INTO media_episode (media_id, episode_id) VALUES (?, ?)")
            .bind(media_id)
            .bind(ep_id)
            .execute(pool)
            .await
            .map_err(|e| ImportError::Database(e.to_string()))?;
    }

    // Status derives from the media row + media_episode links we
    // just created — no explicit UPDATE needed.
    let _ = movie_id;
    let _ = episode_ids;

    Ok(media_id)
}

/// Persist one ffprobe stream row into the `stream` table.
/// `pub(crate)` so `import::trigger` — the
/// torrent-completion import path — can reuse the same shape
/// as the bulk pipeline. Without this, the trigger path wrote
/// media rows but skipped streams, leaving the decision engine
/// blind to audio codecs → every source direct-played → Firefox
/// silent-video regression on EAC-3 content.
pub(crate) async fn create_stream_entity(
    pool: &SqlitePool,
    media_id: i64,
    stream: &ffprobe::ProbeStream,
) -> Result<(), ImportError> {
    let stream_type = stream.codec_type.as_deref().unwrap_or("unknown");
    let language = stream.tags.as_ref().and_then(|t| t.language.as_deref());
    let title = stream.tags.as_ref().and_then(|t| t.title.as_deref());
    let is_default = stream
        .disposition
        .as_ref()
        .and_then(|d| d.default)
        .unwrap_or(0)
        != 0;
    let is_forced = stream
        .disposition
        .as_ref()
        .and_then(|d| d.forced)
        .unwrap_or(0)
        != 0;
    let is_hi = stream
        .disposition
        .as_ref()
        .and_then(|d| d.hearing_impaired)
        .unwrap_or(0)
        != 0;
    let bitrate = stream
        .bit_rate
        .as_deref()
        .and_then(|b| b.parse::<i64>().ok());
    let framerate = stream.r_frame_rate.as_deref().and_then(parse_framerate);
    let sample_rate = stream
        .sample_rate
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok());
    let bit_depth = stream
        .bits_per_raw_sample
        .as_deref()
        .and_then(|b| b.parse::<i64>().ok());

    let hdr = (stream_type == "video").then(|| ffprobe::detect_hdr(stream));
    let is_atmos = stream_type == "audio" && ffprobe::detect_atmos(stream);

    sqlx::query(
        "INSERT INTO stream (media_id, stream_index, stream_type, codec, language, title, is_external, is_default, is_forced, is_hearing_impaired, bitrate, width, height, framerate, pixel_format, color_space, color_transfer, color_primaries, hdr_format, channels, channel_layout, sample_rate, bit_depth, is_atmos, profile) VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(media_id)
    .bind(stream.index)
    .bind(stream_type)
    .bind(stream.codec_name.as_deref())
    .bind(language)
    .bind(title)
    .bind(is_default)
    .bind(is_forced)
    .bind(is_hi)
    .bind(bitrate)
    .bind(stream.width)
    .bind(stream.height)
    .bind(framerate)
    .bind(stream.pix_fmt.as_deref())
    .bind(stream.color_space.as_deref())
    .bind(stream.color_transfer.as_deref())
    .bind(stream.color_primaries.as_deref())
    .bind(hdr)
    .bind(stream.channels)
    .bind(stream.channel_layout.as_deref())
    .bind(sample_rate)
    .bind(bit_depth)
    .bind(is_atmos)
    .bind(stream.profile.as_deref())
    .execute(pool)
    .await
    .map_err(|e| ImportError::Database(e.to_string()))?;

    Ok(())
}

fn parse_framerate(rate: &str) -> Option<f64> {
    let parts: Vec<&str> = rate.split('/').collect();
    if parts.len() == 2 {
        let num: f64 = parts[0].parse().ok()?;
        let den: f64 = parts[1].parse().ok()?;
        if den > 0.0 {
            return Some(num / den);
        }
    }
    rate.parse().ok()
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("transfer failed: {0}")]
    Transfer(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("no video files found")]
    NoVideoFiles,
    #[error("matching failed: {0}")]
    MatchFailed(String),
}
