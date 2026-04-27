//! Movie release on the calendar — uses `physical_release_date` /
//! `digital_release_date` / `release_date` (in priority order via the
//! handler's CASE).

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn calendar_includes_movie_with_release_date_in_range() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at,
                            release_date, monitored)
         VALUES (1, 603, 'matrix', (SELECT id FROM quality_profile LIMIT 1), datetime('now'),
                 '2026-04-15', 1)",
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
    let movie_count = entries.iter().filter(|e| e["item_type"] == "movie").count();
    assert!(movie_count >= 1, "movie surfaces in range; got {body}");
}
