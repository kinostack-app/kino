//! Hardware-accel probe + typed capability matrix.
//!
//! `HwCapabilities` is the one surface every consumer reads:
//! * **Status banner** — "no hardware encoding available"
//!   warning gated on `any_available() == false`.
//! * **Settings UI** — renders per-backend state with typed hints.
//! * **Settings auto-select** — `suggested()` picks the
//!   highest-priority available backend for the host.
//! * **Profile chain (future)** — filters its
//!   `[HwTranscode, Remux, SwTranscode]` chain against the
//!   backends that actually passed their trial encode.
//!
//! Probing runs at startup in a background task (so startup
//! doesn't wait for ffmpeg trial encodes) and again when the
//! operator clicks "Test `FFmpeg`" in settings. The result is
//! cached (`playback::hw_probe_cache`) for cheap reads.
//!
//! The probe runs a real 1-frame encode through each
//! compiled-in backend rather than trusting `ffmpeg -encoders`.
//! An encoder may be compiled in but the runtime driver /
//! device may be missing — `h264_nvenc` linked but no
//! `libcuda.so.1`, `h264_vaapi` linked but no
//! `/dev/dri/renderD128`. The stronger check prevents the class
//! of bug where the user picks NVENC in settings, every
//! transcode immediately fails, and the only diagnostic is
//! "ffmpeg exited 1".
//!
//! Trial encodes run in parallel (`tokio::join!`) so total
//! probe time is bounded by the slowest backend.

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use utoipa::ToSchema;

/// Hardware-accel backend kinds the probe knows about. Pure
/// identifier — separate from `playback::transcode::HwAccel`
/// which also carries per-backend device paths + encoder args.
/// Conversion to `HwAccel` happens at session-creation time when
/// the profile chain picks a backend from the probe result.
///
/// Serialised as lowercase (`"videotoolbox"` not `"video_toolbox"`)
/// so the wire representation matches the `config.hw_acceleration`
/// string values the settings UI stores — one name for one concept
/// across DB, API, and frontend.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum HwBackend {
    Vaapi,
    Nvenc,
    Qsv,
    VideoToolbox,
    Amf,
}

impl HwBackend {
    /// All backends in a canonical order — iteration order for the
    /// probe, the settings UI, `suggested()` fallback.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Vaapi,
            Self::Nvenc,
            Self::Qsv,
            Self::VideoToolbox,
            Self::Amf,
        ]
    }

    /// Display name for logs + UI labels.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vaapi => "VAAPI",
            Self::Nvenc => "NVENC",
            Self::Qsv => "Quick Sync",
            Self::VideoToolbox => "VideoToolbox",
            Self::Amf => "AMF",
        }
    }

    /// Matches the `config.hw_acceleration` string values the
    /// settings page stores and `HwAccel::from_config` parses.
    #[must_use]
    pub const fn as_config_value(self) -> &'static str {
        match self {
            Self::Vaapi => "vaapi",
            Self::Nvenc => "nvenc",
            Self::Qsv => "qsv",
            Self::VideoToolbox => "videotoolbox",
            Self::Amf => "amf",
        }
    }
}

/// Per-backend outcome from a probe.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BackendStatus {
    pub backend: HwBackend,
    pub state: BackendState,
}

/// Tagged union of possible backend states. `#[serde(tag = "status")]`
/// gives the frontend a discriminated-union shape it can exhaustively
/// narrow on.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BackendState {
    /// Encoder compiled in and the 1-frame trial encode
    /// succeeded.
    Available {
        /// Parsed vendor / driver string where stderr yielded one —
        /// "Intel iHD driver", "Mesa Gallium driver", the
        /// NVIDIA driver version, etc. Purely informational.
        driver_fingerprint: Option<String>,
        /// Physical device path (VAAPI / QSV).
        device: Option<String>,
    },
    /// Encoder compiled in but the trial encode failed.
    Unavailable {
        kind: HwaFailureKind,
        /// Operator-readable one-line fix hint.
        hint: String,
        /// Salient stderr line (last error-ish line) for inline
        /// diagnostics.
        stderr_tail: String,
    },
    /// Encoder is not compiled into this ffmpeg build. Common for
    /// minimal / distro builds. Not a failure — the user just
    /// doesn't have that backend available on this install.
    NotCompiled,
    /// Backend is explicitly N/A on this platform (`VideoToolbox`
    /// off macOS, AMF off Windows). Distinct from `NotCompiled`
    /// because there's nothing the operator can install to fix it.
    NotApplicable { reason: String },
}

