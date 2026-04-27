//! `DELETE /api/v1/movies/{id}` — cascades to `download_content` +
//! release rows. With no torrent client, the cancellation
//! side-effects are no-ops, but the DB cascade still runs.

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn delete_movie_404_for_missing_id() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/movies/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_movie_removes_release_and_download_rows() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (1, 603, 'matrix', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    // Seed a release tied to the movie + a download tied to the
    // release via download_content.
    sqlx::query(
        "INSERT INTO release (id, guid, movie_id, title, first_seen_at)
         VALUES (1, 'g', 1, 'rel', datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    let dl_id: i64 = sqlx::query_scalar(
        "INSERT INTO download (release_id, title, state, added_at)
         VALUES (1, 'rel', 'completed', datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, 1)")
        .bind(dl_id)
        .execute(&app.db)
        .await
        .unwrap();

    let resp = app.delete("/api/v1/movies/1").await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let movie_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM movie WHERE id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(movie_left, 0);

    let releases_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM release WHERE movie_id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(releases_left, 0, "releases cascade");

    let dl_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE id = ?")
        .bind(dl_id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(dl_left, 0, "download row removed by handler before cascade");
}
