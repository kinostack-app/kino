//! Additional `/api/v1/fs/test` cases — file-instead-of-dir branch.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn fs_test_reports_file_as_not_a_directory() {
    let app = TestAppBuilder::new().build().await;

    // Create a temporary file we know exists.
    let file = tempfile::NamedTempFile::new().expect("tempfile");
    let path = file.path().to_string_lossy().into_owned();

    let body = json_body(app.get(&format!("/api/v1/fs/test?path={path}")).await).await;
    assert_eq!(body["exists"], true);
    assert_eq!(body["is_dir"], false, "regular file → is_dir false");
    assert_eq!(
        body["writable"], false,
        "files aren't 'writable' as targets"
    );
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("not a directory"),
        "error explains the rejection; got {err}"
    );
}
