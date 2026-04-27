//! Follow-a-movie happy path.
//!
//! Full end-to-end coverage of this flow needs all four seams
//! working together:
//! - `MockTmdbServer` for the initial metadata fetch
//! - `MockTorznabServer` + a DB indexer row for release search
//! - `FakeTorrentSession` for the grab → complete → stream lifecycle
//! - `MockClock` optional, for search-backoff assertions
//!
//! The tests in this file are ordered by surface area: start with
//! the pure-metadata flow, then layer in search, then the whole
//! pipeline through import.

use serde_json::json;

use crate::test_support::{
    FakeTorrentSession, MockTmdbServer, MockTorznabServer, TestAppBuilder, json_body,
};

/// The minimum shape of "follow a movie": POST /movies with a TMDB
/// id, the server fetches metadata via (mocked) TMDB, inserts a
/// movie row + links the default quality profile, and the movie is
/// immediately findable via /movies.
#[tokio::test]
async fn follow_movie_creates_db_row() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let resp = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    assert!(
        resp.status().is_success(),
        "create_movie returned {}",
        resp.status()
    );
    let body = json_body(resp).await;
    assert_eq!(body["tmdb_id"], 603);
    assert_eq!(body["title"], "The Matrix");
    assert_eq!(body["year"], 1999);
    assert_eq!(body["monitored"], true);
    let movie_id = body["id"].as_i64().expect("movie id");

    // Sanity: DB reflects the insert.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM movie WHERE id = ?")
        .bind(movie_id)
        .fetch_one(&app.db)
        .await
        .expect("movie count");
    assert_eq!(count, 1);

    // And the listing endpoint surfaces it.
    let list = json_body(app.get("/api/v1/movies").await).await;
    let results = list["results"].as_array().expect("movies list.results");
    assert_eq!(results.len(), 1, "the newly-followed movie is listed");
    assert_eq!(results[0]["tmdb_id"], 603);
}

/// Double-follow of the same `tmdb_id` should 409 — otherwise two
/// movie rows fight for ownership of a single library entry and
/// grabs end up split between them.
#[tokio::test]
async fn double_follow_rejects_with_409() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let first = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    assert!(first.status().is_success());

    let second = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    assert_eq!(
        second.status(),
        axum::http::StatusCode::CONFLICT,
        "second follow should 409; got {}",
        second.status()
    );
}

/// Wanted-search sweep with an indexer configured should insert at
/// least one release row for the movie. Doesn't cover the grab step
/// (that needs the torrent client too); just asserts that the
/// search phase walks through to a `release` INSERT.
#[tokio::test]
async fn wanted_search_finds_releases_via_mock_indexer() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;

    let torznab = MockTorznabServer::start().await;
    torznab.stub_search_fixture("matrix-releases.xml").await;

    // Fake torrent session so grab_release can call add_torrent
    // without a real librqbit.
    let torrents = FakeTorrentSession::new();

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .with_torrent(std::sync::Arc::new(torrents.clone()))
        .build()
        .await;

    // Register the mock indexer.
    sqlx::query(
        "INSERT INTO indexer (name, url, api_key, indexer_type, enabled, priority)
         VALUES ('MockTorznab', ?, 'test-key', 'torznab', 1, 25)",
    )
    .bind(torznab.base_url())
    .execute(&app.db)
    .await
    .expect("indexer insert");

    // Follow the movie — creates the DB row.
    let follow = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    assert!(follow.status().is_success());

    // Trigger the search sweep synchronously.
    app.run_task("wanted_search")
        .await
        .expect("wanted_search succeeds");

    // Assert: at least one release exists for the movie. The
    // fixture provides 2 candidates; we don't hard-code the exact
    // count because the scorer / target-matcher might accept or
    // reject either.
    let release_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM release r
         JOIN movie m ON m.id = r.movie_id
         WHERE m.tmdb_id = 603",
    )
    .fetch_one(&app.db)
    .await
    .expect("count releases");
    assert!(
        release_count >= 1,
        "expected ≥1 release for The Matrix after sweep; got {release_count}"
    );
}
