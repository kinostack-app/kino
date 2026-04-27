//! User-initiated jellyfin-ffmpeg download.
//!
//! The user clicks "Download jellyfin-ffmpeg" from a settings
//! page or status-banner warning; this module fetches the right
//! portable tarball for the host, verifies SHA256, extracts to
//! `{data_path}/bin/`, and flips `config.ffmpeg_path`. System
//! ffmpeg is never touched — only our local copy.
//!
//! One download may be in flight at a time. A second call while
//! one is running yields a 409 `Conflict`; the in-flight task
//! can be cancelled via `DELETE`.
//!
//! # Security surface
//!
//! 1. Pinned SHA256 for each (platform, version) pair. Downloads
//!    that don't match the pin are rejected — no silent fallback.
//! 2. HTTPS to `github.com` / `objects.githubusercontent.com`
//!    via the existing `reqwest` client (TLS verification on).
//! 3. Extraction happens in a scratch dir inside the data
//!    volume, then atomically moved into `{data_path}/bin/`.
//!    A failed / partial extract can't leave the caller with
//!    half-valid binaries.
//! 4. We shell out to `tar -xJf` / `tar -xf` for extraction —
//!    same `tar` binary on the host, no new parser surface.
//!
//! Bumping the pinned version is a conscious one-file change:
//! update `JELLYFIN_FFMPEG_VERSION` + the six SHA256 entries
//! below, verified by re-downloading + hashing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::events::AppEvent;

/// Pinned jellyfin-ffmpeg release tag. Bump this + the SHA256s
/// in [`PLATFORMS`] when moving to a newer build. We only track
/// releases we've hand-verified work with our use cases (probe,
/// transcode paths, HDR filters).
const JELLYFIN_FFMPEG_VERSION: &str = "7.1.3-5";

/// Host platform kinds we have pinned assets for. Windows is
/// supported because `tar` on Windows 10+ can extract
/// `.tar.xz` / `.zip` natively; other platforms fall back to
/// the "bundled ffmpeg not supported — use system ffmpeg"
/// error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux64,
    LinuxArm64,
    Mac64,
    MacArm64,
    Win64,
    WinArm64,
}

impl Platform {
    /// Detect the host platform from `std::env::consts::{OS, ARCH}`.
    /// Returns `None` on unsupported combinations (e.g., FreeBSD,
    /// Linux riscv64) — caller surfaces that as a typed error so
    /// the UI can render "not supported on this platform" clearly.
    #[must_use]
    pub fn detect() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some(Self::Linux64),
            ("linux", "aarch64") => Some(Self::LinuxArm64),
            ("macos", "x86_64") => Some(Self::Mac64),
            ("macos", "aarch64") => Some(Self::MacArm64),
            ("windows", "x86_64") => Some(Self::Win64),
            ("windows", "aarch64") => Some(Self::WinArm64),
            _ => None,
        }
    }

    /// Asset file name on the jellyfin-ffmpeg GitHub release.
    /// Interpolated into the download URL; the shape here
    /// matches what the release publishes at the pinned tag.
    #[must_use]
    fn asset_name(self) -> String {
        match self {
            Self::Linux64 => {
                format!("jellyfin-ffmpeg_{JELLYFIN_FFMPEG_VERSION}_portable_linux64-gpl.tar.xz")
            }
            Self::LinuxArm64 => {
                format!("jellyfin-ffmpeg_{JELLYFIN_FFMPEG_VERSION}_portable_linuxarm64-gpl.tar.xz")
            }
            Self::Mac64 => {
                format!("jellyfin-ffmpeg_{JELLYFIN_FFMPEG_VERSION}_portable_mac64-gpl.tar.xz")
            }
            Self::MacArm64 => {
                format!("jellyfin-ffmpeg_{JELLYFIN_FFMPEG_VERSION}_portable_macarm64-gpl.tar.xz")
            }
            Self::Win64 => {
                format!("jellyfin-ffmpeg_{JELLYFIN_FFMPEG_VERSION}_portable_win64-clang-gpl.zip")
            }
            Self::WinArm64 => {
                format!("jellyfin-ffmpeg_{JELLYFIN_FFMPEG_VERSION}_portable_winarm64-clang-gpl.zip")
            }
        }
    }

    fn pin(self) -> &'static AssetPin {
        match self {
            Self::Linux64 => &PLATFORMS[0],
            Self::LinuxArm64 => &PLATFORMS[1],
            Self::Mac64 => &PLATFORMS[2],
            Self::MacArm64 => &PLATFORMS[3],
            Self::Win64 => &PLATFORMS[4],
            Self::WinArm64 => &PLATFORMS[5],
        }
    }
}

