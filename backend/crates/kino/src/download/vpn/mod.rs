//! VPN manager — brings up a userspace `WireGuard` tunnel via boringtun,
//! configures the TUN interface, and exposes the interface name so the
//! `BitTorrent` client (librqbit) can bind all outbound traffic to it.
//!
//! Submodules:
//!   - `tunnel` — the boringtun noise + UDP + TUN loop
//!   - `port_forward` — NAT-PMP / provider API for external port mapping
//!   - `health` — periodic handshake check + reconnect

pub mod handlers;
pub mod health;
pub mod killswitch;
pub mod leak_check;
pub mod port_forward;
pub mod tunnel;

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

/// How early (in fraction of `lifetime_secs`) to re-acquire the
/// mapping. 0.5 = refresh at half-life, leaving plenty of runway if
/// the gateway is slow to answer. NAT-PMP lifetimes are usually
/// 3600s so this is ~30 min between refreshes in practice.
const PORT_FORWARD_REFRESH_FRACTION: f64 = 0.5;
/// Floor on the refresh delay — if the gateway gives us a
/// surprisingly short lifetime we still wait at least this long.
/// Also the delay used on acquire failure before retrying.
const PORT_FORWARD_MIN_REFRESH_SECS: u64 = 60;
/// Cap on the refresh delay so a misbehaving gateway that hands out
/// a `lifetime_secs = 0xFFFFFFFF` response can't park us forever.
const PORT_FORWARD_MAX_REFRESH_SECS: u64 = 3600;

/// VPN tunnel configuration read out of the `config` table.
#[derive(Debug, Clone)]
pub struct VpnConfig {
    pub private_key: String,
    pub address: String,
    pub server_public_key: String,
    pub server_endpoint: String,
    pub dns: Option<String>,
    pub port_forward_provider: String,
    pub port_forward_api_key: Option<String>,
}

/// Public status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

fn status_to_u8(s: VpnStatus) -> u8 {
    match s {
        VpnStatus::Disconnected => 0,
        VpnStatus::Connecting => 1,
        VpnStatus::Connected => 2,
        VpnStatus::Error => 3,
    }
}

fn u8_to_status(v: u8) -> VpnStatus {
    match v {
        0 => VpnStatus::Disconnected,
        1 => VpnStatus::Connecting,
        2 => VpnStatus::Connected,
        _ => VpnStatus::Error,
    }
}

/// Manages tunnel lifecycle + reporting. Cheap to clone.
#[derive(Debug, Clone)]
pub struct VpnManager {
    status: Arc<AtomicU8>,
    interface_name: String,
    forwarded_port: Arc<std::sync::Mutex<Option<u16>>>,
    /// Holding this keeps the tunnel alive; dropping cancels it.
    tunnel: Arc<Mutex<Option<TunnelHandle>>>,
    /// Last observed handshake. Used by the health task.
    last_handshake: Arc<RwLock<Option<Instant>>>,
    /// Cancellation token for the port-forward refresh task, if one
    /// is running. Separate from the tunnel token so a port-forward
    /// failure can't tear down the tunnel (and vice versa).
    port_forward_cancel: Arc<std::sync::Mutex<Option<CancellationToken>>>,
    /// Last IP-leak self-test result (subsystem 33 phase B). `None`
    /// until the first probe completes; thereafter holds the most
    /// recent snapshot for `/health` to render.
    leak_status: Arc<std::sync::Mutex<Option<leak_check::LeakStatus>>>,
}

#[derive(Debug)]
struct TunnelHandle {
    cancel: CancellationToken,
}

