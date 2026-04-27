//! `POST /api/v1/episodes/{id}/redownload` — wipes media + clears
//! `last_searched_at` so the next sweep finds an upgrade. Used by the
//! "Replace" overflow on episode rows.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn redownload_missing_episode_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/episodes/9999/redownload", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
