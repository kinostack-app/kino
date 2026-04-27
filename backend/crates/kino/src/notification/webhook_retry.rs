//! Webhook retry — re-enables webhook targets whose backoff has expired.
//!
//! The webhook delivery path sets `disabled_until` to a future time on
//! failure, which skips the target in subsequent `should_fire` lookups
//! via the `disabled_until IS NULL OR disabled_until < datetime('now')`
//! filter. This periodic task clears expired entries so the next event
//! tries delivery again.

use sqlx::SqlitePool;

/// Clear `disabled_until` on webhooks whose backoff has expired.
/// Returns the number of rows re-enabled.
pub async fn retry_sweep(pool: &SqlitePool) -> anyhow::Result<u64> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE webhook_target
             SET disabled_until = NULL
         WHERE disabled_until IS NOT NULL AND disabled_until < ?",
    )
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn clears_only_expired_backoffs() {
        let pool = db::create_test_pool().await;

        let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();

        sqlx::query(
            "INSERT INTO webhook_target (name, url, disabled_until) VALUES ('expired', 'http://x', ?)",
        )
        .bind(&past)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO webhook_target (name, url, disabled_until) VALUES ('still-backing-off', 'http://x', ?)",
        )
        .bind(&future)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("INSERT INTO webhook_target (name, url) VALUES ('healthy', 'http://x')")
            .execute(&pool)
            .await
            .unwrap();

        let cleared = retry_sweep(&pool).await.unwrap();
        assert_eq!(cleared, 1);

        let expired: Option<String> =
            sqlx::query_scalar("SELECT disabled_until FROM webhook_target WHERE name = 'expired'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(expired.is_none(), "expected disabled_until to be cleared");

        let still: Option<String> = sqlx::query_scalar(
            "SELECT disabled_until FROM webhook_target WHERE name = 'still-backing-off'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(still.is_some(), "future backoff should remain");
    }
}
