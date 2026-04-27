//! Trakt action endpoints — disconnect (idempotent), dry-run, and
//! sync-now. Without OAuth credentials configured, dry-run + sync
//! return 400 (Trakt client can't be built) and disconnect returns
//! 204 (idempotent — clears whatever is there).

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn disconnect_is_idempotent_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/integrations/trakt/disconnect", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn dry_run_without_credentials_returns_400() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/integrations/trakt/dry-run").await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sync_now_without_credentials_returns_400() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/integrations/trakt/sync", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}
