//! `POST /api/v1/vpn/test` — runs the VPN health check + egress IP
//! lookup. With no VPN configured (the test default), the endpoint
//! returns 200 with `ok: false` and a "not configured" message —
//! the UI uses this as the disconnected indicator.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn vpn_test_returns_not_configured_when_disabled() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/vpn/test", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::OK);

    let body = json_body(resp).await;
    assert_eq!(body["ok"], false);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("not configured"),
        "message says VPN not configured; got {body}"
    );
    assert!(body["public_ip"].is_null());
}
