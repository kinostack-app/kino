//! Watch progress tracking with smart thresholds.

use sqlx::SqlitePool;

/// Update playback position for a movie or episode.
///
/// Threshold logic:
/// - < 5% of runtime: reset to 0 (didn't really start)
/// - 5-80%: save position for resume
/// - >= 80%: mark as watched (set watched_at, increment play_count, reset position)
///
/// 80% matches Trakt's auto-mark-watched threshold. Earlier versions
/// used 90%, which opened a gap: a user abandoning at 85% wouldn't
/// trigger our local watched flag, which meant we never emitted
/// `/scrobble/stop` to Trakt — so Trakt didn't mark it watched
/// either, despite its *own* policy being to do so at ≥80%. Aligning
/// closes the gap in one place.
pub async fn update_progress(
    pool: &SqlitePool,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
    position_ticks: i64,
    runtime_ticks: Option<i64>,
) -> anyhow::Result<WatchAction> {
    let now = crate::time::Timestamp::now().to_rfc3339();

    let action = if let Some(total) = runtime_ticks {
        if total > 0 {
            #[allow(clippy::cast_precision_loss)]
            let pct = (position_ticks as f64) / (total as f64);
            if pct < 0.05 {
                WatchAction::Reset
            } else if pct >= 0.80 {
                WatchAction::MarkWatched
            } else {
                WatchAction::SavePosition
            }
        } else {
            WatchAction::SavePosition
        }
    } else {
        WatchAction::SavePosition
    };

    match action {
        WatchAction::Reset => {
            if let Some(mid) = movie_id {
                sqlx::query(
                    "UPDATE movie SET playback_position_ticks = 0, last_played_at = ? WHERE id = ?",
                )
                .bind(&now)
                .bind(mid)
                .execute(pool)
                .await?;
            }
            if let Some(eid) = episode_id {
                sqlx::query(
                    "UPDATE episode SET playback_position_ticks = 0, last_played_at = ? WHERE id = ?",
                )
                .bind(&now)
                .bind(eid)
                .execute(pool)
                .await?;
            }
        }
        WatchAction::SavePosition => {
            if let Some(mid) = movie_id {
                sqlx::query(
                    "UPDATE movie SET playback_position_ticks = ?, last_played_at = ? WHERE id = ?",
                )
                .bind(position_ticks)
                .bind(&now)
                .bind(mid)
                .execute(pool)
                .await?;
            }
            if let Some(eid) = episode_id {
                sqlx::query(
                    "UPDATE episode SET playback_position_ticks = ?, last_played_at = ? WHERE id = ?",
                )
                .bind(position_ticks)
                .bind(&now)
                .bind(eid)
                .execute(pool)
                .await?;
            }
        }
        WatchAction::MarkWatched => {
            // `watched_at` is the source of truth for the watched
            // phase. No separate `status` column to flip.
            if let Some(mid) = movie_id {
                sqlx::query(
                    "UPDATE movie SET playback_position_ticks = 0, play_count = play_count + 1, watched_at = ?, last_played_at = ? WHERE id = ?",
                )
                .bind(&now)
                .bind(&now)
                .bind(mid)
                .execute(pool)
                .await?;
            }
            if let Some(eid) = episode_id {
                sqlx::query(
                    "UPDATE episode SET playback_position_ticks = 0, play_count = play_count + 1, watched_at = ?, last_played_at = ? WHERE id = ?",
                )
                .bind(&now)
                .bind(&now)
                .bind(eid)
                .execute(pool)
                .await?;
            }
        }
    }

    Ok(action)
}

/// Result of progress update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAction {
    /// Position < 5%: reset to 0.
    Reset,
    /// Position 5-80%: saved for resume.
    SavePosition,
    /// Position >= 80%: marked as watched. Matches Trakt's threshold.
    MarkWatched,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn progress_reset_under_5_percent() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        // Insert a movie with runtime
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at) VALUES (1, 'Test', 1, 1, '2026-01-01') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let runtime = 7_200_000_000i64; // 2 hours in ticks
        let position = 100_000_000i64; // ~1.4% of runtime

        let action = update_progress(&pool, Some(id), None, position, Some(runtime))
            .await
            .unwrap();
        assert_eq!(action, WatchAction::Reset);

        // Verify position was reset
        let pos: i64 = sqlx::query_scalar("SELECT playback_position_ticks FROM movie WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pos, 0);
    }

    #[tokio::test]
    async fn progress_save_in_middle() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at) VALUES (2, 'Test2', 1, 1, '2026-01-01') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let runtime = 7_200_000_000i64;
        let position = 3_600_000_000i64; // 50%

        let action = update_progress(&pool, Some(id), None, position, Some(runtime))
            .await
            .unwrap();
        assert_eq!(action, WatchAction::SavePosition);

        let pos: i64 = sqlx::query_scalar("SELECT playback_position_ticks FROM movie WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pos, position);
    }

    #[tokio::test]
    async fn progress_mark_watched_at_trakt_threshold() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at) VALUES (3, 'Test3', 1, 1, '2026-01-01') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let runtime = 7_200_000_000i64;
        let position = 6_600_000_000i64; // ~91.7%

        let action = update_progress(&pool, Some(id), None, position, Some(runtime))
            .await
            .unwrap();
        assert_eq!(action, WatchAction::MarkWatched);

        // Verify watched_at was set (derived phase = watched) and
        // play_count incremented.
        let watched_at: Option<String> =
            sqlx::query_scalar("SELECT watched_at FROM movie WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(watched_at.is_some());

        let count: i64 = sqlx::query_scalar("SELECT play_count FROM movie WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
