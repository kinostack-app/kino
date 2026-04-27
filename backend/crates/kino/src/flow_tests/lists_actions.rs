//! Lists action endpoints — refresh + items + ignore. The full
//! create flow needs network or seeded fixtures; here we cover
//! the cheap branches (404 on missing list).

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn refresh_missing_list_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/lists/9999/refresh", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_items_for_missing_list_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/lists/9999/items").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ignore_item_for_missing_list_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post(
            "/api/v1/lists/9999/items/1/ignore",
            &json!({ "ignored": true }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
