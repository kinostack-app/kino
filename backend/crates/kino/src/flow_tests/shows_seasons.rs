//! `/api/v1/shows/{id}/seasons` + `/monitored-seasons` — the data
//! the Manage drawer's tri-state checkboxes consume. Empty/missing
//! responses must keep their shape so the UI doesn't blow up.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_seasons_for_missing_show_returns_empty_array() {
    let app = TestAppBuilder::new().build().await;
    // Endpoint doesn't pre-check show existence — empty result for
    // unknown id is the established behaviour.
    let body = json_body(app.get("/api/v1/shows/9999/seasons").await).await;
    assert!(body.is_array(), "list returns array");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn monitored_seasons_for_missing_show_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/shows/9999/monitored-seasons").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn monitored_seasons_aggregates_episode_acquire_counts() {
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

    // 3 episodes in season 1: 2 acquiring, 1 not.
    for (acquire, ep_number) in [(1_i64, 1_i64), (1, 2), (0, 3)] {
        sqlx::query(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, title, acquire, in_scope)
             VALUES (1, 1, 1, ?, 'pilot', ?, 1)",
        )
        .bind(ep_number)
        .bind(acquire)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/shows/1/monitored-seasons").await).await;
    let seasons = body.as_array().expect("array");
    assert_eq!(seasons.len(), 1, "one season");
    assert_eq!(seasons[0]["season_number"], 1);
    assert_eq!(seasons[0]["acquiring"], 2, "two episodes have acquire=1");
    assert_eq!(seasons[0]["total"], 3);
}

#[tokio::test]
async fn list_seasons_returns_seeded_seasons() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 111, 's', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    for (id, sn) in [(1_i64, 1_i64), (2, 2)] {
        sqlx::query(
            "INSERT INTO series (id, show_id, season_number, monitored) VALUES (?, 1, ?, 1)",
        )
        .bind(id)
        .bind(sn)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/shows/1/seasons").await).await;
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 2, "both seasons listed");
    assert_eq!(rows[0]["season_number"], 1, "ordered by season_number ASC");
}
