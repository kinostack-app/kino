//! Userspace `WireGuard` tunnel via boringtun.
//!
//! Flow:
//!   1. Parse `private_key`, `peer_public_key` from base64.
//!   2. Create a TUN device (`tun` crate) — name set to the manager's
//!      `interface_name`. Requires `CAP_NET_ADMIN`.
//!   3. Configure IP + route via `rtnetlink`.
//!   4. Open a UDP socket bound to 0.0.0.0:0 (any port).
//!   5. Spawn three tasks:
//!      - outbound: read TUN → boringtun encapsulate → UDP
//!      - inbound:  UDP → boringtun decapsulate → TUN
//!      - timers:   periodic `tunn.update_timers()` for handshakes/keepalives
//!   6. Cancellation token stops all three.

use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use boringtun::noise::{Tunn, TunnResult};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::VpnConfig;

/// Buffer size for packets — `WireGuard`'s maximum transport packet is
/// 1500 + 32 bytes of overhead. Use 1600 to round up.
const BUF: usize = 1600;

pub async fn start(
    interface_name: &str,
    cfg: &VpnConfig,
    cancel: CancellationToken,
    last_handshake: Arc<RwLock<Option<Instant>>>,
) -> anyhow::Result<()> {
    let priv_key = decode_key(&cfg.private_key).map_err(|e| anyhow::anyhow!("private_key: {e}"))?;
    let peer_key = decode_key(&cfg.server_public_key)
        .map_err(|e| anyhow::anyhow!("server_public_key: {e}"))?;
    let static_priv = boringtun::x25519::StaticSecret::from(priv_key);
    let peer_public = boringtun::x25519::PublicKey::from(peer_key);

    let endpoint: SocketAddr = cfg
        .server_endpoint
        .parse()
        .map_err(|e| anyhow::anyhow!("server_endpoint `{}`: {e}", cfg.server_endpoint))?;

    // Tunn::new returns Self (not Result) in boringtun 0.7.
    let tunn = Tunn::new(
        static_priv,
        peer_public,
        None,
        Some(25), // keepalive every 25s
        0,
        None,
    );

    // Create TUN interface. `tun::AsyncDevice` is built on tokio and
    // returns a tokio-friendly io handle. Name requests are hints; on
    // Linux we typically get the exact name when free.
    let mut tun_cfg = tun::Configuration::default();
    tun_cfg
        .tun_name(interface_name)
        .address(parse_ipv4_from_cidr(&cfg.address)?)
        .netmask(parse_netmask_from_cidr(&cfg.address))
        .mtu(1420)
        .up();

    let dev = tokio::task::spawn_blocking(move || tun::create_as_async(&tun_cfg))
        .await
        .map_err(|e| anyhow::anyhow!("tun spawn: {e}"))?
        .map_err(|e| anyhow::anyhow!("tun create: {e}"))?;

    // Route the default route through the tunnel. Best-effort: if this
    // fails we still have a usable tunnel the caller can bind to, just
    // not as a default gateway for the host.
    if let Err(e) = configure_default_route(interface_name).await {
        tracing::warn!(error = %e, "failed to set default route via tunnel");
    }

    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    udp.connect(endpoint).await?;
    tracing::info!(iface = %interface_name, endpoint = %endpoint, "WireGuard tunnel up");

    let tunn = Arc::new(tokio::sync::Mutex::new(tunn));

    // Spawn the three workers. They share the cancel token so any one
    // failing doesn't orphan the others.
    let dev = Arc::new(tokio::sync::Mutex::new(dev));
    let udp = Arc::new(udp);

    tokio::spawn(outbound_loop(
        dev.clone(),
        udp.clone(),
        tunn.clone(),
        cancel.clone(),
    ));
    tokio::spawn(inbound_loop(
        dev.clone(),
        udp.clone(),
        tunn.clone(),
        cancel.clone(),
        last_handshake,
    ));
    tokio::spawn(timer_loop(udp.clone(), tunn.clone(), cancel));

    Ok(())
}

/// Read packets from the TUN device, encrypt with boringtun, write to UDP.
async fn outbound_loop(
    dev: Arc<tokio::sync::Mutex<tun::AsyncDevice>>,
    udp: Arc<UdpSocket>,
    tunn: Arc<tokio::sync::Mutex<Tunn>>,
    cancel: CancellationToken,
) {
    let mut in_buf = vec![0u8; BUF];
    let mut out_buf = vec![0u8; BUF];

    loop {
        let n = tokio::select! {
            () = cancel.cancelled() => break,
            r = read_tun(&dev, &mut in_buf) => match r {
                Ok(n) => n,
                Err(e) => { tracing::warn!(error = %e, "tun read"); continue; }
            },
        };

        let mut t = tunn.lock().await;
        match t.encapsulate(&in_buf[..n], &mut out_buf) {
            TunnResult::WriteToNetwork(packet) => {
                if let Err(e) = udp.send(packet).await {
                    tracing::warn!(error = %e, "udp send");
                }
            }
            TunnResult::Err(e) => tracing::warn!(?e, "encapsulate"),
            // Done / WriteToTunnel* (not expected on encap) — nothing to do.
            _ => {}
        }
    }
}

