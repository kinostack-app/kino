//! `GET /api/v1/shows/{id}/seasons/{season_number}/episodes` — feeds
//! the season-detail view. Order by `episode_number` is part of the
//! contract (UI doesn't sort client-side).

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn episodes_endpoint_returns_array_for_unknown_show() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/shows/9999/seasons/1/episodes").await).await;
    assert!(body.is_array(), "endpoint always returns array");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn episodes_returned_in_episode_number_order() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 111, 's', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (1, 1, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    // Insert out of order to verify the ORDER BY.
    for ep in [3_i64, 1, 2] {
        sqlx::query(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, title, acquire, in_scope)
             VALUES (1, 1, 1, ?, 'pilot', 1, 1)",
        )
        .bind(ep)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/shows/1/seasons/1/episodes").await).await;
    let episodes = body.as_array().expect("array");
    assert_eq!(episodes.len(), 3);
    assert_eq!(episodes[0]["episode_number"], 1);
    assert_eq!(episodes[1]["episode_number"], 2);
    assert_eq!(episodes[2]["episode_number"], 3);
}
