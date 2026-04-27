//! Port forwarding provider abstraction + a minimal NAT-PMP implementation.
//!
//! NAT-PMP (RFC 6886) is used by `ProtonVPN`, `AirVPN`, and several self-
//! hosted `WireGuard` servers. Protocol is UDP to port 5351 on the gateway:
//!
//!   request: [version=0][op=2 (map TCP)][reserved 2 bytes]
//!            [`internal_port` 2 bytes][external_port 2 bytes]
//!            [lifetime 4 bytes]
//!
//! We issue two requests (TCP op=2 and UDP op=1) because `BitTorrent` uses
//! both. The response echoes the external port the gateway assigned.

use async_trait::async_trait;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// One mapping request outcome.
#[derive(Debug, Clone, Copy)]
pub struct Mapping {
    pub external_port: u16,
    pub lifetime_secs: u32,
}

#[async_trait]
pub trait PortForwardProvider: Send + Sync {
    /// Attempt to acquire an external port mapping for `internal_port`.
    async fn acquire(&self, gateway: IpAddr, internal_port: u16) -> anyhow::Result<Mapping>;
}

/// Select a provider by the config string.
///
/// `provider_key` carries the provider-specific token the user set
/// in `vpn_port_forward_api_key`. For NAT-PMP it's ignored; for
/// `AirVPN` it's the pre-allocated port number from the user's
/// `AirVPN` member dashboard (as a decimal string).
#[allow(clippy::default_trait_access)]
pub fn by_name(name: &str, provider_key: Option<&str>) -> Option<Box<dyn PortForwardProvider>> {
    match name {
        "natpmp" | "nat-pmp" | "nat_pmp" => Some(Box::new(NatPmp)),
        "airvpn" => {
            let port_str = provider_key?.trim();
            if port_str.is_empty() {
                tracing::warn!(
                    "airvpn port forwarding requires the port number in vpn_port_forward_api_key \
                     (paste the port from your AirVPN member dashboard); skipping"
                );
                return None;
            }
            match port_str.parse::<u16>() {
                Ok(port) if port > 0 => Some(Box::new(AirVpn { port })),
                Ok(_) => {
                    tracing::warn!("airvpn port cannot be 0; skipping");
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        value = port_str,
                        error = %e,
                        "airvpn port must be a decimal number; skipping"
                    );
                    None
                }
            }
        }
        "none" | "" => None,
        other => {
            tracing::warn!(provider = %other, "unknown port-forward provider, skipping");
            None
        }
    }
}

/// Derive the NAT-PMP gateway IP from the tunnel's `address` field
/// (e.g. `"10.2.0.5/32"`). Convention across most WG providers that
/// support NAT-PMP (`ProtonVPN` et al.) is that the gateway is the
/// `.1` address in the same /24. We strip any CIDR suffix and
/// replace the last octet with `1`. IPv6 isn't handled — no major
/// provider ships IPv6-only NAT-PMP today.
pub fn derive_ipv4_gateway(address: &str) -> Option<IpAddr> {
    let host = address.split('/').next()?;
    let ip: IpAddr = host.parse().ok()?;
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            Some(IpAddr::V4(std::net::Ipv4Addr::new(
                octets[0], octets[1], octets[2], 1,
            )))
        }
        IpAddr::V6(_) => None,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NatPmp;

/// `AirVPN` port-forward "provider". `AirVPN` doesn't expose a live
/// PF API — users allocate a port once on their member dashboard,
/// and that port is then statically available through any of their
/// servers. Our job is just to surface it to librqbit's
/// `announce_port`. `acquire` is a pure function returning the
/// configured port; there's no network call, no expiry.
#[derive(Debug, Clone, Copy)]
pub struct AirVpn {
    port: u16,
}

#[async_trait]
impl PortForwardProvider for AirVpn {
    async fn acquire(&self, _gateway: IpAddr, _internal_port: u16) -> anyhow::Result<Mapping> {
        tracing::debug!(external_port = self.port, "airvpn static port returned");
        Ok(Mapping {
            external_port: self.port,
            // 1h matches NAT-PMP's default lifetime — our refresh
            // loop will re-acquire at half-life (no-op) rather than
            // never refreshing at all. Keeps the machinery uniform.
            lifetime_secs: 3600,
        })
    }
}

