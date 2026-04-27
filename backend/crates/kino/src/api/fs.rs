//! Filesystem inspection endpoints — used by the Library settings page
//! to verify configured paths (and by the path browser to navigate the
//! server's directory tree).
//!
//! Security: any holder of the API key can query arbitrary paths on
//! the host. That's acceptable for a home-server tool but something to
//! note if we ever expose kino to untrusted callers.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::Query;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::{AppError, AppResult};

#[derive(Debug, Deserialize, IntoParams)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PathTest {
    /// Canonical (realpath) form of the requested path, if it exists.
    pub canonical: Option<String>,
    pub exists: bool,
    pub is_dir: bool,
    pub writable: bool,
    /// Free space on the mount containing this path, in bytes.
    pub free_bytes: Option<u64>,
    /// Stable-per-mount identifier. Two paths with the same `device_id`
    /// can be hardlinked between; different ids cannot.
    pub device_id: Option<u64>,
    /// Human-readable error when the test couldn't run at all (e.g.
    /// permission denied on the parent directory).
    pub error: Option<String>,
}

/// `POST /api/v1/fs/test` — stat a path and report whether it's a
/// usable target for library / download storage.
#[utoipa::path(
    get,
    path = "/api/v1/fs/test",
    params(PathQuery),
    responses((status = 200, body = PathTest)),
    tag = "filesystem",
    security(("api_key" = []))
)]
pub async fn test_path(Query(q): Query<PathQuery>) -> AppResult<Json<PathTest>> {
    let path = PathBuf::from(&q.path);
    let meta = tokio::fs::metadata(&path).await;

    let (exists, is_dir) = match &meta {
        Ok(m) => (true, m.is_dir()),
        Err(_) => (false, false),
    };

    let canonical = tokio::fs::canonicalize(&path)
        .await
        .ok()
        .map(|p| p.to_string_lossy().into_owned());

    // Writability: try creating a short-lived file in the directory.
    // Ignore the "file already exists" branch — we use a random suffix.
    let writable = if is_dir {
        let probe = path.join(format!(".kino-writetest-{}", uuid::Uuid::new_v4()));
        match tokio::fs::write(&probe, b"").await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&probe).await;
                true
            }
            Err(_) => false,
        }
    } else {
        false
    };

    let device_id = device_id(&path);
    let free_bytes = free_bytes(&path);

    Ok(Json(PathTest {
        canonical,
        exists,
        is_dir,
        writable,
        free_bytes,
        device_id,
        error: if !exists {
            Some("path does not exist".into())
        } else if !is_dir {
            Some("path is not a directory".into())
        } else if !writable {
            Some("path is not writable".into())
        } else {
            None
        },
    }))
}

/// Device id (`st_dev` on Unix). Two paths with the same value share a
/// filesystem and can be hardlinked. Returns None on platforms / errors
/// where we can't read it.
fn device_id(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Free-space lookup. `fs4` wraps `statvfs` on Unix and `GetDiskFreeSpaceEx`
/// on Windows, so this works across Linux / macOS / Windows without us
/// juggling `unsafe`.
fn free_bytes(path: &Path) -> Option<u64> {
    use fs4::available_space;
    available_space(path).ok()
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct BrowseQuery {
    /// Directory to list. Defaults to `/` when omitted.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BrowseEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BrowseResult {
    /// Absolute canonical path this listing is rooted at.
    pub path: String,
    /// Parent directory's canonical path, or None if at filesystem root.
    pub parent: Option<String>,
    pub entries: Vec<BrowseEntry>,
}

/// `GET /api/v1/fs/browse?path=…` — list directory contents. Hidden
/// entries (names starting with `.`) are excluded.
#[utoipa::path(
    get,
    path = "/api/v1/fs/browse",
    params(BrowseQuery),
    responses((status = 200, body = BrowseResult), (status = 404)),
    tag = "filesystem",
    security(("api_key" = []))
)]
pub async fn browse(Query(q): Query<BrowseQuery>) -> AppResult<Json<BrowseResult>> {
    let raw = q.path.unwrap_or_else(|| "/".to_string());
    let path = tokio::fs::canonicalize(&raw)
        .await
        .map_err(|e| AppError::NotFound(format!("{raw}: {e}")))?;

    let mut read = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| AppError::BadRequest(format!("cannot list {}: {e}", path.display())))?;

    let mut entries = Vec::new();
    while let Some(ent) = read
        .next_entry()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read_dir: {e}")))?
    {
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = ent.metadata().await else {
            continue;
        };
        entries.push(BrowseEntry {
            name,
            path: ent.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
        });
    }

    // Directories first, then files, then alphabetical within each.
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(Json(BrowseResult {
        path: path.to_string_lossy().into_owned(),
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
        entries,
    }))
}
