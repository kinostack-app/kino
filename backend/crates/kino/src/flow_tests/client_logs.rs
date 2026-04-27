//! `POST /api/v1/client-logs` — frontend log forwarder. The frontend
//! batches errors and pipes them in here; redaction happens at the
//! handler so we don't trust the client to scrub.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn client_logs_persist_into_log_entry() {
    let app = TestAppBuilder::new().build().await;

    let resp = app
        .post(
            "/api/v1/client-logs",
            &json!({
                "entries": [
                    { "level": "error", "message": "boom in vidstack" },
                    { "level": "warn",  "message": "fallback to direct play" },
                ]
            }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_entry WHERE source = 'frontend'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(count, 2, "both client log entries persisted");
}

#[tokio::test]
async fn client_logs_oversize_batch_rejected_422() {
    let app = TestAppBuilder::new().build().await;
    let entries: Vec<serde_json::Value> = (0..101)
        .map(|i| json!({ "level": "info", "message": format!("entry {i}") }))
        .collect();

    let resp = app
        .post("/api/v1/client-logs", &json!({ "entries": entries }))
        .await;
    assert_status(&resp, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn client_logs_url_appended_to_message() {
    let app = TestAppBuilder::new().build().await;
    app.post(
        "/api/v1/client-logs",
        &json!({
            "entries": [
                { "level": "error", "message": "render error", "url": "/library" },
            ]
        }),
    )
    .await;

    let msg: String =
        sqlx::query_scalar("SELECT message FROM log_entry WHERE source = 'frontend' LIMIT 1")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(
        msg.contains("/library"),
        "url is appended to message; got {msg}"
    );
}