/// Classified failure cause. Used by the settings UI to pick a
/// fix-it icon / copy variant without re-parsing stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HwaFailureKind {
    /// Userspace driver library missing (`libcuda.so.1`, etc.).
    DriverMissing,
    /// Device node missing or inaccessible (`/dev/dri/renderD128`).
    DeviceUnavailable,
    /// Hardware present but can't serve this encoder (old CPU,
    /// unsupported generation).
    NoCapableHardware,
    /// Trial encode failed for a reason the classifier didn't
    /// match. Hint falls back to quoting the stderr tail.
    Unknown,
}

/// Full typed probe result. Every future consumer reads from
/// this single shape; no legacy flat-bool mirror.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HwCapabilities {
    /// ffmpeg itself is runnable and `-version` succeeded.
    pub ffmpeg_ok: bool,
    /// First line of `ffmpeg -version` (e.g., "ffmpeg version 6.1.1").
    pub ffmpeg_version: Option<String>,
    /// Major version number extracted from the version string.
    /// `None` when parsing failed or ffmpeg wasn't runnable. Drives
    /// the tiered "out of date" warnings: major < 4 is an error,
    /// major < 7 is a warning (Blackwell-class compat issues +
    /// missing libplacebo APIs).
    pub ffmpeg_major: Option<u32>,
    /// True when the version line contains "Jellyfin", meaning
    /// this is a jellyfin-ffmpeg build with their hardware
    /// compat patches + a known-good feature matrix. Surfaced on
    /// the settings page as "(Jellyfin build ✓)" so operators
    /// can see they're on a tested binary.
    pub is_jellyfin_build: bool,
    /// `--enable-libplacebo` present in the configure string.
    /// Gates HW tone-mapping + adaptive HDR10+ tone curves +
    /// proper DV-profile-5 handling (tracker MAJOR items that
    /// all depend on libplacebo).
    pub has_libplacebo: bool,
    /// `--enable-libass` present in the configure string. Gates
    /// styled ASS / SSA subtitle rendering without a full
    /// burn-in transcode.
    pub has_libass: bool,
    /// Software encoder names present in this ffmpeg build that
    /// we care about (libx264, libx265, ...). Empty when ffmpeg
    /// exists but has no encoder support (minimal build).
    pub software_codecs: Vec<String>,
    /// Per-backend state. Contains one entry for every variant of
    /// `HwBackend`, in `HwBackend::all()` order.
    pub backends: Vec<BackendStatus>,
}

impl HwCapabilities {
    /// True when at least one hardware backend is `Available`.
    /// Drives the "using software encoding" status banner warning.
    #[must_use]
    pub fn any_available(&self) -> bool {
        self.backends
            .iter()
            .any(|b| matches!(b.state, BackendState::Available { .. }))
    }

    /// True when the given backend passed its trial encode. Used
    /// by the profile chain when filtering candidate profiles.
    #[must_use]
    pub fn is_available(&self, backend: HwBackend) -> bool {
        self.backends
            .iter()
            .any(|b| b.backend == backend && matches!(b.state, BackendState::Available { .. }))
    }

    /// Highest-priority available backend, by the platform
    /// convention (`VideoToolbox` on macOS, NVENC > VAAPI > QSV >
    /// AMF elsewhere). Returns `None` when nothing is available —
    /// callers treat that as "use software encoding".
    #[must_use]
    pub fn suggested(&self) -> Option<HwBackend> {
        const PRIORITY: [HwBackend; 5] = [
            HwBackend::VideoToolbox,
            HwBackend::Nvenc,
            HwBackend::Vaapi,
            HwBackend::Qsv,
            HwBackend::Amf,
        ];
        PRIORITY.iter().copied().find(|b| self.is_available(*b))
    }
}

// ─── Probe runner ────────────────────────────────────────────────

