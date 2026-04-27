//! `GET /api/v1/images/{content_type}/{id}/logo` — subsystem 29.
//!
//! The endpoint has three cache states:
//!   - `logo_path` populated → serve from disk
//!   - `logo_path = ""` → negative-cache, return 404 immediately
//!   - `logo_path` NULL → lazy-fetch from TMDB
//!
//! Tests here cover the cached round-trip, lazy-fetch writing the
//! sentinel when TMDB has no logos, and the 404-without-refetch
//! behaviour on a sentinel row.

use crate::test_support::{MockTmdbServer, TestAppBuilder, assert_status};

#[tokio::test]
async fn logo_endpoint_streams_cached_svg_with_correct_mime() {
    let app = TestAppBuilder::new().build().await;

    // Pre-seed: movie row with a `logo_path` pointing at a file we
    // lay down by hand. The test app's `data_path` lives under
    // `/tmp/kino-test`; the served file resolves to that root.
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at, logo_path, logo_palette)
         VALUES (1, 603, 'matrix', (SELECT id FROM quality_profile LIMIT 1), datetime('now'),
                 'logos/movie/603.svg', 'mono')",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let dir = app.state.data_path.join("images/logos/movie");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let body =
        r#"<svg xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M0 0"/></svg>"#;
    tokio::fs::write(dir.join("603.svg"), body).await.unwrap();

    let resp = app.get("/api/v1/images/movies/1/logo").await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert_eq!(ct, "image/svg+xml");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), body);
}

#[tokio::test]
async fn logo_sentinel_returns_404_without_hitting_tmdb() {
    // `logo_path = ""` means we already tried TMDB and got nothing.
    // The endpoint must 404 immediately — no TMDB client configured
    // proves it doesn't refetch.
    let app = TestAppBuilder::new().build().await;
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at, logo_path)
         VALUES (1, 603, 'matrix', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), '')",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let resp = app.get("/api/v1/images/movies/1/logo").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lazy_fetch_writes_sentinel_when_tmdb_has_no_logos() {
    // Mock TMDB returns `{ "logos": [] }` for the entity. The first
    // request lazy-fetches, the fetch finds nothing, we persist the
    // empty-string sentinel so the next request short-circuits.
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_empty_logos("movie", 603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (1, 603, 'matrix', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let resp = app.get("/api/v1/images/movies/1/logo").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);

    // Sentinel must now be persisted so subsequent requests don't
    // round-trip to TMDB again.
    let stored: Option<String> = sqlx::query_scalar("SELECT logo_path FROM movie WHERE id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some(""),
        "empty-string sentinel written after failed lazy-fetch"
    );
}
