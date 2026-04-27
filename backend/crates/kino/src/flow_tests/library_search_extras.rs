//! Extra library-search coverage — limit clamping + show inclusion.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn library_search_includes_followed_show() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 1399, 'Game of Thrones', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let body = json_body(app.get("/api/v1/library/search?q=thrones").await).await;
    let hits = body.as_array().expect("array");
    assert!(!hits.is_empty(), "show found by substring; got {body}");
    assert_eq!(hits[0]["item_type"], "show");
    assert_eq!(hits[0]["title"], "Game of Thrones");
}

#[tokio::test]
async fn library_search_limit_clamps_to_max() {
    let app = TestAppBuilder::new().build().await;

    // Seed > 100 followed movies — endpoint clamps limit at 100 even
    // when caller asks for more, so result count maxes at 100.
    for i in 1..=150 {
        sqlx::query(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, added_at)
             VALUES (?, 'matrix matches', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
        )
        .bind(i)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/library/search?q=matches&limit=999").await).await;
    let hits = body.as_array().expect("array");
    assert!(
        hits.len() <= 100,
        "limit clamps at 100; got {} hits",
        hits.len()
    );
}