/// Run the full probe against the given ffmpeg binary. Pure
/// function — no cache writes; callers persist via
/// `hw_probe_cache::set_cached`. Duration ~200–500 ms
/// depending on how many backends pass their trial encode.
pub async fn run_probe(ffmpeg_path: &str) -> HwCapabilities {
    let info = probe_version(ffmpeg_path).await;
    let ffmpeg_ok = info.ok;

    // Encoder list is used to decide which trial encodes to
    // attempt. A backend whose encoder isn't compiled in returns
    // `NotCompiled` without running ffmpeg at all.
    let encoders = if ffmpeg_ok {
        list_encoders(ffmpeg_path).await
    } else {
        Vec::new()
    };
    let sw_codecs: Vec<String> = ["libx264", "libx265", "libvpx-vp9", "libaom-av1", "aac"]
        .iter()
        .filter(|c| encoders.iter().any(|e| e == *c))
        .map(|&c| c.to_owned())
        .collect();

    let backends = if ffmpeg_ok {
        probe_backends(ffmpeg_path, &encoders).await
    } else {
        // No ffmpeg binary => every backend reports NotCompiled
        // with an explanatory trail.
        HwBackend::all()
            .into_iter()
            .map(|b| BackendStatus {
                backend: b,
                state: BackendState::NotCompiled,
            })
            .collect()
    };

    tracing::info!(
        ffmpeg_ok,
        version = ?info.version_line,
        major = ?info.major,
        is_jellyfin = info.is_jellyfin,
        has_libplacebo = info.has_libplacebo,
        has_libass = info.has_libass,
        available = backends
            .iter()
            .filter(|b| matches!(b.state, BackendState::Available { .. }))
            .count(),
        "hw probe complete",
    );

    HwCapabilities {
        ffmpeg_ok,
        ffmpeg_version: info.version_line,
        ffmpeg_major: info.major,
        is_jellyfin_build: info.is_jellyfin,
        has_libplacebo: info.has_libplacebo,
        has_libass: info.has_libass,
        software_codecs: sw_codecs,
        backends,
    }
}

/// Parsed snapshot of `ffmpeg -version`. Split out from
/// `HwCapabilities` so the parser is unit-testable in
/// isolation without a running ffmpeg.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FfmpegBuildInfo {
    pub ok: bool,
    pub version_line: Option<String>,
    pub major: Option<u32>,
    pub is_jellyfin: bool,
    pub has_libplacebo: bool,
    pub has_libass: bool,
}

async fn probe_version(ffmpeg: &str) -> FfmpegBuildInfo {
    match Command::new(ffmpeg).arg("-version").output().await {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_ffmpeg_version(&stdout)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            tracing::warn!(stderr = %stderr, "ffmpeg -version failed");
            FfmpegBuildInfo::default()
        }
        Err(e) => {
            tracing::warn!(error = %e, "ffmpeg -version spawn failed");
            FfmpegBuildInfo::default()
        }
    }
}

/// Parse `ffmpeg -version` stdout. Pulls:
/// * First line → `version_line` + `is_jellyfin` detection
/// * Version major (first integer after "version ")
/// * `configuration:` line → `has_libplacebo` / `has_libass`
///
/// All fields degrade to `None` / `false` when the expected
/// shape isn't there. ffmpeg has kept this output format
/// stable across 3.x → 8.x; the per-field fallbacks mean a
/// weird / truncated output still parses cleanly.
fn parse_ffmpeg_version(stdout: &str) -> FfmpegBuildInfo {
    let first_line = stdout.lines().next().unwrap_or("").trim().to_owned();
    let lc_first = first_line.to_ascii_lowercase();
    let is_jellyfin = lc_first.contains("jellyfin");

    // Major version: find the token immediately after
    // "version" and parse the leading integer. Anchoring on
    // "version" matters because the copyright year ("2000")
    // would otherwise match a free-ranging digit scan.
    // Handles "7.1.3-Jellyfin", "5.1.8-0+deb12u1", and the
    // "n6.1-dev-12345" nightly-snapshot prefix; returns None
    // for unparseable strings like "N-12345-abc".
    let mut tokens = lc_first.split_whitespace();
    let major = loop {
        match tokens.next() {
            Some("version") => {
                let candidate = tokens.next().unwrap_or("");
                let candidate = candidate.strip_prefix('n').unwrap_or(candidate);
                let head: String = candidate.chars().take_while(char::is_ascii_digit).collect();
                break head.parse::<u32>().ok();
            }
            Some(_) => {}
            None => break None,
        }
    };

    // Configuration flags live on a line starting with
    // `configuration:`. Substring-match for the features we
    // care about — the flags are stable across ffmpeg versions
    // and the line is the canonical place to find them.
    let config_line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("configuration:"))
        .unwrap_or("")
        .to_ascii_lowercase();
    let has_libplacebo = config_line.contains("--enable-libplacebo");
    let has_libass = config_line.contains("--enable-libass");

    FfmpegBuildInfo {
        ok: !first_line.is_empty(),
        version_line: (!first_line.is_empty()).then_some(first_line),
        major,
        is_jellyfin,
        has_libplacebo,
        has_libass,
    }
}

