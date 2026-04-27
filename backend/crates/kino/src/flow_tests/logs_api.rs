//! `GET /api/v1/logs` — structured log query endpoint powering the
//! Logs page. Uses a `QueryBuilder` with optional filters; mis-wiring
//! a filter branch would silently return too many (or zero) rows.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn empty_logs_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/logs").await).await;
    assert!(body.is_array(), "returns array");
    // Fresh install may have `tracing` events emitted during boot.
    // Don't hard-assert on count — just the shape.
}

#[tokio::test]
async fn filter_by_subsystem_narrows_the_result() {
    let app = TestAppBuilder::new().build().await;

    // Seed two rows in different subsystems + levels so we can
    // exercise two filter branches at once. `ts_us` is NOT NULL.
    for (level, subsystem, message) in [
        (2_i64, "test_a", "hello from a"),
        (2, "test_b", "hello from b"),
        (0, "test_a", "err in a"),
    ] {
        sqlx::query(
            "INSERT INTO log_entry (ts_us, level, target, subsystem, message)
             VALUES (1, ?, 'test', ?, ?)",
        )
        .bind(level)
        .bind(subsystem)
        .bind(message)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let all = json_body(app.get("/api/v1/logs?level=4").await).await;
    assert_eq!(all.as_array().unwrap().len(), 3, "no subsystem filter → 3");

    let only_a = json_body(app.get("/api/v1/logs?level=4&subsystem=test_a").await).await;
    assert_eq!(
        only_a.as_array().unwrap().len(),
        2,
        "subsystem narrows to 2"
    );
}

#[tokio::test]
async fn query_substring_matches_message_text() {
    let app = TestAppBuilder::new().build().await;
    for msg in ["alpha beta", "beta gamma", "delta"] {
        sqlx::query(
            "INSERT INTO log_entry (ts_us, level, target, subsystem, message)
             VALUES (1, 2, 'test', 'q', ?)",
        )
        .bind(msg)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let hits = json_body(app.get("/api/v1/logs?level=2&q=beta").await).await;
    assert_eq!(hits.as_array().unwrap().len(), 2, "two rows contain `beta`");
}
