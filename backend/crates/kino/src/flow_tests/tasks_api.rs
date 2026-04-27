//! `GET /api/v1/tasks` + `POST /api/v1/tasks/{name}/run`. The
//! scheduler runtime is covered in detail by the grab/import flows;
//! here we just pin the listing contract (all the well-known task
//! names surface) and the 404 behaviour for unknown names.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn task_list_includes_known_defaults() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/tasks").await).await;
    let names: Vec<String> = body
        .as_array()
        .expect("tasks returns array")
        .iter()
        .map(|t| t["name"].as_str().unwrap_or("").to_owned())
        .collect();

    for expected in [
        "wanted_search",
        "stale_download_check",
        "metadata_refresh",
        "cleanup",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "task list missing '{expected}'; got {names:?}"
        );
    }
}

#[tokio::test]
async fn running_unknown_task_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/tasks/does_not_exist/run", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}