#[async_trait]
impl PortForwardProvider for NatPmp {
    async fn acquire(&self, gateway: IpAddr, internal_port: u16) -> anyhow::Result<Mapping> {
        // Request both TCP (op=2) and UDP (op=1). Return the TCP port —
        // typically NAT-PMP gives you the same port for both.
        let tcp = send_mapping(gateway, 2, internal_port, internal_port, 3600).await?;
        // Best-effort UDP mapping; TCP is the return value, UDP is just a
        // courtesy to trackers that probe both.
        if let Err(e) = send_mapping(gateway, 1, internal_port, internal_port, 3600).await {
            tracing::debug!(
                internal_port,
                error = %e,
                "NAT-PMP UDP mapping failed (TCP succeeded, continuing)",
            );
        }
        Ok(tcp)
    }
}

async fn send_mapping(
    gateway: IpAddr,
    op: u8,
    internal: u16,
    external_hint: u16,
    lifetime_secs: u32,
) -> anyhow::Result<Mapping> {
    // Request:
    //   [0] version = 0
    //   [1] op (1=UDP, 2=TCP)
    //   [2..4] reserved
    //   [4..6] internal port (BE)
    //   [6..8] suggested external port (BE)
    //   [8..12] lifetime (BE)
    let mut req = [0u8; 12];
    req[1] = op;
    req[4..6].copy_from_slice(&internal.to_be_bytes());
    req[6..8].copy_from_slice(&external_hint.to_be_bytes());
    req[8..12].copy_from_slice(&lifetime_secs.to_be_bytes());

    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let addr = SocketAddr::new(gateway, 5351);
    sock.connect(addr).await?;
    sock.send(&req).await?;

    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(3), sock.recv(&mut buf))
        .await
        .map_err(|_| anyhow::anyhow!("nat-pmp timeout"))??;
    if n < 16 {
        anyhow::bail!("short nat-pmp response ({n} bytes)");
    }

    // Response:
    //   [0] version
    //   [1] op + 128
    //   [2..4] result code
    //   [4..8] seconds since epoch (ignored)
    //   [8..10] internal port
    //   [10..12] mapped external port
    //   [12..16] mapping lifetime
    let result_code = u16::from_be_bytes([buf[2], buf[3]]);
    if result_code != 0 {
        anyhow::bail!("nat-pmp result code {result_code}");
    }
    let external_port = u16::from_be_bytes([buf[10], buf[11]]);
    let lifetime = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
    Ok(Mapping {
        external_port,
        lifetime_secs: lifetime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_routes_natpmp_variants() {
        assert!(by_name("natpmp", None).is_some());
        assert!(by_name("nat-pmp", None).is_some());
        assert!(by_name("nat_pmp", None).is_some());
    }

    #[test]
    fn by_name_rejects_none_and_unknown() {
        assert!(by_name("", None).is_none());
        assert!(by_name("none", None).is_none());
        assert!(by_name("mystery-provider", None).is_none());
    }

    #[test]
    fn by_name_airvpn_requires_valid_port() {
        assert!(
            by_name("airvpn", Some("51820")).is_some(),
            "valid port should produce a provider"
        );
        assert!(
            by_name("airvpn", Some("  51820  ")).is_some(),
            "whitespace around port should be tolerated"
        );
        assert!(
            by_name("airvpn", None).is_none(),
            "missing port should skip (warn logged)"
        );
        assert!(
            by_name("airvpn", Some("")).is_none(),
            "empty port should skip"
        );
        assert!(by_name("airvpn", Some("0")).is_none(), "port 0 is invalid");
        assert!(
            by_name("airvpn", Some("not-a-number")).is_none(),
            "non-numeric should fail gracefully"
        );
        assert!(
            by_name("airvpn", Some("70000")).is_none(),
            "port > u16::MAX should fail gracefully"
        );
    }

    #[test]
    fn derive_gateway_strips_cidr_and_replaces_last_octet() {
        assert_eq!(
            derive_ipv4_gateway("10.2.0.5/32"),
            Some("10.2.0.1".parse().unwrap())
        );
        assert_eq!(
            derive_ipv4_gateway("10.66.7.42"),
            Some("10.66.7.1".parse().unwrap())
        );
    }

    #[test]
    fn derive_gateway_handles_malformed_input() {
        assert!(derive_ipv4_gateway("not-an-ip").is_none());
        assert!(derive_ipv4_gateway("").is_none());
        assert!(derive_ipv4_gateway("fe80::1").is_none()); // IPv6 unsupported
    }
}
