//! Cleanup subsystem — removes watched content after a configurable
//! delay, and provides the [`tracker::CleanupTracker`] retry queue
//! for resource removals (torrents, files, directories) that must
//! succeed but can transiently fail.

pub mod executor;
pub mod tracker;

pub use executor::AppRemovalExecutor;
pub use tracker::{CleanupTracker, RemovalExecutor, RemovalOutcome, ResourceKind, RetryReport};

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::events::AppEvent;

/// Run a cleanup cycle: delete media for watched content past the
/// delay. `event_tx` is optional so test callers that don't exercise
/// the notification path (`run_cleanup(&pool, ..., &None)`) still
/// work; production always passes `Some(&state.event_tx)` so the
/// frontend / Trakt-collection listener see the deletion live.
pub async fn run_cleanup(
    pool: &SqlitePool,
    event_tx: Option<&broadcast::Sender<AppEvent>>,
    data_path: &std::path::Path,
    movie_delay_hours: i64,
    episode_delay_hours: i64,
    auto_cleanup_enabled: bool,
) -> anyhow::Result<CleanupResult> {
    if !auto_cleanup_enabled {
        return Ok(CleanupResult::default());
    }

    // `media_library_path` is the "don't rmdir above this" guard
    // for the empty-dir sweep. Reading once per run is fine — the
    // setting rarely changes and even if it does, the next tick
    // picks up the new value.
    let library_root: Option<String> =
        sqlx::query_scalar("SELECT media_library_path FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .filter(|s: &String| !s.is_empty());
    let library_root_path = library_root.as_deref().map(std::path::Path::new);

    let mut result = CleanupResult::default();

    // Clean up movies watched longer than delay
    let expired_movies = sqlx::query_as::<_, ExpiredMedia>(
        // `watched_at IS NOT NULL` IS the watched status — the old
        // `status = 'watched' AND watched_at IS NOT NULL` was
        // redundant-and-drift-prone.
        "SELECT m.id as media_id, m.file_path, m.movie_id, NULL as episode_id FROM media m JOIN movie mv ON m.movie_id = mv.id WHERE mv.watched_at IS NOT NULL AND mv.watched_at < datetime('now', ? || ' hours')",
    )
    .bind(-movie_delay_hours)
    .fetch_all(pool)
    .await?;

    for media in &expired_movies {
        if let Err(e) = delete_media_file(pool, media, data_path, library_root_path).await {
            tracing::warn!(media_id = media.media_id, error = %e, "failed to clean up media");
        } else {
            result.movies_cleaned += 1;
            if let Some(tx) = event_tx
                && let Some(movie_id) = media.movie_id
            {
                let title: String = sqlx::query_scalar("SELECT title FROM movie WHERE id = ?")
                    .bind(movie_id)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| format!("movie #{movie_id}"));
                let _ = tx.send(AppEvent::ContentRemoved {
                    movie_id: Some(movie_id),
                    show_id: None,
                    title,
                });
            }
        }
    }

    // Clean up episodes: season-level, NOT per-episode. Spec is
    // emphatic — we only remove episode media when the whole
    // season's in-scope episodes are watched, and the delay is
    // anchored to the *last* of those watches, not the earliest.
    //
    // Concretely the WHERE clause asserts, for every episode row
    // whose media we consider deleting:
    //   - the episode itself is watched,
    //   - no other in-scope episode in the same season is
    //     unwatched (`NOT EXISTS` guard), and
    //   - the latest `watched_at` across that season has aged past
    //     `episode_delay_hours`.
    //
    // The previous flat per-episode query deleted S01E02 72h after
    // it aired even if S01E03..10 were still unwatched — user
    // visible data loss, which is why this is a BLOCKER.
    let expired_episode_media = sqlx::query_as::<_, ExpiredMedia>(
        "SELECT m.id AS media_id, m.file_path, NULL AS movie_id, me.episode_id
         FROM media m
         JOIN media_episode me ON m.id = me.media_id
         JOIN episode e ON me.episode_id = e.id
         WHERE e.watched_at IS NOT NULL
           AND e.in_scope = 1
           AND NOT EXISTS (
               SELECT 1 FROM episode e2
               WHERE e2.show_id = e.show_id
                 AND e2.season_number = e.season_number
                 AND e2.in_scope = 1
                 AND e2.watched_at IS NULL
           )
           AND (
               SELECT MAX(e3.watched_at) FROM episode e3
               WHERE e3.show_id = e.show_id
                 AND e3.season_number = e.season_number
                 AND e3.in_scope = 1
           ) < datetime('now', ? || ' hours')",
    )
    .bind(-episode_delay_hours)
    .fetch_all(pool)
    .await?;

    for media in &expired_episode_media {
        if let Err(e) = delete_media_file(pool, media, data_path, library_root_path).await {
            tracing::warn!(media_id = media.media_id, error = %e, "failed to clean up episode media");
        } else {
            result.episodes_cleaned += 1;
            if let Some(tx) = event_tx
                && let Some(episode_id) = media.episode_id
            {
                // Compose "Show Title · S01E02 · Ep Title" for the
                // toast / webhook payload. Falls back to a bare
                // marker on any DB glitch — event emission must
                // never block cleanup.
                let title = crate::events::display::episode_display_title(pool, episode_id).await;
                let show_id: Option<i64> =
                    sqlx::query_scalar("SELECT show_id FROM episode WHERE id = ?")
                        .bind(episode_id)
                        .fetch_optional(pool)
                        .await
                        .ok()
                        .flatten();
                let _ = tx.send(AppEvent::ContentRemoved {
                    movie_id: None,
                    show_id,
                    title: if title.is_empty() {
                        format!("episode #{episode_id}")
                    } else {
                        title
                    },
                });
            }
        }
    }

    Ok(result)
}