async fn list_encoders(ffmpeg: &str) -> Vec<String> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .await;
    let Ok(out) = out else { return Vec::new() };
    // ffmpeg -encoders output: after a header block, each line is
    // of the form `  V..... h264_nvenc   NVIDIA NVENC H.264 encoder`.
    // Second whitespace-separated token is the codec name.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            // Encoder lines start with codec-type flags (V/A/S
            // followed by dots/letters). Skip the header.
            let first = t.split_whitespace().next()?;
            if first.len() < 6 || !first.chars().next().is_some_and(|c| "VAS".contains(c)) {
                return None;
            }
            t.split_whitespace().nth(1).map(str::to_owned)
        })
        .collect()
}

async fn probe_backends(ffmpeg: &str, encoders: &[String]) -> Vec<BackendStatus> {
    let (vaapi, nvenc, qsv, vt, amf) = tokio::join!(
        trial(ffmpeg, encoders, HwBackend::Vaapi),
        trial(ffmpeg, encoders, HwBackend::Nvenc),
        trial(ffmpeg, encoders, HwBackend::Qsv),
        trial(ffmpeg, encoders, HwBackend::VideoToolbox),
        trial(ffmpeg, encoders, HwBackend::Amf),
    );
    vec![vaapi, nvenc, qsv, vt, amf]
}

async fn trial(ffmpeg: &str, encoders: &[String], backend: HwBackend) -> BackendStatus {
    // Platform-N/A short-circuits before checking encoder
    // presence — a Linux build might compile in
    // `h264_videotoolbox` from a stray Homebrew header, but it
    // still can't actually encode off macOS. Be explicit.
    if let Some(reason) = platform_not_applicable(backend) {
        return BackendStatus {
            backend,
            state: BackendState::NotApplicable { reason },
        };
    }

    if !backend_compiled_in(encoders, backend) {
        return BackendStatus {
            backend,
            state: BackendState::NotCompiled,
        };
    }

    let args = trial_args(backend);
    let outcome = match Command::new(ffmpeg).args(&args).output().await {
        Ok(out) if out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            BackendState::Available {
                driver_fingerprint: parse_driver_fingerprint(backend, &stderr),
                device: trial_device(backend),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let (kind, hint) = classify_failure(backend, &stderr);
            BackendState::Unavailable {
                kind,
                hint,
                stderr_tail: salient_error_line(&stderr),
            }
        }
        Err(e) => BackendState::Unavailable {
            kind: HwaFailureKind::Unknown,
            hint: format!("spawn failed: {e}"),
            stderr_tail: String::new(),
        },
    };

    BackendStatus {
        backend,
        state: outcome,
    }
}

fn platform_not_applicable(backend: HwBackend) -> Option<String> {
    match backend {
        HwBackend::VideoToolbox if !cfg!(target_os = "macos") => {
            Some("VideoToolbox is a macOS-only backend.".into())
        }
        HwBackend::Amf if !cfg!(target_os = "windows") => {
            Some("AMF is Windows-only — Linux AMD GPUs should use VAAPI instead.".into())
        }
        _ => None,
    }
}

fn backend_compiled_in(encoders: &[String], backend: HwBackend) -> bool {
    let needles: &[&str] = match backend {
        HwBackend::Vaapi => &["h264_vaapi", "hevc_vaapi"],
        HwBackend::Nvenc => &["h264_nvenc", "hevc_nvenc"],
        HwBackend::Qsv => &["h264_qsv", "hevc_qsv"],
        HwBackend::VideoToolbox => &["h264_videotoolbox", "hevc_videotoolbox"],
        HwBackend::Amf => &["h264_amf", "hevc_amf"],
    };
    encoders.iter().any(|e| needles.iter().any(|n| e == n))
}

