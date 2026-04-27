//! Every committed `show` row has at least one `series` (season).
//! Shows with `partial = 1` (mid-fanout) are excluded — they're
//! expected to be season-less until the `create_show` flow flips
//! `partial = 0`. Persistent partials are caught by the
//! `stuck_partial_follow` invariant instead.

use sqlx::SqlitePool;

use super::Violation;

pub const NAME: &str = "show_has_seasons";
pub const DESCRIPTION: &str = "Every committed show row has at least one series row.";

pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT s.id, s.title FROM show s
         WHERE s.partial = 0
           AND NOT EXISTS (SELECT 1 FROM series WHERE show_id = s.id)",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, title)| Violation {
            invariant: NAME,
            detail: format!("show id={id} title={title:?} has no seasons"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::tests::fresh_pool;
    use super::*;

    async fn insert_show(pool: &SqlitePool, with_season: bool) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        let show_id: i64 = sqlx::query_scalar(
            "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, monitor_new_items, added_at)
             VALUES (?, 'X', 1, 1, 'future', ?) RETURNING id",
        )
        .bind(rand::random::<i32>())
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap();
        if with_season {
            sqlx::query("INSERT INTO series (show_id, season_number) VALUES (?, 1)")
                .bind(show_id)
                .execute(pool)
                .await
                .unwrap();
        }
        show_id
    }

    #[tokio::test]
    async fn passes_when_show_has_seasons() {
        let pool = fresh_pool().await;
        insert_show(&pool, true).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_when_show_has_no_seasons() {
        let pool = fresh_pool().await;
        let id = insert_show(&pool, false).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={id}")));
    }
}
