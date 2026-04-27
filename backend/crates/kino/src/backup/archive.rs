//! `tar.gz` archive writer + reader for backup snapshots.
//!
//! The archive ships the canonical kino state (DB + WAL + SHM)
//! plus a small `manifest.json` carrying version + checksum metadata
//! so a future restore can decide whether the archive is compatible
//! before swapping anything in.
//!
//! Media files on disk are deliberately **not** included — they're
//! managed outside kino and would balloon the archive.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use chrono::Utc;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use super::model::BackupKind;
use crate::events::AppEvent;

/// Pinned schema version stamped into the manifest. Bumped any time
/// the migration ladder evolves; restore refuses archives whose
/// `schema_version` is *higher* than what the running binary knows
/// about (no forward compat).
pub const CURRENT_SCHEMA_VERSION: i64 = 1;

/// Archive layout — written and read by both ends.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub kino_version: String,
    pub schema_version: i64,
    pub kind: String,
    pub created_at: String,
    pub size_bytes: u64,
    pub checksum_sha256: String,
}

// We snapshot the DB via `VACUUM INTO` rather than copying
// `kino.db` + `-wal` + `-shm` raw — VACUUM is transactional and
// produces a defragmented file. WAL / SHM are runtime artefacts of
// the live DB and aren't shipped in the archive.

/// Resolved + ensured backup target directory. Falls back to
/// `{data_path}/backups/` when config carries an empty path
/// (default first-boot state). Creates the directory if missing.
pub async fn ensure_location(pool: &SqlitePool, data_path: &Path) -> anyhow::Result<PathBuf> {
    let configured: Option<String> =
        sqlx::query_scalar("SELECT backup_location_path FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await?
            .filter(|s: &String| !s.trim().is_empty());
    let path = configured.map_or_else(|| data_path.join("backups"), std::path::PathBuf::from);
    tokio::fs::create_dir_all(&path)
        .await
        .with_context(|| format!("create backup dir {}", path.display()))?;
    Ok(path)
}

/// Create a new archive. Steps:
///
/// 1. `VACUUM INTO` the live DB to a tmp file (transactional
///    snapshot — safe under concurrent writes).
/// 2. Write the tmp DB + companions into a `.tar.gz`.
/// 3. Compute SHA-256 over the archive.
/// 4. Insert a `backup` row + emit `BackupCreated`.
/// 5. Apply retention to scheduled-kind rows.
///
/// Returns the new row's `id` so callers can render
/// "Backup created (4.7 MB)" with the size from the row.
pub async fn create(
    pool: &SqlitePool,
    data_path: &Path,
    kind: BackupKind,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
) -> anyhow::Result<i64> {
    let location = ensure_location(pool, data_path).await?;
    let now = Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let kino_version = env!("CARGO_PKG_VERSION");
    let filename = format!(
        "kino-backup-{prefix}{timestamp}-v{kino_version}.tar.gz",
        prefix = match kind {
            BackupKind::PreRestore => "pre-restore-",
            _ => "",
        },
    );
    let archive_path = location.join(&filename);

    // VACUUM INTO into a tempfile — sqlx exposes the raw connection
    // for this since `VACUUM` isn't a regular query. The tempfile
    // lives in the same directory as the final archive so we know
    // free-space + permissions are right.
    let tmp_db = tempfile::Builder::new()
        .prefix("kino-backup-")
        .suffix(".sqlite")
        .tempfile_in(&location)
        .with_context(|| format!("open tempfile in {}", location.display()))?;
    let tmp_db_path = tmp_db.path().to_path_buf();
    drop(tmp_db); // Just want the unique path; sqlite creates the file itself.

    let escaped = tmp_db_path.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(pool)
        .await
        .context("VACUUM INTO snapshot")?;

    // Build the manifest first (size_bytes + checksum filled after
    // the archive's been written; we re-serialise then).
    let mut manifest = Manifest {
        kino_version: kino_version.to_owned(),
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: kind.as_str().to_owned(),
        created_at: now.to_rfc3339(),
        size_bytes: 0,
        checksum_sha256: String::new(),
    };

    // Stream the DB snapshot + companions + manifest into the .tar.gz.
    // Done synchronously off the runtime — backups are infrequent
    // and the tokio thread shouldn't block anyway.
    let archive_path_clone = archive_path.clone();
    let tmp_db_path_clone = tmp_db_path.clone();
    let manifest_for_write = serde_json::to_vec_pretty(&manifest)?;
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = File::create(&archive_path_clone)
            .with_context(|| format!("create archive {}", archive_path_clone.display()))?;
        let writer = BufWriter::new(file);
        let gz = GzEncoder::new(writer, Compression::default());
        let mut tar = tar::Builder::new(gz);

        // The vacuumed snapshot lands at `kino.db` inside the
        // archive — restore expects this path.
        let mut snap = File::open(&tmp_db_path_clone)
            .with_context(|| format!("open snapshot {}", tmp_db_path_clone.display()))?;
        tar.append_file("kino.db", &mut snap)?;

        // Manifest sidecar.
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_for_write.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, "manifest.json", manifest_for_write.as_slice())?;

        tar.into_inner()?.finish()?.flush()?;
        Ok(())
    })
    .await
    .context("backup writer task panicked")??;

    // Drop the tempfile now we're done with it.
    let _ = tokio::fs::remove_file(&tmp_db_path).await;

    // Compute size + checksum of the finished archive.
    let metadata = tokio::fs::metadata(&archive_path).await?;
    let size_bytes = metadata.len();
    let checksum = sha256_file(&archive_path).await?;
    manifest.size_bytes = size_bytes;
    manifest.checksum_sha256 = checksum.clone();

    // Persist the row.
    let now_iso = now.to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO backup
            (kind, filename, size_bytes, kino_version, schema_version, checksum_sha256, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(kind.as_str())
    .bind(&filename)
    .bind(i64::try_from(size_bytes).unwrap_or(i64::MAX))
    .bind(kino_version)
    .bind(CURRENT_SCHEMA_VERSION)
    .bind(&checksum)
    .bind(&now_iso)
    .fetch_one(pool)
    .await?;

    let _ = event_tx.send(AppEvent::BackupCreated {
        backup_id: id,
        kind: kind.as_str().to_owned(),
        size_bytes: i64::try_from(size_bytes).unwrap_or(i64::MAX),
    });

    // Apply retention to scheduled-kind rows. Manual + pre-restore
    // backups are exempt — user explicitly created the manual ones,
    // pre-restore are recovery points the user might still need.
    if matches!(kind, BackupKind::Scheduled) {
        prune_scheduled(pool, data_path, location.as_path()).await?;
    }

    tracing::info!(
        backup_id = id,
        kind = kind.as_str(),
        size_bytes,
        path = %archive_path.display(),
        "backup created",
    );

    Ok(id)
}