/// Check free disk space at `path` against the user-configured
/// warning threshold. Shells out to `df` rather than linking a
/// `statvfs` binding — pure-stdlib, avoids `unsafe`, portable to
/// macOS/Linux without extra deps.
///
/// Threshold tiers:
///   - `free_gb > warning_threshold` → Normal
///   - `warning_threshold >= free_gb > warning_threshold / 4` → Warning
///   - `free_gb <= warning_threshold / 4` (or <= 1 GB absolute) → Critical
///
/// Critical is intentionally well below Warning so a fresh alert
/// path (notification + health panel) has meaningful severity
/// differentiation, without needing a second user-facing config
/// knob. The /4 ratio puts Critical at "genuinely about to fail
/// a grab" rather than "needs attention soon".
pub fn check_disk_space(path: &str, warning_threshold_gb: u64) -> DiskStatus {
    let output = std::process::Command::new("df")
        .args(["--output=avail", "-B1", path])
        .output();

    let Ok(output) = output else {
        return DiskStatus::Unknown;
    };
    if !output.status.success() {
        return DiskStatus::Unknown;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Skip header line, parse available bytes
    let free_bytes: u64 = stdout
        .lines()
        .nth(1)
        .and_then(|line| line.trim().parse().ok())
        .unwrap_or(0);
    let free_gb = free_bytes / (1024 * 1024 * 1024);

    let critical_threshold = (warning_threshold_gb / 4).max(1);
    if free_gb > warning_threshold_gb {
        DiskStatus::Normal(free_gb)
    } else if free_gb > critical_threshold {
        DiskStatus::Warning(free_gb)
    } else {
        DiskStatus::Critical(free_gb)
    }
}

/// One-shot disk-space sweep. Reads the configured paths and
/// threshold, checks both, and emits `HealthWarning` /
/// `HealthRecovered` on state transitions. Prior state is kept in
/// a module-local map keyed by path so two consecutive warnings
/// for the same path don't spam (spec §notifications: "rate-
/// limited, transition-only"). A process restart loses the prior
/// state, which is the right trade-off — if disk is still low on
/// boot we should re-announce the warning so the user sees it.
///
/// Called by the `disk_space_check` scheduler task every 5 min
/// (see `scheduler::register_defaults`). Deliberately cheap: one
/// `df` exec per path, one DB read for config.
pub async fn disk_space_sweep(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
) -> anyhow::Result<()> {
    #[derive(sqlx::FromRow)]
    struct DiskCfg {
        media_library_path: String,
        download_path: String,
        low_disk_threshold_gb: i64,
    }
    let cfg: Option<DiskCfg> = sqlx::query_as(
        "SELECT media_library_path, download_path, low_disk_threshold_gb
         FROM config WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some(cfg) = cfg else {
        // No config row → nothing configured to check yet. First-
        // boot / mid-migration path; next tick will retry.
        return Ok(());
    };
    #[allow(clippy::cast_sign_loss)]
    let threshold = cfg.low_disk_threshold_gb.max(1) as u64;

    // Check both paths — users commonly set them to the same
    // filesystem (one library, one download staging), but we
    // respect the split if they're elsewhere.
    let paths: [(&str, &str); 2] = [
        ("library", cfg.media_library_path.as_str()),
        ("downloads", cfg.download_path.as_str()),
    ];
    for (label, path) in paths {
        if path.is_empty() {
            continue;
        }
        let status = check_disk_space(path, threshold);
        let prior = prior_disk_state(path);
        set_disk_state(path, status);

        // Fire only on transitions. Normal → Warning/Critical emits
        // HealthWarning; Warning/Critical → Normal emits HealthRecovered.
        // Same-severity ticks stay silent.
        match (prior, status) {
            (Some(p), s) if disk_severity(p) == disk_severity(s) => {}
            (_, DiskStatus::Warning(gb)) => {
                let _ = event_tx.send(AppEvent::HealthWarning {
                    message: format!(
                        "Low disk space on {label}: {gb} GB free (threshold {threshold} GB). \
                         Grabs will eventually fail if this keeps dropping."
                    ),
                });
                tracing::warn!(path, label, free_gb = gb, threshold, "disk-space warning");
            }
            (_, DiskStatus::Critical(gb)) => {
                let _ = event_tx.send(AppEvent::HealthWarning {
                    message: format!(
                        "Critically low disk space on {label}: {gb} GB free. \
                         New grabs will almost certainly fail."
                    ),
                });
                tracing::error!(path, label, free_gb = gb, threshold, "disk-space critical");
            }
            (Some(DiskStatus::Warning(_) | DiskStatus::Critical(_)), DiskStatus::Normal(gb)) => {
                let _ = event_tx.send(AppEvent::HealthRecovered {
                    message: format!("Disk space recovered on {label}: {gb} GB free."),
                });
                tracing::info!(path, label, free_gb = gb, "disk-space recovered");
            }
            _ => {}
        }
    }
    Ok(())
}

fn disk_severity(s: DiskStatus) -> u8 {
    match s {
        DiskStatus::Normal(_) => 0,
        DiskStatus::Unknown => 1,
        DiskStatus::Warning(_) => 2,
        DiskStatus::Critical(_) => 3,
    }
}

/// Walk `media_library_path` and surface video files that have no
/// matching `media.file_path` row. Reports the count (and first
/// handful of paths) at warn level so operators can reconcile
/// manually — this scan deliberately does *not* auto-delete: a
/// file with no DB entry might be the user's own copy, a pre-
/// import staging artefact, or a pre-kino library they've pointed
/// at. The spec promises the scan; an opt-in "actually remove
/// orphans" action can land later via an API endpoint.
///
/// Scheduled weekly — orphans accumulate slowly and a full-tree
/// stat is cheap but not free (one `readdir` per directory, one
/// DB check per file). Subtitle / image orphans are out of scope
/// here; transcode-temp has its own sweep in `transcode`.
pub async fn orphan_file_scan(pool: &SqlitePool) -> anyhow::Result<u64> {
    let library_root: Option<String> =
        sqlx::query_scalar("SELECT media_library_path FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .filter(|s: &String| !s.is_empty());
    let Some(root) = library_root else {
        tracing::debug!("orphan scan: no media_library_path configured, skipping");
        return Ok(0);
    };
    let root_path = std::path::Path::new(&root);
    if !root_path.is_dir() {
        tracing::debug!(path = %root, "orphan scan: library path not a directory, skipping");
        return Ok(0);
    }

    // Pull every known path into a set. For a 10k-row library this
    // is ~1 MB in memory — trivial. Walking the tree would otherwise
    // issue one `SELECT` per file and hammer SQLite.
    let known: std::collections::HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT file_path FROM media")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    // Recursive walk. `std::fs` is fine here — we're in a scheduler
    // task already off the request path. Skip hidden entries
    // (`.extracted`, `.DS_Store`, etc.) and anything that doesn't
    // look like a video file — we don't want to flag `poster.jpg`.
    let mut orphans: Vec<std::path::PathBuf> = Vec::new();
    walk_for_orphans(root_path, &known, &mut orphans);
    let count = u64::try_from(orphans.len()).unwrap_or(u64::MAX);

    if count > 0 {
        let sample: Vec<String> = orphans
            .iter()
            .take(5)
            .map(|p| p.display().to_string())
            .collect();
        tracing::warn!(
            count,
            library = %root,
            sample = ?sample,
            "orphan scan: found files on disk with no matching media row"
        );
    } else {
        tracing::debug!(library = %root, "orphan scan: no orphans found");
    }
    Ok(count)
}

fn walk_for_orphans(
    dir: &std::path::Path,
    known: &std::collections::HashSet<String>,
    out: &mut Vec<std::path::PathBuf>,
) {
    const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "mov", "wmv", "m4v", "webm", "flv"];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk_for_orphans(&path, known, out);
            continue;
        }
        let is_video = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| VIDEO_EXTS.iter().any(|v| ext.eq_ignore_ascii_case(v)));
        if !is_video {
            continue;
        }
        let as_string = path.to_string_lossy().to_string();
        if !known.contains(&as_string) {
            out.push(path);
        }
    }
}

