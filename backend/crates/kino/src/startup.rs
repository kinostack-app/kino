//! Startup reconciliation — ensures database, filesystem, and
//! torrent client are consistent after an unclean shutdown.
//!
//! Runs once on boot, before the API server and scheduler start.
//! Conservative: prefers marking failed + retrying over complex
//! recovery. Idempotent: running twice produces the same result.
//! Logged: every action is an `INFO` tracing event so day-to-day
//! diagnostics from `just logs` shows exactly what happened.
//!
//! Phase order is load-bearing — each phase assumes the previous
//! has committed. Phase 3 (VPN + librqbit restore) happens
//! externally in `main.rs`; we pick up at Phase 4.

use sqlx::SqlitePool;

use crate::download::{DownloadPhase, TorrentSession};

/// Filesystem-verification threshold for Phase 5. Libraries above
/// this row count skip the per-file `exists()` sweep on boot —
/// that many `stat()` syscalls against a networked mount can add
/// seconds to startup, and missing files surface naturally on
/// first playback attempt anyway (the playback path already
/// cleans up Media rows whose file is gone). Small libraries
/// (the common case) stay eagerly verified so stale rows from a
/// mid-disk-failure state get cleaned up pre-UI.
const EAGER_VERIFY_THRESHOLD: i64 = 1000;

/// Result of the startup reconciliation process. Every counter is
/// reported in the summary log line so operators can spot "large
/// numbers of fixes" as a signal of something going wrong upstream.
#[derive(Debug, Default)]
pub struct ReconcileResult {
    pub orphans_cleaned: u64,
    pub ghost_torrents_removed: u64,
    pub downloads_reconciled: u64,
    pub entities_fixed: u64,
    pub files_verified: u64,
    /// True when Phase 5 skipped the eager disk check because the
    /// library exceeds `EAGER_VERIFY_THRESHOLD`. Missing files will
    /// be cleaned up lazily on first playback attempt.
    pub files_verified_lazily: bool,
}

/// Run all reconciliation phases in order. The torrent-session
/// argument is optional — early boot may have skipped starting it
/// (VPN required but failed, or user disabled downloads). In that
/// case we skip phases that need to cross-reference librqbit, and
/// continue with the DB-only phases so the UI is still coherent.
#[tracing::instrument(skip(pool, media_library_path, torrents))]
pub async fn reconcile(
    pool: &SqlitePool,
    media_library_path: &str,
    torrents: Option<&dyn TorrentSession>,
) -> anyhow::Result<ReconcileResult> {
    let mut result = ReconcileResult::default();

    // Phase 1: DB integrity — WAL recovery is automatic on
    // connection; migrations already ran before this function is
    // called. Nothing to do here but log the checkpoint.
    tracing::info!("reconcile: phase 1 — database integrity verified");

    // Phase 2: Orphan cleanup (DB-only, no external deps).
    result.orphans_cleaned = cleanup_orphans(pool).await?;
    tracing::info!(
        orphans = result.orphans_cleaned,
        "reconcile: phase 2 — orphan cleanup",
    );

    // Phase 2b: Reset trickplay rows stuck at claim states (2 or
    // 3). A killed ffmpeg child from a previous process would
    // otherwise leave the row claimed forever, blocking every
    // future sweep from retrying.
    let unstuck = crate::playback::trickplay_gen::reset_stale_in_progress(pool).await?;
    if unstuck > 0 {
        tracing::info!(rows = unstuck, "reconcile: reset stale trickplay claims");
    }

    // Phase 3: VPN + librqbit startup handled externally (main.rs)
    // — runtime-specific setup. We just note the checkpoint.
    tracing::info!(
        torrents_attached = torrents.is_some(),
        "reconcile: phase 3 — VPN/librqbit startup (external)",
    );

    // Phase 4: Download state reconciliation. Uses the session to
    // cross-check "does this hash still exist in librqbit?" for
    // every non-terminal DB row.
    result.downloads_reconciled = reconcile_downloads(pool, torrents).await?;
    tracing::info!(
        downloads = result.downloads_reconciled,
        "reconcile: phase 4a — download state reconciliation",
    );

    // Phase 4b: Unknown-torrent cleanup. librqbit's own persistence
    // restored torrents that have no matching DB row (e.g. because
    // we crashed mid-grab between `session.add_torrent` and writing
    // the `download` row). These ghost torrents would otherwise
    // consume peers, disk, and bandwidth forever without ever
    // surfacing in the UI.
    result.ghost_torrents_removed = cleanup_unknown_torrents(pool, torrents).await?;
    if result.ghost_torrents_removed > 0 {
        tracing::info!(
            ghosts = result.ghost_torrents_removed,
            "reconcile: phase 4b — ghost torrents removed",
        );
    }

    // Phase 5: Entity status reconciliation. Eager disk-existence
    // check for small libraries; lazy (skip) for large ones.
    let (entities, files, lazy) = reconcile_entities(pool, media_library_path).await?;
    result.entities_fixed = entities;
    result.files_verified = files;
    result.files_verified_lazily = lazy;
    tracing::info!(
        entities_fixed = entities,
        files_verified = files,
        lazy = lazy,
        "reconcile: phase 5 — entity status reconciliation",
    );

    Ok(result)
}