/// Stage a restore: validate the archive, take an automatic
/// pre-restore snapshot, then swap the live DB files in place.
/// Returns `Ok(())` on a successful swap; the caller surfaces the
/// "please restart kino" prompt to the user.
///
/// The implementation is intentionally simple in Phase 1 — we
/// don't try to gracefully tear down + re-init `AppState` mid-
/// process. Re-opening the DB pool, restarting the scheduler, and
/// re-attaching librqbit + the cast workers in-place would be a
/// big intrusive change for a feature operators will hit once a
/// year. Phase 2 can revisit if there's demand.
pub async fn restore_backup_id(
    pool: &SqlitePool,
    data_path: &Path,
    backup_id: i64,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
) -> anyhow::Result<()> {
    let row: Option<(String, String, i64)> =
        sqlx::query_as("SELECT filename, checksum_sha256, schema_version FROM backup WHERE id = ?")
            .bind(backup_id)
            .fetch_optional(pool)
            .await?;
    let (filename, checksum, schema_version) =
        row.ok_or_else(|| anyhow!("backup {backup_id} not found"))?;

    let location = ensure_location(pool, data_path).await?;
    let archive_path = location.join(&filename);
    restore_path(
        pool,
        data_path,
        &archive_path,
        Some(&checksum),
        schema_version,
        event_tx,
    )
    .await
}

