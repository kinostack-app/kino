//! `GET /api/v1/playback/transcode-stats` + `transcode-sessions` —
//! playback diagnostics surfaced on the Settings card. With no
//! transcoder installed in tests, sessions list is empty and stats
//! reflect config defaults.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn transcode_sessions_empty_without_transcoder() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/playback/transcode-sessions").await).await;
    assert!(body.is_array(), "sessions returns an array");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn transcode_stats_reports_config_defaults() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/playback/transcode-stats").await).await;
    assert_eq!(body["active_sessions"], 0, "no live transcodes in tests");
    // `max_concurrent` and `enabled` are config-driven; just check they
    // exist and are typed as we expect.
    assert!(body["max_concurrent"].is_i64());
    assert!(body["enabled"].is_boolean());
}
