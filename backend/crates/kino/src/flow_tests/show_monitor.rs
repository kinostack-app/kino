//! `PATCH /api/v1/shows/{id}/monitor` — adjust episode monitor flags
//! without re-fetching TMDB. The Manage drawer uses this to swap
//! between "all", "first-season only", and "nothing".

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn patch_monitor_on_missing_show_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .patch(
            "/api/v1/shows/9999/monitor",
            &json!({ "monitor_new_items": "none" }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn seasons_to_monitor_empty_drops_everything_out_of_scope() {
    let app = TestAppBuilder::new().build().await;

    // Seed a show with two episodes, both currently monitored.
    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 111, 's', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
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
    for ep_number in 1..=2_i64 {
        sqlx::query(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, title, acquire, in_scope)
             VALUES (1, 1, 1, ?, 'pilot', 1, 1)",
        )
        .bind(ep_number)
        .execute(&app.db)
        .await
        .unwrap();
    }

    // PATCH with seasons_to_monitor = [] (Manage → "Nothing"). Both
    // axes drop to 0 — matches create_show_inner's empty-branch.
    // User intent is "stop tracking this show's episodes"; ghost
    // progress counts would have been confusing.
    let resp = app
        .patch(
            "/api/v1/shows/1/monitor",
            &json!({ "seasons_to_monitor": [] }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let (acquire_sum, scope_sum): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(acquire),0), COALESCE(SUM(in_scope),0)
         FROM episode WHERE show_id = 1",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    assert_eq!(acquire_sum, 0, "scheduler muted across both episodes");
    assert_eq!(scope_sum, 0, "episodes drop out of Next Up / progress");
}