impl VpnManager {
    pub fn new(interface_name: &str) -> Self {
        Self {
            status: Arc::new(AtomicU8::new(status_to_u8(VpnStatus::Disconnected))),
            interface_name: interface_name.to_owned(),
            forwarded_port: Arc::new(std::sync::Mutex::new(None)),
            tunnel: Arc::new(Mutex::new(None)),
            last_handshake: Arc::new(RwLock::new(None)),
            port_forward_cancel: Arc::new(std::sync::Mutex::new(None)),
            leak_status: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Bring the tunnel up. Waits for the first handshake and only
    /// reports `Connected` once it lands — without this, the torrent
    /// client would bind to a tunnel that never completed and start
    /// leaking traffic to the host network. The packet loop runs in
    /// a background task spawned by `tunnel::start`; this function
    /// returns once we've confirmed the tunnel is actually usable.
    ///
    /// On a 10s handshake timeout the manager:
    /// * tears down the half-up tunnel so a stale `wg0` interface
    ///   doesn't linger,
    /// * flips status to `Error` (the gate that blocks acquisition),
    /// * returns an error so the caller surfaces it to the UI.
    pub async fn connect(&self, config: &VpnConfig) -> anyhow::Result<()> {
        if self.is_connected() {
            return Ok(());
        }
        self.set_status(VpnStatus::Connecting);

        let cancel = CancellationToken::new();
        if let Err(e) = tunnel::start(
            &self.interface_name,
            config,
            cancel.clone(),
            self.last_handshake.clone(),
        )
        .await
        {
            self.set_status(VpnStatus::Error);
            return Err(e);
        }
        *self.tunnel.lock().await = Some(TunnelHandle { cancel });

        // Wait up to 10s for the first handshake. Anything else is a
        // dead tunnel — the torrent client must NOT proceed to bind
        // its socket to `wg0`, so we tear down and surface an error.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.last_handshake.read().await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if self.last_handshake.read().await.is_none() {
            tracing::warn!(
                iface = %self.interface_name,
                "no handshake within 10s — tearing down tunnel; downloads stay paused",
            );
            // Pull the tunnel down so we don't leave a stale interface
            // that misleads later status reads.
            if let Some(h) = self.tunnel.lock().await.take() {
                h.cancel.cancel();
            }
            self.set_status(VpnStatus::Error);
            anyhow::bail!(
                "VPN handshake did not complete within 10s on {iface}",
                iface = self.interface_name,
            );
        }

        self.set_status(VpnStatus::Connected);
        Ok(())
    }

    pub async fn disconnect(&self) -> anyhow::Result<()> {
        // Cancel the port-forward refresh loop first — the tunnel
        // going away will also kill its networking, but cancelling
        // explicitly avoids a spurious "refresh failed" log line.
        self.stop_port_forwarding();
        if let Some(h) = self.tunnel.lock().await.take() {
            h.cancel.cancel();
        }
        *self.forwarded_port.lock().expect("lock") = None;
        *self.last_handshake.write().await = None;
        self.set_status(VpnStatus::Disconnected);
        tracing::info!(iface = %self.interface_name, "VPN disconnected");
        Ok(())
    }

    /// Start the port-forward lifecycle: an inline acquire so
    /// `forwarded_port()` is populated before the caller uses it,
    /// followed by a background refresh loop. Safe to call at most
    /// once per connect; a no-op if a loop is already running.
    ///
    /// `internal_port` is the TCP port librqbit listens on locally;
    /// the gateway maps an external port that peers use to reach us.
    /// Returns `Ok(())` on successful acquire; returns `Err(_)` if
    /// the acquire failed so the caller can decide whether to
    /// continue without PF (some VPN setups are NAT-free already).
    pub async fn start_port_forwarding(
        &self,
        provider_name: &str,
        provider_key: Option<&str>,
        gateway: IpAddr,
        internal_port: u16,
    ) -> anyhow::Result<()> {
        let Some(provider) = port_forward::by_name(provider_name, provider_key) else {
            tracing::info!(
                provider = provider_name,
                "port forwarding disabled (no provider selected or config missing)"
            );
            return Ok(());
        };

        // Inline acquire so the port is available when the caller
        // reads `forwarded_port()` next.
        let mapping = provider
            .acquire(gateway, internal_port)
            .await
            .map_err(|e| anyhow::anyhow!("port forward acquire failed: {e}"))?;
        self.set_forwarded_port(mapping.external_port);
        tracing::info!(
            provider = provider_name,
            external_port = mapping.external_port,
            lifetime_secs = mapping.lifetime_secs,
            "port forward acquired"
        );

        // Spawn the refresh loop. Hold a clone of the manager so the
        // task can update `forwarded_port` when the mapping renews.
        let cancel = CancellationToken::new();
        *self.port_forward_cancel.lock().expect("lock") = Some(cancel.clone());
        let manager_forwarded = self.forwarded_port.clone();
        let provider_name = provider_name.to_owned();
        let provider_key = provider_key.map(str::to_owned);
        tokio::spawn(async move {
            // Re-box the provider so we own it inside the task.
            let Some(provider) = port_forward::by_name(&provider_name, provider_key.as_deref())
            else {
                return;
            };
            let mut next_delay = refresh_delay_for(mapping.lifetime_secs);
            loop {
                tokio::select! {
                    () = cancel.cancelled() => {
                        tracing::debug!(
                            provider = %provider_name,
                            "port forward refresh task cancelled"
                        );
                        return;
                    }
                    () = tokio::time::sleep(next_delay) => {}
                }
                match provider.acquire(gateway, internal_port).await {
                    Ok(m) => {
                        *manager_forwarded.lock().expect("lock") = Some(m.external_port);
                        tracing::debug!(
                            provider = %provider_name,
                            external_port = m.external_port,
                            lifetime_secs = m.lifetime_secs,
                            "port forward refreshed"
                        );
                        next_delay = refresh_delay_for(m.lifetime_secs);
                    }
                    Err(e) => {
                        tracing::warn!(
                            provider = %provider_name,
                            error = %e,
                            "port forward refresh failed; retrying"
                        );
                        next_delay = Duration::from_secs(PORT_FORWARD_MIN_REFRESH_SECS);
                    }
                }
            }
        });
        Ok(())
    }

    fn stop_port_forwarding(&self) {
        if let Some(cancel) = self.port_forward_cancel.lock().expect("lock").take() {
            cancel.cancel();
        }
    }

    pub fn is_connected(&self) -> bool {
        self.status() == VpnStatus::Connected
    }

    pub fn status(&self) -> VpnStatus {
        u8_to_status(self.status.load(Ordering::Relaxed))
    }

    fn set_status(&self, s: VpnStatus) {
        self.status.store(status_to_u8(s), Ordering::Relaxed);
    }

    pub fn interface_name(&self) -> &str {
        &self.interface_name
    }

    pub fn forwarded_port(&self) -> Option<u16> {
        *self.forwarded_port.lock().expect("lock")
    }

    pub fn set_forwarded_port(&self, port: u16) {
        *self.forwarded_port.lock().expect("lock") = Some(port);
    }

    pub async fn last_handshake(&self) -> Option<Instant> {
        *self.last_handshake.read().await
    }

    /// Most recent IP-leak self-test result. `None` until the first
    /// `vpn_killswitch_check` tick completes after connect.
    pub fn leak_status(&self) -> Option<leak_check::LeakStatus> {
        self.leak_status.lock().expect("lock").clone()
    }

    pub fn set_leak_status(&self, status: leak_check::LeakStatus) {
        *self.leak_status.lock().expect("lock") = Some(status);
    }
}

/// Compute the refresh delay from a mapping's advertised lifetime.
/// Clamped to `[MIN, MAX]` so outliers don't wedge the loop.
fn refresh_delay_for(lifetime_secs: u32) -> Duration {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let half = (f64::from(lifetime_secs) * PORT_FORWARD_REFRESH_FRACTION) as u64;
    let clamped = half.clamp(PORT_FORWARD_MIN_REFRESH_SECS, PORT_FORWARD_MAX_REFRESH_SECS);
    Duration::from_secs(clamped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_enum_round_trips_through_u8() {
        for s in [
            VpnStatus::Disconnected,
            VpnStatus::Connecting,
            VpnStatus::Connected,
            VpnStatus::Error,
        ] {
            assert_eq!(u8_to_status(status_to_u8(s)), s);
        }
    }

    #[tokio::test]
    async fn idle_manager_reports_disconnected() {
        let m = VpnManager::new("wg0");
        assert_eq!(m.status(), VpnStatus::Disconnected);
        assert!(!m.is_connected());
        assert!(m.forwarded_port().is_none());
    }

    #[tokio::test]
    async fn set_forwarded_port_roundtrip() {
        let m = VpnManager::new("wg0");
        assert!(m.forwarded_port().is_none());
        m.set_forwarded_port(51820);
        assert_eq!(m.forwarded_port(), Some(51820));
    }

    #[test]
    fn refresh_delay_uses_half_lifetime_with_bounds() {
        // Typical: 3600s lifetime → refresh at 1800s
        assert_eq!(refresh_delay_for(3600), Duration::from_secs(1800));
        // Floor: 10s lifetime clamps up to the 60s minimum
        assert_eq!(
            refresh_delay_for(10),
            Duration::from_secs(PORT_FORWARD_MIN_REFRESH_SECS)
        );
        // Ceiling: an overly-generous lifetime clamps down to max
        assert_eq!(
            refresh_delay_for(u32::MAX),
            Duration::from_secs(PORT_FORWARD_MAX_REFRESH_SECS)
        );
    }

    #[tokio::test]
    async fn start_port_forwarding_no_provider_is_noop() {
        let m = VpnManager::new("wg0");
        let gateway: IpAddr = "127.0.0.1".parse().unwrap();
        // "none" means "user opted out of port forwarding entirely"
        m.start_port_forwarding("none", None, gateway, 6881)
            .await
            .unwrap();
        assert!(m.forwarded_port().is_none());
    }

    /// `AirVPN` acquire is pure (no network), so we can exercise the
    /// happy path end-to-end here: "acquire" returns the configured
    /// port and `forwarded_port()` reflects it.
    #[tokio::test]
    async fn airvpn_static_port_is_returned_from_config() {
        let m = VpnManager::new("wg0");
        let gateway: IpAddr = "10.2.0.1".parse().unwrap();
        m.start_port_forwarding("airvpn", Some("51820"), gateway, 6881)
            .await
            .unwrap();
        assert_eq!(m.forwarded_port(), Some(51820));
        // Cancel the refresh task so the test doesn't leak a tokio task.
        m.disconnect().await.unwrap();
    }
}
