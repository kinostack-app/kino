//! Single-instance enforcement for the tray.
//!
//! `acquire()` returns a guard whose `Drop` releases the OS lock when
//! the tray exits. A second `kino tray` invocation in the same user
//! session sees `try_lock_exclusive` fail and returns a clean
//! "already running" error instead of producing a duplicate icon.
//!
//! Cross-platform via `fs4` — `flock` on Unix, `LockFileEx` on
//! Windows. The lock file lives in `$XDG_RUNTIME_DIR` when set
//! (Linux/macOS GUI sessions usually have it), with a per-user
//! fallback to the system temp dir so multi-user systems still get
//! one tray per user.

use anyhow::Context as _;
use fs4::fs_std::FileExt as _;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;

#[derive(Debug)]
pub struct InstanceLock {
    // The lock is held for the lifetime of the open `File`. Dropping
    // the file handle releases the lock; we keep both around to make
    // the contract explicit. `_path` is retained for diagnostics
    // (logs already mention it on acquire failure).
    _file: File,
    _path: PathBuf,
}

pub fn acquire() -> anyhow::Result<InstanceLock> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening tray lock file at {}", path.display()))?;
    file.try_lock_exclusive()
        .map_err(|e| anyhow::anyhow!("Tray is already running ({e})"))?;
    Ok(InstanceLock {
        _file: file,
        _path: path,
    })
}

fn lock_path() -> PathBuf {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("kino-tray.lock");
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "anon".to_string());
    std::env::temp_dir().join(format!("kino-tray-{user}.lock"))
}
