//! File transfer: hardlink preferred, copy fallback.

use std::path::Path;

/// Transfer a file to the library. Tries hardlink first, falls back to copy.
pub async fn transfer_file(
    source: &Path,
    destination: &Path,
    use_hardlinks: bool,
) -> Result<TransferMethod, TransferError> {
    // Ensure parent directory exists
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| TransferError::CreateDir(e.to_string()))?;
    }

    if use_hardlinks {
        // Try hardlink first
        match tokio::fs::hard_link(source, destination).await {
            Ok(()) => return Ok(TransferMethod::Hardlink),
            Err(e) => {
                // EXDEV = cross-device link, fall back to copy
                if e.raw_os_error() == Some(18) {
                    // EXDEV = 18 (cross-device link)
                    tracing::debug!(
                        source = %source.display(),
                        dest = %destination.display(),
                        "cross-device link, falling back to copy"
                    );
                } else {
                    tracing::warn!(
                        source = %source.display(),
                        dest = %destination.display(),
                        error = %e,
                        "hardlink failed, falling back to copy"
                    );
                }
            }
        }
    }

    // Copy fallback
    tokio::fs::copy(source, destination)
        .await
        .map_err(|e| TransferError::Copy(e.to_string()))?;

    Ok(TransferMethod::Copy)
}

/// Copy sidecar subtitle files alongside the media file.
pub async fn copy_sidecar_subtitles(
    source_dir: &Path,
    dest_media_path: &Path,
) -> Result<Vec<SubtitleFile>, TransferError> {
    let mut subtitles = Vec::new();

    let sub_extensions = ["srt", "ass", "ssa", "sub", "idx", "vtt"];
    let media_stem = dest_media_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let mut entries = tokio::fs::read_dir(source_dir)
        .await
        .map_err(|e| TransferError::ReadDir(e.to_string()))?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if !sub_extensions.contains(&ext.as_str()) {
            continue;
        }

        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

        // Parse language and flags from filename
        let (language, is_forced, is_hi) = parse_subtitle_filename(filename);

        // Build destination path
        let sub_suffix = build_subtitle_suffix(language.as_deref(), is_forced, is_hi);
        let dest_sub = dest_media_path.with_file_name(format!("{media_stem}{sub_suffix}.{ext}"));

        tokio::fs::copy(&path, &dest_sub)
            .await
            .map_err(|e| TransferError::Copy(e.to_string()))?;

        subtitles.push(SubtitleFile {
            path: dest_sub,
            language,
            is_forced,
            is_hearing_impaired: is_hi,
        });
    }

    Ok(subtitles)
}

/// Parse language and flags from a subtitle filename.
/// e.g. "Movie.en.forced.srt" → ("en", true, false)
fn parse_subtitle_filename(filename: &str) -> (Option<String>, bool, bool) {
    let parts: Vec<&str> = filename.rsplitn(2, '.').collect();
    if parts.len() < 2 {
        return (None, false, false);
    }
    let name_part = parts[1]; // everything before extension
    let segments: Vec<&str> = name_part.split('.').collect();

    let mut language = None;
    let mut is_forced = false;
    let mut is_hi = false;

    for seg in segments.iter().rev() {
        let lower = seg.to_ascii_lowercase();
        match lower.as_str() {
            "forced" => is_forced = true,
            "hi" | "sdh" => is_hi = true,
            s if s.len() == 2 || s.len() == 3 => {
                // Likely a language code
                if language.is_none() {
                    language = Some(lower);
                }
            }
            _ => {}
        }
    }

    (language, is_forced, is_hi)
}

fn build_subtitle_suffix(language: Option<&str>, is_forced: bool, is_hi: bool) -> String {
    let mut suffix = String::new();
    if let Some(lang) = language {
        suffix.push('.');
        suffix.push_str(lang);
    }
    if is_forced {
        suffix.push_str(".forced");
    }
    if is_hi {
        suffix.push_str(".hi");
    }
    suffix
}

#[derive(Debug, Clone)]
pub struct SubtitleFile {
    pub path: std::path::PathBuf,
    pub language: Option<String>,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMethod {
    Hardlink,
    Copy,
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("failed to create directory: {0}")]
    CreateDir(String),
    #[error("copy failed: {0}")]
    Copy(String),
    #[error("read directory failed: {0}")]
    ReadDir(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_english_subtitle() {
        let (lang, forced, hi) = parse_subtitle_filename("Movie.en.srt");
        assert_eq!(lang.as_deref(), Some("en"));
        assert!(!forced);
        assert!(!hi);
    }

    #[test]
    fn parse_forced_subtitle() {
        let (lang, forced, hi) = parse_subtitle_filename("Movie.en.forced.srt");
        assert_eq!(lang.as_deref(), Some("en"));
        assert!(forced);
        assert!(!hi);
    }

    #[test]
    fn parse_hearing_impaired_subtitle() {
        let (lang, forced, hi) = parse_subtitle_filename("Movie.en.hi.srt");
        assert_eq!(lang.as_deref(), Some("en"));
        assert!(!forced);
        assert!(hi);
    }

    #[test]
    fn parse_no_language_subtitle() {
        let (lang, forced, hi) = parse_subtitle_filename("Movie.srt");
        assert!(lang.is_none());
        assert!(!forced);
        assert!(!hi);
    }
}
