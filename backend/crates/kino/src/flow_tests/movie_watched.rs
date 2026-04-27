//! `POST / DELETE /api/v1/movies/{id}/watched` — mirror of the
//! episode watched flow but for movies.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn mark_missing_movie_watched_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/movies/9999/watched", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unmark_missing_movie_watched_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/movies/9999/watched").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mark_then_unmark_movie_round_trips_watched_at() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (1, 603, 'The Matrix', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let resp = app.post("/api/v1/movies/1/watched", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::OK);

    let watched: Option<String> = sqlx::query_scalar("SELECT watched_at FROM movie WHERE id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert!(watched.is_some(), "watched_at populated");

    let resp = app.delete("/api/v1/movies/1/watched").await;
    assert_status(&resp, axum::http::StatusCode::OK);

    let watched: Option<String> = sqlx::query_scalar("SELECT watched_at FROM movie WHERE id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert!(watched.is_none(), "watched_at cleared");
}
