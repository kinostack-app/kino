//! Follow conflicts: re-following an already-followed movie or show
//! must return 409, not silently re-create or 500. The Library page
//! relies on this to surface "already in your library" toasts.

use serde_json::json;

use crate::test_support::{MockTmdbServer, TestAppBuilder, assert_status};

#[tokio::test]
async fn following_movie_twice_returns_409() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let first = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    assert_status(&first, axum::http::StatusCode::CREATED);

    let second = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    assert_status(&second, axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn following_show_twice_returns_409() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_show(1399).await;
    tmdb.stub_season(1399, 1).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let first = app.post("/api/v1/shows", &json!({ "tmdb_id": 1399 })).await;
    assert_status(&first, axum::http::StatusCode::CREATED);

    let second = app.post("/api/v1/shows", &json!({ "tmdb_id": 1399 })).await;
    assert_status(&second, axum::http::StatusCode::CONFLICT);
}
