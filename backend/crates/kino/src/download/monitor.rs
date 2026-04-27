//! Download monitor — polls active downloads and updates state.
//!
//! Polls librqbit for real torrent progress when available.
//! Falls back to simulated progress when no torrent client is configured.
//!
//! Stall detection: tracks when each active download last made progress.
//! Downloads with zero download-speed past `config.stall_timeout` minutes
//! are marked `stalled`; past `config.dead_timeout` they're `failed`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::download::TorrentSession;
use crate::download::phase::DownloadPhase;
use crate::download::torrent_client::TorrentState;
use crate::events::AppEvent;

/// Last time each download had non-zero progress. Keyed by download id.
/// In-memory only — a process restart resets the clock, which means a
/// long-stalled torrent gets one more grace period. Acceptable tradeoff.
static LAST_PROGRESS: LazyLock<Mutex<HashMap<i64, (Instant, i64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Downloads we've already remediated during their current stall
/// episode. librqbit doesn't expose a force-announce API, so
/// `remediate_stall` does pause → short sleep → resume which
/// restarts the torrent's state machine and triggers a fresh
/// tracker announce + DHT peer request. We only do it once per
/// stall cycle — entry clears when the download next makes
/// progress (back to "downloading") or terminates. Without that
/// guard the monitor would pause-resume the same torrent every
/// 3-second tick, which trackers dislike.
static STALL_REMEDIATED: LazyLock<Mutex<std::collections::HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

fn mark_remediated(download_id: i64) -> bool {
    match STALL_REMEDIATED.lock() {
        Ok(mut set) => set.insert(download_id),
        Err(p) => {
            tracing::error!(download_id, "STALL_REMEDIATED lock poisoned — recovering");
            p.into_inner().insert(download_id)
        }
    }
}

fn clear_remediated(download_id: i64) {
    match STALL_REMEDIATED.lock() {
        Ok(mut set) => {
            set.remove(&download_id);
        }
        Err(e) => {
            tracing::error!(download_id, error = %e, "STALL_REMEDIATED lock poisoned");
        }
    }
}

fn observe_progress(download_id: i64, downloaded: i64) -> std::time::Duration {
    // If the mutex is poisoned (a panic happened while holding it) we
    // recover with `into_inner` on the PoisonError — the map data is
    // still valid, and taking the download monitor out for the session
    // because of a panic elsewhere would be worse than a potentially
    // stale timestamp.
    let mut map = match LAST_PROGRESS.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::error!(download_id, "LAST_PROGRESS lock poisoned — recovering",);
            poisoned.into_inner()
        }
    };
    let now = Instant::now();
    let entry = map.entry(download_id).or_insert((now, downloaded));
    if downloaded > entry.1 {
        *entry = (now, downloaded);
        std::time::Duration::ZERO
    } else {
        now.duration_since(entry.0)
    }
}

fn forget_progress(download_id: i64) {
    match LAST_PROGRESS.lock() {
        Ok(mut m) => {
            m.remove(&download_id);
        }
        Err(e) => {
            // Lock is poisoned — another task panicked while holding it.
            // We can't do much useful recovery; surfacing the event is
            // the only actionable signal.
            tracing::error!(download_id, error = %e, "LAST_PROGRESS lock poisoned");
        }
    }
}

