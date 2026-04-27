//! `/api/v1/config` — GET, PUT (partial via COALESCE), rotate-api-key.
//! Config is the Settings page's data model; lock down the public
//! JSON contract so a regression here doesn't silently break the UI.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn get_config_returns_non_empty_api_key() {
    let app = TestAppBuilder::new().build().await;
    let cfg = json_body(app.get("/api/v1/config").await).await;
    let key = cfg["api_key"].as_str().expect("api_key is a string");
    assert!(!key.is_empty(), "ensure_defaults seeds a deterministic key");
}

#[tokio::test]
async fn put_config_partial_update_survives_round_trip() {
    let app = TestAppBuilder::new().build().await;
    let before = json_body(app.get("/api/v1/config").await).await;
    let before_media_path = before["media_library_path"].as_str().unwrap().to_owned();

    // Touch a single scalar: max_concurrent_downloads.
    let patched = json_body(
        app.put("/api/v1/config", &json!({ "max_concurrent_downloads": 7 }))
            .await,
    )
    .await;
    assert_eq!(patched["max_concurrent_downloads"], 7);
    assert_eq!(
        patched["media_library_path"].as_str().unwrap(),
        before_media_path,
        "unrelated field preserved"
    );
}

#[tokio::test]
async fn rotate_api_key_replaces_current_key() {
    let app = TestAppBuilder::new().build().await;
    let before = json_body(app.get("/api/v1/config").await).await;
    let old = before["api_key"].as_str().unwrap().to_owned();

    let resp = app.post("/api/v1/config/rotate-api-key", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let rotated = json_body(resp).await;
    let new_key = rotated["api_key"].as_str().unwrap().to_owned();

    assert_ne!(old, new_key, "rotate actually mints a new key");
    assert!(
        !new_key.is_empty(),
        "rotated key is populated, got {new_key}"
    );
}
