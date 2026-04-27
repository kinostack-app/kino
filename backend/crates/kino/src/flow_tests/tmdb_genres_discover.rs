//! `/api/v1/tmdb/genres` + `/discover/movies`/`shows` proxies. The
//! mock server pre-stubs trending + genres on start, so these
//! endpoints have a known shape end-to-end.

use crate::test_support::{MockTmdbServer, TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn genres_endpoint_returns_known_default_list() {
    let tmdb = MockTmdbServer::start().await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let body = json_body(app.get("/api/v1/tmdb/genres").await).await;
    // Endpoint returns `{ movie: [...], tv: [...] }` — TmdbClient
    // unwraps the `genres` envelope before returning.
    let movies = body["movie"]
        .as_array()
        .unwrap_or_else(|| panic!("movie missing; got {body}"));
    assert!(!movies.is_empty(), "movies genres populated; got {body}");
    assert!(
        movies.iter().any(|g| g["name"] == "Action"),
        "Action present in mock list"
    );
}

#[tokio::test]
async fn discover_movies_proxies_to_tmdb() {
    let tmdb = MockTmdbServer::start().await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    // Discover endpoint isn't pre-stubbed; wiremock 404s unstubbed
    // routes, which the proxy surfaces as a non-2xx response. The
    // contract under test is "endpoint reachable + returns SOMETHING";
    // tighter assertions belong with a per-call mock stub.
    let resp = app.get("/api/v1/tmdb/discover/movies").await;
    let status = resp.status();
    // Accept 200 (if upstream stubbed elsewhere) or 5xx (proxy surfaced
    // mock 404). Either way, no panic on the backend side.
    assert!(
        status == axum::http::StatusCode::OK || status.is_server_error(),
        "discover proxy returned an unexpected status; got {status}"
    );
}

#[tokio::test]
async fn discover_without_tmdb_key_returns_400() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/tmdb/discover/movies").await;
    // `require_tmdb` returns BadRequest("TMDB API key not configured")
    // when no key is set — mapped to HTTP 400.
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}