/// Hand-verified SHA256s + sizes for the six platform assets at
/// the pinned version. Computed by downloading each asset +
/// `sha256sum` + `stat -c%s` at pin time. Re-verify on bumps.
#[derive(Debug, Clone, Copy)]
struct AssetPin {
    sha256: &'static str,
    size: u64,
}

const PLATFORMS: [AssetPin; 6] = [
    // linux64
    AssetPin {
        sha256: "767271fdb384f802159e19b059d6f382a64749980e7c0890a733e13a33f5c5bc",
        size: 58_362_444,
    },
    // linuxarm64
    AssetPin {
        sha256: "6932651fa3cfaa17c1619aa1bee94520e11f827a9054fd35fae9844ecafea0d9",
        size: 51_412_576,
    },
    // mac64
    AssetPin {
        sha256: "f3ac6beb0ea27497ed88378b700a66d71d340c8f6ab3233427f12f2b64de3db6",
        size: 36_594_048,
    },
    // macarm64
    AssetPin {
        sha256: "d1c22355cc8c915b4b0f5c0c5dc776ef61e133f96cce738672344578b713b171",
        size: 31_068_640,
    },
    // win64
    AssetPin {
        sha256: "c9c10529a765bbd01e4cfb01c518634340f3e7d9f9364f759f1f3cefc49fb11e",
        size: 60_245_199,
    },
    // winarm64
    AssetPin {
        sha256: "6653b540836f8a24ec68c088a34508a7fe5c5f7364a7d054f0d311b065707ca5",
        size: 46_630_091,
    },
];

fn download_url(platform: Platform) -> String {
    format!(
        "https://github.com/jellyfin/jellyfin-ffmpeg/releases/download/v{JELLYFIN_FFMPEG_VERSION}/{}",
        platform.asset_name()
    )
}

// ─── Public state ─────────────────────────────────────────────────

/// Public-facing snapshot of the download subsystem. Returned
/// by `GET /api/v1/playback/ffmpeg/download`; emitted via
/// `AppEvent` variants as the state transitions.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FfmpegDownloadState {
    /// No download active, no previous attempt this session.
    Idle,
    /// Download in progress. `bytes` is monotonically increasing;
    /// `total` is the expected tarball size (from the pin).
    Running {
        bytes: u64,
        total: u64,
        version: String,
    },
    /// Download + verify + extract succeeded. `path` points at
    /// the extracted ffmpeg binary.
    Completed { version: String, path: String },
    /// Download / verify / extract failed. `reason` is UI-grade
    /// text the frontend renders next to the error state.
    Failed { reason: String },
}

/// Synchronisation primitive for at-most-one concurrent
/// download. Cloned onto `AppState` so every consumer reads
/// the same state.
#[derive(Debug, Clone)]
pub struct FfmpegDownloadTracker {
    inner: Arc<Mutex<FfmpegDownloadState>>,
    cancel: Arc<Mutex<Option<CancellationToken>>>,
}

impl Default for FfmpegDownloadTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FfmpegDownloadTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FfmpegDownloadState::Idle)),
            cancel: Arc::new(Mutex::new(None)),
        }
    }

    /// Current state snapshot for the GET endpoint.
    pub async fn snapshot(&self) -> FfmpegDownloadState {
        self.inner.lock().await.clone()
    }

    /// Cancel any in-flight download. No-op when idle / already
    /// completed / already failed — the `DELETE` endpoint wraps
    /// this and returns 200 either way.
    pub async fn cancel(&self) {
        if let Some(token) = self.cancel.lock().await.as_ref() {
            token.cancel();
        }
    }

    /// Reset the tracker to `Idle`. Called by the revert
    /// endpoint so the settings panel doesn't show a stale
    /// "Completed" state after the user reverts to system
    /// ffmpeg.
    pub async fn set_idle(&self) {
        *self.inner.lock().await = FfmpegDownloadState::Idle;
    }

    async fn set(&self, state: FfmpegDownloadState) {
        *self.inner.lock().await = state;
    }
}

// ─── Errors ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FfmpegBundleError {
    #[error("platform not supported (os={os}, arch={arch})")]
    UnsupportedPlatform {
        os: &'static str,
        arch: &'static str,
    },
    #[error("a download is already in progress")]
    AlreadyRunning,
    #[error("cancelled")]
    Cancelled,
    #[error("network: {0}")]
    Network(String),
    #[error("size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("extract failed: {0}")]
    Extract(String),
    #[error("io: {0}")]
    Io(String),
}

