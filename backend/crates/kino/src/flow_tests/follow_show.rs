//! Follow-a-show happy path. Mirrors `follow_movie` but with the
//! TV-specific plumbing: series + episodes get persisted on follow,
//! wanted-search walks the episode table rather than the movie one.
//!
//! Season pack handling + per-episode search matrix are tested in
//! their own files; this one covers the create path.

use serde_json::json;

use crate::test_support::{MockTmdbServer, TestAppBuilder, json_body};

#[tokio::test]
async fn follow_show_creates_series_and_episodes() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_show(1399).await;
    tmdb.stub_season(1399, 1).await;
    // create_show iterates every season listed on the show payload;
    // the fixture advertises season 1 only, so just that is enough.

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let resp = app.post("/api/v1/shows", &json!({ "tmdb_id": 1399 })).await;
    assert!(
        resp.status().is_success(),
        "create_show returned {}",
        resp.status()
    );
    let body = json_body(resp).await;
    assert_eq!(body["tmdb_id"], 1399);
    assert_eq!(body["title"], "Game of Thrones");
    let show_id = body["id"].as_i64().expect("show id");

    // Series row for season 1 should exist.
    let series_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE show_id = ? AND season_number = 1")
            .bind(show_id)
            .fetch_one(&app.db)
            .await
            .expect("series count");
    assert_eq!(series_count, 1, "season 1 series row created on follow");

    // Both episodes from the fixture should be persisted.
    let episode_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM episode WHERE show_id = ? AND season_number = 1")
            .bind(show_id)
            .fetch_one(&app.db)
            .await
            .expect("episode count");
    assert_eq!(
        episode_count, 2,
        "two episodes from the season-1 fixture should exist"
    );
}

#[tokio::test]
async fn double_follow_of_same_show_rejects_409() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_show(1399).await;
    tmdb.stub_season(1399, 1).await;

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let first = app.post("/api/v1/shows", &json!({ "tmdb_id": 1399 })).await;
    assert!(first.status().is_success());

    let second = app.post("/api/v1/shows", &json!({ "tmdb_id": 1399 })).await;
    assert_eq!(
        second.status(),
        axum::http::StatusCode::CONFLICT,
        "second follow should 409; got {}",
        second.status()
    );
}
