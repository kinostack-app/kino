//! Background trickplay generation — scans for media rows with
//! `trickplay_generated = 0` and builds sprite sheets + VTT, a few at a
//! time so the scheduler tick doesn't block for long.
//!
//! Called by the `trickplay_generation` scheduler task.

use std::path::PathBuf;
use std::sync::OnceLock;

use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::playback::trickplay::{self, Params, TrickplayError};
use crate::state::AppState;

/// Maximum media rows processed per sweep. Trickplay generation is
/// IO- and CPU-bound; one at a time is plenty — even on a fast host
/// a 4K file takes 1–2 minutes to sample, and we'd rather spread
/// that across several scheduler ticks than burn multiple cores.
const PER_SWEEP: i64 = 1;

/// Retry budget for transient trickplay failures. After N attempts
/// the sweep gives up and marks the row `trickplay_generated = 1`
/// so a genuinely-broken file doesn't loop every tick forever.
/// Three attempts is enough to ride through a brief IO / ffmpeg
/// blip while bounding the wasted-work window to three full probe +
/// decode passes per media row.
const MAX_ATTEMPTS: i64 = 3;

/// Process-wide mutex guarding against concurrent sweeps. The
/// scheduler tries hard to avoid this (see `claim_task`), but
/// startup reconciliation, manual triggers, and spawn/mark races
/// can still produce overlaps that would otherwise spawn N parallel
/// ffmpegs against the same media. This mutex is the defence in
/// depth — `try_lock` bails out fast when a sweep is already running.
fn sweep_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Find up to `PER_SWEEP` media rows that haven't had trickplay generated
/// yet and generate for each. On success, mark `trickplay_generated = 1`.
/// On failure (short clip, bad file, etc.) also mark done so we don't
/// keep re-trying the same broken file every tick.
pub async fn sweep(state: &AppState) -> anyhow::Result<u64> {
    let Ok(_guard) = sweep_lock().try_lock() else {
        tracing::debug!("trickplay sweep already running, skipping");
        return Ok(0);
    };
    // Don't compete with an active transcode — trickplay is
    // best-effort background work; a user watching a video gets
    // priority for the CPU. Resume on the next tick once the
    // session goes idle.
    if transcode_busy(state) {
        tracing::debug!("transcode active, deferring trickplay sweep");
        return Ok(0);
    }

    let pool = &state.db;
    let params = read_params(pool).await;

    // We hold the process-wide sweep mutex, so any row still at
    // `trickplay_generated = 2` must be a stale claim from a crashed
    // prior run (watchexec kill, panic, host shutdown). Reset it so
    // this sweep can pick it back up rather than leaving it stuck
    // forever. Startup reconcile handles the same case at boot time;
    // this covers in-process crashes that don't restart the server.
    let _ = sqlx::query("UPDATE media SET trickplay_generated = 0 WHERE trickplay_generated = 2")
        .execute(pool)
        .await?;

    // Claim rows atomically by flipping `trickplay_generated` from 0
    // to 2 ("in-progress") in the same UPDATE…RETURNING. Any other
    // sweep that sneaks past the mutex (different process, bad luck)
    // sees the rows as already claimed and picks different ones.
    // The in-progress value is never exposed via the API; the
    // playback endpoint gates on `= 1`. Failure handling happens
    // per-row below — permanent errors mark done (1), transient
    // errors bump `trickplay_attempts` and roll back to 0 so the
    // next sweep retries (up to `MAX_ATTEMPTS`).
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "UPDATE media SET trickplay_generated = 2
         WHERE id IN (
           SELECT id FROM media WHERE trickplay_generated = 0 LIMIT ?
         )
         RETURNING id, file_path, trickplay_attempts",
    )
    .bind(PER_SWEEP)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let mut done = 0u64;
    for (media_id, file_path, attempts) in rows {
        let path = PathBuf::from(&file_path);
        if !path.exists() {
            tracing::debug!(media_id, path = %file_path, "media file missing, skipping trickplay");
            mark_generated(pool, media_id).await;
            continue;
        }
        let out = trickplay::trickplay_dir(&state.data_path, media_id);

        match trickplay::generate(&path, &out, &params).await {
            Ok(sheets) => {
                tracing::info!(media_id, sheets, "trickplay generated");
                mark_generated(pool, media_id).await;
                done += 1;
            }
            Err(e) => {
                handle_failure(pool, media_id, attempts, &e).await;
            }
        }
    }
    Ok(done)
}

