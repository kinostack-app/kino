//! Runtime HWA-failure classifier for mid-stream ffmpeg exits.
//!
//! The [`hw_probe`][crate::playback::hw_probe] classifier runs at
//! startup against a 1-frame trial encode; this module classifies
//! failures that happen *during* playback, where ffmpeg exited
//! non-zero after successfully producing some segments. The two
//! share a taxonomy — [`HwaFailureKind`] from `hw_probe` — but
//! need different patterns: startup failures surface as
//! "Cannot load libcuda.so.1" or "Failed to open VAAPI device",
//! while mid-stream failures surface as "NVENC session reset",
//! "VAAPI: Failed to destroy encoder" after the encoder has
//! already been running for minutes.
//!
//! # Matching strategy
//!
//! We scan the *last* N lines of the stderr tail rather than the
//! whole buffer. Mid-stream failures nearly always print the
//! precipitating error in the final few lines — earlier output is
//! progress ticks + warnings that would cause false positives
//! ("non-fatal" lines that contain HWA keywords without being
//! the actual cause of death).
//!
//! Matches are case-insensitive lowercase substring checks. This
//! is intentionally loose — we prefer to classify too aggressively
//! (triggering a fallback when an unrelated error happened) over
//! missing a real HWA failure. The cost of a false positive is a
//! single extra restart on the SW profile; the cost of a false
//! negative is the user sees a permanent stall.

use crate::playback::HwaFailureKind;

/// How many final stderr lines to scan. Picked empirically —
/// ffmpeg's progress output is ~10 lines per second, so 16
/// lines ≈ 1.5 s of final output, which comfortably captures
/// the error message + its context without reaching back into
/// earlier progress blocks.
const TAIL_SCAN_LINES: usize = 16;

/// `DriverMissing`: library went away mid-session. Rare but
/// possible after a driver uninstall / reinstall. Same
/// signatures the startup classifier uses.
const DRIVER_MISSING: &[&str] = &[
    "cannot load libcuda",
    "cannot find libnvidia",
    "failed to dlopen",
    "cannot load libva",
    "cannot load libmfx",
];

/// `DeviceUnavailable`: device node / context invalidated.
/// Includes `PCIe` resets, TDR (display timeout), GPU hang
/// recovery.
const DEVICE_UNAVAILABLE: &[&str] = &[
    "device disappeared",
    "device has been reset",
    "device lost",
    "failed to open vaapi device",
    "failed to initialise vaapi",
    "vaapi_device_create",
    "no such file or directory", // /dev/dri/renderD128 disappeared
    "input/output error",
    "cannot open display",
];

/// `NoCapableHardware`: encoder rejected a combination the
/// hardware can't handle mid-session (resolution change, exotic
/// pixel format, unsupported profile / level).
///
/// Deliberately excludes the generic "invalid argument" string —
/// that fires for argv ordering mistakes (e.g. `-hwaccel`
/// emitted after `-i`) which are orchestration bugs on our
/// side, not hardware capability failures. Classifying argv
/// bugs as `NoCapableHardware` would hide them by triggering a
/// bogus "HW unavailable" fallback when the real fix is fixing
/// the command line.
const NO_CAPABLE_HW: &[&str] = &[
    "no capable devices found",
    "no nvenc capable devices",
    "nvenc capability error",
    "no device available for encoder",
    "unsupported pixel format",
    "resolution not supported",
];

/// Generic HWA-session failures. These cover the "NVENC session
/// died, reason unclear" class — we still want to fall back, we
/// just can't say exactly why. Bucketed as `Unknown` rather than
/// a specific cause so the UI doesn't claim a root cause it
/// can't back up.
const HWA_GENERIC: &[&str] = &[
    "nvenc",
    "h264_nvenc",
    "hevc_nvenc",
    "h264_vaapi",
    "hevc_vaapi",
    "h264_qsv",
    "hevc_qsv",
    "h264_videotoolbox",
    "hevc_videotoolbox",
    "h264_amf",
    "hevc_amf",
    "vaapi",
    "videotoolbox",
    "cuda error",
    "cuvid",
];

/// Failure-indicating words. An HWA keyword without one of these
/// nearby is an info line (e.g. "Using `h264_nvenc`"), not a
/// failure — without this gate every session would be classified
/// as HWA-failed because ffmpeg always announces its encoder.
const FAILURE_WORDS: &[&str] = &[
    "error",
    "failed",
    "fatal",
    "cannot",
    "could not",
    "unable to",
];

