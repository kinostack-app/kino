//! `GET /api/v1/fs/browse` — directory listing helper. Used by
//! the path-picker dialog. Hidden entries (`.foo`) are filtered out.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn browse_lists_tmp() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/fs/browse?path=/tmp").await).await;
    assert!(body["entries"].is_array(), "shape: {{ entries: [] }}");
    assert!(
        body["path"].as_str().unwrap_or("").starts_with('/'),
        "canonical path is absolute; got {body}"
    );
}

#[tokio::test]
async fn browse_missing_path_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .get("/api/v1/fs/browse?path=/does/not/exist/anywhere/kino-test")
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browse_filters_hidden_entries() {
    let app = TestAppBuilder::new().build().await;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("visible.txt"), "x").unwrap();
    std::fs::write(dir.path().join(".hidden.txt"), "x").unwrap();

    let path_param = dir.path().to_string_lossy().into_owned();
    let body = json_body(
        app.get(&format!("/api/v1/fs/browse?path={path_param}"))
            .await,
    )
    .await;

    let names: Vec<String> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap_or("").to_owned())
        .collect();
    assert!(names.contains(&"visible.txt".to_owned()));
    assert!(
        !names.iter().any(|n| n.starts_with('.')),
        "no dot-files in listing; got {names:?}"
    );
}