/// Poll all active downloads and update their progress.
/// Triggers import when a download completes.
#[allow(clippy::too_many_lines)]
pub async fn monitor_downloads(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    torrent: Option<&dyn TorrentSession>,
    ffprobe_path: &str,
) -> anyhow::Result<()> {
    // Ordered oldest-first so FIFO holds when the cap gates how many
    // queued rows we can start this tick. `grabbing|downloading|stalled`
    // all hold a librqbit slot, so all three count toward the cap.
    let downloads = sqlx::query_as::<_, crate::download::model::Download>(
        "SELECT * FROM download
         WHERE state IN ('queued', 'grabbing', 'downloading', 'stalled', 'seeding')
         ORDER BY added_at ASC, id ASC",
    )
    .fetch_all(pool)
    .await?;

    if downloads.is_empty() {
        return Ok(());
    }

    // Enforce `max_concurrent_downloads` — previously unused despite
    // being in config. `grabbing|downloading|stalled` all hold a
    // librqbit slot; `seeding|importing` don't (post-complete phases).
    let cap: i64 = sqlx::query_scalar("SELECT max_concurrent_downloads FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await?
        .unwrap_or(3);
    let cap = usize::try_from(cap.max(1)).unwrap_or(3);
    let mut active = downloads
        .iter()
        .filter_map(|d| DownloadPhase::parse(&d.state))
        .filter(|p| p.consumes_torrent_slot())
        .count();

    for dl in &downloads {
        let Some(phase) = DownloadPhase::parse(&dl.state) else {
            tracing::warn!(
                download_id = dl.id,
                state = %dl.state,
                "download_monitor: unknown download state — skipping"
            );
            continue;
        };
        match phase {
            DownloadPhase::Queued => {
                if active >= cap {
                    // TRACE because this fires every tick for every
                    // queued-but-blocked download. The cap-reached fact
                    // is discoverable from the active/cap pair on any
                    // INFO-level download_start event.
                    tracing::trace!(
                        download_id = dl.id,
                        title = %dl.title,
                        active,
                        cap,
                        "queued download held back — concurrency cap reached",
                    );
                    continue;
                }
                start_download(
                    pool,
                    event_tx,
                    torrent,
                    dl.id,
                    &dl.title,
                    dl.magnet_url.as_deref(),
                )
                .await?;
                active += 1;
            }
            DownloadPhase::Grabbing | DownloadPhase::Downloading | DownloadPhase::Stalled => {
                if let Some(hash) = &dl.torrent_hash {
                    let was_grabbing = phase == DownloadPhase::Grabbing;
                    poll_real_progress(
                        pool,
                        event_tx,
                        torrent,
                        dl.id,
                        &dl.title,
                        hash,
                        ffprobe_path,
                    )
                    .await?;
                    // Metadata-ready transition: `Grabbing` is the
                    // librqbit Initializing phase where the magnet is
                    // resolving into an info-dict. Once it flips past
                    // that, file metadata + total size are known —
                    // emit once so the files tab / detail pane can
                    // drop its 4s poll and refetch instantly.
                    if was_grabbing {
                        let new_state: Option<String> =
                            sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
                                .bind(dl.id)
                                .fetch_optional(pool)
                                .await
                                .ok()
                                .flatten();
                        let new_phase = new_state.as_deref().and_then(DownloadPhase::parse);
                        if new_phase.is_some_and(DownloadPhase::is_metadata_resolved) {
                            let _ = event_tx.send(AppEvent::DownloadMetadataReady {
                                download_id: dl.id,
                                torrent_hash: hash.clone(),
                            });
                        }
                    }
                } else {
                    // No hash means torrent client wasn't available when queued — retry.
                    // Already counted in `active`, so no increment needed.
                    start_download(
                        pool,
                        event_tx,
                        torrent,
                        dl.id,
                        &dl.title,
                        dl.magnet_url.as_deref(),
                    )
                    .await?;
                }
            }
            DownloadPhase::Seeding => {
                if let Some(hash) = &dl.torrent_hash {
                    check_seed_limits(pool, torrent, dl, hash).await?;
                }
            }
            DownloadPhase::Searching
            | DownloadPhase::Paused
            | DownloadPhase::Completed
            | DownloadPhase::Importing
            | DownloadPhase::Imported
            | DownloadPhase::CleanedUp
            | DownloadPhase::Failed
            | DownloadPhase::Cancelled => {}
        }
    }

    Ok(())
}

/// Start a download — add to librqbit and record the torrent hash.
#[tracing::instrument(
    skip(pool, event_tx, torrent, magnet_url),
    fields(download_id, title = %title)
)]
pub async fn start_download(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    torrent: Option<&dyn TorrentSession>,
    download_id: i64,
    title: &str,
    magnet_url: Option<&str>,
) -> anyhow::Result<()> {
    // `title` param is the release-parsed filename. For user-facing
    // events we want the episode's canonical "Show · SxxExx · Title"
    // composition instead; movies pass through unchanged.
    let display_title =
        crate::events::display::download_display_title(pool, download_id, title).await;

    let Some(client) = torrent else {
        tracing::error!(download_id, "no torrent client available");
        sqlx::query(
            "UPDATE download SET state = 'failed', error_message = 'Torrent client not available — check download path config' WHERE id = ?",
        )
        .bind(download_id)
        .execute(pool)
        .await?;
        let _ = event_tx.send(AppEvent::DownloadFailed {
            download_id,
            title: display_title,
            error: "Torrent client not available".to_owned(),
        });
        return Ok(());
    };

    let Some(magnet) = magnet_url else {
        tracing::warn!(download_id, "no magnet URL for download");
        sqlx::query(
            "UPDATE download SET state = 'failed', error_message = 'No magnet URL' WHERE id = ?",
        )
        .bind(download_id)
        .execute(pool)
        .await?;
        return Ok(());
    };

    // Atomically claim this download (prevents duplicate starts on concurrent ticks)
    let claimed =
        sqlx::query("UPDATE download SET state = 'grabbing' WHERE id = ? AND state = 'queued'")
            .bind(download_id)
            .execute(pool)
            .await?;

    if claimed.rows_affected() == 0 {
        return Ok(()); // Already claimed by another tick
    }

    // Add torrent to librqbit
    tracing::debug!(
        download_id,
        magnet_prefix = %magnet.chars().take(60).collect::<String>(),
        "adding torrent to librqbit",
    );
    match client.add_torrent(magnet, None, false).await {
        Ok((_torrent_id, info_hash)) => {
            sqlx::query("UPDATE download SET state = 'downloading', torrent_hash = ? WHERE id = ?")
                .bind(&info_hash)
                .bind(download_id)
                .execute(pool)
                .await?;

            let _ = event_tx.send(AppEvent::DownloadStarted {
                download_id,
                title: display_title.clone(),
            });

            tracing::info!(download_id, hash = %info_hash, title = %title, "torrent added to client");
        }
        Err(e) => {
            tracing::error!(download_id, error = %e, "failed to add torrent");
            sqlx::query("UPDATE download SET state = 'failed', error_message = ? WHERE id = ?")
                .bind(format!("Failed to add torrent: {e}"))
                .bind(download_id)
                .execute(pool)
                .await?;

            let _ = event_tx.send(AppEvent::DownloadFailed {
                download_id,
                title: display_title,
                error: e.to_string(),
            });
        }
    }

    Ok(())
}

