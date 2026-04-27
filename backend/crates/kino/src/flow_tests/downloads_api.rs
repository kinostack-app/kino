//! `/api/v1/downloads` — list + per-id get/delete. Covers the
//! dashboard-query path without needing to drive an actual grab
//! (which is already covered in `grab_to_import`). The goal is to
//! lock the JSON contract the Activity page depends on.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_downloads_empty_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/downloads").await).await;
    let r = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert!(r.is_empty());
}

#[tokio::test]
async fn get_missing_download_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/downloads/999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_missing_download_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/downloads/999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_downloads_preserves_state_column() {
    let app = TestAppBuilder::new().build().await;

    // Insert a queued download without going through the grab path so
    // the test stays focused on the list endpoint's shape.
    sqlx::query(
        "INSERT INTO download (title, state, added_at)
         VALUES ('fake release', 'queued', datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let body = json_body(app.get("/api/v1/downloads").await).await;
    let rows = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(rows.len(), 1, "list picks up the seeded row");
    assert_eq!(rows[0]["state"], "queued", "state round-trips through JSON");
}
