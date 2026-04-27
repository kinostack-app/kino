//! Read-path smoke tests for integration status endpoints: Trakt + VPN.
//!
//! These surface `configured`/`connected` booleans that the Settings
//! page uses to gate the OAuth dance and the Tunnel panel. Regression
//! here silently breaks the "Connect Trakt" / "VPN connected" LEDs.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn trakt_status_is_disconnected_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/integrations/trakt/status").await).await;

    assert_eq!(
        body["configured"], false,
        "no client_id/secret on fresh install"
    );
    assert_eq!(body["connected"], false, "no device-code auth yet");
    // Sync toggles are config fields with default = false; reading is
    // always safe even when disconnected.
    for key in ["scrobble", "sync_watched", "sync_ratings"] {
        assert!(
            body.get(key).is_some(),
            "status missing {key}; body = {body}"
        );
    }
}

#[tokio::test]
async fn vpn_status_reports_disabled_when_no_tunnel_configured() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/vpn/status").await).await;

    assert_eq!(body["enabled"], false, "no VPN → reports disabled");
    assert_eq!(body["status"], "disconnected");
    assert!(body["interface"].is_null(), "no tunnel → no interface name");
}

#[tokio::test]
async fn health_endpoint_reachable() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/health").await;
    assert_status(&resp, axum::http::StatusCode::OK);
}
