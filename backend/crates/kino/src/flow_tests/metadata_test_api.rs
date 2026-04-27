//! `POST /api/v1/metadata/test-tmdb` + `/test-opensubtitles` — the
//! "Test connection" buttons on Settings → Metadata.
//!
//! These return 200 with `ok: false` when credentials are missing —
//! the UI distinguishes "credentials wrong" from "endpoint broken".

use serde_json::json;

use crate::test_support::{MockTmdbServer, TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn tmdb_test_without_key_reports_missing_key() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/metadata/test-tmdb", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["ok"], false, "no key → ok:false");
    assert!(
        body["message"].as_str().unwrap_or("").contains("TMDB"),
        "message mentions TMDB; got {body}"
    );
}

#[tokio::test]
async fn tmdb_test_with_mock_server_returns_ok() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    let body = json_body(app.post("/api/v1/metadata/test-tmdb", &json!({})).await).await;
    assert_eq!(body["ok"], true, "valid mock TMDB → ok:true; got {body}");
}

#[tokio::test]
async fn opensubtitles_test_without_creds_reports_incomplete() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(
        app.post("/api/v1/metadata/test-opensubtitles", &json!({}))
            .await,
    )
    .await;
    assert_eq!(body["ok"], false);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("incomplete"),
        "message mentions incomplete creds; got {body}"
    );
}