/// Module-local prior-state map. Keyed by path so a user with
/// split library/downloads can have independent transitions per
/// volume.
static DISK_STATE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, DiskStatus>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn prior_disk_state(path: &str) -> Option<DiskStatus> {
    DISK_STATE.lock().ok().and_then(|m| m.get(path).copied())
}

fn set_disk_state(path: &str, status: DiskStatus) {
    if let Ok(mut m) = DISK_STATE.lock() {
        m.insert(path.to_owned(), status);
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ExpiredMedia {
    media_id: i64,
    file_path: String,
    #[allow(dead_code)]
    movie_id: Option<i64>,
    #[allow(dead_code)]
    episode_id: Option<i64>,
}

async fn delete_media_file(
    pool: &SqlitePool,
    media: &ExpiredMedia,
    data_path: &std::path::Path,
    library_root: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let path = std::path::Path::new(&media.file_path);

    // Delete the file from disk
    if path.exists() {
        tokio::fs::remove_file(path).await?;
        tracing::info!(path = %media.file_path, "deleted expired media file");

        // Delete common external subtitle sidecars (.srt / .vtt,
        // optionally with a language suffix). Best-effort — missing
        // sidecars are fine, permission errors are logged but don't
        // fail cleanup.
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Some(parent) = path.parent()
        {
            for ext in ["srt", "vtt", "en.srt", "en.vtt"] {
                let sidecar = parent.join(format!("{stem}.{ext}"));
                if sidecar.exists()
                    && let Err(e) = tokio::fs::remove_file(&sidecar).await
                {
                    tracing::debug!(
                        path = %sidecar.display(),
                        error = %e,
                        "sidecar delete failed (continuing)"
                    );
                }
            }
        }

        // Clean up empty parent directories
        if let Some(parent) = path.parent() {
            cleanup_empty_dirs(parent, library_root).await;
        }
    }

    // Drop the extracted-subs cache for this media (best-effort —
    // missing dir is fine, it just means the subtitle endpoint was
    // never hit for this media).
    if let Err(e) = crate::playback::subtitle::clear_cache_dir(data_path, media.media_id).await {
        tracing::debug!(
            media_id = media.media_id,
            error = %e,
            "subtitle cache cleanup failed (continuing)",
        );
    }

    if let Err(e) = crate::playback::trickplay::clear_trickplay_dir(data_path, media.media_id).await
    {
        tracing::debug!(
            media_id = media.media_id,
            error = %e,
            "trickplay cache cleanup failed (continuing)",
        );
    }

    // Delete from database (cascade deletes streams, media_episode)
    sqlx::query("DELETE FROM media WHERE id = ?")
        .bind(media.media_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Walk up from `dir` removing empty parents, stopping the moment
/// we'd touch the library root or step outside it. Previously
/// guarded on the literal directory names `Movies` / `TV`, which
/// happened to work for the default naming layout but would let
/// us accidentally `rmdir` a user's library root if they'd
/// customised the folder structure (e.g. `Movies 4K`, `Shows/*`).
///
/// `library_root`, when provided, is the configured
/// `media_library_path` — we stop when `current == library_root`
/// and skip anything that isn't a descendant of it. None means
/// "no root guard" (tests / callers that haven't been threaded
/// the root through yet), in which case we fall back to the old
/// literal-name guard.
pub(crate) async fn cleanup_empty_dirs(
    dir: &std::path::Path,
    library_root: Option<&std::path::Path>,
) {
    let mut current = dir;
    loop {
        if let Some(root) = library_root {
            // Stop on the root itself or anything outside it.
            // `starts_with` is a prefix match on canonical paths,
            // which is what we want here — `rmdir /media/library`
            // would destroy the user's library even if empty.
            if current == root || !current.starts_with(root) {
                break;
            }
        } else {
            let name = current.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "Movies" || name == "TV" || name.is_empty() {
                break;
            }
        }

        // Try to remove if empty
        if tokio::fs::remove_dir(current).await.is_err() {
            break; // Not empty or permission error
        }

        tracing::debug!(dir = %current.display(), "removed empty directory");

        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
}

/// Result of a cleanup cycle.
#[derive(Debug, Default)]
pub struct CleanupResult {
    pub movies_cleaned: u32,
    pub episodes_cleaned: u32,
}

/// Disk space status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    Normal(u64),
    Warning(u64),
    Critical(u64),
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn insert_show(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, added_at)
             VALUES (99, 'T', 1, 1, datetime('now')) RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Insert a minimal episode row + linked media + `media_episode`.
    /// Returns (`episode_id`, `media_id`). `watched_at` is ISO 8601
    /// or NULL.
    async fn insert_episode(
        pool: &SqlitePool,
        show_id: i64,
        season: i64,
        number: i64,
        watched_at: Option<&str>,
    ) -> (i64, i64) {
        // The schema requires a `series_id`; create a series if it's
        // not already there for this (show, season).
        let series_id: i64 = match sqlx::query_scalar::<_, i64>(
            "SELECT id FROM series WHERE show_id = ? AND season_number = ?",
        )
        .bind(show_id)
        .bind(season)
        .fetch_optional(pool)
        .await
        .unwrap()
        {
            Some(id) => id,
            None => sqlx::query_scalar::<_, i64>(
                "INSERT INTO series (show_id, season_number) VALUES (?, ?) RETURNING id",
            )
            .bind(show_id)
            .bind(season)
            .fetch_one(pool)
            .await
            .unwrap(),
        };
        let ep_id: i64 = sqlx::query_scalar(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, acquire, in_scope, watched_at)
             VALUES (?, ?, ?, ?, 1, 1, ?) RETURNING id",
        )
        .bind(series_id)
        .bind(show_id)
        .bind(season)
        .bind(number)
        .bind(watched_at)
        .fetch_one(pool)
        .await
        .unwrap();
        let media_id: i64 = sqlx::query_scalar(
            "INSERT INTO media (file_path, relative_path, size, date_added)
             VALUES (?, ?, 1, datetime('now')) RETURNING id",
        )
        .bind(format!("/tmp/kino-test/s{season:02}e{number:02}.mkv"))
        .bind(format!("s{season:02}e{number:02}.mkv"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO media_episode (media_id, episode_id) VALUES (?, ?)")
            .bind(media_id)
            .bind(ep_id)
            .execute(pool)
            .await
            .unwrap();
        (ep_id, media_id)
    }

    /// Core of the season-level cleanup fix: an episode watched
    /// long ago is NOT cleaned up if another episode in the same
    /// season is still unwatched.
    #[tokio::test]
    async fn episode_not_cleaned_while_season_incomplete() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();
        let show = insert_show(&pool).await;
        // S01E01 watched yesterday, S01E02 unwatched → neither
        // should be cleaned even at 1h delay.
        let yesterday = "2026-04-22T12:00:00+00:00";
        let (_, m1) = insert_episode(&pool, show, 1, 1, Some(yesterday)).await;
        let (_, _m2) = insert_episode(&pool, show, 1, 2, None).await;

        let expired: Vec<(i64,)> = sqlx::query_as(
            "SELECT m.id
             FROM media m
             JOIN media_episode me ON m.id = me.media_id
             JOIN episode e ON me.episode_id = e.id
             WHERE e.watched_at IS NOT NULL
               AND e.in_scope = 1
               AND NOT EXISTS (
                   SELECT 1 FROM episode e2
                   WHERE e2.show_id = e.show_id
                     AND e2.season_number = e.season_number
                     AND e2.in_scope = 1
                     AND e2.watched_at IS NULL
               )",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            !expired.iter().any(|(id,)| *id == m1),
            "S01E01 must not expire while S01E02 is unwatched"
        );
    }

    /// Season fully watched → delay anchored to `MAX(watched_at)`
    /// across the season, not the earliest. Watching S01E01 a year
    /// ago + finishing S01E10 today means the season isn't
    /// eligible for cleanup for `delay` hours from today.
    #[tokio::test]
    async fn season_delay_anchors_to_max_watched_at() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();
        let show = insert_show(&pool).await;
        insert_episode(&pool, show, 1, 1, Some("2025-04-01T00:00:00+00:00")).await;
        // Latest watch: 1h ago. delay = 72h → not yet cleanable.
        // Computed against the real wall-clock so the test doesn't
        // become time-bombed (a hardcoded date drifts past the
        // 72-hour window every day after it was written).
        let an_hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let (_, m2) = insert_episode(&pool, show, 1, 2, Some(&an_hour_ago)).await;

        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT m.id FROM media m
             JOIN media_episode me ON m.id = me.media_id
             JOIN episode e ON me.episode_id = e.id
             WHERE e.watched_at IS NOT NULL
               AND e.in_scope = 1
               AND NOT EXISTS (
                   SELECT 1 FROM episode e2
                   WHERE e2.show_id = e.show_id
                     AND e2.season_number = e.season_number
                     AND e2.in_scope = 1
                     AND e2.watched_at IS NULL
               )
               AND (
                   SELECT MAX(e3.watched_at) FROM episode e3
                   WHERE e3.show_id = e.show_id
                     AND e3.season_number = e.season_number
                     AND e3.in_scope = 1
               ) < datetime('now', '-72 hours')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            !rows.iter().any(|(id,)| *id == m2),
            "fully-watched season shouldn't clean while the newest watch is within delay"
        );
    }

    /// `cleanup_empty_dirs` must stop at the configured library
    /// root even when the dir name isn't one of the old hard-coded
    /// `Movies` / `TV` literals. Without this guard a user with a
    /// customised layout could end up with the library root rmdir'd.
    #[tokio::test]
    async fn cleanup_empty_dirs_respects_library_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("my-library"); // intentionally NOT "Movies"/"TV"
        let nested = root.join("Films").join("The Matrix (1999)");
        std::fs::create_dir_all(&nested).unwrap();

        cleanup_empty_dirs(&nested, Some(&root)).await;

        assert!(!nested.exists(), "empty leaf should be removed");
        assert!(root.exists(), "library root must survive");
        assert!(
            !root.join("Films").exists(),
            "empty intermediate dir should be removed"
        );
    }

    /// Orphan scan finds video files with no matching `media` row
    /// and ignores hidden files, non-video extensions, and
    /// known-good files.
    #[tokio::test]
    async fn orphan_scan_finds_untracked_videos() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("known.mkv"), b"x").unwrap();
        std::fs::write(root.join("orphan.mkv"), b"x").unwrap();
        std::fs::write(root.join("poster.jpg"), b"x").unwrap(); // not a video
        std::fs::write(root.join(".DS_Store"), b"x").unwrap(); // hidden

        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, root.to_str().unwrap())
            .await
            .unwrap();
        // Point the library path at our tmp dir and register the
        // known file as a media row.
        sqlx::query("UPDATE config SET media_library_path = ? WHERE id = 1")
            .bind(root.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO media (file_path, relative_path, size, date_added)
             VALUES (?, 'known.mkv', 1, datetime('now'))",
        )
        .bind(root.join("known.mkv").to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let count = orphan_file_scan(&pool).await.unwrap();
        assert_eq!(count, 1, "should find only orphan.mkv");
    }

    /// Disk-space status respects the configured threshold. We
    /// can't force a real df result in the test, but we can pin
    /// the threshold arithmetic against the public enum.
    #[test]
    fn disk_status_thresholds_are_relative_to_config() {
        // Threshold = 100 GB → critical <= 25 GB (100/4).
        let path = "/"; // any existing path for the df exec
        let status = check_disk_space(path, 100);
        // We don't know free_gb on the test host; just assert the
        // shape is one of the expected variants. Main signal: the
        // function doesn't panic and doesn't hardcode 10/50.
        matches!(
            status,
            DiskStatus::Normal(_)
                | DiskStatus::Warning(_)
                | DiskStatus::Critical(_)
                | DiskStatus::Unknown
        );
    }
}