/// Classify a stderr tail from a mid-stream ffmpeg exit.
///
/// Returns `Some(kind)` when at least one known HWA-failure
/// signature matches, `None` otherwise. A `None` result means the
/// caller should treat the failure as non-HWA — a malformed input,
/// OOM, disk full, etc. — and surface it to the user without
/// attempting a fallback (since falling back from HW to SW won't
/// fix those causes).
#[must_use]
pub fn classify_runtime_failure(stderr_tail: &str) -> Option<HwaFailureKind> {
    let tail: Vec<&str> = stderr_tail.lines().rev().take(TAIL_SCAN_LINES).collect();
    let haystack: String = tail
        .iter()
        .rev()
        .map(|l| l.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");

    if DRIVER_MISSING.iter().any(|p| haystack.contains(p)) {
        return Some(HwaFailureKind::DriverMissing);
    }
    if DEVICE_UNAVAILABLE.iter().any(|p| haystack.contains(p)) {
        return Some(HwaFailureKind::DeviceUnavailable);
    }
    if NO_CAPABLE_HW.iter().any(|p| haystack.contains(p)) {
        return Some(HwaFailureKind::NoCapableHardware);
    }
    let has_hwa_token = HWA_GENERIC.iter().any(|p| haystack.contains(p));
    let has_failure_word = FAILURE_WORDS.iter().any(|p| haystack.contains(p));
    if has_hwa_token && has_failure_word {
        return Some(HwaFailureKind::Unknown);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_nvenc_driver_unload() {
        let tail = "\
            [h264_nvenc @ 0x55] CUDA error: invalid context\n\
            Cannot load libcuda.so.1\n\
            Error initializing an encoder\n\
        ";
        assert_eq!(
            classify_runtime_failure(tail),
            Some(HwaFailureKind::DriverMissing)
        );
    }

    #[test]
    fn classifies_vaapi_device_reset() {
        let tail = "\
            [h264_vaapi @ 0x55] Failed to encode frame: Input/output error\n\
            VAAPI device has been reset\n\
        ";
        assert_eq!(
            classify_runtime_failure(tail),
            Some(HwaFailureKind::DeviceUnavailable)
        );
    }

    #[test]
    fn classifies_vaapi_device_missing() {
        let tail = "\
            [AVHWDeviceContext @ 0x55] Failed to open VAAPI device\n\
            /dev/dri/renderD128: No such file or directory\n\
        ";
        assert_eq!(
            classify_runtime_failure(tail),
            Some(HwaFailureKind::DeviceUnavailable)
        );
    }

    #[test]
    fn classifies_nvenc_no_capable_devices() {
        let tail = "\
            [h264_nvenc @ 0x55] No NVENC capable devices found\n\
            Error initializing output stream\n\
        ";
        assert_eq!(
            classify_runtime_failure(tail),
            Some(HwaFailureKind::NoCapableHardware)
        );
    }

    #[test]
    fn classifies_generic_nvenc_failure_as_unknown() {
        let tail = "\
            [h264_nvenc @ 0x55] Generic encoding error at frame 1234\n\
            Error submitting a frame for encoding\n\
        ";
        assert_eq!(
            classify_runtime_failure(tail),
            Some(HwaFailureKind::Unknown)
        );
    }

    #[test]
    fn non_hwa_error_returns_none() {
        // Out of disk space — unrelated to HWA. A fallback to SW
        // won't fix this, so we MUST return None.
        let tail = "\
            Error writing trailer: No space left on device\n\
            muxer error\n\
        ";
        assert_eq!(classify_runtime_failure(tail), None);
    }

    #[test]
    fn malformed_input_returns_none() {
        let tail = "\
            [mov,mp4,m4a,3gp,3g2,mj2 @ 0x55] moov atom not found\n\
            Invalid data found when processing input\n\
        ";
        assert_eq!(classify_runtime_failure(tail), None);
    }

    #[test]
    fn innocuous_nvenc_info_line_is_not_a_failure() {
        // No failure word present — just an informational line
        // mentioning the encoder. Must NOT classify as HWA
        // failure.
        let tail = "\
            Using h264_nvenc for encoding\n\
            frame= 1024 fps= 60 q=23 size=...\n\
            progress=end\n\
        ";
        assert_eq!(classify_runtime_failure(tail), None);
    }

    #[test]
    fn only_scans_final_lines() {
        // An early HWA error followed by a long tail of non-HWA
        // progress shouldn't classify as HWA — the death cause
        // is whatever's at the end. We cap at TAIL_SCAN_LINES
        // (16) so 20 lines of progress after an NVENC error
        // pushes the error out of scope.
        use std::fmt::Write;
        let mut tail = String::from("[h264_nvenc @ 0x55] Fatal encoder error\n");
        for i in 0..20 {
            let _ = writeln!(tail, "frame= {i} fps=60 q=23");
        }
        tail.push_str("progress=end\n");
        assert_eq!(classify_runtime_failure(&tail), None);
    }

    #[test]
    fn empty_tail_returns_none() {
        assert_eq!(classify_runtime_failure(""), None);
    }

    #[test]
    fn case_insensitive_matching() {
        let tail = "FAILED to open VAAPI device\n";
        assert_eq!(
            classify_runtime_failure(tail),
            Some(HwaFailureKind::DeviceUnavailable)
        );
    }
}