fn trial_device(backend: HwBackend) -> Option<String> {
    match backend {
        HwBackend::Vaapi | HwBackend::Qsv => Some("/dev/dri/renderD128".into()),
        HwBackend::Nvenc | HwBackend::VideoToolbox | HwBackend::Amf => None,
    }
}

/// Build ffmpeg args for a backend's smallest-possible trial
/// encode: ~0.1 s of a synthetic test pattern at a tiny
/// resolution, output discarded. Kept minimal so the probe stays
/// well under a second even with all backends running.
fn trial_args(backend: HwBackend) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "verbose", // verbose so VAAPI-init driver lines surface
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=0.1:size=320x180:rate=10",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    match backend {
        HwBackend::Vaapi => args.extend(
            [
                "-vf",
                "format=nv12,hwupload",
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-c:v",
                "h264_vaapi",
            ]
            .map(String::from),
        ),
        HwBackend::Nvenc => args.extend(["-c:v", "h264_nvenc"].map(String::from)),
        HwBackend::Qsv => args.extend(["-c:v", "h264_qsv"].map(String::from)),
        HwBackend::VideoToolbox => args.extend(["-c:v", "h264_videotoolbox"].map(String::from)),
        HwBackend::Amf => args.extend(["-c:v", "h264_amf"].map(String::from)),
    }
    args.extend(["-t", "0.1", "-f", "null", "-"].map(String::from));
    args
}

// ─── Stderr classifiers ─────────────────────────────────────────

/// Classify a trial encode failure into a typed `HwaFailureKind`
/// plus a one-line operator-readable hint. Pattern matching is on
/// substrings ffmpeg / driver libs print verbatim across versions
/// — anchor to user-visible driver messages, not ffmpeg internals
/// that may change.
#[allow(clippy::too_many_lines)] // one branch per backend × stderr pattern is inherently long; splitting obscures the mapping
fn classify_failure(backend: HwBackend, stderr: &str) -> (HwaFailureKind, String) {
    let lower = stderr.to_lowercase();
    match backend {
        HwBackend::Nvenc => {
            if stderr.contains("libcuda.so.1") || lower.contains("cannot load libcuda") {
                (
                    HwaFailureKind::DriverMissing,
                    "Install the NVIDIA proprietary driver on this host. If running in Docker, \
                     also install `nvidia-container-toolkit` and pass `--gpus all` (or set \
                     `runtime: nvidia` in compose)."
                        .into(),
                )
            } else if stderr.contains("No capable devices") || stderr.contains("No NVENC capable") {
                (
                    HwaFailureKind::NoCapableHardware,
                    "No NVENC-capable NVIDIA GPU detected on this host.".into(),
                )
            } else {
                (
                    HwaFailureKind::Unknown,
                    format!(
                        "NVENC is compiled in but the trial encode failed: {}",
                        salient_error_line(stderr)
                    ),
                )
            }
        }
        HwBackend::Vaapi => {
            if lower.contains("vainitialize failed") || lower.contains("failed to initialize") {
                (
                    HwaFailureKind::DriverMissing,
                    "VAAPI driver failed to initialise. Install `intel-media-va-driver` \
                     (Intel) or `mesa-va-drivers` (AMD) on the host."
                        .into(),
                )
            } else if stderr.contains("/dev/dri") {
                (
                    HwaFailureKind::DeviceUnavailable,
                    "Render node `/dev/dri/renderD128` not accessible. On bare metal, ensure \
                     the user is in the `render` (or `video`) group. In Docker, add \
                     `--device /dev/dri`."
                        .into(),
                )
            } else {
                (
                    HwaFailureKind::Unknown,
                    format!(
                        "VAAPI is compiled in but the trial encode failed: {}",
                        salient_error_line(stderr)
                    ),
                )
            }
        }
        HwBackend::Qsv => {
            if stderr.contains("MFX_ERR_UNSUPPORTED") || lower.contains("no device available") {
                (
                    HwaFailureKind::NoCapableHardware,
                    "Quick Sync not supported on this CPU. Needs an Intel iGPU plus \
                     `intel-media-va-driver` or `libmfx-gen1`."
                        .into(),
                )
            } else if stderr.contains("/dev/dri") {
                (
                    HwaFailureKind::DeviceUnavailable,
                    "Render node missing. On bare metal install `intel-media-va-driver`; \
                     in Docker add `--device /dev/dri`."
                        .into(),
                )
            } else {
                (
                    HwaFailureKind::Unknown,
                    format!(
                        "Quick Sync is compiled in but the trial encode failed: {}",
                        salient_error_line(stderr)
                    ),
                )
            }
        }
        HwBackend::VideoToolbox => {
            // Should be short-circuited by `platform_not_applicable`
            // on non-macOS, so reaching this arm on Linux means a
            // stray compile-in that runtime-fails. macOS with a
            // working VT very rarely fails.
            (
                HwaFailureKind::Unknown,
                format!("VideoToolbox trial failed: {}", salient_error_line(stderr)),
            )
        }
        HwBackend::Amf => {
            if lower.contains("no amf hardware") || lower.contains("amf failed to initialize") {
                (
                    HwaFailureKind::NoCapableHardware,
                    "AMF requires a Windows host with an AMD GPU and the AMD Adrenalin \
                     driver installed. On Linux, use VAAPI instead."
                        .into(),
                )
            } else {
                (
                    HwaFailureKind::Unknown,
                    format!(
                        "AMF is compiled in but the trial encode failed: {}",
                        salient_error_line(stderr)
                    ),
                )
            }
        }
    }
}