/// Route a failed trickplay generation to the right terminal
/// state. Permanent errors (see `TrickplayError::is_permanent`)
/// go straight to `trickplay_generated = 1` because no amount of
/// retrying will unbreak them. Transient errors bump
/// `trickplay_attempts`; once the counter reaches `MAX_ATTEMPTS`
/// we give up and mark done anyway, otherwise we roll the row
/// back to `trickplay_generated = 0` so the next sweep picks it
/// up.
async fn handle_failure(pool: &SqlitePool, media_id: i64, attempts: i64, err: &TrickplayError) {
    if err.is_permanent() {
        tracing::info!(
            media_id,
            error = %err,
            "trickplay permanent failure, marking done"
        );
        mark_generated(pool, media_id).await;
        return;
    }

    let next_attempts = attempts + 1;
    if next_attempts >= MAX_ATTEMPTS {
        tracing::warn!(
            media_id,
            attempts = next_attempts,
            error = %err,
            "trickplay giving up after max attempts, marking done"
        );
        if let Err(e) = sqlx::query(
            "UPDATE media SET trickplay_generated = 1, trickplay_attempts = ? WHERE id = ?",
        )
        .bind(next_attempts)
        .bind(media_id)
        .execute(pool)
        .await
        {
            tracing::warn!(media_id, error = %e, "failed to mark trickplay exhausted");
        }
        return;
    }

    tracing::warn!(
        media_id,
        attempts = next_attempts,
        max = MAX_ATTEMPTS,
        error = %err,
        "trickplay transient failure, scheduling retry"
    );
    if let Err(e) =
        sqlx::query("UPDATE media SET trickplay_generated = 0, trickplay_attempts = ? WHERE id = ?")
            .bind(next_attempts)
            .bind(media_id)
            .execute(pool)
            .await
    {
        tracing::warn!(media_id, error = %e, "failed to roll trickplay row back to pending");
    }
}

/// True when the transcode manager has any active session. Trickplay
/// sweeps defer in that case — a user watching is the priority,
/// background preview generation can wait a tick.
fn transcode_busy(state: &AppState) -> bool {
    state
        .transcode
        .as_ref()
        .is_some_and(crate::playback::transcode::TranscodeManager::has_active_sessions)
}

/// Reset any rows stuck at `trickplay_generated = 2` (sweep claim)
/// or `= 3` (stream-task post-import claim) to 0 so the next sweep
/// picks them up. Called on startup — a previous process may have
/// been killed mid-ffmpeg and left its claim dangling.
pub async fn reset_stale_in_progress(pool: &SqlitePool) -> anyhow::Result<u64> {
    let r =
        sqlx::query("UPDATE media SET trickplay_generated = 0 WHERE trickplay_generated IN (2, 3)")
            .execute(pool)
            .await?;
    Ok(r.rows_affected())
}

async fn mark_generated(pool: &SqlitePool, media_id: i64) {
    // Reset the attempts counter alongside marking done — a
    // future force-regenerate (e.g. after a params change) will
    // start fresh rather than inheriting an exhausted budget.
    // If this UPDATE fails we'd loop on the same file forever,
    // so surface it rather than swallow silently.
    if let Err(e) =
        sqlx::query("UPDATE media SET trickplay_generated = 1, trickplay_attempts = 0 WHERE id = ?")
            .bind(media_id)
            .execute(pool)
            .await
    {
        tracing::warn!(media_id, error = %e, "failed to mark trickplay_generated");
    }
}

