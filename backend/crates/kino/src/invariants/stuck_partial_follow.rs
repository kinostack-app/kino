//! A `show` row stuck at `partial = 1` for longer than the safe
//! window. The follow flow inserts the show with `partial = 1`,
//! fans out seasons + episodes from TMDB, then flips
//! `partial = 0`. A stuck partial means the fanout crashed; the
//! row is invisible to user reads (every list filters
//! `partial = 0`) but it still exists. Surface it so the operator
//! can retry or remove.

use sqlx::SqlitePool;

use super::Violation;

pub const NAME: &str = "stuck_partial_follow";
pub const DESCRIPTION: &str = "Every partial=1 show row is younger than the stuck threshold.";

/// Window after which a `partial = 1` row counts as stuck. The
/// fanout normally takes ~1-3 seconds (one TMDB season call per
/// season); 10 minutes is comfortably past any retry the create
/// path itself might do.
const STUCK_AFTER_MINUTES: i64 = 10;

pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, title, added_at FROM show
         WHERE partial = 1
           AND datetime(added_at) < datetime('now', ?)",
    )
    .bind(format!("-{STUCK_AFTER_MINUTES} minutes"))
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, title, added_at)| Violation {
            invariant: NAME,
            detail: format!("show id={id} title={title:?} stuck partial=1 since {added_at}"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::tests::fresh_pool;
    use super::*;

    async fn insert_show(pool: &SqlitePool, partial: bool, added_at: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, monitor_new_items, added_at, partial)
             VALUES (?, 'X', 1, 1, 'future', ?, ?) RETURNING id",
        )
        .bind(rand::random::<i32>())
        .bind(added_at)
        .bind(i64::from(partial))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn passes_when_no_partial_rows() {
        let pool = fresh_pool().await;
        let now = chrono::Utc::now().to_rfc3339();
        insert_show(&pool, false, &now).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn passes_when_partial_is_recent() {
        let pool = fresh_pool().await;
        let now = chrono::Utc::now().to_rfc3339();
        insert_show(&pool, true, &now).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_when_partial_is_stuck() {
        let pool = fresh_pool().await;
        let stuck_at = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        let id = insert_show(&pool, true, &stuck_at).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={id}")));
    }
}