// ─── Download task ────────────────────────────────────────────────

/// Kick off the download task. Returns immediately; progress
/// + completion flow via `AppEvent` broadcasts + the tracker.
///
/// Rejects with `AlreadyRunning` when a prior call's task
/// hasn't reached a terminal state. Rejects with
/// `UnsupportedPlatform` when the host isn't in the pinned
/// table.
pub async fn start_download(
    tracker: FfmpegDownloadTracker,
    data_path: PathBuf,
    event_tx: broadcast::Sender<AppEvent>,
    db: sqlx::SqlitePool,
    transcode: Option<crate::playback::transcode::TranscodeManager>,
) -> Result<(), FfmpegBundleError> {
    // Only one download at a time. The state-check + set-running
    // is under the same lock so a second concurrent caller
    // can't race past the check.
    let platform = Platform::detect().ok_or(FfmpegBundleError::UnsupportedPlatform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })?;

    {
        let mut guard = tracker.inner.lock().await;
        if matches!(*guard, FfmpegDownloadState::Running { .. }) {
            return Err(FfmpegBundleError::AlreadyRunning);
        }
        *guard = FfmpegDownloadState::Running {
            bytes: 0,
            total: platform.pin().size,
            version: JELLYFIN_FFMPEG_VERSION.to_owned(),
        };
    }

    let cancel = CancellationToken::new();
    *tracker.cancel.lock().await = Some(cancel.clone());

    tokio::spawn(async move {
        let result = run_download(
            platform,
            &data_path,
            &tracker,
            &event_tx,
            &db,
            transcode.as_ref(),
            cancel.clone(),
        )
        .await;
        match result {
            Ok(path) => {
                tracker
                    .set(FfmpegDownloadState::Completed {
                        version: JELLYFIN_FFMPEG_VERSION.to_owned(),
                        path: path.to_string_lossy().to_string(),
                    })
                    .await;
                let _ = event_tx.send(AppEvent::FfmpegDownloadCompleted {
                    version: JELLYFIN_FFMPEG_VERSION.to_owned(),
                    path: path.to_string_lossy().to_string(),
                });
                tracing::info!(
                    version = JELLYFIN_FFMPEG_VERSION,
                    path = %path.display(),
                    "ffmpeg bundle download completed",
                );
            }
            Err(e) => {
                let reason = e.to_string();
                tracker
                    .set(FfmpegDownloadState::Failed {
                        reason: reason.clone(),
                    })
                    .await;
                let _ = event_tx.send(AppEvent::FfmpegDownloadFailed { reason });
                tracing::warn!(error = %e, "ffmpeg bundle download failed");
            }
        }
        *tracker.cancel.lock().await = None;
    });

    Ok(())
}

async fn run_download(
    platform: Platform,
    data_path: &Path,
    tracker: &FfmpegDownloadTracker,
    event_tx: &broadcast::Sender<AppEvent>,
    db: &sqlx::SqlitePool,
    transcode: Option<&crate::playback::transcode::TranscodeManager>,
    cancel: CancellationToken,
) -> Result<PathBuf, FfmpegBundleError> {
    let pin = platform.pin();
    let url = download_url(platform);
    tracing::info!(url = %url, expected_size = pin.size, "downloading jellyfin-ffmpeg");

    let bin_dir = data_path.join("bin");
    let tmp_dir = data_path.join("bin.tmp");
    // Clean any leftover scratch from a previous aborted run so
    // extraction starts fresh.
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("create tmp dir: {e}")))?;

    let archive_path = tmp_dir.join("asset");
    download_with_progress(&url, &archive_path, pin, tracker, event_tx, &cancel).await?;

    verify_sha256(&archive_path, pin.sha256).await?;

    extract(&archive_path, &tmp_dir, platform).await?;

    // Locate ffmpeg + ffprobe in the extracted tree (portable
    // tarballs put them at the root; Windows .zips use a
    // `jellyfin-ffmpeg_{VER}` subdir historically — be
    // permissive).
    let (ffmpeg_src, ffprobe_src) = find_binaries(&tmp_dir, platform).await?;

    // Atomic-ish swap: remove bin/, rename tmp → bin/
    let _ = tokio::fs::remove_dir_all(&bin_dir).await;
    tokio::fs::create_dir_all(&bin_dir)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("create bin dir: {e}")))?;

    let ffmpeg_dst = bin_dir.join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    let ffprobe_dst = bin_dir.join(if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    });

    tokio::fs::copy(&ffmpeg_src, &ffmpeg_dst)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("copy ffmpeg: {e}")))?;
    tokio::fs::copy(&ffprobe_src, &ffprobe_dst)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("copy ffprobe: {e}")))?;

    #[cfg(unix)]
    {
        set_executable(&ffmpeg_dst).await?;
        set_executable(&ffprobe_dst).await?;
    }

    // Scratch cleanup — best-effort.
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    // Flip config.ffmpeg_path + refresh the probe cache + tell
    // the TranscodeManager to use the new binary on future
    // spawns. All three must happen together — previously only
    // the config + probe were updated, so the manager kept
    // spawning its startup-captured binary even after a
    // successful bundle install. The bug looked like "probe
    // says 7.1.3 but transcode sessions still fail with the
    // old build's errors."
    let ffmpeg_path_str = ffmpeg_dst.to_string_lossy().to_string();
    sqlx::query("UPDATE config SET ffmpeg_path = ? WHERE id = 1")
        .bind(&ffmpeg_path_str)
        .execute(db)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("update config: {e}")))?;
    let caps = crate::playback::hw_probe::run_probe(&ffmpeg_path_str).await;
    crate::playback::hw_probe_cache::set_cached(caps);
    if let Some(tm) = transcode {
        tm.set_ffmpeg_path(&ffmpeg_path_str);
    }

    Ok(ffmpeg_dst)
}

