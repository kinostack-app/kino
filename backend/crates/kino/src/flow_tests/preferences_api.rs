//! `GET/PATCH/POST /api/v1/preferences/home` — the per-user Home
//! layout prefs endpoint. Pure-DB shape; covered at the unit level in
//! `home::preferences::tests`, this file adds router-level regression
//! coverage for the partial-update COALESCE contract.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn fresh_install_returns_defaults() {
    let app = TestAppBuilder::new().build().await;
    let prefs = json_body(app.get("/api/v1/preferences/home").await).await;

    assert_eq!(prefs["hero_enabled"], true);
    let order = prefs["section_order"].as_array().expect("section_order");
    assert!(
        order.iter().any(|v| v == "up_next"),
        "up_next is in the v1 default order; got {order:?}"
    );
    assert!(
        prefs["section_hidden"].as_array().unwrap().is_empty(),
        "no hidden rows on fresh install"
    );
}

#[tokio::test]
async fn patch_only_touches_supplied_fields() {
    let app = TestAppBuilder::new().build().await;
    // Hydrate the row.
    let before = json_body(app.get("/api/v1/preferences/home").await).await;
    let original_order = before["section_order"].clone();

    // Flip just the hero toggle. Section order must survive.
    let patched = json_body(
        app.patch(
            "/api/v1/preferences/home",
            &json!({ "hero_enabled": false }),
        )
        .await,
    )
    .await;
    assert_eq!(patched["hero_enabled"], false);
    assert_eq!(
        patched["section_order"], original_order,
        "order preserved across disjoint PATCH"
    );
}

#[tokio::test]
async fn reset_restores_defaults_after_customisation() {
    let app = TestAppBuilder::new().build().await;

    // Customise: hide a row + flip hero off.
    app.patch(
        "/api/v1/preferences/home",
        &json!({
            "hero_enabled": false,
            "section_hidden": ["popular_movies"],
        }),
    )
    .await;

    let resp = app.post("/api/v1/preferences/home/reset", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let reset = json_body(resp).await;

    assert_eq!(reset["hero_enabled"], true, "hero back on");
    assert!(
        reset["section_hidden"].as_array().unwrap().is_empty(),
        "hidden wiped"
    );
}