/// Phase 2: Delete orphaned rows with broken foreign-key
/// relationships. Logs the per-table counts at DEBUG for
/// diagnostics when the summary number is unexpectedly large.
async fn cleanup_orphans(pool: &SqlitePool) -> anyhow::Result<u64> {
    async fn purge(
        pool: &SqlitePool,
        label: &'static str,
        sql: &'static str,
    ) -> anyhow::Result<u64> {
        let r = sqlx::query(sql).execute(pool).await?;
        let n = r.rows_affected();
        if n > 0 {
            tracing::debug!(table = label, rows = n, "orphan rows deleted");
        }
        Ok(n)
    }

    let mut total = 0u64;
    total += purge(
        pool,
        "stream",
        "DELETE FROM stream WHERE media_id NOT IN (SELECT id FROM media)",
    )
    .await?;
    total += purge(
        pool,
        "media_episode.media",
        "DELETE FROM media_episode WHERE media_id NOT IN (SELECT id FROM media)",
    )
    .await?;
    total += purge(
        pool,
        "media_episode.episode",
        "DELETE FROM media_episode WHERE episode_id NOT IN (SELECT id FROM episode)",
    )
    .await?;
    total += purge(
        pool,
        "download_content.download",
        "DELETE FROM download_content WHERE download_id NOT IN (SELECT id FROM download)",
    )
    .await?;
    total += purge(
        pool,
        "download_content.movie",
        "DELETE FROM download_content WHERE movie_id IS NOT NULL AND movie_id NOT IN (SELECT id FROM movie)",
    )
    .await?;
    total += purge(
        pool,
        "download_content.episode",
        "DELETE FROM download_content WHERE episode_id IS NOT NULL AND episode_id NOT IN (SELECT id FROM episode)",
    )
    .await?;
    Ok(total)
}