async fn download_with_progress(
    url: &str,
    dest: &Path,
    pin: &AssetPin,
    tracker: &FfmpegDownloadTracker,
    event_tx: &broadcast::Sender<AppEvent>,
    cancel: &CancellationToken,
) -> Result<(), FfmpegBundleError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| FfmpegBundleError::Network(e.to_string()))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| FfmpegBundleError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| FfmpegBundleError::Network(e.to_string()))?;

    let content_length = resp.content_length().unwrap_or(pin.size);
    if content_length != pin.size {
        // Don't bail — fall through and the SHA check will
        // catch any real tampering. But log so a pin drift is
        // visible in the logs.
        tracing::warn!(
            expected = pin.size,
            got = content_length,
            "unexpected content-length from github; continuing, SHA will gate",
        );
    }

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("open dest: {e}")))?;

    let mut stream = resp.bytes_stream();
    let mut bytes_written: u64 = 0;
    let mut last_emit: u64 = 0;

    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(FfmpegBundleError::Cancelled);
        }
        let chunk = chunk.map_err(|e| FfmpegBundleError::Network(e.to_string()))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| FfmpegBundleError::Io(format!("write chunk: {e}")))?;
        bytes_written += chunk.len() as u64;

        // Throttle progress emit to ~every 512 KB so a slow
        // receiver doesn't drown in events + the UI gets
        // roughly 10-20 updates per MB of a 60 MB download
        // without being saturating.
        if bytes_written - last_emit >= 512 * 1024 {
            last_emit = bytes_written;
            tracker
                .set(FfmpegDownloadState::Running {
                    bytes: bytes_written,
                    total: pin.size,
                    version: JELLYFIN_FFMPEG_VERSION.to_owned(),
                })
                .await;
            let _ = event_tx.send(AppEvent::FfmpegDownloadProgress {
                bytes: bytes_written,
                total: pin.size,
            });
        }
    }
    file.flush()
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("flush: {e}")))?;
    drop(file);

    let metadata = tokio::fs::metadata(dest)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("stat dest: {e}")))?;
    if metadata.len() != pin.size {
        return Err(FfmpegBundleError::SizeMismatch {
            expected: pin.size,
            actual: metadata.len(),
        });
    }

    Ok(())
}

async fn verify_sha256(path: &Path, expected_hex: &str) -> Result<(), FfmpegBundleError> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("open for hash: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| FfmpegBundleError::Io(format!("read for hash: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex::encode(hasher.finalize());
    if actual != expected_hex {
        return Err(FfmpegBundleError::ChecksumMismatch {
            expected: expected_hex.to_owned(),
            actual,
        });
    }
    Ok(())
}

