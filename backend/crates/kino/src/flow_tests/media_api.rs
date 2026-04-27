//! `/api/v1/media` — the list / by-id / delete endpoints powering
//! the file-level view under each movie or episode. Real import is
//! covered by `grab_to_import`; here we lock the REST shape.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_media_empty_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/media").await).await;
    let results = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("results array in PaginatedResponse envelope");
    assert!(results.is_empty(), "fresh install has no media");
    assert_eq!(
        body.get("has_more").and_then(serde_json::Value::as_bool),
        Some(false),
    );
}

#[tokio::test]
async fn get_missing_media_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/media/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_missing_media_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/media/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn filter_by_movie_id_narrows_list() {
    let app = TestAppBuilder::new().build().await;

    // Seed: one movie, one media row attached to it, and a second
    // orphan media row to prove the filter excludes it.
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (1, 111, 'fake', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media (movie_id, file_path, relative_path, size, date_added)
         VALUES (1, '/tmp/a.mkv', 'a.mkv', 1, datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media (movie_id, file_path, relative_path, size, date_added)
         VALUES (NULL, '/tmp/b.mkv', 'b.mkv', 2, datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let filtered = json_body(app.get("/api/v1/media?movie_id=1").await).await;
    let filtered_results = filtered
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(
        filtered_results.len(),
        1,
        "movie_id filter returns just the attached row",
    );

    let unfiltered = json_body(app.get("/api/v1/media").await).await;
    let unfiltered_results = unfiltered
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(unfiltered_results.len(), 2, "no filter → both rows");
}
