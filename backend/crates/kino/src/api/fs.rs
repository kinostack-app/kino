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
    /// When the requested path didn't exist, the original input the
    /// caller asked for. The UI surfaces this as "couldn't open X,
    /// showing nearest existing parent Y instead." `None` on success.
    pub fallback_from: Option<String>,
}

/// `GET /api/v1/fs/browse?path=…` — list directory contents. Hidden
/// entries (names starting with `.`) are excluded. When the
/// requested path doesn't exist, walks up to the nearest existing
/// ancestor and lists that — `result.fallback_from` carries the
/// original input so the UI can show "couldn't open X, showing Y".
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
    let mut probe = std::path::PathBuf::from(&raw);
    let mut fallback_from: Option<String> = None;
    let path = loop {
        match tokio::fs::canonicalize(&probe).await {
            Ok(p) if tokio::fs::metadata(&p).await.is_ok_and(|m| m.is_dir()) => break p,
            _ => {
                if fallback_from.is_none() {
                    fallback_from = Some(raw.clone());
                }
                let Some(parent) = probe.parent() else {
                    return Err(AppError::NotFound(format!(
                        "no readable directory near {raw}"
                    )));
                };
                if parent == probe {
                    return Err(AppError::NotFound(format!(
                        "no readable directory near {raw}"
                    )));
                }
                probe = parent.to_path_buf();
            }
        }
    };

    let mut read = tokio::fs::read_dir(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            AppError::Forbidden(format!(
                "kino doesn't have permission to read {}",
                path.display()
            ))
        } else {
            AppError::BadRequest(format!("cannot list {}: {e}", path.display()))
        }
    })?;

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
        fallback_from,
    }))
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
pub struct MkdirRequest {
    /// Absolute path of the directory to create. Parents are created
    /// as needed (`mkdir -p` semantics) so the user doesn't have to
    /// click through and create each segment.
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MkdirResult {
    /// Canonical path of the directory after creation.
    pub canonical: String,
}

/// `POST /api/v1/fs/mkdir` — create a directory (recursive). Used
/// by the path-picker's "+ New folder" affordance so users on a
/// fresh install don't have to drop into a terminal to set up
/// `/var/lib/kino/library` etc.
#[utoipa::path(
    post,
    path = "/api/v1/fs/mkdir",
    request_body = MkdirRequest,
    responses(
        (status = 200, body = MkdirResult),
        (status = 400, description = "permission denied / invalid path"),
    ),
    tag = "filesystem",
    security(("api_key" = []))
)]
pub async fn mkdir(Json(req): Json<MkdirRequest>) -> AppResult<Json<MkdirResult>> {
    let path = PathBuf::from(&req.path);
    tokio::fs::create_dir_all(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            AppError::Forbidden(format!(
                "kino can't write to {} — try /var/lib/kino, or grant the kino service user access (chgrp / setfacl)",
                req.path
            ))
        } else {
            AppError::BadRequest(format!("mkdir {}: {e}", req.path))
        }
    })?;
    let canonical = tokio::fs::canonicalize(&path)
        .await
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(req.path);
    Ok(Json(MkdirResult { canonical }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlaceEntry {
    /// Human label for the sidebar.
    pub label: String,
    /// Absolute path to navigate to.
    pub path: String,
    /// Kind hint for icon selection — `"home"`, `"root"`, `"drive"`,
    /// `"network"`, `"system"`. The frontend maps these to icons.
    pub kind: String,
    /// Optional sublabel shown under the label (e.g. "230 GB free"
    /// for drives).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlacesResult {
    pub places: Vec<PlaceEntry>,
}

/// `GET /api/v1/fs/places` — produce the path-picker sidebar entries
/// the way GNOME Files / Finder / Explorer do: only paths that
/// exist, that the service can read, and (for top-level system
/// dirs) only if they actually contain something. Prevents dead
/// links and silent "click does nothing" UX from /Volumes on
/// Linux, an empty /mnt, or other-user homes the service can't
/// see.
#[utoipa::path(
    get,
    path = "/api/v1/fs/places",
    responses((status = 200, body = PlacesResult)),
    tag = "filesystem",
    security(("api_key" = []))
)]
pub async fn places() -> AppResult<Json<PlacesResult>> {
    let mut out: Vec<PlaceEntry> = Vec::new();

    out.push(PlaceEntry {
        label: "/ (filesystem root)".into(),
        path: "/".into(),
        kind: "root".into(),
        sublabel: None,
    });

    if let Ok(read) = std::fs::read_dir("/home") {
        for entry in read.flatten() {
            let path = entry.path();
            if !is_readable_dir(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            out.push(PlaceEntry {
                label: format!("Home ({name})"),
                path: path.to_string_lossy().into_owned(),
                kind: "home".into(),
                sublabel: None,
            });
        }
    }
    if let Ok(read) = std::fs::read_dir("/Users") {
        for entry in read.flatten() {
            let path = entry.path();
            if !is_readable_dir(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') || name == "Shared" {
                continue;
            }
            out.push(PlaceEntry {
                label: format!("Home ({name})"),
                path: path.to_string_lossy().into_owned(),
                kind: "home".into(),
                sublabel: None,
            });
        }
    }

    for m in enumerate_mounts() {
        let kind = match m.fs_type.as_str() {
            "nfs" | "nfs4" | "cifs" | "smbfs" | "smb3" => "network",
            _ => "drive",
        };
        let sublabel = m
            .free_bytes
            .map(|b| format!("{} free · {}", format_bytes(b), m.fs_type));
        out.push(PlaceEntry {
            label: m.label,
            path: m.path,
            kind: kind.into(),
            sublabel,
        });
    }

    for sys in ["/mnt", "/media", "/srv", "/Volumes"] {
        if let Some(entry) = useful_system_place(sys) {
            out.push(entry);
        }
    }

    Ok(Json(PlacesResult { places: out }))
}

fn is_readable_dir(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_dir() {
        return false;
    }
    std::fs::read_dir(path).is_ok()
}

fn useful_system_place(path: &str) -> Option<PlaceEntry> {
    let p = std::path::Path::new(path);
    if !is_readable_dir(p) {
        return None;
    }
    let mut entries = std::fs::read_dir(p).ok()?;
    if entries.next().is_none() {
        return None;
    }
    Some(PlaceEntry {
        label: path.into(),
        path: path.into(),
        kind: "system".into(),
        sublabel: None,
    })
}

fn format_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 10.0 {
        format!("{:.0} {}", v, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MountEntry {
    /// Display label — last segment of the mount point on Linux,
    /// the volume name on macOS. Falls back to the mount point
    /// itself when the label can't be derived cheaply.
    pub label: String,
    /// Absolute mount point (filesystem path the user can navigate
    /// to from the path picker).
    pub path: String,
    /// Filesystem type as reported by the kernel (`ext4`, `nfs4`,
    /// `cifs`, `apfs`, `ntfs3`, etc.). Surfaced as a small badge
    /// so users can tell a network share from a local drive.
    pub fs_type: String,
    /// Free bytes on the mount, when readable.
    pub free_bytes: Option<u64>,
    /// Total bytes on the mount, when readable.
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MountsResult {
    pub mounts: Vec<MountEntry>,
}

/// `GET /api/v1/fs/mounts` — enumerate user-relevant mount points
/// (local disks, USB drives, NFS / SMB shares). Excludes kernel /
/// container plumbing (procfs, sysfs, tmpfs, cgroup, overlay) so
/// the path-picker sidebar only surfaces things the user might
/// actually want to navigate to.
///
/// Linux: parses `/proc/mounts` directly (stable interface, no
/// extra deps). macOS / Windows: returns an empty list for now —
/// the "Common locations" fallback in the picker UI covers
/// `/Volumes` etc. until per-OS mount enumeration lands.
#[utoipa::path(
    get,
    path = "/api/v1/fs/mounts",
    responses((status = 200, body = MountsResult)),
    tag = "filesystem",
    security(("api_key" = []))
)]
pub async fn mounts() -> AppResult<Json<MountsResult>> {
    Ok(Json(MountsResult {
        mounts: enumerate_mounts(),
    }))
}

#[cfg(target_os = "linux")]
fn enumerate_mounts() -> Vec<MountEntry> {
    // /proc/mounts is a stable kernel interface; format is
    // `<spec> <mountpoint> <fstype> <opts> <dump> <pass>` per line,
    // space-separated, with octal-escaped spaces in the path. We
    // only need spec / mountpoint / fstype and don't bother with
    // the escape decoding because the user-relevant filesystems
    // we surface (ext4, btrfs, nfs, cifs) all have ASCII paths in
    // every install we've seen. If a user ever has a path with a
    // literal space they'll see a partial label; the navigation
    // still works because the value goes through the canonical
    // browse endpoint.
    let Ok(text) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _spec = parts.next();
        let Some(mountpoint) = parts.next() else {
            continue;
        };
        let Some(fs_type) = parts.next() else {
            continue;
        };
        if !user_relevant_fs(fs_type) {
            continue;
        }
        if !user_relevant_mountpoint(mountpoint) {
            continue;
        }
        // Drop file-level bind mounts (e.g. docker injects /etc/resolv.conf,
        // /etc/hostname, /etc/hosts). The user-relevant-mountpoint
        // prefix list catches most of these but devcontainer / k8s
        // setups can mount files anywhere; gate on `is_dir` as a
        // belt-and-braces check.
        let path = std::path::PathBuf::from(mountpoint);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        // Dedupe — bind-mounts and overlay layers can list the same
        // path under multiple fstypes. Keep the first.
        if !seen.insert(mountpoint.to_string()) {
            continue;
        }
        let free = fs4::available_space(&path).ok();
        let total = fs4::total_space(&path).ok();
        let label = derive_label(mountpoint);
        out.push(MountEntry {
            label,
            path: mountpoint.to_string(),
            fs_type: fs_type.to_string(),
            free_bytes: free,
            total_bytes: total,
        });
    }
    out
}

/// Mount-point allowlist. Conservative on purpose — devcontainers
/// and Kubernetes pods scatter bind-mounts of cargo caches /
/// node_modules / config files all over the FS, and surfacing them
/// in the path-picker sidebar is noise users don't want. Stick to
/// the prefixes a real user would navigate to: filesystem root,
/// per-user homes (`/home/<user>`, `/root`), the conventional
/// Linux mount roots (`/mnt`, `/media`, `/srv`), Pop!_OS / Ubuntu
/// 22+ auto-mount path (`/run/media/<user>`), and macOS-style
/// `/Volumes` for any cross-distro tooling that mounts there.
#[cfg(target_os = "linux")]
fn user_relevant_mountpoint(path: &str) -> bool {
    matches!(
        path,
        "/" | "/home" | "/root" | "/mnt" | "/media" | "/srv" | "/Volumes"
    ) || path.starts_with("/home/")
        || path.starts_with("/mnt/")
        || path.starts_with("/media/")
        || path.starts_with("/run/media/")
        || path.starts_with("/srv/")
        || path.starts_with("/Volumes/")
}

#[cfg(not(target_os = "linux"))]
fn enumerate_mounts() -> Vec<MountEntry> {
    // macOS: TODO — shell out to `mount` or use getmntinfo() via libc.
    // Windows: TODO — GetLogicalDriveStringsW + GetVolumeInformationW.
    Vec::new()
}

/// Filesystem types the path-picker surfaces. Allowlist rather
/// than denylist so a new exotic kernel pseudo-fs doesn't sneak
/// into the user's sidebar. Covers ext / xfs / btrfs / f2fs (Linux),
/// vfat / exfat / ntfs / ntfs3 (USB drives), nfs / cifs / smbfs / smb3
/// (network shares), fuseblk (NTFS-3G + most FUSE-mounted disks),
/// apfs / hfs (macOS local disks if we ever surface them).
#[cfg(target_os = "linux")]
fn user_relevant_fs(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "ext2"
            | "ext3"
            | "ext4"
            | "xfs"
            | "btrfs"
            | "f2fs"
            | "zfs"
            | "vfat"
            | "exfat"
            | "ntfs"
            | "ntfs3"
            | "nfs"
            | "nfs4"
            | "cifs"
            | "smbfs"
            | "smb3"
            | "fuseblk"
            | "apfs"
            | "hfs"
            | "hfsplus"
    )
}

#[cfg(target_os = "linux")]
fn derive_label(mountpoint: &str) -> String {
    // The mount point's last path segment is usually the most
    // recognisable name — `/media/robertsmith/USB-Stick` →
    // `USB-Stick`, `/mnt/nas-photos` → `nas-photos`. Falls back
    // to the full mount point when it's too short or empty
    // (e.g. `/`).
    let last = std::path::Path::new(mountpoint)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if last.is_empty() {
        mountpoint.to_string()
    } else {
        last.to_string()
    }
}