/// Phase 4a: Reconcile each non-terminal `download` row against
/// what librqbit is actually doing.
///
/// | DB state      | Torrent in librqbit? | Action                                          |
/// |---------------|----------------------|-------------------------------------------------|
/// | `grabbing`    | (any)                | mark `failed` — was mid-add when we crashed     |
/// | `importing`   | (any)                | mark `failed` — mid-import, search will retry   |
/// | `stalled`     | yes                  | flip to `downloading`; stall sweep re-evaluates |
/// | `stalled`     | no                   | mark `failed` — torrent gone, client can re-add |
/// | `seeding`     | yes                  | leave alone; seed-limit sweep owns transitions  |
/// | `seeding`     | no                   | mark `imported`; clean source files             |
/// | `imported`    | any                  | delete source files still on disk → `cleaned_up`|
///
/// Every action logs at INFO with the `download_id`, the
/// transition, and any path that was touched — the log line is
/// the primary diagnostic surface.
#[allow(clippy::too_many_lines)] // three transition branches each need their own logging + SQL — splitting obscures the per-state narrative
async fn reconcile_downloads(
    pool: &SqlitePool,
    torrents: Option<&dyn TorrentSession>,
) -> anyhow::Result<u64> {
    let mut fixed = 0u64;

    // `grabbing` / `importing` were transient at crash time; no
    // meaningful recovery, mark failed so search can retry.
    fixed += mark_interrupted(pool, DownloadPhase::Grabbing, "interrupted during startup").await?;
    fixed += mark_interrupted(pool, DownloadPhase::Importing, "interrupted during import").await?;

    // `stalled` / `seeding` / `imported` need librqbit cross-check.
    // When the torrent session is unavailable (VPN failed to start)
    // we leave them alone — better to show stale state than
    // accidentally mark a live torrent dead based on its absence
    // from a session that hasn't actually booted.
    let Some(torrents) = torrents else {
        tracing::debug!(
            "reconcile: librqbit session unavailable — skipping stalled/seeding/imported cross-check",
        );
        return Ok(fixed);
    };

    let live_hashes: std::collections::HashSet<String> = torrents
        .list_torrent_hashes()
        .into_iter()
        .map(|h| h.to_lowercase())
        .collect();
    tracing::debug!(
        live_count = live_hashes.len(),
        "reconcile: librqbit reports {} managed torrents",
        live_hashes.len(),
    );

    // `stalled` — keep state in sync with what librqbit is doing.
    // Torrent exists → reset to `downloading` so stall detection
    // gets a fresh evaluation. Torrent absent → mark `failed` so
    // search ladder picks up the slack.
    let stalled_rows: Vec<(i64, Option<String>, String)> =
        sqlx::query_as("SELECT id, torrent_hash, title FROM download WHERE state = ?")
            .bind(DownloadPhase::Stalled)
            .fetch_all(pool)
            .await?;
    for (id, hash, title) in stalled_rows {
        let present = hash
            .as_deref()
            .is_some_and(|h| live_hashes.contains(&h.to_lowercase()));
        if present {
            sqlx::query("UPDATE download SET state = ? WHERE id = ? AND state = ?")
                .bind(DownloadPhase::Downloading)
                .bind(id)
                .bind(DownloadPhase::Stalled)
                .execute(pool)
                .await?;
            tracing::info!(
                download_id = id,
                title = %title,
                "reconcile: stalled → downloading (torrent present in librqbit)",
            );
        } else {
            sqlx::query(
                "UPDATE download SET state = ?, error_message = 'torrent missing from librqbit on startup' WHERE id = ? AND state = ?",
            )
            .bind(DownloadPhase::Failed)
            .bind(id)
            .bind(DownloadPhase::Stalled)
            .execute(pool)
            .await?;
            tracing::warn!(
                download_id = id,
                title = %title,
                hash = ?hash,
                "reconcile: stalled → failed (torrent absent from librqbit)",
            );
        }
        fixed += 1;
    }

    // `seeding` — if the torrent's gone from librqbit (mid-seed
    // crash, or librqbit persistence drift), mark `imported` so
    // the download is out of the active set and the UI stops
    // reporting it as "still seeding". Also clean up its source
    // directory if the library already owns a copy (which it
    // should — `seeding` only exists post-import).
    let seeding_rows: Vec<(i64, Option<String>, Option<String>, String)> =
        sqlx::query_as("SELECT id, torrent_hash, output_path, title FROM download WHERE state = ?")
            .bind(DownloadPhase::Seeding)
            .fetch_all(pool)
            .await?;
    for (id, hash, output_path, title) in seeding_rows {
        let present = hash
            .as_deref()
            .is_some_and(|h| live_hashes.contains(&h.to_lowercase()));
        if present {
            continue;
        }
        sqlx::query("UPDATE download SET state = ? WHERE id = ? AND state = ?")
            .bind(DownloadPhase::Imported)
            .bind(id)
            .bind(DownloadPhase::Seeding)
            .execute(pool)
            .await?;
        tracing::warn!(
            download_id = id,
            title = %title,
            hash = ?hash,
            "reconcile: seeding → imported (torrent absent from librqbit)",
        );
        // Best-effort source cleanup. We deliberately don't block
        // the reconciliation summary on it — a permission error
        // or a path that already doesn't exist shouldn't fail
        // startup.
        clean_download_output(id, output_path.as_deref(), &title).await;
        fixed += 1;
    }

    // `imported` rows exist in the DB for seeding metadata + for
    // the seed-limit sweep to decide when to clean up. On startup
    // we opportunistically clean source files that are still on
    // disk (i.e. a previous run stopped seeding but didn't get to
    // the file cleanup), then move the row to `cleaned_up` which
    // is terminal.
    let imported_rows: Vec<(i64, Option<String>, String)> =
        sqlx::query_as("SELECT id, output_path, title FROM download WHERE state = ?")
            .bind(DownloadPhase::Imported)
            .fetch_all(pool)
            .await?;
    for (id, output_path, title) in imported_rows {
        let Some(path) = output_path.as_deref() else {
            continue;
        };
        if !std::path::Path::new(path).exists() {
            continue;
        }
        clean_download_output(id, Some(path), &title).await;
        sqlx::query(
            "UPDATE download SET state = ?, seed_target_reached_at = ? WHERE id = ? AND state = ?",
        )
        .bind(DownloadPhase::CleanedUp)
        .bind(crate::time::Timestamp::now().to_rfc3339())
        .bind(id)
        .bind(DownloadPhase::Imported)
        .execute(pool)
        .await?;
        tracing::info!(
            download_id = id,
            title = %title,
            path = %path,
            "reconcile: imported → cleaned_up (source files removed)",
        );
        fixed += 1;
    }

    Ok(fixed)
}

