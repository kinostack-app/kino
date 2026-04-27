//! Every download in the `imported` state has at least one linked
//! media row (movie via `media.movie_id`, or episode via
//! `media_episode.episode_id`). A violation means the import path
//! committed the state transition without persisting the media row
//! — playback would later 404.

use sqlx::SqlitePool;

use super::Violation;

pub const NAME: &str = "imported_has_media";
pub const DESCRIPTION: &str =
    "Every download in the imported state has at least one linked media row.";

pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT d.id, d.title
         FROM download d
         WHERE d.state = 'imported'
           AND NOT EXISTS (
               SELECT 1 FROM download_content dc
               LEFT JOIN media m ON m.movie_id IS NOT NULL AND m.movie_id = dc.movie_id
               LEFT JOIN media_episode me ON me.episode_id = dc.episode_id
               WHERE dc.download_id = d.id
                 AND (m.id IS NOT NULL OR me.id IS NOT NULL)
           )",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, title)| Violation {
            invariant: NAME,
            detail: format!("download id={id} title={title:?} is imported but has no media row"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::tests::fresh_pool;
    use super::*;

    async fn insert_imported_download(pool: &SqlitePool, with_media: bool) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        let movie_id: i64 = sqlx::query_scalar(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at)
             VALUES (?, 'X', 1, 1, ?) RETURNING id",
        )
        .bind(rand::random::<i32>())
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap();
        let dl_id: i64 = sqlx::query_scalar(
            "INSERT INTO download (title, state, added_at) VALUES ('X', 'imported', ?) RETURNING id",
        )
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, ?)")
            .bind(dl_id)
            .bind(movie_id)
            .execute(pool)
            .await
            .unwrap();
        if with_media {
            sqlx::query(
                "INSERT INTO media (movie_id, file_path, relative_path, size, date_added)
                 VALUES (?, '/x.mkv', 'x.mkv', 1, ?)",
            )
            .bind(movie_id)
            .bind(&now)
            .execute(pool)
            .await
            .unwrap();
        }
        dl_id
    }

    #[tokio::test]
    async fn passes_when_imported_download_has_media() {
        let pool = fresh_pool().await;
        insert_imported_download(&pool, true).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_when_imported_download_missing_media() {
        let pool = fresh_pool().await;
        let dl_id = insert_imported_download(&pool, false).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={dl_id}")));
        assert_eq!(violations[0].invariant, NAME);
    }
}
