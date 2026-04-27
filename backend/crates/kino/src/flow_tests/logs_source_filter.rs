//! `?source=` log filter — narrows by event source (frontend vs
//! backend etc.). Mirror of the subsystem filter test.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn logs_filter_by_source_narrows_result() {
    let app = TestAppBuilder::new().build().await;

    for (source, message) in [("backend", "from backend"), ("frontend", "from fe")] {
        sqlx::query(
            "INSERT INTO log_entry (ts_us, level, target, subsystem, message, source)
             VALUES (1, 2, 'test', 's', ?, ?)",
        )
        .bind(message)
        .bind(source)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/logs?level=4&source=frontend").await).await;
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 1, "frontend filter narrows to 1; got {body}");
    assert_eq!(rows[0]["source"], "frontend");
}
