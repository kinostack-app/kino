//! `GET /api/v1/shows`, `/api/v1/shows/{id}`, and the by-tmdb
//! `watch-state` summary that powers the `ShowDetail` "Play S01E03"
//! CTA. The watch-state endpoint never 404s — for unfollowed shows
//! it returns `followed: false` so the frontend can still render
//! the "Start watching" branch.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_shows_paginated_response_shape() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/shows").await).await;
    // PaginatedResponse: { results: [], has_more: false }
    assert!(
        body.get("results").is_some(),
        "shows list is paginated; got {body}"
    );
    assert_eq!(body["results"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn get_missing_show_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/shows/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn watch_state_for_unfollowed_show_reports_followed_false() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/shows/by-tmdb/9999/watch-state").await).await;
    assert_eq!(body["followed"], false);
    assert_eq!(body["watched_count"], 0);
    assert_eq!(body["aired_count"], 0);
    assert!(
        body["next_up"].is_null(),
        "no next-up for unfollowed; got {body}"
    );
}

#[tokio::test]
async fn watch_state_for_followed_show_counts_episodes() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 1399, 'Game of Thrones', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (1, 1, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    for ep in 1..=3_i64 {
        sqlx::query(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, title, acquire, in_scope, air_date_utc)
             VALUES (1, 1, 1, ?, 'pilot', 1, 1, '2010-01-01')",
        )
        .bind(ep)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/shows/by-tmdb/1399/watch-state").await).await;
    assert_eq!(body["followed"], true);
    assert_eq!(body["aired_count"], 3, "all three episodes have aired");
    assert_eq!(body["watched_count"], 0, "none watched yet");
}
