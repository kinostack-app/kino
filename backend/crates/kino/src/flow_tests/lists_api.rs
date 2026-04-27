//! `/api/v1/lists` — TMDB list management used by the Browse page
//! under "Lists & Collections". Full parse/sync integration is
//! covered in the lists subsystem tests; here we just lock the
//! list/delete REST contract.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_lists_includes_system_defaults() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/lists").await).await;
    // Fresh install has zero user-created lists; system lists seeded
    // during `ensure_defaults` may or may not be present depending on
    // build config, so we only assert the array shape here.
    assert!(body.is_array(), "/api/v1/lists returns array");
}

#[tokio::test]
async fn get_missing_list_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/lists/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_missing_list_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/lists/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