/// Phase 4b: Remove torrents from librqbit that have no matching
/// DB row. These are "ghosts" — usually a mid-grab crash where
/// `session.add_torrent` succeeded but we never got to persist
/// the `download` row. Left alone they burn peers, disk, and
/// bandwidth indefinitely with nothing in the UI to surface them.
async fn cleanup_unknown_torrents(
    pool: &SqlitePool,
    torrents: Option<&dyn TorrentSession>,
) -> anyhow::Result<u64> {
    let Some(torrents) = torrents else {
        return Ok(0);
    };

    let known: std::collections::HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT torrent_hash FROM download WHERE torrent_hash IS NOT NULL",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|h| h.to_lowercase())
    .collect();

    let tracker = crate::cleanup::CleanupTracker::new(pool.clone());
    let mut removed = 0u64;
    for hash in torrents.list_torrent_hashes() {
        if known.contains(&hash.to_lowercase()) {
            continue;
        }
        // delete_files=true: a ghost torrent has no library copy
        // (import never ran), so the on-disk bytes under
        // download_path are the only trace of it. Removing the
        // torrent without the files leaves orphan bytes nobody
        // will ever clean up.
        let outcome = tracker
            .try_remove(crate::cleanup::ResourceKind::Torrent, &hash, || async {
                torrents.remove(&hash, true).await
            })
            .await?;
        match outcome {
            crate::cleanup::RemovalOutcome::Removed => {
                tracing::warn!(
                    hash = %hash,
                    "reconcile: removed ghost torrent (no DB row)",
                );
                removed += 1;
            }
            other => {
                tracing::warn!(
                    hash = %hash,
                    outcome = ?other,
                    "reconcile: ghost-torrent removal queued for retry",
                );
            }
        }
    }
    Ok(removed)
}

/// Phase 5: Fix stale entity states.
///
/// Returns `(entities_fixed, files_verified, lazily_skipped)`.
/// Libraries with more than `EAGER_VERIFY_THRESHOLD` media rows
/// skip the per-file disk check and return `(_, 0, true)` — the
/// playback path already handles missing files on first access.
async fn reconcile_entities(
    pool: &SqlitePool,
    media_library_path: &str,
) -> anyhow::Result<(u64, u64, bool)> {
    let mut entities_fixed = 0u64;
    let mut files_verified = 0u64;

    // Skip the filesystem sweep entirely when we have no library
    // path configured (fresh install pre-wizard).
    if media_library_path.is_empty() {
        return Ok((0, 0, false));
    }

    let media_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media")
        .fetch_one(pool)
        .await?;

    if media_count > EAGER_VERIFY_THRESHOLD {
        tracing::info!(
            media_count,
            threshold = EAGER_VERIFY_THRESHOLD,
            "reconcile: media count exceeds eager-verify threshold — \
             deferring per-file existence check to first access",
        );
        return Ok((0, 0, true));
    }

    let media_files: Vec<(i64, String, Option<i64>)> =
        sqlx::query_as("SELECT id, file_path, movie_id FROM media")
            .fetch_all(pool)
            .await?;

    for (media_id, file_path, movie_id) in &media_files {
        files_verified += 1;
        let path = std::path::Path::new(file_path);
        if path.exists() {
            continue;
        }
        tracing::warn!(
            media_id,
            movie_id = ?movie_id,
            path = %file_path,
            "reconcile: media file missing from disk — deleting row",
        );
        // Cascade deletes stream + media_episode. Status is derived
        // from (media, active_download, watched_at), so removing
        // the row automatically moves the linked movie/episode
        // back to the `wanted` phase.
        sqlx::query("DELETE FROM media WHERE id = ?")
            .bind(media_id)
            .execute(pool)
            .await?;
        entities_fixed += 1;
    }

    Ok((entities_fixed, files_verified, false))
}

