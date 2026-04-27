//! `POST / DELETE /api/v1/episodes/{id}/watched` — marks an episode
//! as watched from the overflow menu (without playing it). The
//! Trakt push side-effect is gated on being connected; we just
//! assert the DB update round-trips.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn mark_missing_episode_watched_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/episodes/9999/watched", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unmark_missing_episode_watched_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/episodes/9999/watched").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mark_then_unmark_round_trips_watched_at() {
    let app = TestAppBuilder::new().build().await;

    // Seed: show + series (season in kino speak) + episode. Enough
    // NOT NULLs that a minimal helper would only be marginally
    // shorter than this inline seed.
    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 111, 'fake show', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO series (id, show_id, season_number, monitored)
         VALUES (1, 1, 1, 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO episode (id, series_id, show_id, season_number, episode_number, title)
         VALUES (1, 1, 1, 1, 1, 'pilot')",
    )
    .execute(&app.db)
    .await
    .unwrap();

    // Mark watched.
    let resp = app.post("/api/v1/episodes/1/watched", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let watched_at: Option<String> =
        sqlx::query_scalar("SELECT watched_at FROM episode WHERE id = 1")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(watched_at.is_some(), "watched_at populated");

    // Unmark.
    let resp = app.delete("/api/v1/episodes/1/watched").await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let watched_at: Option<String> =
        sqlx::query_scalar("SELECT watched_at FROM episode WHERE id = 1")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(watched_at.is_none(), "watched_at cleared on unmark");
}
