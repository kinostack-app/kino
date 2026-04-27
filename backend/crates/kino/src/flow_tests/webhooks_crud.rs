//! Webhook target CRUD. The delivery path hits the network and has
//! its own unit tests in `notification::webhook`; here we lock the
//! REST contract that drives the Settings → Notifications UI.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn empty_webhook_list_returns_array() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/webhooks").await).await;
    assert!(body.is_array(), "webhooks is an array; got {body}");
    assert_eq!(body.as_array().unwrap().len(), 0, "fresh install → empty");
}

#[tokio::test]
async fn create_webhook_defaults_fill_in_toggles() {
    let app = TestAppBuilder::new().build().await;

    // Only the two required fields — name + url. The defaults for the
    // on_* toggles are part of the contract: grab/complete/import/
    // upgrade/failure/health ON, watched OFF.
    let created = json_body(
        app.post(
            "/api/v1/webhooks",
            &json!({ "name": "discord", "url": "http://example.invalid/hook" }),
        )
        .await,
    )
    .await;
    assert_eq!(created["name"], "discord");
    assert_eq!(created["method"], "POST");
    assert_eq!(created["on_grab"], true);
    assert_eq!(created["on_download_complete"], true);
    assert_eq!(created["on_import"], true);
    assert_eq!(created["on_upgrade"], true);
    assert_eq!(created["on_failure"], true);
    assert_eq!(created["on_health_issue"], true);
    assert_eq!(created["on_watched"], false);

    let listed = json_body(app.get("/api/v1/webhooks").await).await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "list picks up create");
}

#[tokio::test]
async fn update_webhook_partial_replaces_only_given_fields() {
    let app = TestAppBuilder::new().build().await;
    let created = json_body(
        app.post(
            "/api/v1/webhooks",
            &json!({ "name": "old", "url": "http://example.invalid/a" }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_i64().expect("created id");

    // Rename only; URL + toggles must survive. Confirms the COALESCE
    // pattern works from the handler end (not just the SQL).
    let updated = json_body(
        app.put(&format!("/api/v1/webhooks/{id}"), &json!({ "name": "new" }))
            .await,
    )
    .await;
    assert_eq!(updated["name"], "new");
    assert_eq!(
        updated["url"], "http://example.invalid/a",
        "URL preserved across partial update"
    );
}

#[tokio::test]
async fn delete_webhook_returns_404_on_missing() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/webhooks/999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_webhook_removes_from_list() {
    let app = TestAppBuilder::new().build().await;
    let created = json_body(
        app.post(
            "/api/v1/webhooks",
            &json!({ "name": "tmp", "url": "http://example.invalid/x" }),
        )
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    let resp = app.delete(&format!("/api/v1/webhooks/{id}")).await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let listed = json_body(app.get("/api/v1/webhooks").await).await;
    assert_eq!(listed.as_array().unwrap().len(), 0, "gone after delete");
}