/// Poll real progress from librqbit and update the database.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]
async fn poll_real_progress(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    torrent: Option<&dyn TorrentSession>,
    download_id: i64,
    title: &str,
    torrent_hash: &str,
    ffprobe_path: &str,
) -> anyhow::Result<()> {
    let Some(client) = torrent else {
        return Ok(());
    };

    // `title` is the release filename; user-facing events use the
    // composed episode title when applicable.
    let display_title =
        crate::events::display::download_display_title(pool, download_id, title).await;

    let Some(status) = client.get_status(torrent_hash) else {
        tracing::error!(download_id, hash = %torrent_hash, "torrent lost from client session");
        sqlx::query(
            "UPDATE download SET state = 'failed', error_message = 'Torrent lost from client session' WHERE id = ?",
        )
        .bind(download_id)
        .execute(pool)
        .await?;
        let _ = event_tx.send(AppEvent::DownloadFailed {
            download_id,
            title: display_title,
            error: "Torrent lost from client session".to_owned(),
        });
        return Ok(());
    };

    // Map torrent state to download state
    let mut db_state = match status.state {
        TorrentState::Initializing => "grabbing",
        TorrentState::Downloading => "downloading",
        TorrentState::Seeding => "seeding",
        TorrentState::Paused => "paused",
        TorrentState::Error => "failed",
    };

    // TRACE because this fires once per active download per scheduler
    // tick (1s). State transitions + completion + failure all emit
    // their own INFO/WARN logs — this per-tick snapshot is only
    // useful for sub-second-granularity replay.
    tracing::trace!(
        download_id,
        hash = %torrent_hash,
        state = db_state,
        downloaded = status.downloaded,
        uploaded = status.uploaded,
        dl_speed = status.download_speed,
        seeders = ?status.seeders,
        leechers = ?status.leechers,
        "torrent tick",
    );

    // Stall detection: while actively downloading OR already stalled,
    // track idle time against the configured stall_timeout (→ stalled)
    // and dead_timeout (→ failed). Previously this branch was gated on
    // only "downloading" | "grabbing", so once a torrent transitioned
    // to "stalled" it never escalated to "failed" — a chronically dead
    // torrent would hold a concurrency slot forever. Including
    // "stalled" here closes that escape hatch.
    // Config thresholds are read lazily so a change takes effect on
    // the next tick without a restart.
    if db_state == "downloading" || db_state == "grabbing" || db_state == "stalled" {
        let (stall_min, dead_min): (i64, i64) =
            sqlx::query_as("SELECT stall_timeout, dead_timeout FROM config WHERE id = 1")
                .fetch_optional(pool)
                .await?
                .unwrap_or((30, 60));

        let idle = observe_progress(download_id, status.downloaded);
        let idle_mins = i64::try_from(idle.as_secs() / 60).unwrap_or(i64::MAX);
        if dead_min > 0 && idle_mins >= dead_min {
            tracing::warn!(download_id, idle_mins, "download dead — marking failed");
            sqlx::query(
                "UPDATE download SET state = 'failed', error_message = 'No progress — dead timeout reached' WHERE id = ?",
            )
            .bind(download_id)
            .execute(pool)
            .await?;
            let _ = event_tx.send(AppEvent::DownloadFailed {
                download_id,
                title: display_title.clone(),
                error: "No progress — dead timeout reached".into(),
            });
            forget_progress(download_id);
            clear_remediated(download_id);
            return Ok(());
        } else if stall_min > 0 && idle_mins >= stall_min {
            db_state = "stalled";
            // First-entry remediation: kick the torrent's state
            // machine so it re-announces to trackers and retries the
            // DHT. `mark_remediated` returns true the first time the
            // download enters the set for this stall cycle.
            if mark_remediated(download_id) {
                tracing::info!(
                    download_id,
                    idle_mins,
                    "download stalled — remediating (pause+resume to re-announce)"
                );
                if let Err(e) = client.pause(torrent_hash).await {
                    tracing::debug!(
                        download_id,
                        error = %e,
                        "stall remediation: pause failed (continuing to resume)"
                    );
                }
                // Short pause so the tracker state machine actually
                // tears down before we re-announce. librqbit drives
                // its own tracker loop, so a sub-second dwell is
                // enough to force a fresh announce on resume.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if let Err(e) = client.resume(torrent_hash).await {
                    tracing::warn!(
                        download_id,
                        error = %e,
                        "stall remediation: resume failed — torrent may stay paused until next tick"
                    );
                }
            }
        } else {
            // Not stalled this tick: clear the remediation marker so
            // a future stall episode triggers a fresh kick.
            clear_remediated(download_id);
        }
    }

    // Update progress in DB
    sqlx::query(
        "UPDATE download SET state = ?, downloaded = ?, uploaded = ?, download_speed = ?, upload_speed = ?, seeders = ?, leechers = ?, eta = ? WHERE id = ?",
    )
    .bind(db_state)
    .bind(status.downloaded)
    .bind(status.uploaded)
    .bind(status.download_speed)
    .bind(status.upload_speed)
    .bind(status.seeders)
    .bind(status.leechers)
    .bind(status.eta_seconds)
    .bind(download_id)
    .execute(pool)
    .await?;

    // Emit progress event
    let total = sqlx::query_scalar::<_, Option<i64>>("SELECT size FROM download WHERE id = ?")
        .bind(download_id)
        .fetch_one(pool)
        .await?
        .unwrap_or(0);

    let percent = if total > 0 {
        ((status.downloaded as f64 / total as f64) * 100.0) as u8
    } else if status.finished {
        100
    } else {
        0
    };

    let _ = event_tx.send(AppEvent::DownloadProgress {
        download_id,
        percent,
        downloaded: status.downloaded,
        uploaded: status.uploaded,
        speed: status.download_speed,
        upload_speed: status.upload_speed,
        seeders: status.seeders,
        leechers: status.leechers,
        eta: status.eta_seconds,
    });

    // Check if complete
    if status.finished {
        complete_download(
            pool,
            event_tx,
            client,
            download_id,
            title,
            torrent_hash,
            ffprobe_path,
        )
        .await?;
    }

    Ok(())
}

