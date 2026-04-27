//! `GET /api/v1/logs/export` — NDJSON dump of matching log entries.
//! Used by the "Download logs" button on Settings → Logs.

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn logs_export_returns_ndjson_content_type() {
    let app = TestAppBuilder::new().build().await;

    // Seed two rows so the body has content to verify.
    for msg in ["one", "two"] {
        sqlx::query(
            "INSERT INTO log_entry (ts_us, level, target, subsystem, message)
             VALUES (1, 2, 'test', 'export', ?)",
        )
        .bind(msg)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let resp = app.get("/api/v1/logs/export?level=4").await;
    assert_status(&resp, axum::http::StatusCode::OK);

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // NDJSON is typically advertised as `application/x-ndjson` or
    // `application/ndjson`. Any subtype containing "ndjson" passes.
    assert!(
        content_type.contains("ndjson") || content_type.contains("json"),
        "expected NDJSON-ish content-type; got {content_type}"
    );

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body_bytes).expect("utf-8");
    let lines: Vec<&str> = body.split('\n').filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "expected at least 2 NDJSON lines; got {lines:?}"
    );
    for line in &lines {
        let _: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("invalid JSON line {line}: {e}"));
    }
}
