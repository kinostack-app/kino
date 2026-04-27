//! Archive extraction — detects ZIP / 7z / RAR in a download dir and
//! extracts video-bearing archives to a sibling temp dir so the normal
//! video-discovery path can find the files.
//!
//! ZIP and 7z are handled in pure Rust. RAR requires the `unrar` binary
//! to be installed; if it's missing we log a warning and skip.

use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    SevenZ,
    Rar,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("io: {0}")]
    Io(String),
    #[error("zip: {0}")]
    Zip(String),
    #[error("7z: {0}")]
    SevenZ(String),
    #[error("rar: {0}")]
    Rar(String),
}

/// Detect the archive type from a file path. Returns None for non-archives
/// (or unsupported types).
pub fn detect_kind(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    // Skip multi-part secondary volumes (.r00, .r01, .part2.rar etc.) —
    // extracting the first volume pulls the rest in automatically.
    if is_secondary_rar_volume(&name) {
        return None;
    }
    let ext = Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "zip" => Some(ArchiveKind::Zip),
        "7z" => Some(ArchiveKind::SevenZ),
        "rar" => Some(ArchiveKind::Rar),
        _ => None,
    }
}

fn is_secondary_rar_volume(name: &str) -> bool {
    // .r00, .r01, ..., .r999
    if let Some(dot) = name.rfind('.')
        && name[dot + 1..].starts_with('r')
        && name[dot + 2..].chars().all(|c| c.is_ascii_digit())
        && !name[dot + 2..].is_empty()
    {
        return true;
    }
    // name.partN.rar where N > 1
    if let Some(idx) = name.rfind(".part") {
        let rest = &name[idx + ".part".len()..];
        if let Some(dot) = rest.find('.') {
            let n = &rest[..dot];
            if let Ok(num) = n.parse::<u32>() {
                return num > 1;
            }
        }
    }
    false
}

/// Walk `dir` and return all first-volume archive files found.
pub fn find_archives(dir: &Path) -> Vec<(PathBuf, ArchiveKind)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, ArchiveKind)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Some(kind) = detect_kind(&path) {
                out.push((path, kind));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

/// Extract a single archive to `dest`. Pure-Rust for ZIP and 7z; shells
/// out to `unrar` for RAR.
pub async fn extract(archive: &Path, kind: ArchiveKind, dest: &Path) -> Result<(), ArchiveError> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| ArchiveError::Io(e.to_string()))?;

    match kind {
        ArchiveKind::Zip => extract_zip(archive, dest).await,
        ArchiveKind::SevenZ => extract_7z(archive, dest).await,
        ArchiveKind::Rar => extract_rar(archive, dest).await,
    }
}

async fn extract_zip(archive: &Path, dest: &Path) -> Result<(), ArchiveError> {
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), ArchiveError> {
        let file = std::fs::File::open(&archive).map_err(|e| ArchiveError::Io(e.to_string()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| ArchiveError::Zip(e.to_string()))?;

        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| ArchiveError::Zip(e.to_string()))?;
            // Reject path traversal attempts; derive a sanitized relative path.
            let Some(rel) = entry.enclosed_name() else {
                continue;
            };
            let out_path = dest.join(&rel);

            if entry.is_dir() {
                std::fs::create_dir_all(&out_path).map_err(|e| ArchiveError::Io(e.to_string()))?;
                continue;
            }
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ArchiveError::Io(e.to_string()))?;
            }
            let mut out =
                std::fs::File::create(&out_path).map_err(|e| ArchiveError::Io(e.to_string()))?;
            // 64 KiB chunks on the heap (clippy: avoid large stack arrays).
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = entry
                    .read(&mut buf)
                    .map_err(|e| ArchiveError::Zip(e.to_string()))?;
                if n == 0 {
                    break;
                }
                std::io::Write::write_all(&mut out, &buf[..n])
                    .map_err(|e| ArchiveError::Io(e.to_string()))?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| ArchiveError::Io(e.to_string()))?
}

