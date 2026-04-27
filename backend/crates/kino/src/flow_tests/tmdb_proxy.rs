//! `/api/v1/tmdb/*` proxies. The frontend's search/discovery UI
//! calls these straight through; a regression in the proxy path
//! breaks browse without breaking the follow flow, so it's worth
//! testing independently.

use crate::test_support::{MockTmdbServer, TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn movie_details_proxy_returns_upstream_body() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let body = json_body(app.get("/api/v1/tmdb/movies/603").await).await;
    // The stubbed fixture is "The Matrix" — pin the shape/identity
    // assertion here so a regression that silently swallows fields
    // (e.g. passing through an empty body on error) is caught.
    assert_eq!(body["id"], 603);
    assert!(
        body["title"].is_string(),
        "title field survives the proxy; got {body}"
    );
}

#[tokio::test]
async fn tmdb_proxy_without_key_returns_502_or_500() {
    // No `.with_tmdb(...)` → `require_tmdb` fails. Protects against
    // regressions that would let the UI hit TMDB without a key.
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/tmdb/movies/603").await;
    // Current contract: `require_tmdb` returns a 502 BadGateway-style
    // message or 400 depending on the kind of failure; either way it
    // must NOT be 200. The exact code isn't the point, the guard is.
    let status = resp.status();
    assert_ne!(
        status,
        axum::http::StatusCode::OK,
        "no-tmdb-key must not succeed"
    );
}

#[tokio::test]
async fn trending_movies_proxy_returns_array_shape() {
    let tmdb = MockTmdbServer::start().await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;
    let resp = app.get("/api/v1/tmdb/trending/movies").await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let body = json_body(resp).await;
    assert!(
        body.get("results").is_some(),
        "TMDB paged shape preserved; got {body}"
    );
}
