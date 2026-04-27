//! Every `media` row links to either a movie (via `media.movie_id`)
//! or to at least one episode (via `media_episode`). A media row
//! with neither link is orphan content — file on disk, no entity to
//! attribute it to. Pure-DB check; the filesystem-side orphan scan
//! lives in `cleanup::orphan_file_scan`.

use sqlx::SqlitePool;

use super::Violation;

pub const NAME: &str = "media_has_owner";
pub const DESCRIPTION: &str = "Every media row links to a movie or to at least one episode.";

pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT m.id, m.file_path FROM media m
         WHERE m.movie_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM media_episode WHERE media_id = m.id)",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, path)| Violation {
            invariant: NAME,
            detail: format!("media id={id} path={path:?} has neither movie_id nor episode link"),
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

    async fn insert_show_and_episode(pool: &SqlitePool) -> (i64, i64) {
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
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (show_id, season_number) VALUES (?, 1) RETURNING id",
        )
        .bind(show_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let ep_id: i64 = sqlx::query_scalar(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, acquire, in_scope)
             VALUES (?, ?, 1, 1, 1, 1) RETURNING id",
        )
        .bind(series_id)
        .bind(show_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (show_id, ep_id)
    }

    async fn insert_media(pool: &SqlitePool, movie_id: Option<i64>) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query_scalar(
            "INSERT INTO media (movie_id, file_path, relative_path, size, date_added)
             VALUES (?, '/x.mkv', 'x.mkv', 1, ?) RETURNING id",
        )
        .bind(movie_id)
        .bind(&now)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn passes_for_movie_media() {
        let pool = fresh_pool().await;
        let movie_id = insert_movie(&pool).await;
        insert_media(&pool, Some(movie_id)).await;
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn passes_for_episode_media_via_media_episode() {
        let pool = fresh_pool().await;
        let (_, ep_id) = insert_show_and_episode(&pool).await;
        let media_id = insert_media(&pool, None).await;
        sqlx::query("INSERT INTO media_episode (media_id, episode_id) VALUES (?, ?)")
            .bind(media_id)
            .bind(ep_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(check(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_for_orphan_media() {
        let pool = fresh_pool().await;
        let media_id = insert_media(&pool, None).await;
        let violations = check(&pool).await.unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains(&format!("id={media_id}")));
    }
}
