//! Process-wide cache of the last hardware-accel probe.
//!
//! Probing is expensive (up to six ffmpeg subprocesses), so
//! consumers that poll frequently (status banner, settings page)
//! can't re-run it on every request. A single probe runs at
//! startup in a background task and again on any manual re-probe
//! via the settings UI; the result is cached here for lock-free
//! reads.
//!
//! The full typed shape lives in `playback::hw_probe::HwCapabilities`
//! — this module is just a wrapper around an `Arc<HwCapabilities>`
//! slot.

use std::sync::{Arc, RwLock};

use crate::playback::HwCapabilities;
use crate::playback::hw_probe;

static CACHE: RwLock<Option<Arc<HwCapabilities>>> = RwLock::new(None);

/// Replace the cached probe result. Readers see the new value on
/// their next `cached()` call.
pub fn set_cached(caps: HwCapabilities) {
    if let Ok(mut guard) = CACHE.write() {
        *guard = Some(Arc::new(caps));
    }
}

/// Read the last-cached probe. Returns `None` when the probe
/// hasn't run yet — callers treat that as "don't warn" so the
/// status banner doesn't flash on a fresh boot before detection
/// completes.
pub fn cached() -> Option<Arc<HwCapabilities>> {
    CACHE.read().ok().and_then(|g| g.clone())
}

/// Run a probe using the given ffmpeg path and seed the cache.
/// Called in a background task from `main.rs` so we don't block
/// startup on ~200–500 ms of trial encodes.
pub async fn detect_and_cache(ffmpeg_path: &str) {
    let caps = hw_probe::run_probe(ffmpeg_path).await;
    tracing::info!(
        ffmpeg_ok = caps.ffmpeg_ok,
        version = ?caps.ffmpeg_version,
        suggested = ?caps.suggested(),
        any_hw = caps.any_available(),
        "hw-probe cached",
    );
    set_cached(caps);
}