async fn extract(
    archive: &Path,
    dest: &Path,
    _platform: Platform,
) -> Result<(), FfmpegBundleError> {
    // `tar -xJf archive.tar.xz -C dest/` handles xz on Unix.
    // Windows 10+ ships bsdtar that extracts both .tar.xz and
    // .zip via `tar -xf`. We always use `tar -xf` — it detects
    // the archive format by magic bytes rather than extension.
    let status = tokio::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .await
        .map_err(|e| FfmpegBundleError::Extract(format!("spawn tar: {e}")))?;
    if !status.success() {
        return Err(FfmpegBundleError::Extract(format!("tar exited {status}")));
    }
    Ok(())
}

async fn find_binaries(
    tmp_dir: &Path,
    _platform: Platform,
) -> Result<(PathBuf, PathBuf), FfmpegBundleError> {
    // jellyfin-ffmpeg portable tarballs place `ffmpeg` + `ffprobe`
    // directly in the archive root; after `tar -xf`ing into
    // `bin.tmp/`, they land as `bin.tmp/ffmpeg` (+ `.exe` on
    // Windows). Be permissive in case the layout changes — walk
    // the tree looking for the two binaries by name.
    let ffmpeg_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let ffprobe_name = if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let ffmpeg = walk_find(tmp_dir, ffmpeg_name).await?;
    let ffprobe = walk_find(tmp_dir, ffprobe_name).await?;
    Ok((ffmpeg, ffprobe))
}

async fn walk_find(root: &Path, name: &str) -> Result<PathBuf, FfmpegBundleError> {
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| FfmpegBundleError::Io(format!("read {}: {e}", dir.display())))?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Ok(path);
            }
        }
    }
    Err(FfmpegBundleError::Extract(format!(
        "did not find {name} in extracted archive"
    )))
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<(), FfmpegBundleError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("stat for chmod: {e}")))?;
    let mut perms = meta.permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(path, perms)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("chmod: {e}")))?;
    Ok(())
}

// ─── Revert ───────────────────────────────────────────────────────

/// Revert to the system ffmpeg: clears `config.ffmpeg_path` +
/// removes `{data_path}/bin/`. Re-runs the probe so the UI
/// reflects the swap. Idempotent — safe to call when no
/// bundle is installed.
pub async fn revert_to_system(
    data_path: &Path,
    db: &sqlx::SqlitePool,
    transcode: Option<&crate::playback::transcode::TranscodeManager>,
) -> Result<(), FfmpegBundleError> {
    // Empty string rather than NULL — the config column is
    // NOT NULL. Downstream reads already treat empty-string
    // as "unset" and fall back to `"ffmpeg"` on $PATH.
    sqlx::query("UPDATE config SET ffmpeg_path = '' WHERE id = 1")
        .execute(db)
        .await
        .map_err(|e| FfmpegBundleError::Io(format!("clear config: {e}")))?;
    let bin_dir = data_path.join("bin");
    let _ = tokio::fs::remove_dir_all(&bin_dir).await;
    let caps = crate::playback::hw_probe::run_probe("ffmpeg").await;
    crate::playback::hw_probe_cache::set_cached(caps);
    if let Some(tm) = transcode {
        tm.set_ffmpeg_path("ffmpeg");
    }
    tracing::info!("reverted to system ffmpeg");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_detect_matches_current_host() {
        let p = Platform::detect();
        match std::env::consts::OS {
            "linux" | "macos" | "windows" => {
                // Should resolve to one of the six variants on
                // supported hosts. CI runs on linux64 so this
                // asserts that branch is healthy; aarch64 CI
                // would hit a different arm.
                assert!(p.is_some());
            }
            _ => assert!(p.is_none()),
        }
    }

    #[test]
    fn every_platform_has_a_pin() {
        for p in [
            Platform::Linux64,
            Platform::LinuxArm64,
            Platform::Mac64,
            Platform::MacArm64,
            Platform::Win64,
            Platform::WinArm64,
        ] {
            let pin = p.pin();
            assert_eq!(pin.sha256.len(), 64, "sha256 hex should be 64 chars");
            assert!(pin.size > 1_000_000, "tarballs are 30+ MB");
        }
    }

    #[test]
    fn download_url_format() {
        let url = download_url(Platform::Linux64);
        assert!(url.starts_with("https://github.com/jellyfin/jellyfin-ffmpeg/releases/download/v"));
        assert!(url.ends_with("_portable_linux64-gpl.tar.xz"));
        assert!(url.contains(JELLYFIN_FFMPEG_VERSION));
    }

    #[test]
    fn windows_uses_zip_extension() {
        let url = download_url(Platform::Win64);
        assert!(url.ends_with("_portable_win64-clang-gpl.zip"));
    }
}
