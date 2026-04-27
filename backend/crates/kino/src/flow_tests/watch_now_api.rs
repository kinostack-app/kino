//! `POST /api/v1/watch-now` — single entry point for all four
//! watch-now variants (direct movie / episode / episode-by-tmdb /
//! smart-show). The happy paths need a full grab+indexer setup and
//! are covered end-to-end in `grab_to_import`; this file locks the
//! input-validation boundaries.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn watch_now_on_unknown_movie_errors_cleanly() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post(
            "/api/v1/watch-now",
            &json!({ "kind": "movie", "tmdb_id": 99_999_999 }),
        )
        .await;
    // Exact code is a judgment call (404 vs 409 vs 502) depending on
    // where in the flow it fails — the key assertion is "not 200".
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::OK,
        "unknown movie must not return a successful reply"
    );
}

#[tokio::test]
async fn watch_now_on_unknown_episode_id_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post(
            "/api/v1/watch-now",
            &json!({ "kind": "episode", "episode_id": 9999 }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn watch_now_with_bad_request_body_is_rejected() {
    let app = TestAppBuilder::new().build().await;
    // Missing `kind` discriminator.
    let resp = app
        .post("/api/v1/watch-now", &json!({ "foo": "bar" }))
        .await;
    assert_status(&resp, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}