async fn extract_7z(archive: &Path, dest: &Path) -> Result<(), ArchiveError> {
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || {
        sevenz_rust2::decompress_file(&archive, &dest)
            .map_err(|e| ArchiveError::SevenZ(e.to_string()))
    })
    .await
    .map_err(|e| ArchiveError::Io(e.to_string()))?
}

async fn extract_rar(archive: &Path, dest: &Path) -> Result<(), ArchiveError> {
    let output = tokio::process::Command::new("unrar")
        .arg("x")
        .arg("-y") // yes to all prompts
        .arg("-o+") // overwrite existing
        .arg(archive)
        .arg(dest)
        .output()
        .await
        .map_err(|e| {
            ArchiveError::Rar(format!(
                "unrar binary not found or failed to spawn: {e}. Install `unrar` to enable RAR extraction."
            ))
        })?;

    if !output.status.success() {
        return Err(ArchiveError::Rar(format!(
            "unrar exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(200)
                .collect::<String>()
        )));
    }
    Ok(())
}

/// Find archives in `source`, extract each to a `.extracted` sibling dir,
/// and return that dir if any were extracted. Returns None if no archives
/// were found. Extraction errors are logged but do not abort — one bad
/// archive shouldn't fail the import of a mixed download.
pub async fn extract_all(source: &Path) -> anyhow::Result<Option<PathBuf>> {
    let archives = find_archives(source);
    if archives.is_empty() {
        return Ok(None);
    }

    let extracted_dir = source.join(".extracted");
    tokio::fs::create_dir_all(&extracted_dir).await?;

    for (archive, kind) in &archives {
        match extract(archive, *kind, &extracted_dir).await {
            Ok(()) => {
                tracing::info!(
                    archive = %archive.display(),
                    kind = ?kind,
                    "extracted archive"
                );
            }
            Err(e) => {
                tracing::warn!(
                    archive = %archive.display(),
                    kind = ?kind,
                    error = %e,
                    "archive extraction failed — continuing"
                );
            }
        }
    }

    Ok(Some(extracted_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_kind_matches_expected() {
        assert_eq!(detect_kind(Path::new("movie.zip")), Some(ArchiveKind::Zip));
        assert_eq!(
            detect_kind(Path::new("movie.7z")),
            Some(ArchiveKind::SevenZ)
        );
        assert_eq!(detect_kind(Path::new("movie.rar")), Some(ArchiveKind::Rar));
        assert_eq!(detect_kind(Path::new("movie.mkv")), None);
        assert_eq!(detect_kind(Path::new("Movie.ZIP")), Some(ArchiveKind::Zip));
    }

    #[test]
    fn secondary_rar_volumes_are_ignored() {
        assert_eq!(detect_kind(Path::new("movie.r00")), None);
        assert_eq!(detect_kind(Path::new("movie.r15")), None);
        assert_eq!(detect_kind(Path::new("movie.part2.rar")), None);
        assert_eq!(
            detect_kind(Path::new("movie.part1.rar")),
            Some(ArchiveKind::Rar)
        );
        // .r0 alone is nothing meaningful but we treat it as secondary.
        assert_eq!(detect_kind(Path::new("movie.r0")), None);
    }

    #[tokio::test]
    async fn extract_zip_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("test.zip");

        // Build a minimal ZIP in-memory.
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            w.start_file("hello.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut w, b"world").unwrap();
            w.finish().unwrap();
        }

        let dest = tmp.path().join("out");
        extract(&zip_path, ArchiveKind::Zip, &dest).await.unwrap();

        let content = tokio::fs::read_to_string(dest.join("hello.txt"))
            .await
            .unwrap();
        assert_eq!(content, "world");
    }

    #[tokio::test]
    async fn extract_all_no_archives_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        tokio::fs::write(tmp.path().join("movie.mkv"), b"fake")
            .await
            .unwrap();
        let out = extract_all(tmp.path()).await.unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn extract_all_extracts_zip_and_returns_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("release.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            w.start_file("movie.mkv", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut w, b"video-bytes").unwrap();
            w.finish().unwrap();
        }

        let extracted = extract_all(tmp.path()).await.unwrap().unwrap();
        assert!(extracted.join("movie.mkv").exists());
    }
}
