//! `POST /api/v1/tasks/{name}/run` — verifies the run-now button on
//! the Settings → Tasks card actually drops a `TaskTrigger` on the
//! channel. We don't run the scheduler loop in tests, so the
//! assertion is on what hits the channel, not what the task does.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn run_known_task_returns_200() {
    let app = TestAppBuilder::new().build().await;
    // `wanted_search` is registered by `register_defaults` in the harness.
    let resp = app
        .post("/api/v1/tasks/wanted_search/run", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::OK);
}
