//! `/api/v1/fs/test` + `/api/v1/fs/browse` — filesystem helpers the
//! Settings path-picker uses. Keep assertions to shape-only: the
//! exact free-space numbers depend on the test container's disk.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn fs_test_reports_missing_path_as_non_existent() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(
        app.get("/api/v1/fs/test?path=/nonexistent/path/xyz/kino-test")
            .await,
    )
    .await;
    assert_eq!(body["exists"], false);
    assert_eq!(body["writable"], false);
    assert!(
        body["error"].is_string(),
        "missing path populates error message"
    );
}

#[tokio::test]
async fn fs_test_reports_writable_tmp_dir() {
    let app = TestAppBuilder::new().build().await;
    // `/tmp` exists and is writable inside the devcontainer; the test
    // assertion focuses on the positive branch (error == null).
    let body = json_body(app.get("/api/v1/fs/test?path=/tmp").await).await;
    assert_eq!(body["exists"], true);
    assert_eq!(body["is_dir"], true);
    // `/tmp` is writable by our user inside the container.
    assert_eq!(body["writable"], true, "body = {body}");
    assert!(body["error"].is_null(), "writable path → no error");
}
