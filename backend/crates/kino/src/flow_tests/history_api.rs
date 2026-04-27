//! `GET /api/v1/history` — timeline feed for the History page and
//! per-movie/per-episode detail views. Event emission is covered in
//! the flow tests that drive the underlying action (grab, import,
//! watched); this file tests the *read* path filters.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn empty_history_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/history").await).await;
    let r = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert!(r.is_empty());
}

#[tokio::test]
async fn event_type_filter_narrows_result() {
    let app = TestAppBuilder::new().build().await;

    // Seed three events; we'll filter to one type.
    for event_type in ["release_grabbed", "imported", "watched"] {
        sqlx::query(
            "INSERT INTO history (event_type, source_title, date)
             VALUES (?, 'fake', datetime('now'))",
        )
        .bind(event_type)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let all = json_body(app.get("/api/v1/history").await).await;
    assert_eq!(
        all.get("results")
            .and_then(serde_json::Value::as_array)
            .expect("paginated envelope")
            .len(),
        3
    );

    let filtered = json_body(app.get("/api/v1/history?event_type=imported").await).await;
    let rows = filtered
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["event_type"], "imported");
}

#[tokio::test]
async fn event_types_multi_filter_returns_union() {
    let app = TestAppBuilder::new().build().await;
    for event_type in ["release_grabbed", "imported", "watched", "failed"] {
        sqlx::query(
            "INSERT INTO history (event_type, source_title, date)
             VALUES (?, 'fake', datetime('now'))",
        )
        .bind(event_type)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(
        app.get("/api/v1/history?event_types=release_grabbed,watched")
            .await,
    )
    .await;
    let rows = body
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(rows.len(), 2, "OR-union across listed types; got {rows:?}");
    let types: Vec<&str> = rows
        .iter()
        .map(|r| r["event_type"].as_str().unwrap_or(""))
        .collect();
    assert!(types.contains(&"release_grabbed"));
    assert!(types.contains(&"watched"));
}
