//! `GET /api/v1/library/search` — cross-media substring search over
//! followed content. Drives the global command palette + the Library
//! page search box.

use serde_json::json;

use crate::test_support::{MockTmdbServer, TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn empty_library_returns_no_hits() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/library/search?q=matrix").await).await;
    assert!(body.is_array(), "returns array; got {body}");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn missing_query_parameter_is_a_400() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/library/search?q=").await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn followed_movie_is_findable_by_substring() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;

    let body = json_body(app.get("/api/v1/library/search?q=matrix").await).await;
    let hits = body.as_array().expect("array");
    assert!(
        !hits.is_empty(),
        "followed movie must match its own title substring; got {body}"
    );
    assert_eq!(hits[0]["item_type"], "movie");
    assert_eq!(hits[0]["status"], "wanted", "newly-followed → wanted phase");
}
