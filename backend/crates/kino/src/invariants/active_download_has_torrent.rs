//! Every download in a state that needs librqbit-side anchoring
//! (`needs_startup_reconcile`) has a non-empty `torrent_hash`.
//! A violation means the row claims an active or recoverable
//! download but holds no handle to the torrent client.

use sqlx::SqlitePool;

use super::Violation;
use crate::download::DownloadPhase;

pub const NAME: &str = "active_download_has_torrent";
pub const DESCRIPTION: &str =
    "Every download in a librqbit-anchored state has a non-empty torrent_hash.";

pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
    let states = DownloadPhase::sql_in_clause(DownloadPhase::needs_startup_reconcile);
    let sql = format!(
        "SELECT id, title, state FROM download
         WHERE state IN ({states})
           AND (torrent_hash IS NULL OR torrent_hash = '')"
    );
    let rows: Vec<(i64, String, String)> = sqlx::query_as(&sql).fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|(id, title, state)| Violation {
            invariant: NAME,
            detail: format!("download id={id} title={title:?} state={state} has no torrent_hash"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::tests::fresh_pool;
    use super::*;

    async fn insert_download(pool: &SqlitePool, state: &str, hash: Option<&str>) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query_scalar(
            "INSERT INTO download (title, state, torrent_hash, added_at)
             VALUES ('X', ?, ?, ?) RETURNING id",
        )
        .bind(state)
        .bind(hash)
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn passes_when_active_downloads_have_hash() {
        let pool = fresh_pool().await;
        insert_download(&pool, "downloading", Some("aaaa")).await;
        insert_download(&pool, "imported", Some("bbbb")).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn passes_when_inactive_downloads_have_no_hash() {
        let pool = fresh_pool().await;
        // `searching` and `queued` aren't in `needs_startup_reconcile`,
        // so a NULL hash there is fine.
        insert_download(&pool, "searching", None).await;
        insert_download(&pool, "queued", None).await;
        insert_download(&pool, "cleaned_up", None).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_when_active_download_missing_hash() {
        let pool = fresh_pool().await;
        let id = insert_download(&pool, "downloading", None).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={id}")));
    }

    #[tokio::test]
    async fn fails_when_active_download_has_empty_hash() {
        let pool = fresh_pool().await;
        let id = insert_download(&pool, "imported", Some("")).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={id}")));
    }
}
