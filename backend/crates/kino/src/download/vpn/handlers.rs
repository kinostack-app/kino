//! VPN status + test endpoints for the /settings/vpn page.
//!
//! `status` is cheap (reads in-memory `VpnManager` state) and is
//! polled by the status card. `test` is expensive — it runs a
//! health check and looks up the egress IP so the user can confirm
//! traffic is actually leaving through the tunnel.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::download::vpn::VpnStatus;
use crate::download::vpn::health;
use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct VpnStatusResponse {
    pub enabled: bool,
    /// "disconnected" | "connecting" | "connected" | "error"
    pub status: String,
    pub interface: Option<String>,
    pub forwarded_port: Option<u16>,
    /// Seconds since the last `WireGuard` handshake. None if the tunnel
    /// has never completed one.
    pub last_handshake_ago_secs: Option<u64>,
}

/// `GET /api/v1/vpn/status` — snapshot of the in-memory tunnel state.
#[utoipa::path(
    get, path = "/api/v1/vpn/status",
    responses((status = 200, body = VpnStatusResponse)),
    tag = "vpn",
    security(("api_key" = []))
)]
pub async fn get_status(State(state): State<AppState>) -> AppResult<Json<VpnStatusResponse>> {
    let Some(vpn) = state.vpn.clone() else {
        return Ok(Json(VpnStatusResponse {
            enabled: false,
            status: "disconnected".into(),
            interface: None,
            forwarded_port: None,
            last_handshake_ago_secs: None,
        }));
    };

    let status_str = match vpn.status() {
        VpnStatus::Disconnected => "disconnected",
        VpnStatus::Connecting => "connecting",
        VpnStatus::Connected => "connected",
        VpnStatus::Error => "error",
    };

    Ok(Json(VpnStatusResponse {
        enabled: true,
        status: status_str.into(),
        interface: Some(vpn.interface_name().to_string()),
        forwarded_port: vpn.forwarded_port(),
        last_handshake_ago_secs: vpn.last_handshake().await.map(|t| t.elapsed().as_secs()),
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VpnTestResponse {
    pub ok: bool,
    pub message: String,
    /// Egress IP observed from an external lookup service. None on
    /// failure or when the tunnel isn't up.
    pub public_ip: Option<String>,
}

/// `POST /api/v1/vpn/test` — runs the VPN health check (confirms a
/// recent handshake, reconnects if stale) and looks up the public IP
/// via api.ipify.org so the user can confirm traffic is actually
/// routing through the tunnel.
#[utoipa::path(
    post, path = "/api/v1/vpn/test",
    responses((status = 200, body = VpnTestResponse)),
    tag = "vpn",
    security(("api_key" = []))
)]
pub async fn test_connection(State(state): State<AppState>) -> AppResult<Json<VpnTestResponse>> {
    let Some(vpn) = state.vpn.clone() else {
        return Ok(Json(VpnTestResponse {
            ok: false,
            message: "VPN not configured".into(),
            public_ip: None,
        }));
    };

    match health::check_once(
        &state.db,
        vpn.clone(),
        state.torrent.as_deref(),
        &state.event_tx,
    )
    .await
    {
        Ok(false) => {
            return Ok(Json(VpnTestResponse {
                ok: false,
                message: "VPN is not connected".into(),
                public_ip: None,
            }));
        }
        Err(e) => {
            return Ok(Json(VpnTestResponse {
                ok: false,
                message: format!("health check failed: {e}"),
                public_ip: None,
            }));
        }
        Ok(true) => {}
    }

    // Look up the observed egress IP. We use api.ipify.org because
    // it returns a plain-text body and has permissive CORS. Note that
    // this request goes out via the default network namespace — kino's
    // VPN wraps torrent traffic only — so the returned IP reflects the
    // server's regular egress, not what torrents see. Users who want
    // to confirm the torrent path should compare this IP with the one
    // shown by their tracker (or a test torrent).
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok();
    let public_ip = match http {
        Some(c) => match c.get("https://api.ipify.org").send().await {
            Ok(r) if r.status().is_success() => r.text().await.ok(),
            _ => None,
        },
        None => None,
    };

    Ok(Json(VpnTestResponse {
        ok: true,
        message: "VPN handshake healthy".into(),
        public_ip,
    }))
}