async fn read_params(pool: &SqlitePool) -> Params {
    let ffmpeg: Option<String> = sqlx::query_scalar("SELECT ffmpeg_path FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    Params {
        ffmpeg_path: ffmpeg.clone().unwrap_or_else(|| "ffmpeg".into()),
        // Use the same dir as ffmpeg — ffprobe lives next to it in the
        // standard build. If the config only has a path to the bin dir
        // this falls back to the PATH.
        ffprobe_path: ffmpeg
            .as_ref()
            .map_or_else(|| "ffprobe".into(), |p| p.replace("ffmpeg", "ffprobe")),
        ..Params::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Insert a media row in `trickplay_generated = 2` (claimed)
    /// with a fresh attempts counter. Mirrors the state `sweep()`
    /// hands to `handle_failure` after claiming a row.
    async fn insert_claimed_media(pool: &SqlitePool, attempts: i64) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO media (file_path, relative_path, size, date_added, trickplay_generated, trickplay_attempts)
             VALUES ('/tmp/x.mkv', 'x.mkv', 1, '2026-01-01T00:00:00Z', 2, ?)
             RETURNING id",
        )
        .bind(attempts)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn read_trickplay_state(pool: &SqlitePool, media_id: i64) -> (i64, i64) {
        sqlx::query_as("SELECT trickplay_generated, trickplay_attempts FROM media WHERE id = ?")
            .bind(media_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn permanent_error_marks_done_and_resets_attempts() {
        let pool = db::create_test_pool().await;
        // Two prior transient attempts shouldn't block a permanent
        // classification from marking done immediately — a file
        // that's too short will never succeed, so we don't need
        // to wait out the retry budget.
        let media_id = insert_claimed_media(&pool, 2).await;
        handle_failure(&pool, media_id, 2, &TrickplayError::TooShort(3.0)).await;
        let (generated, attempts) = read_trickplay_state(&pool, media_id).await;
        assert_eq!(generated, 1, "permanent error should mark done");
        assert_eq!(
            attempts, 0,
            "permanent path routes through mark_generated which clears the counter"
        );
    }

    #[tokio::test]
    async fn transient_error_under_budget_rolls_back_to_pending() {
        let pool = db::create_test_pool().await;
        let media_id = insert_claimed_media(&pool, 0).await;
        handle_failure(&pool, media_id, 0, &TrickplayError::Io("disk blip".into())).await;
        let (generated, attempts) = read_trickplay_state(&pool, media_id).await;
        assert_eq!(
            generated, 0,
            "first transient failure should leave the row available for retry"
        );
        assert_eq!(attempts, 1, "attempts counter should bump");
    }

    #[tokio::test]
    async fn transient_error_at_budget_marks_done() {
        let pool = db::create_test_pool().await;
        // Two prior attempts already banked. The third transient
        // failure hits `next_attempts == MAX_ATTEMPTS` → give up.
        let media_id = insert_claimed_media(&pool, 2).await;
        handle_failure(&pool, media_id, 2, &TrickplayError::Ffmpeg("oom".into())).await;
        let (generated, attempts) = read_trickplay_state(&pool, media_id).await;
        assert_eq!(generated, 1, "exhausted budget should mark done");
        assert_eq!(
            attempts, MAX_ATTEMPTS,
            "exhaustion path preserves the final count so operators can spot loop-givers"
        );
    }

    #[tokio::test]
    async fn mark_generated_clears_attempts() {
        let pool = db::create_test_pool().await;
        let media_id = insert_claimed_media(&pool, 5).await;
        mark_generated(&pool, media_id).await;
        let (generated, attempts) = read_trickplay_state(&pool, media_id).await;
        assert_eq!(generated, 1);
        assert_eq!(attempts, 0, "success should reset the retry counter");
    }
}
