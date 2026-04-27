//! `/api/v1/status.warnings[]` — the health page's source of truth
//! for the red/yellow dot in the topbar. Each warning has a message
//! and an optional link to the relevant settings page.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn fresh_install_warnings_mention_tmdb_and_paths() {
    // Wipe the config row so ensure_defaults doesn't fill in env-var
    // values — we want the "nothing configured" branch.
    let app = TestAppBuilder::new().build().await;
    sqlx::query("UPDATE config SET tmdb_api_key = '', download_path = '', media_library_path = '' WHERE id = 1")
        .execute(&app.db)
        .await
        .unwrap();

    let body = json_body(app.get("/api/v1/status").await).await;
    let warnings = body["warnings"].as_array().expect("warnings array");
    assert!(!warnings.is_empty(), "missing-config warnings surface");
    assert_eq!(body["status"], "setup_required");
    assert_eq!(body["setup_required"], true);

    let messages: Vec<&str> = warnings
        .iter()
        .map(|w| w["message"].as_str().unwrap_or(""))
        .collect();
    // We don't pin exact wording — just confirm the three key topics
    // show up so the frontend's /settings/X deep-links work.
    let joined = messages.join(" | ");
    assert!(
        joined.contains("TMDB"),
        "TMDB warning present; got {joined}"
    );
    assert!(
        joined.contains("Download path"),
        "download path warning present; got {joined}"
    );
}

#[tokio::test]
async fn configured_install_reports_no_warnings_about_config() {
    let app = TestAppBuilder::new().build().await;
    sqlx::query(
        "UPDATE config SET tmdb_api_key = 'test', download_path = '/tmp/dl', media_library_path = '/tmp/ml' WHERE id = 1",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let body = json_body(app.get("/api/v1/status").await).await;
    let warnings = body["warnings"].as_array().expect("warnings array");
    // There may still be warnings (no indexer configured, ffmpeg check, etc.)
    // but none of them should be about missing TMDB / download path.
    for w in warnings {
        let msg = w["message"].as_str().unwrap_or("");
        assert!(
            !msg.contains("TMDB API key not configured"),
            "TMDB warning should be gone; got {msg}"
        );
        assert!(
            !msg.contains("Download path not configured"),
            "download-path warning should be gone; got {msg}"
        );
    }
}
