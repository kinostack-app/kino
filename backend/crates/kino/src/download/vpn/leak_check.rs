//! VPN IP-leak self-test (subsystem 33 phase B).
//!
//! Periodically asks an external IP-discovery endpoint what our public
//! IP looks like and compares against the VPN endpoint's resolved IP.
//! A mismatch means traffic is escaping the tunnel — the killswitch
//! responds by pausing every active download immediately and emitting
//! [`AppEvent::IpLeakDetected`].
//!
//! The HTTP request binds its source address to the tunnel IP
//! (parsed from `vpn_address`) so the kernel routes through wg0 the
//! same way librqbit's peer sockets do under `bind_device_name`. If
//! routing for that source address ever fell back to the bare
//! interface, this test would catch it.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, SystemTime};

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use super::VpnConfig;
use crate::events::AppEvent;

/// Snapshot of the most recent self-test result. Kept on
/// `VpnManager` so `/health` can render it without re-running the
/// network probe on every request.
#[derive(Debug, Clone)]
pub struct LeakStatus {
    pub checked_at: SystemTime,
    /// `Some(true)` = observed matched expected. `Some(false)` =
    /// mismatch (real leak). `None` = test couldn't run cleanly
    /// (network blip, DNS, parse error). The health surface treats
    /// `None` as "unknown" rather than "leaking".
    pub protected: Option<bool>,
    pub observed_ip: Option<IpAddr>,
    pub expected_ip: Option<IpAddr>,
    pub last_error: Option<String>,
}

/// HTTP client timeout for the discovery probe. 8s is generous for
/// a single GET that returns a few bytes; longer would risk
/// overlapping with the next 5-min tick on a wedged endpoint.
const PROBE_TIMEOUT_SECS: u64 = 8;

/// Run one self-test pass against `check_url`, binding the local
/// address to the tunnel's IP. Returns a [`LeakStatus`] regardless of
/// outcome — caller decides what to do with mismatches.
pub async fn check(config: &VpnConfig, check_url: &str) -> LeakStatus {
    let now = SystemTime::now();
    let expected_ip = resolve_endpoint_ip(&config.server_endpoint);
    let tunnel_ip = parse_tunnel_ip(&config.address);

    let observed_ip = match tunnel_ip {
        Some(ip) => fetch_observed_ip(check_url, ip).await,
        None => Err(format!(
            "VPN tunnel address '{}' is not a parseable IP",
            config.address
        )),
    };

    match (observed_ip, expected_ip) {
        (Ok(observed), Some(expected)) => LeakStatus {
            checked_at: now,
            protected: Some(observed == expected),
            observed_ip: Some(observed),
            expected_ip: Some(expected),
            last_error: None,
        },
        (Ok(observed), None) => LeakStatus {
            checked_at: now,
            protected: None,
            observed_ip: Some(observed),
            expected_ip: None,
            last_error: Some(format!(
                "could not resolve VPN endpoint '{}'",
                config.server_endpoint
            )),
        },
        (Err(e), expected_ip) => LeakStatus {
            checked_at: now,
            protected: None,
            observed_ip: None,
            expected_ip,
            last_error: Some(e),
        },
    }
}

/// Strip the CIDR suffix and parse the IP. Tunnel addresses are
/// stored as `10.2.0.5/32` in config; the `/32` is for the kernel's
/// route-add, not the bind.
fn parse_tunnel_ip(address: &str) -> Option<IpAddr> {
    address.split('/').next()?.trim().parse().ok()
}

/// Resolve the VPN's server endpoint to its first IP. Endpoint is
/// stored as `host:port` (e.g. `de-fra.example.com:51820`); we pull
/// the host, do a DNS lookup, and pick the first record. For dual-
/// stack endpoints we prefer IPv4 — the tunnel IP we compare against
/// is also IPv4 in the typical home setup.
fn resolve_endpoint_ip(endpoint: &str) -> Option<IpAddr> {
    let socket_addrs: Vec<SocketAddr> = endpoint.to_socket_addrs().ok()?.collect();
    socket_addrs
        .iter()
        .find(|a| a.ip().is_ipv4())
        .or_else(|| socket_addrs.first())
        .map(SocketAddr::ip)
}

