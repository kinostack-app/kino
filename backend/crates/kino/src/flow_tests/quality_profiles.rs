//! Quality profile CRUD — creating, editing tiers, setting the
//! default. The default-profile rule is subtle: exactly one row at a
//! time should have `is_default=1`. Test asserts on that invariant.

use axum::http::StatusCode;
use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn default_profile_exists_on_fresh_install() {
    // `ensure_defaults` seeds one profile marked as default. The
    // setup wizard relies on a sensible starting state.
    let app = TestAppBuilder::new().build().await;

    let resp = app.get("/api/v1/quality-profiles").await;
    assert_status(&resp, StatusCode::OK);

    let body = json_body(resp).await;
    let items = body.as_array().expect("profiles list");
    assert!(
        !items.is_empty(),
        "fresh install should seed at least one quality profile"
    );
    let default_count = items
        .iter()
        .filter(|p| p["is_default"].as_bool().unwrap_or(false))
        .count();
    assert_eq!(
        default_count, 1,
        "exactly one profile should be default on a fresh install"
    );
}

#[tokio::test]
async fn creating_new_profile_doesnt_change_default() {
    let app = TestAppBuilder::new().build().await;

    // Create a second profile. `items` is stored as a JSON string
    // (not a nested object) — matches the schema's TEXT column.
    let items_json = json!([
        { "quality_id": "bluray_1080p", "rank": 10, "allowed": true, "name": "BluRay 1080p" }
    ])
    .to_string();
    let create = app
        .post(
            "/api/v1/quality-profiles",
            &json!({
                "name": "1080p tier",
                "items": items_json,
                "cutoff": "bluray_1080p"
            }),
        )
        .await;
    assert!(
        create.status().is_success(),
        "create profile returned {}",
        create.status()
    );

    // Both should exist, original still default, new one not.
    let list = json_body(app.get("/api/v1/quality-profiles").await).await;
    let items = list.as_array().expect("list");
    assert_eq!(items.len(), 2, "two profiles after create");
    let defaults: Vec<_> = items
        .iter()
        .filter(|p| p["is_default"].as_bool().unwrap_or(false))
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "default-profile invariant: exactly one after create"
    );
}

/// Deleting the default quality profile must be rejected — the
/// "new follow" codepath resolves "the default" to get a profile id,
/// so a missing default turns into a confusing FK error on the next
/// grab. Forcing the caller to promote another profile first keeps
/// the invariant "exactly one default exists" intact.
#[tokio::test]
async fn deleting_default_profile_is_rejected() {
    let app = TestAppBuilder::new().build().await;

    let list = json_body(app.get("/api/v1/quality-profiles").await).await;
    let default_id = list
        .as_array()
        .expect("list")
        .iter()
        .find(|p| p["is_default"].as_bool().unwrap_or(false))
        .and_then(|p| p["id"].as_i64())
        .expect("default profile has an id");

    let resp = app
        .delete(&format!("/api/v1/quality-profiles/{default_id}"))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Row must still be present — 409 must not side-effect.
    let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quality_profile WHERE id = ?")
        .bind(default_id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(still_there, 1);
}
