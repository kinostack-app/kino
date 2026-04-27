//! `?episode_id=` history filter — mirror of the per-movie test in
//! `history_filters` but for episodes.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn history_filter_by_episode_id_returns_only_episode_events() {
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
    for ep_number in 1..=2_i64 {
        sqlx::query(
            "INSERT INTO episode (id, series_id, show_id, season_number, episode_number, title)
             VALUES (?, 1, 1, 1, ?, 'p')",
        )
        .bind(ep_number)
        .bind(ep_number)
        .execute(&app.db)
        .await
        .unwrap();
    }

    for episode_id in [1_i64, 1, 2] {
        sqlx::query(
            "INSERT INTO history (episode_id, event_type, source_title, date)
             VALUES (?, 'grabbed', 'rel', datetime('now'))",
        )
        .bind(episode_id)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/history?episode_id=1").await).await;
    let rows = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(rows.len(), 2);
    for r in rows {
        assert_eq!(r["episode_id"], 1);
    }
}
