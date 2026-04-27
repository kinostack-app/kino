//! Indexer management flow: add → test → enable → update → delete.
//!
//! Indexers are the torrent search providers (Prowlarr /
//! Jackett-style, or standalone Torznab). CRUD correctness matters
//! because a misconfigured indexer either returns zero results
//! (silent failure) or spams identical searches on every sweep.

use axum::http::StatusCode;
use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn empty_indexer_list_returns_empty_array() {
    let app = TestAppBuilder::new().build().await;

    let resp = app.get("/api/v1/indexers").await;
    assert_status(&resp, StatusCode::OK);

    let body = json_body(resp).await;
    let arr = body.as_array().expect("indexers response is an array");
    assert!(arr.is_empty(), "fresh install has no indexers");
}

#[tokio::test]
async fn create_indexer_persists_and_appears_in_list() {
    let app = TestAppBuilder::new().build().await;

    let resp = app
        .post(
            "/api/v1/indexers",
            &json!({
                "name": "Test Indexer",
                "url": "https://indexer.example.com/api",
                "api_key": "sample-key",
                "indexer_type": "torznab",
                "priority": 25,
                "enabled": true,
            }),
        )
        .await;
    assert!(
        resp.status().is_success(),
        "create_indexer returned {}",
        resp.status()
    );

    let list = json_body(app.get("/api/v1/indexers").await).await;
    let items = list.as_array().expect("list is array");
    assert_eq!(items.len(), 1, "created indexer should be listed");
    assert_eq!(items[0]["name"], "Test Indexer");
    assert_eq!(items[0]["url"], "https://indexer.example.com/api");
    assert_eq!(items[0]["enabled"], true);
}

#[tokio::test]
async fn delete_indexer_removes_from_list() {
    let app = TestAppBuilder::new().build().await;

    // Insert via DB so we get a stable id back.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO indexer (name, url, api_key, indexer_type, enabled, priority)
         VALUES (?, ?, ?, ?, 1, 25)
         RETURNING id",
    )
    .bind("DB-inserted")
    .bind("https://dbi.example.com/api")
    .bind("k")
    .bind("torznab")
    .fetch_one(&app.db)
    .await
    .expect("indexer insert");

    let resp = app.delete(&format!("/api/v1/indexers/{id}")).await;
    assert!(
        resp.status().is_success() || resp.status() == StatusCode::NO_CONTENT,
        "delete returned {}",
        resp.status()
    );

    let list = json_body(app.get("/api/v1/indexers").await).await;
    assert!(
        list.as_array().expect("array").is_empty(),
        "indexer should be gone after delete"
    );
}

#[tokio::test]
async fn wanted_search_sweep_with_no_indexers_is_graceful() {
    // Regression: the sweep used to stamp last_searched_at even when
    // no indexers were enabled, locking every wanted movie into the
    // backoff tier until the cooldown expired. Today it bails early
    // and leaves timestamps untouched.
    let app = TestAppBuilder::new().build().await;

    // Insert a wanted movie that *would* be picked up if an indexer
    // existed — it's not going to be, so we want to assert
    // last_searched_at stays null.
    let mid: i64 = sqlx::query_scalar(
        "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at)
         VALUES (999888, 'Ghost Movie', 1, 1, datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .expect("insert");

    // Trigger the sweep synchronously.
    app.run_task("wanted_search")
        .await
        .expect("wanted_search with no indexers is Ok");

    let after: Option<String> =
        sqlx::query_scalar("SELECT last_searched_at FROM movie WHERE id = ?")
            .bind(mid)
            .fetch_one(&app.db)
            .await
            .expect("fetch");
    assert!(
        after.is_none(),
        "last_searched_at should stay null when no indexers exist; got {after:?}"
    );
}
