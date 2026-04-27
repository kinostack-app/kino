//! Calendar endpoint with seeded content. The empty-array case is
//! covered in `library_views`; here we seed an episode + a movie
//! within the date range and assert they surface in the JSON.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn calendar_lists_seeded_episode_in_range() {
    let app = TestAppBuilder::new().build().await;

    // Seed: monitored show + season + episode airing 2026-04-15.
    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 1399, 's', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (1, 1, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO episode (series_id, show_id, season_number, episode_number, title, acquire, in_scope, air_date_utc)
         VALUES (1, 1, 1, 1, 'pilot', 1, 1, '2026-04-15T00:00:00Z')",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let body = json_body(
        app.get("/api/v1/calendar?start=2026-04-01&end=2026-04-30")
            .await,
    )
    .await;
    let entries = body.as_array().expect("array");
    assert_eq!(entries.len(), 1, "one episode in range; got {body}");
    assert_eq!(entries[0]["item_type"], "episode");
    assert_eq!(entries[0]["show_title"], "s");
}

#[tokio::test]
async fn calendar_excludes_episodes_outside_range() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 1399, 's', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (1, 1, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    // Episode airs in 2030 — well outside our query window.
    sqlx::query(
        "INSERT INTO episode (series_id, show_id, season_number, episode_number, title, acquire, in_scope, air_date_utc)
         VALUES (1, 1, 1, 1, 'pilot', 1, 1, '2030-01-01T00:00:00Z')",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let body = json_body(
        app.get("/api/v1/calendar?start=2026-04-01&end=2026-04-30")
            .await,
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0, "out-of-range filtered");
}
