//! Indexer retry + test actions. CRUD is covered by `indexer_crud`;
//! this file focuses on the per-id action endpoints that reset
//! escalation state and run a synthetic search.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn retry_missing_indexer_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/indexers/9999/retry", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_missing_indexer_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/indexers/9999/test", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn retry_resets_escalation_state() {
    let app = TestAppBuilder::new().build().await;

    // Seed an indexer in a degraded state.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO indexer (
            name, indexer_type, url,
            enabled, priority,
            escalation_level,
            most_recent_failure_time,
            disabled_until
         )
         VALUES ('degraded', 'torznab', 'http://x.invalid', 1, 25,
                 3, datetime('now'), datetime('now'))
         RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    let resp = app
        .post(&format!("/api/v1/indexers/{id}/retry"), &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    // DB side: escalation cleared.
    let escalation: i64 = sqlx::query_scalar("SELECT escalation_level FROM indexer WHERE id = ?")
        .bind(id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(escalation, 0, "escalation reset to 0");

    let disabled_until: Option<String> =
        sqlx::query_scalar("SELECT disabled_until FROM indexer WHERE id = ?")
            .bind(id)
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(
        disabled_until.is_none(),
        "disabled_until cleared; got {disabled_until:?}"
    );
}