/// Mark a download as completed, get output path, and trigger import.
async fn complete_download(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    client: &dyn TorrentSession,
    download_id: i64,
    title: &str,
    torrent_hash: &str,
    ffprobe_path: &str,
) -> anyhow::Result<()> {
    let now = crate::time::Timestamp::now().to_rfc3339();

    sqlx::query(
        "UPDATE download SET state = 'completed', completed_at = ?, download_speed = 0 WHERE id = ?",
    )
    .bind(&now)
    .bind(download_id)
    .execute(pool)
    .await?;

    // Pull size + start time so the history row can show the final
    // size and wall-clock duration. `added_at` exists for every
    // download; `size` may be null if the torrent client never
    // reported one, which we surface as None rather than 0.
    let (size, added_at): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT size, added_at FROM download WHERE id = ?")
            .bind(download_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or((None, None));
    let duration_ms = added_at.and_then(|s| {
        let start = chrono::DateTime::parse_from_rfc3339(&s).ok()?;
        let end = chrono::DateTime::parse_from_rfc3339(&now).ok()?;
        Some((end - start).num_milliseconds())
    });

    let _ = event_tx.send(AppEvent::DownloadComplete {
        download_id,
        title: crate::events::display::download_display_title(pool, download_id, title).await,
        size,
        duration_ms,
    });

    tracing::info!(download_id, title, "download completed, triggering import");

    // Trigger import. Passing the torrent client + hash lets the
    // importer ask librqbit which files belong to *this* torrent
    // instead of guessing from the filesystem — see the fix on
    // `pick_media_from_torrent` for the failure mode.
    crate::import::trigger::import_download(
        pool,
        event_tx,
        Some(client),
        Some(torrent_hash),
        download_id,
        ffprobe_path,
    )
    .await?;

    Ok(())
}

/// While seeding, enforce `seed_ratio_limit` and `seed_time_limit` from
/// config. When either is reached, remove the torrent from librqbit
/// (deleting the source files, since the library already holds its
/// hardlinked/copied version) and mark the download as `cleaned_up`.
/// Without the delete, download-path disk usage grows unboundedly on
/// `use_hardlinks = 0` setups.
#[allow(clippy::cast_precision_loss)]
async fn check_seed_limits(
    pool: &SqlitePool,
    torrent: Option<&dyn TorrentSession>,
    dl: &crate::download::model::Download,
    torrent_hash: &str,
) -> anyhow::Result<()> {
    let Some(client) = torrent else {
        return Ok(());
    };

    // Pull fresh stats so the DB uploaded/ratio are up to date.
    let Some(status) = client.get_status(torrent_hash) else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE download SET uploaded = ?, upload_speed = ?, seeders = ?, leechers = ? WHERE id = ?",
    )
    .bind(status.uploaded)
    .bind(status.upload_speed)
    .bind(status.seeders)
    .bind(status.leechers)
    .bind(dl.id)
    .execute(pool)
    .await?;

    let (ratio_limit, time_limit_min): (f64, i64) =
        sqlx::query_as("SELECT seed_ratio_limit, seed_time_limit FROM config WHERE id = 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or((1.0, 0));

    let downloaded = dl.size.unwrap_or(status.downloaded).max(1);
    let ratio = status.uploaded as f64 / downloaded as f64;

    let ratio_reached = ratio_limit > 0.0 && ratio >= ratio_limit;

    let time_reached = if time_limit_min > 0 {
        let started_at = dl
            .completed_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
        started_at.is_some_and(|t| {
            let elapsed = chrono::Utc::now().signed_duration_since(t.with_timezone(&chrono::Utc));
            elapsed.num_minutes() >= time_limit_min
        })
    } else {
        false
    };

    if ratio_reached || time_reached {
        let reason = if ratio_reached { "ratio" } else { "time" };
        tracing::info!(
            download_id = dl.id,
            ratio,
            ratio_limit,
            time_limit_min,
            reason,
            "seed target reached — stopping torrent and deleting source files"
        );
        // delete_files=true. Source files live under `download_path`;
        // the library already holds its own hardlinked/copied copy
        // via the import pipeline, so deleting the source is lossless.
        // On `use_hardlinks=1` setups this just removes the extra
        // directory entry (same inode); on copy setups it actually
        // frees disk.
        let outcome = crate::cleanup::CleanupTracker::new(pool.clone())
            .try_remove(
                crate::cleanup::ResourceKind::Torrent,
                torrent_hash,
                || async { client.remove(torrent_hash, true).await },
            )
            .await?;
        if !outcome.is_removed() {
            tracing::warn!(
                download_id = dl.id,
                ?outcome,
                "torrent removal queued for retry after seed target; state flip continues"
            );
        }
        sqlx::query(
            "UPDATE download SET state = 'cleaned_up', seed_target_reached_at = ?, upload_speed = 0
             WHERE id = ?",
        )
        .bind(crate::time::Timestamp::now().to_rfc3339())
        .bind(dl.id)
        .execute(pool)
        .await?;
    }
    Ok(())
}
