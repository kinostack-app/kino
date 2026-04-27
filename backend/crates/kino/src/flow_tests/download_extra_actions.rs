//! Less-trodden download endpoints — peers, pieces, file-select,
//! blocklist-and-search. All trip 404 paths cleanly when the download
//! id doesn't exist; full happy paths need a torrent client.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn peers_endpoint_404_for_missing_download() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/downloads/9999/peers").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pieces_endpoint_404_for_missing_download() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/downloads/9999/pieces").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn files_select_404_for_missing_download() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post(
            "/api/v1/downloads/9999/files/select",
            &json!({ "file_indices": [0] }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blocklist_and_search_404_for_missing_download() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/downloads/9999/blocklist-and-search", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
