//! Catch-all for endpoints whose only interesting bit (in tests
//! without a real torrent client / definitions loader / etc.) is the
//! cheap rejection branch: 404 for missing resources, 400 for
//! missing prerequisites.

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn indexer_definitions_not_loaded_returns_400() {
    let app = TestAppBuilder::new().build().await;
    // No `DefinitionLoader` installed in tests → `require_definitions`
    // returns BadRequest, the UI uses this to render the
    // "definitions still loading" placeholder.
    let resp = app.get("/api/v1/indexer-definitions").await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn media_streams_404_for_missing_media() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/media/9999/streams").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn media_streams_returns_array_for_known_media_with_no_streams() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO media (id, file_path, relative_path, size, date_added)
         VALUES (1, '/tmp/m.mkv', 'm.mkv', 100, datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let resp = app.get("/api/v1/media/1/streams").await;
    assert_status(&resp, axum::http::StatusCode::OK);
}

#[tokio::test]
async fn redownload_episode_404_path() {
    use serde_json::json;
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/episodes/9999/redownload", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
