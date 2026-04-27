//! Every `blocklist.torrent_info_hash` value is lowercase. The
//! match path compares case-insensitively, but a row written with
//! mixed case still trips equality-only consumers (admin search,
//! event payloads). Auto-repair candidate: `UPDATE … SET hash =
//! lower(hash)`.

use sqlx::SqlitePool;

use super::Violation;

pub const NAME: &str = "blocklist_hashes_normalized";
pub const DESCRIPTION: &str = "Every blocklist.torrent_info_hash is lowercase.";

pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, torrent_info_hash FROM blocklist
         WHERE torrent_info_hash IS NOT NULL
           AND torrent_info_hash != lower(torrent_info_hash)",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, hash)| Violation {
            invariant: NAME,
            detail: format!("blocklist id={id} hash={hash:?} is not lowercase"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::tests::fresh_pool;
    use super::*;

    async fn insert_movie(pool: &SqlitePool) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query_scalar(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at)
             VALUES (?, 'X', 1, 1, ?) RETURNING id",
        )
        .bind(rand::random::<i32>())
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn insert_blocklist(pool: &SqlitePool, hash: Option<&str>) -> i64 {
        let movie_id = insert_movie(pool).await;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query_scalar(
            "INSERT INTO blocklist (movie_id, source_title, torrent_info_hash, date)
             VALUES (?, 'X', ?, ?) RETURNING id",
        )
        .bind(movie_id)
        .bind(hash)
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn passes_when_all_hashes_lowercase_or_null() {
        let pool = fresh_pool().await;
        insert_blocklist(&pool, Some("abcdef0123")).await;
        insert_blocklist(&pool, None).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_on_uppercase_hash() {
        let pool = fresh_pool().await;
        let id = insert_blocklist(&pool, Some("ABCDEF")).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={id}")));
    }

    #[tokio::test]
    async fn fails_on_mixed_case_hash() {
        let pool = fresh_pool().await;
        insert_blocklist(&pool, Some("AbCdEf")).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
    }
}
