//! Per-OS path resolution.
//!
//! When `kino` is invoked **without** an explicit `--data-path` (and
//! the `KINO_DATA_PATH` env var isn't set), this module supplies the
//! platform-appropriate default — `$XDG_DATA_HOME/kino` on Linux,
//! `~/Library/Application Support/Kino` on macOS, `%LOCALAPPDATA%\Kino`
//! on Windows. Implemented via the `etcetera` crate, the same pick
//! helix and uv use.
//!
//! **Service-mode paths are NOT auto-detected here.** Native packages
//! ship a systemd unit / launchd plist / Windows Service that pass
//! `--data-path /var/lib/kino` (or the OS equivalent) explicitly, so
//! the binary never has to guess "am I a service?". Keeping the auto-
//! detect surface zero avoids the failure mode where a user-mode
//! invocation accidentally writes to `/var/lib/kino` because some
//! environment variable looked service-shaped.
//!
//! Today only `default_data_dir()` is consumed (`init.rs` and the CLI
//! plumbing both fall back to it). The other roles — config, cache,
//! logs, runtime — are exposed for symmetry; future migrations
//! (separating cache from data, switching log destination per OS) can
//! pick them up without re-touching the CLI.
//!
//! See [`docs/architecture/cross-platform-paths.md`] for the full
//! per-OS matrix and the reasoning behind each role.

use std::path::PathBuf;

use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

const APP_DIR: &str = "kino";

fn strategy() -> Option<impl BaseStrategy> {
    // `choose_base_strategy()` returns Err if the OS doesn't expose
    // the home directory at all (only happens in oddly-configured
    // service contexts). Caller falls back to `./data` in that case.
    choose_base_strategy().ok()
}

/// Per-OS data directory. `SQLite` DB, librqbit session, image cache,
/// trickplay sprites, and backups all live under this root today.
pub fn default_data_dir() -> PathBuf {
    strategy().map_or_else(|| PathBuf::from("./data"), |s| s.data_dir().join(APP_DIR))
}

/// Per-OS config directory. Currently unused — every config setting
/// lives in the `SQLite` `config` table. Exposed for future use (e.g.
/// per-machine overrides like `~/.config/kino/local.toml`).
#[allow(dead_code)]
pub fn default_config_dir() -> PathBuf {
    strategy().map_or_else(
        || PathBuf::from("./config"),
        |s| s.config_dir().join(APP_DIR),
    )
}

/// Per-OS cache directory. Reserved for future migration of the
/// transcode-temp + image-cache roots out of `data_dir` so users can
/// `rm -rf` cache without losing library state.
#[allow(dead_code)]
pub fn default_cache_dir() -> PathBuf {
    strategy().map_or_else(|| PathBuf::from("./cache"), |s| s.cache_dir().join(APP_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_ends_with_kino() {
        let p = default_data_dir();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some(APP_DIR));
    }

    #[test]
    fn cache_and_config_use_app_dir() {
        assert_eq!(
            default_config_dir().file_name().and_then(|s| s.to_str()),
            Some(APP_DIR)
        );
        assert_eq!(
            default_cache_dir().file_name().and_then(|s| s.to_str()),
            Some(APP_DIR)
        );
    }
}