/// Read packets from UDP, decrypt, write decrypted IP packets to TUN.
async fn inbound_loop(
    dev: Arc<tokio::sync::Mutex<tun::AsyncDevice>>,
    udp: Arc<UdpSocket>,
    tunn: Arc<tokio::sync::Mutex<Tunn>>,
    cancel: CancellationToken,
    last_handshake: Arc<RwLock<Option<Instant>>>,
) {
    let mut in_buf = vec![0u8; BUF];
    let mut out_buf = vec![0u8; BUF];

    loop {
        let n = tokio::select! {
            () = cancel.cancelled() => break,
            r = udp.recv(&mut in_buf) => match r {
                Ok(n) => n,
                Err(e) => { tracing::warn!(error = %e, "udp recv"); continue; }
            },
        };

        let mut t = tunn.lock().await;
        let mut result = t.decapsulate(None, &in_buf[..n], &mut out_buf);

        // decapsulate may need to emit multiple packets in a single call.
        loop {
            match result {
                TunnResult::Done => break,
                TunnResult::WriteToNetwork(packet) => {
                    if let Err(e) = udp.send(packet).await {
                        tracing::warn!(error = %e, "udp send (decap)");
                    }
                    result = t.decapsulate(None, &[], &mut out_buf);
                }
                TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
                    *last_handshake.write().await = Some(Instant::now());
                    if let Err(e) = write_tun(&dev, packet).await {
                        tracing::warn!(error = %e, "tun write");
                    }
                    break;
                }
                TunnResult::Err(e) => {
                    tracing::debug!(?e, "decapsulate");
                    break;
                }
            }
        }
    }
}

/// Call `tunn.update_timers` periodically; may emit handshake packets.
async fn timer_loop(
    udp: Arc<UdpSocket>,
    tunn: Arc<tokio::sync::Mutex<Tunn>>,
    cancel: CancellationToken,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let mut buf = vec![0u8; BUF];
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = ticker.tick() => {
                let mut t = tunn.lock().await;
                match t.update_timers(&mut buf) {
                    TunnResult::WriteToNetwork(packet) => {
                        if let Err(e) = udp.send(packet).await {
                            tracing::warn!(error = %e, "udp send (timer)");
                        }
                    }
                    TunnResult::Err(e) => tracing::debug!(?e, "update_timers"),
                    _ => {}
                }
            }
        }
    }
}

async fn read_tun(
    dev: &Arc<tokio::sync::Mutex<tun::AsyncDevice>>,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    use tokio::io::AsyncReadExt;
    let mut guard = dev.lock().await;
    guard.read(buf).await
}

async fn write_tun(
    dev: &Arc<tokio::sync::Mutex<tun::AsyncDevice>>,
    buf: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut guard = dev.lock().await;
    guard.write_all(buf).await
}

fn decode_key(encoded: &str) -> Result<[u8; 32], String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| e.to_string())?;
    if bytes.len() != 32 {
        return Err(format!("expected 32 bytes, got {}", bytes.len()));
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&bytes);
    Ok(k)
}

fn parse_ipv4_from_cidr(cidr: &str) -> anyhow::Result<Ipv4Addr> {
    let ip = cidr.split('/').next().unwrap_or(cidr);
    Ipv4Addr::from_str(ip).map_err(|e| anyhow::anyhow!("address `{ip}`: {e}"))
}

fn parse_netmask_from_cidr(cidr: &str) -> Ipv4Addr {
    let prefix: u8 = cidr
        .split('/')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(32);
    let mask: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0)
    };
    Ipv4Addr::from(mask)
}

/// Add a default route through the given interface. Uses rtnetlink so
/// we shell out to `ip` — simpler than netlink handling for one route
/// and `iproute2` is standard on every Linux distro we ship on.
/// No-op on macOS / Windows.
#[cfg(target_os = "linux")]
async fn configure_default_route(interface: &str) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("ip")
        .args(["route", "replace", "default", "dev", interface])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "ip route replace: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(
    clippy::unused_async,
    reason = "matches Linux signature for cfg-symmetric callers"
)]
async fn configure_default_route(_interface: &str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_32_byte_base64_key() {
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let decoded = decode_key(&key).unwrap();
        assert_eq!(decoded, [7u8; 32]);
    }

    #[test]
    fn rejects_wrong_length_key() {
        let key = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(decode_key(&key).is_err());
    }

    #[test]
    fn parses_cidr_address_and_mask() {
        let ip = parse_ipv4_from_cidr("10.0.0.1/24").unwrap();
        let mask = parse_netmask_from_cidr("10.0.0.1/24");
        assert_eq!(ip.octets(), [10, 0, 0, 1]);
        assert_eq!(mask.octets(), [255, 255, 255, 0]);
    }

    #[test]
    fn parses_bare_ip_as_host_route() {
        let ip = parse_ipv4_from_cidr("10.0.0.1").unwrap();
        let mask = parse_netmask_from_cidr("10.0.0.1");
        assert_eq!(ip.octets(), [10, 0, 0, 1]);
        assert_eq!(mask.octets(), [255, 255, 255, 255]);
    }

    #[test]
    fn netmask_zero_prefix_is_zeros() {
        let mask = parse_netmask_from_cidr("0.0.0.0/0");
        assert_eq!(mask.octets(), [0, 0, 0, 0]);
    }
}