/// HTTP GET the discovery URL with the local source address pinned
/// to `tunnel_ip`. Most discovery services (api.ipify.org,
/// ifconfig.me, icanhazip.com) return the IP as a bare line of
/// plain text, which is what we expect.
async fn fetch_observed_ip(check_url: &str, tunnel_ip: IpAddr) -> Result<IpAddr, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .local_address(Some(tunnel_ip))
        .build()
        .map_err(|e| format!("build client: {e}"))?;
    let body = client
        .get(check_url)
        .send()
        .await
        .map_err(|e| format!("GET {check_url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("{check_url} returned error: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read body: {e}"))?;
    body.trim()
        .parse::<IpAddr>()
        .map_err(|e| format!("response '{}' is not an IP: {e}", body.trim()))
}

/// Run one tick of the self-test loop: probe, store result on the
/// manager, and on confirmed leak (`protected = Some(false)`) pause
/// every active download + emit the leak event. Inconclusive results
/// (`protected = None`) update the snapshot but don't trigger
/// pause-all — we don't know it's a leak.
pub async fn tick(
    pool: &SqlitePool,
    vpn: &super::VpnManager,
    torrent: Option<&dyn crate::download::session::TorrentSession>,
    event_tx: &broadcast::Sender<AppEvent>,
) -> anyhow::Result<()> {
    if !vpn.is_connected() {
        return Ok(());
    }
    let cfg = super::health::load_config(pool).await?;
    let check_url = load_check_url(pool).await.unwrap_or_else(default_check_url);

    let status = check(&cfg, &check_url).await;

    if let Some(false) = status.protected {
        let observed = status
            .observed_ip
            .map(|i| i.to_string())
            .unwrap_or_default();
        let expected = status
            .expected_ip
            .map(|i| i.to_string())
            .unwrap_or_default();
        tracing::error!(
            observed = %observed,
            expected = %expected,
            "VPN IP leak detected — pausing all active downloads",
        );
        super::killswitch::pause_all_active(pool, torrent, event_tx).await?;
        let _ = event_tx.send(AppEvent::IpLeakDetected {
            observed_ip: observed,
            expected_ip: expected,
        });
    } else if let Some(err) = status.last_error.as_deref() {
        tracing::warn!(error = err, "VPN leak self-test inconclusive");
    } else {
        tracing::debug!(
            observed = ?status.observed_ip,
            "VPN leak self-test passed",
        );
    }

    vpn.set_leak_status(status);
    Ok(())
}

/// Default discovery URL used when `vpn_killswitch_check_url` is
/// missing from config (shouldn't happen given the schema default,
/// but the fallback keeps us shipping under a corrupted-config
/// recovery scenario).
pub fn default_check_url() -> String {
    "https://api.ipify.org".to_owned()
}

async fn load_check_url(pool: &SqlitePool) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT vpn_killswitch_check_url FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tunnel_ip_strips_cidr() {
        assert_eq!(
            parse_tunnel_ip("10.2.0.5/32"),
            Some("10.2.0.5".parse().unwrap()),
        );
        assert_eq!(
            parse_tunnel_ip("10.2.0.5"),
            Some("10.2.0.5".parse().unwrap()),
        );
        assert!(parse_tunnel_ip("not-an-ip").is_none());
        assert!(parse_tunnel_ip("").is_none());
    }

    #[test]
    fn resolve_endpoint_ip_handles_literal_ipv4() {
        // Literal IPs round-trip through the resolver without DNS.
        assert_eq!(
            resolve_endpoint_ip("203.0.113.7:51820"),
            Some("203.0.113.7".parse().unwrap()),
        );
    }

    #[test]
    fn resolve_endpoint_ip_returns_none_on_missing_port() {
        assert!(resolve_endpoint_ip("203.0.113.7").is_none());
    }
}