/// Mark every row in `from` as `Failed` with the given reason.
/// Separated from the main flow for reuse + to let the call site
/// read as a narrative.
async fn mark_interrupted(
    pool: &SqlitePool,
    from: DownloadPhase,
    reason: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query("UPDATE download SET state = ?, error_message = ? WHERE state = ?")
        .bind(DownloadPhase::Failed)
        .bind(reason)
        .bind(from)
        .execute(pool)
        .await?;
    let n = r.rows_affected();
    if n > 0 {
        tracing::warn!(
            count = n,
            from = %from,
            reason,
            "reconcile: marking transient downloads failed"
        );
    }
    Ok(n)
}

/// Remove a download's source directory (best-effort). Called
/// from the `seeding` → `imported` and `imported` → `cleaned_up`
/// transitions during startup reconciliation, symmetric with the
/// runtime `check_seed_limits` cleanup.
async fn clean_download_output(download_id: i64, output_path: Option<&str>, title: &str) {
    let Some(path) = output_path else {
        return;
    };
    let p = std::path::Path::new(path);
    if !p.exists() {
        return;
    }
    let removed = if p.is_dir() {
        tokio::fs::remove_dir_all(p).await
    } else {
        tokio::fs::remove_file(p).await
    };
    match removed {
        Ok(()) => tracing::info!(
            download_id,
            title = %title,
            path = %path,
            "reconcile: cleaned source files post-seed",
        ),
        Err(e) => tracing::warn!(
            download_id,
            title = %title,
            path = %path,
            error = %e,
            "reconcile: failed to remove source path (continuing)",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::test_support::FakeTorrentSession;

    async fn fresh_pool() -> SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn reconcile_on_clean_db() {
        let pool = fresh_pool().await;
        let result = reconcile(&pool, "", None).await.unwrap();
        assert_eq!(result.orphans_cleaned, 0);
        assert_eq!(result.downloads_reconciled, 0);
        assert_eq!(result.entities_fixed, 0);
        assert_eq!(result.ghost_torrents_removed, 0);
    }

    #[tokio::test]
    async fn reconcile_marks_stalled_failed_when_torrent_absent() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO download (title, state, torrent_hash, added_at) VALUES ('Stale', 'stalled', 'deadbeef', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let torrents = FakeTorrentSession::new();
        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.downloads_reconciled, 1);

        let state: String = sqlx::query_scalar("SELECT state FROM download WHERE title = 'Stale'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[tokio::test]
    async fn reconcile_resumes_stalled_when_torrent_present() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO download (title, state, torrent_hash, added_at) VALUES ('Live', 'stalled', 'feedface', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let torrents = FakeTorrentSession::new();
        torrents.add_hash("feedface");
        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.downloads_reconciled, 1);

        let state: String = sqlx::query_scalar("SELECT state FROM download WHERE title = 'Live'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "downloading");
    }

    #[tokio::test]
    async fn reconcile_promotes_seeding_to_imported_when_torrent_gone() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO download (title, state, torrent_hash, added_at) VALUES ('Gone Seeder', 'seeding', 'cafebabe', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let torrents = FakeTorrentSession::new();
        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.downloads_reconciled, 1);

        let state: String =
            sqlx::query_scalar("SELECT state FROM download WHERE title = 'Gone Seeder'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "imported");
    }

    #[tokio::test]
    async fn reconcile_leaves_live_seeder_alone() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO download (title, state, torrent_hash, added_at) VALUES ('Still Seeding', 'seeding', 'badc0de5', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let torrents = FakeTorrentSession::new();
        torrents.add_hash("badc0de5");
        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.downloads_reconciled, 0);

        let state: String =
            sqlx::query_scalar("SELECT state FROM download WHERE title = 'Still Seeding'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "seeding");
    }

    #[tokio::test]
    async fn reconcile_cleans_imported_source_files() {
        let pool = fresh_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("leftover");
        tokio::fs::create_dir_all(&source).await.unwrap();
        tokio::fs::write(source.join("dummy.bin"), b"hi")
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO download (title, state, output_path, added_at) VALUES ('Old Import', 'imported', ?, '2026-01-01')",
        )
        .bind(source.to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let torrents = FakeTorrentSession::new();
        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.downloads_reconciled, 1);

        let state: String =
            sqlx::query_scalar("SELECT state FROM download WHERE title = 'Old Import'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "cleaned_up");
        assert!(!source.exists(), "source dir should be gone");
    }

    #[tokio::test]
    async fn reconcile_removes_ghost_torrents() {
        let pool = fresh_pool().await;
        let torrents = FakeTorrentSession::new();
        torrents.add_hash("ghostghost");

        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.ghost_torrents_removed, 1);
        assert!(torrents.list_torrent_hashes().is_empty());
    }

    #[tokio::test]
    async fn reconcile_skips_ghost_cleanup_when_hash_matches_db() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO download (title, state, torrent_hash, added_at) VALUES ('Known', 'downloading', 'knownhash', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let torrents = FakeTorrentSession::new();
        torrents.add_hash("knownhash");

        let result = reconcile(&pool, "", Some(&torrents)).await.unwrap();
        assert_eq!(result.ghost_torrents_removed, 0);
        assert_eq!(torrents.list_torrent_hashes().len(), 1);
    }

    #[tokio::test]
    async fn reconcile_resets_grabbing_downloads() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO download (title, state, added_at) VALUES ('Stuck DL', 'grabbing', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let result = reconcile(&pool, "", None).await.unwrap();
        assert_eq!(result.downloads_reconciled, 1);

        let state: String =
            sqlx::query_scalar("SELECT state FROM download WHERE title = 'Stuck DL'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "failed");
    }

    #[tokio::test]
    async fn reconcile_cleans_orphan_streams() {
        let pool = fresh_pool().await;

        let media_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO media (file_path, relative_path, size, date_added) VALUES ('/tmp/test.mkv', 'test.mkv', 1000, '2026-01-01') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO stream (media_id, stream_index, stream_type) VALUES (?, 0, 'video')",
        )
        .bind(media_id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM media WHERE id = ?")
            .bind(media_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        let result = reconcile(&pool, "", None).await.unwrap();
        assert!(result.orphans_cleaned >= 1);
    }

    #[tokio::test]
    async fn reconcile_is_idempotent() {
        let pool = fresh_pool().await;

        sqlx::query(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at) VALUES (2, 'Test', 1, 1, '2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let r1 = reconcile(&pool, "", None).await.unwrap();
        let r2 = reconcile(&pool, "", None).await.unwrap();
        assert_eq!(r1.entities_fixed, r2.entities_fixed);
    }

    #[tokio::test]
    async fn reconcile_defers_file_verify_on_large_libraries() {
        use std::fmt::Write as _;

        let pool = fresh_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let library_path = tmp.path().to_string_lossy().to_string();

        // Seed > threshold media rows. Using a bulk insert to stay
        // fast without stressing the test runner.
        let mut sql =
            String::from("INSERT INTO media (file_path, relative_path, size, date_added) VALUES ");
        for i in 0..(EAGER_VERIFY_THRESHOLD + 5) {
            if i > 0 {
                sql.push(',');
            }
            let _ = write!(
                sql,
                "('/tmp/missing-{i}.mkv', 'missing-{i}.mkv', 1000, '2026-01-01')"
            );
        }
        sqlx::query(&sql).execute(&pool).await.unwrap();

        let result = reconcile(&pool, &library_path, None).await.unwrap();
        assert!(result.files_verified_lazily);
        assert_eq!(result.files_verified, 0);
        assert_eq!(result.entities_fixed, 0);
    }
}
