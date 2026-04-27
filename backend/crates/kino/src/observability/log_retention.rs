//! Log retention sweep — caps `log_entry` at a fixed row count and
//! opportunistically checkpoints WAL so the file doesn't grow unbounded.
//!
//! Runs from the `log_retention` scheduled task, default interval 1h.

use sqlx::SqlitePool;

/// Keep at most N rows (oldest dropped). Tweakable from config later;
/// for now 100k is ~25 MB of on-disk logs which is plenty and cheap.
pub const MAX_ROWS: i64 = 100_000;

/// Sweep old rows + checkpoint WAL. Returns rows deleted.
pub async fn sweep(pool: &SqlitePool) -> anyhow::Result<u64> {
    // Cap rows by id so we always keep the most recent. `MAX(id)` is O(1)
    // on an indexed PK.
    let deleted = sqlx::query(
        "DELETE FROM log_entry WHERE id <= (SELECT IFNULL(MAX(id), 0) - ? FROM log_entry)",
    )
    .bind(MAX_ROWS)
    .execute(pool)
    .await?
    .rows_affected();

    // Nightly-equivalent WAL truncation: cheap and keeps kino.db-wal
    // from growing unbounded on a busy logger.
    let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await;

    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn sweep_caps_row_count() {
        let pool = db::create_test_pool().await;

        // Insert MAX_ROWS + 50 rows.
        for i in 0..(MAX_ROWS + 50) {
            sqlx::query(
                "INSERT INTO log_entry (ts_us, level, target, message, source) VALUES (?, 2, 'test', ?, 'backend')",
            )
            .bind(i)
            .bind(format!("line {i}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let deleted = sweep(&pool).await.unwrap();
        assert_eq!(deleted, 50);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_entry")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, MAX_ROWS);
    }

    #[tokio::test]
    async fn sweep_noop_when_under_cap() {
        let pool = db::create_test_pool().await;
        for i in 0..10 {
            sqlx::query(
                "INSERT INTO log_entry (ts_us, level, target, message, source) VALUES (?, 2, 'test', ?, 'backend')",
            )
            .bind(i)
            .bind(format!("line {i}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let deleted = sweep(&pool).await.unwrap();
        assert_eq!(deleted, 0);
    }
}