/// Extract a driver / vendor fingerprint from the verbose stderr
/// of a successful trial. Informational only — surfaced in the
/// settings UI so the user can tell "iHD driver 24.1" from "Mesa
/// Gallium driver" and match to install state.
fn parse_driver_fingerprint(backend: HwBackend, stderr: &str) -> Option<String> {
    match backend {
        HwBackend::Vaapi | HwBackend::Qsv => {
            // libva logs "Driver version: Intel iHD driver for Intel(R) Gen Graphics - 24.1.3"
            // or "Mesa Gallium driver 24.3.0". Match either.
            for line in stderr.lines() {
                if let Some(idx) = line.find("Driver version:") {
                    return Some(line[idx + "Driver version:".len()..].trim().to_owned());
                }
                if line.contains("Intel iHD driver") || line.contains("Mesa Gallium driver") {
                    return Some(line.trim().to_owned());
                }
            }
            None
        }
        HwBackend::Nvenc => stderr
            .lines()
            .find(|l| l.contains("NVIDIA") || l.contains("NVENC"))
            .map(|l| l.trim().to_owned()),
        HwBackend::VideoToolbox | HwBackend::Amf => None,
    }
}

/// Pick the single most useful line out of a chunk of ffmpeg
/// stderr: prefer lines that look like errors ("Error", "Cannot",
/// "Failed", "Unsupported"), fall back to the last non-blank
/// line. Trimmed so it fits inline in a hint without a linebreak.
fn salient_error_line(stderr: &str) -> String {
    let is_error_ish = |l: &&str| {
        let lo = l.to_lowercase();
        lo.contains("error")
            || lo.contains("cannot")
            || lo.contains("failed")
            || lo.contains("unsupported")
    };
    stderr
        .lines()
        .rev()
        .find(is_error_ish)
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("")
        .trim()
        .to_string()
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_nvenc_driver_missing() {
        let stderr = "Cannot load libcuda.so.1\nFailed to initialize NVENC";
        let (k, hint) = classify_failure(HwBackend::Nvenc, stderr);
        assert_eq!(k, HwaFailureKind::DriverMissing);
        assert!(hint.contains("NVIDIA proprietary driver"));
    }

    #[test]
    fn classify_nvenc_no_capable_gpu() {
        let stderr = "No NVENC capable devices found";
        let (k, _) = classify_failure(HwBackend::Nvenc, stderr);
        assert_eq!(k, HwaFailureKind::NoCapableHardware);
    }

    #[test]
    fn classify_vaapi_device_unavailable() {
        let stderr = "Failed to open /dev/dri/renderD128: permission denied";
        let (k, _) = classify_failure(HwBackend::Vaapi, stderr);
        assert_eq!(k, HwaFailureKind::DeviceUnavailable);
    }

    #[test]
    fn classify_vaapi_driver_init_failed() {
        let stderr = "vaInitialize failed: unknown libva error";
        let (k, _) = classify_failure(HwBackend::Vaapi, stderr);
        assert_eq!(k, HwaFailureKind::DriverMissing);
    }

    #[test]
    fn classify_qsv_no_device() {
        let stderr = "No device available for this request\nMFX_ERR_UNSUPPORTED";
        let (k, _) = classify_failure(HwBackend::Qsv, stderr);
        assert_eq!(k, HwaFailureKind::NoCapableHardware);
    }

    #[test]
    fn classify_amf_no_hardware() {
        let stderr = "AMF failed to initialize: No AMF hardware detected";
        let (k, _) = classify_failure(HwBackend::Amf, stderr);
        assert_eq!(k, HwaFailureKind::NoCapableHardware);
    }

    #[test]
    fn classify_unknown_stderr_falls_through() {
        let stderr = "Some obscure error nobody has anticipated";
        let (k, hint) = classify_failure(HwBackend::Nvenc, stderr);
        assert_eq!(k, HwaFailureKind::Unknown);
        assert!(hint.contains("the trial encode failed"));
    }

    #[test]
    fn fingerprint_vaapi_intel() {
        let stderr = "[AVHWDeviceContext @ 0x..] libva: Driver version: Intel iHD driver for Intel(R) Gen - 24.1.3";
        let fp = parse_driver_fingerprint(HwBackend::Vaapi, stderr).unwrap();
        assert!(fp.starts_with("Intel iHD driver"), "got: {fp}");
    }

    #[test]
    fn fingerprint_vaapi_mesa() {
        let stderr = "[AVHWDeviceContext @ 0x..] Mesa Gallium driver 24.3.0";
        let fp = parse_driver_fingerprint(HwBackend::Vaapi, stderr).unwrap();
        assert!(fp.contains("Mesa Gallium driver"), "got: {fp}");
    }

    #[test]
    fn fingerprint_missing_is_none() {
        let stderr = "generic success output with no driver string";
        assert!(parse_driver_fingerprint(HwBackend::Vaapi, stderr).is_none());
    }

    #[test]
    fn salient_line_prefers_errors() {
        let stderr = "some info\nError: Cannot allocate memory\nmore info";
        let line = salient_error_line(stderr);
        assert_eq!(line, "Error: Cannot allocate memory");
    }

    #[test]
    fn salient_line_falls_back_to_last() {
        let stderr = "just some\ninformational\noutput";
        let line = salient_error_line(stderr);
        assert_eq!(line, "output");
    }

    #[test]
    fn platform_gating() {
        // VideoToolbox: NotApplicable everywhere except macOS.
        if !cfg!(target_os = "macos") {
            assert!(platform_not_applicable(HwBackend::VideoToolbox).is_some());
        }
        // AMF: NotApplicable everywhere except Windows.
        if !cfg!(target_os = "windows") {
            assert!(platform_not_applicable(HwBackend::Amf).is_some());
        }
        // The Linux-available three are never platform-gated.
        assert!(platform_not_applicable(HwBackend::Vaapi).is_none());
        assert!(platform_not_applicable(HwBackend::Nvenc).is_none());
        assert!(platform_not_applicable(HwBackend::Qsv).is_none());
    }

    #[test]
    fn any_available_and_suggested() {
        let caps = HwCapabilities {
            ffmpeg_ok: true,
            ffmpeg_version: Some("ffmpeg version 6.1".into()),
            ffmpeg_major: Some(6),
            is_jellyfin_build: false,
            has_libplacebo: false,
            has_libass: false,
            software_codecs: vec!["libx264".into()],
            backends: vec![
                BackendStatus {
                    backend: HwBackend::Vaapi,
                    state: BackendState::NotCompiled,
                },
                BackendStatus {
                    backend: HwBackend::Nvenc,
                    state: BackendState::Available {
                        driver_fingerprint: Some("NVIDIA 550.x".into()),
                        device: None,
                    },
                },
                BackendStatus {
                    backend: HwBackend::Qsv,
                    state: BackendState::Unavailable {
                        kind: HwaFailureKind::DeviceUnavailable,
                        hint: "...".into(),
                        stderr_tail: "...".into(),
                    },
                },
                BackendStatus {
                    backend: HwBackend::VideoToolbox,
                    state: BackendState::NotApplicable {
                        reason: "not macOS".into(),
                    },
                },
                BackendStatus {
                    backend: HwBackend::Amf,
                    state: BackendState::NotApplicable {
                        reason: "not Windows".into(),
                    },
                },
            ],
        };
        assert!(caps.any_available());
        assert!(caps.is_available(HwBackend::Nvenc));
        assert!(!caps.is_available(HwBackend::Vaapi));
        assert_eq!(caps.suggested(), Some(HwBackend::Nvenc));
    }

    #[test]
    fn suggested_returns_none_when_nothing_available() {
        let caps = HwCapabilities {
            ffmpeg_ok: true,
            ffmpeg_version: None,
            ffmpeg_major: None,
            is_jellyfin_build: false,
            has_libplacebo: false,
            has_libass: false,
            software_codecs: vec![],
            backends: HwBackend::all()
                .into_iter()
                .map(|b| BackendStatus {
                    backend: b,
                    state: BackendState::NotCompiled,
                })
                .collect(),
        };
        assert!(!caps.any_available());
        assert_eq!(caps.suggested(), None);
    }

    // ─── ffmpeg -version parser ──────────────────────────────

    const JELLYFIN_SAMPLE: &str = concat!(
        "ffmpeg version 7.1.3-Jellyfin Copyright (c) 2000-2025 the FFmpeg developers\n",
        "built with gcc 15.2.0 (crosstool-NG 1.28.0.23_185f348)\n",
        "configuration: --prefix=/ffbuild/prefix --enable-gpl --enable-version3 ",
        "--enable-libplacebo --enable-libass --enable-libx264 --enable-libx265 --enable-nvenc\n",
        "libavutil      59. 39.100 / 59. 39.100\n",
    );

    const DEBIAN_51_SAMPLE: &str = concat!(
        "ffmpeg version 5.1.8-0+deb12u1 Copyright (c) 2000-2024 the FFmpeg developers\n",
        "built with gcc 12 (Debian 12.2.0-14)\n",
        "configuration: --prefix=/usr --extra-version=0+deb12u1 --enable-gpl --enable-libass --enable-libx264\n",
        "libavutil      57. 28.100 / 57. 28.100\n",
    );

    const MINIMAL_SAMPLE: &str = concat!(
        "ffmpeg version N-12345-abc Copyright (c) 2000-2019 the FFmpeg developers\n",
        "built with gcc 13.2.0\n",
        "configuration: --disable-everything\n",
    );

    #[test]
    fn parser_identifies_jellyfin_build() {
        let info = parse_ffmpeg_version(JELLYFIN_SAMPLE);
        assert!(info.ok);
        assert_eq!(info.major, Some(7));
        assert!(info.is_jellyfin);
        assert!(info.has_libplacebo);
        assert!(info.has_libass);
        assert!(info.version_line.as_deref().unwrap().contains("Jellyfin"));
    }

    #[test]
    fn parser_identifies_debian_stock_ffmpeg_as_non_jellyfin() {
        // The exact build we hit the Blackwell bug on. Ensures
        // the parser doesn't false-positive on non-Jellyfin
        // builds + correctly flags libplacebo missing.
        let info = parse_ffmpeg_version(DEBIAN_51_SAMPLE);
        assert!(info.ok);
        assert_eq!(info.major, Some(5));
        assert!(!info.is_jellyfin);
        assert!(!info.has_libplacebo);
        assert!(info.has_libass);
    }

    #[test]
    fn parser_handles_nightly_version_prefix() {
        // Some ffmpeg builds use "N-12345-abc" for dev snapshots
        // rather than a numeric major. Parser should degrade
        // gracefully rather than blowing up.
        let info = parse_ffmpeg_version(MINIMAL_SAMPLE);
        assert!(info.ok);
        // "N-12345" doesn't give us a meaningful major → None
        assert_eq!(info.major, None);
        assert!(!info.is_jellyfin);
        assert!(!info.has_libplacebo);
        assert!(!info.has_libass);
    }

    #[test]
    fn parser_handles_empty_input() {
        let info = parse_ffmpeg_version("");
        assert!(!info.ok);
        assert!(info.version_line.is_none());
        assert_eq!(info.major, None);
    }

    #[test]
    fn parser_handles_missing_configuration_line() {
        // First line present but no configure-string line.
        // We get major + version but features default to false.
        let info = parse_ffmpeg_version("ffmpeg version 7.0.2 Copyright (c) …\n");
        assert!(info.ok);
        assert_eq!(info.major, Some(7));
        assert!(!info.has_libplacebo);
        assert!(!info.has_libass);
    }
}
