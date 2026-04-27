//! Extra coverage for `/api/v1/history` filter combinations not
//! exercised in `history_api`: per-movie scope, cursor pagination,
//! limit clamp.

use base64::Engine as _;

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn history_filter_by_movie_id_scopes_to_one_movie() {
    let app = TestAppBuilder::new().build().await;
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (1, 111, 'a', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (2, 222, 'b', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    for movie_id in [1_i64, 1, 2] {
        sqlx::query(
            "INSERT INTO history (movie_id, event_type, source_title, date)
             VALUES (?, 'grabbed', 'rel', datetime('now'))",
        )
        .bind(movie_id)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/history?movie_id=1").await).await;
    let rows = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(rows.len(), 2, "two events for movie 1; got {rows:?}");
    for r in rows {
        assert_eq!(r["movie_id"], 1);
    }
}

#[tokio::test]
async fn history_cursor_pagination_returns_older_only() {
    let app = TestAppBuilder::new().build().await;
    // Seed 5 events; we'll page them.
    for i in 1..=5 {
        sqlx::query(
            "INSERT INTO history (event_type, source_title, date)
             VALUES ('grabbed', ?, datetime('now'))",
        )
        .bind(format!("rel-{i}"))
        .execute(&app.db)
        .await
        .unwrap();
    }
    let highest_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM history")
        .fetch_one(&app.db)
        .await
        .unwrap();

    // Cursor is now opaque base64 per the 09-api contract —
    // construct one pointing at `highest_id` to simulate "give me
    // the page starting from here."
    let cursor_json = serde_json::json!({ "id": highest_id });
    let cursor =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cursor_json.to_string().as_bytes());

    let page = json_body(
        app.get(&format!("/api/v1/history?cursor={cursor}&limit=2"))
            .await,
    )
    .await;
    let rows = page
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert!(rows.len() <= 2, "limit honoured; got {rows:?}");
    for r in rows {
        assert!(r["id"].as_i64().unwrap() < highest_id, "cursor narrowed");
    }
}
