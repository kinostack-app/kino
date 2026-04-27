//! Quality-profile create + set-default flows. The CRUD basics are
//! in `quality_profiles`; here we cover the validation guard
//! (invalid items JSON returns 400) and the set-default
//! "exactly one default" invariant.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn create_with_invalid_items_json_returns_400() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post(
            "/api/v1/quality-profiles",
            &json!({
                "name": "borked",
                "cutoff": "Bluray-1080p",
                "items": "this is not JSON",
            }),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn set_default_404_for_missing_profile() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/quality-profiles/9999/set-default", &json!({}))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn set_default_makes_target_the_only_default() {
    let app = TestAppBuilder::new().build().await;

    // The default profile is created by `ensure_defaults`. Add a new
    // profile and set it as default; the original must lose the flag.
    let new = json_body(
        app.post(
            "/api/v1/quality-profiles",
            &json!({
                "name": "alt",
                "cutoff": "Bluray-2160p",
                "items": "[]",
            }),
        )
        .await,
    )
    .await;
    let new_id = new["id"].as_i64().expect("created id");

    let resp = app
        .post(
            &format!("/api/v1/quality-profiles/{new_id}/set-default"),
            &json!({}),
        )
        .await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM quality_profile WHERE is_default = 1")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(count, 1, "exactly one default after set-default");

    let new_is_default: bool =
        sqlx::query_scalar("SELECT is_default FROM quality_profile WHERE id = ?")
            .bind(new_id)
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(new_is_default, "target now is_default=1");
}
