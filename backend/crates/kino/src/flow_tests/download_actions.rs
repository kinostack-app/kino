//! Download lifecycle action endpoints — pause, resume, retry,
//! blocklist-and-search. Each one has a state-machine guard that
//! rejects the wrong starting state with a 400. The happy paths
//! exercise librqbit; here we test the rejection branches and the
//! 404 paths.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn pause_missing_download_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/downloads/9999/pause", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resume_missing_download_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/downloads/9999/resume", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn retry_missing_download_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/downloads/9999/retry", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pause_queued_download_rejected_400() {
    let app = TestAppBuilder::new().build().await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, added_at)
         VALUES ('fake', 'queued', datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    let resp = app
        .post(&format!("/api/v1/downloads/{id}/pause"), &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn resume_non_paused_download_rejected_400() {
    let app = TestAppBuilder::new().build().await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, added_at)
         VALUES ('fake', 'queued', datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    let resp = app
        .post(&format!("/api/v1/downloads/{id}/resume"), &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn retry_succeeds_on_failed_download() {
    let app = TestAppBuilder::new().build().await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, added_at, error_message, torrent_hash)
         VALUES ('fake', 'failed', datetime('now'), 'oops', 'hash123') RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    let resp = app
        .post(&format!("/api/v1/downloads/{id}/retry"), &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::OK);

    let (state, err, hash): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT state, error_message, torrent_hash FROM download WHERE id = ?")
            .bind(id)
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(state, "queued", "retry resets state to queued");
    assert!(err.is_none(), "retry clears error_message");
    assert!(hash.is_none(), "retry clears stale torrent_hash");
}
