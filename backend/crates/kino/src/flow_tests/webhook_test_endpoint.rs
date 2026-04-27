//! `POST /api/v1/webhooks/{id}/test` — fires a synthetic event at
//! the configured URL. The endpoint always returns 200 and surfaces
//! ok/false-with-message via the body so the UI can render either
//! a green check or a red toast without branching on HTTP status.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn test_webhook_with_unreachable_url_reports_failure_in_body() {
    let app = TestAppBuilder::new().build().await;

    let created = json_body(
        app.post(
            "/api/v1/webhooks",
            &json!({
                "name": "broken",
                "url": "http://127.0.0.1:1/never-listening",
            }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    let resp = app
        .post(&format!("/api/v1/webhooks/{id}/test"), &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let body = json_body(resp).await;

    assert_eq!(body["ok"], false, "unreachable URL → ok:false");
    assert!(
        body["status_code"].is_null(),
        "no HTTP response → status_code is null; got {body}"
    );
    assert!(
        body["duration_ms"].is_i64(),
        "duration_ms is always reported"
    );
    assert!(
        !body["message"].as_str().unwrap_or("").is_empty(),
        "message describes the failure"
    );
}

#[tokio::test]
async fn test_missing_webhook_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/webhooks/9999/test", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