/// Restore from an arbitrary on-disk archive (used by the upload
/// path — a backup made on another machine, dropped via the
/// "Restore from file" UI). When `expected_checksum` is provided we
/// verify it matches before doing anything destructive.
pub async fn restore_path(
    pool: &SqlitePool,
    data_path: &Path,
    archive_path: &Path,
    expected_checksum: Option<&str>,
    expected_schema_version: i64,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
) -> anyhow::Result<()> {
    if expected_schema_version > CURRENT_SCHEMA_VERSION {
        return Err(anyhow!(
            "backup uses schema {expected_schema_version}, kino only knows up to {CURRENT_SCHEMA_VERSION} — upgrade kino and try again",
        ));
    }
    if let Some(expected) = expected_checksum {
        let actual = sha256_file(archive_path).await?;
        if actual != expected {
            return Err(anyhow!(
                "archive checksum mismatch (expected {expected}, got {actual})",
            ));
        }
    }

    // Pre-restore safety snapshot. No matter what happens after
    // this point, the user can always undo via the row this
    // creates.
    create(pool, data_path, BackupKind::PreRestore, event_tx).await?;

    // Extract the snapshot to a tempfile next to the live DB so
    // the rename is atomic on the same filesystem.
    let live_db = data_path.join("kino.db");
    let staged = data_path.join("kino.db.restore-staged");
    let archive_path_owned = archive_path.to_path_buf();
    let staged_for_task = staged.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = File::open(&archive_path_owned)
            .with_context(|| format!("open archive {}", archive_path_owned.display()))?;
        let mut archive = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
        let mut found_db = false;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if path.to_string_lossy() == "kino.db" {
                let mut out = File::create(&staged_for_task)
                    .with_context(|| format!("write staged {}", staged_for_task.display()))?;
                std::io::copy(&mut entry, &mut out)?;
                out.flush()?;
                found_db = true;
            }
        }
        if !found_db {
            return Err(anyhow!("archive contains no kino.db"));
        }
        Ok(())
    })
    .await
    .context("restore extractor task panicked")??;

    // Swap. The live DB → .pre-restore alongside (kept until next
    // boot for one-off recovery if the restored DB itself is
    // corrupt), then staged → live.
    let backup_aside = data_path.join("kino.db.pre-restore");
    if live_db.exists() {
        if backup_aside.exists() {
            tokio::fs::remove_file(&backup_aside).await.ok();
        }
        tokio::fs::rename(&live_db, &backup_aside).await?;
    }
    // Companions: WAL + SHM stale after the swap. Remove cleanly.
    for ext in ["-wal", "-shm"] {
        let companion = data_path.join(format!("kino.db{ext}"));
        if companion.exists() {
            tokio::fs::remove_file(&companion).await.ok();
        }
    }
    tokio::fs::rename(&staged, &live_db).await?;

    let _ = event_tx.send(AppEvent::BackupRestored {
        backup_id: 0,
        message: format!("Restored from {}", archive_path.display()),
    });

    tracing::warn!(
        path = %archive_path.display(),
        "backup restored — kino must be restarted to load the new database"
    );
    Ok(())
}

/// SHA-256 over a file's bytes. Streamed so we don't allocate the
/// whole archive in memory.
pub async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file =
            File::open(&path).with_context(|| format!("open {} for hashing", path.display()))?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        // Heap-allocated so the 64 KB buffer doesn't bloat this
        // closure's stack frame; clippy's large-stack-arrays lint
        // would otherwise fire.
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok::<String, anyhow::Error>(format!("{:x}", hasher.finalize()))
    })
    .await
    .context("sha256 task panicked")?
}

/// Drop scheduled-kind backups beyond `backup_retention_count`,
/// oldest first. Manual + pre-restore rows untouched.
pub async fn prune_scheduled(
    pool: &SqlitePool,
    _data_path: &Path,
    location: &Path,
) -> anyhow::Result<()> {
    let keep: i64 = sqlx::query_scalar("SELECT backup_retention_count FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await?
        .unwrap_or(7);
    if keep <= 0 {
        return Ok(());
    }
    let stale: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, filename FROM backup
         WHERE kind = 'scheduled'
         ORDER BY created_at DESC
         LIMIT -1 OFFSET ?",
    )
    .bind(keep)
    .fetch_all(pool)
    .await?;
    for (id, filename) in stale {
        let path = location.join(&filename);
        if path.exists() {
            let _ = tokio::fs::remove_file(&path).await;
        }
        let _ = sqlx::query("DELETE FROM backup WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await;
        tracing::info!(backup_id = id, "pruned scheduled backup beyond retention");
    }
    Ok(())
}

/// Delete a single backup (file + row). Manual + pre-restore + the
/// row that's currently the most recent scheduled snapshot can all
/// be deleted; `is_protected` semantics are a UI concern.
pub async fn delete_one(
    pool: &SqlitePool,
    data_path: &Path,
    backup_id: i64,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
) -> anyhow::Result<bool> {
    let row: Option<String> = sqlx::query_scalar("SELECT filename FROM backup WHERE id = ?")
        .bind(backup_id)
        .fetch_optional(pool)
        .await?;
    let Some(filename) = row else {
        return Ok(false);
    };
    let location = ensure_location(pool, data_path).await?;
    let path = location.join(&filename);
    let _ = tokio::fs::remove_file(&path).await;
    sqlx::query("DELETE FROM backup WHERE id = ?")
        .bind(backup_id)
        .execute(pool)
        .await?;
    let _ = event_tx.send(AppEvent::BackupDeleted { backup_id });
    Ok(true)
}
