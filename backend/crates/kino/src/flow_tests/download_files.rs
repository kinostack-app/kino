//! `GET /api/v1/downloads/{id}/files` — multi-file picker for the
//! "Files" sub-pane of a download. Without a torrent client (test
//! mode default), returns `metadata_pending: true` with no files.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn download_files_404_for_missing_id() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/downloads/9999/files").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_files_pending_when_no_torrent_client() {
    let app = TestAppBuilder::new().build().await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, added_at, torrent_hash)
         VALUES ('fake', 'downloading', datetime('now'), 'abc') RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    let body = json_body(app.get(&format!("/api/v1/downloads/{id}/files")).await).await;
    assert_eq!(body["metadata_pending"], true, "no client → pending");
    assert_eq!(body["files"].as_array().unwrap().len(), 0);
}
