//! `/api/v1/network/*` — LAN reachability + mDNS settings probe.
//!
//! The wizard's "Networking" step + the Settings → Networking section
//! both consume these endpoints to answer two questions:
//!
//! 1. **What LAN `IPv4s` did the kino server bind to?** (`lan_probe`)
//!    The frontend then races browser-side `fetch(http://<ip>:<port>/...)`
//!    against each one with a 2s abort. If localhost works (we know it
//!    does — the wizard is loaded from kino) but no LAN IP responds, a
//!    firewall is blocking inbound — surface remediation per OS.
//!
//! 2. **Does mDNS actually resolve from this browser tab?** (`mdns_test`)
//!    We probe the configured hostname (`<host>.local`) via DNS-over-HTTPS
//!    inside the *backend* — same network namespace as the responder, so
//!    a successful probe proves the responder is live + reachable from
//!    the local resolver. Insufficient for "phone on the LAN works" but
//!    catches the responder-not-running case.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::AppResult;
use crate::state::AppState;

/// Reply for `GET /api/v1/network/lan-probe`. Lists the kino server's
/// non-loopback, non-virtual IPv4 addresses + the port it's bound on,
/// so the SPA can probe each from the browser.
#[derive(Serialize, ToSchema)]
pub struct LanProbeReply {
    /// IPv4 addresses kino is bound on. Excludes loopback, link-local,
    /// and virtual / bridge interfaces (docker, libvirt, VPN tunnels)
    /// so the list only contains addresses actual LAN clients can use.
    pub ipv4s: Vec<String>,
    /// HTTP port kino is bound on (read from `/run/kino/port` on
    /// Linux; falls back to 8080 elsewhere).
    pub port: u16,
    /// Configured mDNS hostname (defaults to `kino`). The wizard
    /// surfaces `http://<hostname>.local:<port>` as the LAN URL.
    pub mdns_hostname: String,
    /// Whether the mDNS responder is enabled in config. False
    /// disables the LAN-IP probe banner because the user opted out.
    pub mdns_enabled: bool,
    /// What's actually publishing mDNS records on this host
    /// (Avahi / Bonjour / nothing). Drives the Settings →
    /// Networking provider-status indicator.
    pub mdns_provider: crate::mdns::ProviderStatus,
}

/// `GET /api/v1/network/lan-probe` — non-loopback `IPv4s` + bound port.
///
/// Public so the wizard can call it without holding a session yet.
#[utoipa::path(
    get,
    path = "/api/v1/network/lan-probe",
    responses((status = 200, body = LanProbeReply)),
    tag = "system",
)]
pub async fn lan_probe(State(state): State<AppState>) -> AppResult<Json<LanProbeReply>> {
    let (ips, _virt) = crate::mdns::lan_ipv4s_with_virtual_filtered();
    let ipv4s = ips.iter().map(std::string::ToString::to_string).collect();
    let port = state.http_port;
    let settings = crate::mdns::load_settings(&state.db).await;
    Ok(Json(LanProbeReply {
        ipv4s,
        port,
        mdns_hostname: settings.hostname,
        mdns_enabled: settings.enabled,
        mdns_provider: crate::mdns::detect_provider(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MdnsTestRequest {
    /// Override the stored hostname for a one-shot test. Lets the
    /// settings page test "what would happen if I changed it to
    /// `media`" without saving first.
    pub hostname: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct MdnsTestReply {
    /// True when at least one resolution path succeeded.
    pub ok: bool,
    /// Human-readable result. The UI surfaces this verbatim under
    /// the Test button on Settings → Networking.
    pub message: String,
    /// Resolved `IPv4s` for `<hostname>.local`. Empty on failure.
    pub resolved_ipv4s: Vec<String>,
    /// Hostname tested (echoed back so the UI can confirm).
    pub hostname: String,
}

/// `POST /api/v1/network/mdns-test` — resolve `<hostname>.local`
/// from the backend's perspective. Validates that the responder is
/// live and that the local resolver chain can find it. Doesn't
/// guarantee LAN clients can — that's what `lan_probe` + browser
/// probe is for.
#[utoipa::path(
    post,
    path = "/api/v1/network/mdns-test",
    request_body = MdnsTestRequest,
    responses((status = 200, body = MdnsTestReply)),
    tag = "system",
    security(("api_key" = []))
)]
pub async fn mdns_test(
    State(state): State<AppState>,
    Json(body): Json<MdnsTestRequest>,
) -> AppResult<Json<MdnsTestReply>> {
    let settings = crate::mdns::load_settings(&state.db).await;
    let override_provided = body.hostname.is_some();
    let hostname = body
        .hostname
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| settings.hostname.clone());

    if !settings.enabled && !override_provided {
        return Ok(Json(MdnsTestReply {
            ok: false,
            message: "mDNS responder is disabled in settings".into(),
            resolved_ipv4s: Vec::new(),
            hostname,
        }));
    }

    // Resolve `<hostname>.local` via the system's getaddrinfo. On
    // Linux this consults nss-mdns / avahi if installed; on macOS
    // it consults mDNSResponder; on Windows it falls back to LLMNR
    // unless Bonjour is installed. Best-effort: a "no result" here
    // doesn't necessarily mean the responder is broken — the local
    // resolver chain might just not be mDNS-aware. The UI surfaces
    // this nuance.
    let target = format!("{hostname}.local");
    let target_for_lookup = format!("{target}:0");
    let lookup = tokio::net::lookup_host(target_for_lookup).await;
    match lookup {
        Ok(addrs) => {
            let resolved_ipv4s: Vec<String> = addrs
                .filter_map(|s| match s {
                    std::net::SocketAddr::V4(v4) => Some(v4.ip().to_string()),
                    std::net::SocketAddr::V6(_) => None,
                })
                .collect();
            if resolved_ipv4s.is_empty() {
                Ok(Json(MdnsTestReply {
                    ok: false,
                    message: format!(
                        "{target} resolved but returned no IPv4. Check that the mDNS responder is bound to a real LAN interface."
                    ),
                    resolved_ipv4s,
                    hostname,
                }))
            } else {
                Ok(Json(MdnsTestReply {
                    ok: true,
                    message: format!("{target} resolves to {}", resolved_ipv4s.join(", ")),
                    resolved_ipv4s,
                    hostname,
                }))
            }
        }
        Err(e) => Ok(Json(MdnsTestReply {
            ok: false,
            message: format!(
                "{target} did not resolve: {e}. On Linux this usually means nss-mdns / avahi-daemon isn't installed; on Windows mDNS needs Bonjour or modern Win11."
            ),
            resolved_ipv4s: Vec::new(),
            hostname,
        })),
    }
}
