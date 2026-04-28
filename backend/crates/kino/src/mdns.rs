//! mDNS responder (subsystem 25). Advertises kino on the local
//! network so users reach it at `http://{hostname}.local:{port}` from
//! any device on the LAN without knowing the host's IP.
//!
//! The responder is built on `mdns-sd`, a pure-Rust mDNS implementation
//! — no Avahi / Bonjour required at runtime, so the same binary
//! advertises on Linux, macOS, and Windows.
//!
//! Two records are published:
//! - **A** for `{hostname}.local` → host's primary LAN IPv4
//! - **`_http._tcp.local.`** service entry for Bonjour browsers
//!   (`avahi-browse -a`, macOS Finder), with TXT entries `path=/`
//!   and `version={kino_version}`
//!
//! Lifecycle: `start` is called once after the HTTP server has bound
//! and returns a [`Handle`] whose `Drop` sends the unregister
//! goodbye. The handle lives for the duration of the process.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::Context;
use mdns_sd::{HostnameResolutionEvent, IfKind, ServiceDaemon, ServiceInfo};
use sqlx::SqlitePool;

/// Service type for the HTTP service record. Standardised; every
/// Bonjour-aware browser knows this name. We're not advertising any
/// kino-specific protocol — the goal is "discoverable HTTP server",
/// not a custom mDNS namespace.
const SERVICE_TYPE: &str = "_http._tcp.local.";

/// mDNS settings as read from the `config` row.
#[derive(Debug, Clone)]
pub struct Settings {
    pub enabled: bool,
    pub hostname: String,
    pub service_name: String,
}

/// Live responder. The daemon thread inside `mdns-sd` keeps running
/// until this handle drops; on drop we send the unregister goodbye so
/// neighbours' caches drop the name promptly rather than waiting for
/// the TTL.
pub struct Handle {
    daemon: ServiceDaemon,
    fullname: String,
    /// The hostname we ended up publishing under — may differ from
    /// the configured value when collision detection bumped to a
    /// suffixed alternative (`kino-2.local`, `kino-3.local`, …).
    /// `discovered_url()` reads this so the tray + `kino open`
    /// match the actually-claimed name.
    pub resolved_hostname: String,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("fullname", &self.fullname)
            .finish_non_exhaustive()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Best-effort: if the daemon is already gone or the channel
        // backed up there's nothing meaningful we can do at process
        // shutdown.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Read mDNS settings from config. Defaults applied on missing-row
/// (which only happens in pre-init test paths).
pub async fn load_settings(pool: &SqlitePool) -> Settings {
    let row: Option<(bool, String, String)> = sqlx::query_as(
        "SELECT mdns_enabled, mdns_hostname, mdns_service_name
         FROM config WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    match row {
        Some((enabled, hostname, service_name)) => Settings {
            enabled,
            hostname,
            service_name,
        },
        None => Settings {
            enabled: true,
            hostname: "kino".to_owned(),
            service_name: "Kino".to_owned(),
        },
    }
}

/// Bring the responder up. Returns `Ok(None)` when mDNS is disabled
/// in config or we couldn't enumerate any LAN interfaces — both are
/// "not an error, just nothing to advertise" cases that the caller
/// logs and moves on.
pub async fn start(settings: &Settings, port: u16) -> anyhow::Result<Option<Handle>> {
    if !settings.enabled {
        tracing::info!("mDNS disabled in config");
        return Ok(None);
    }
    let (ips, _virtual_interfaces) = lan_ipv4s_with_virtual_filtered();
    if ips.is_empty() {
        tracing::warn!("no LAN IPv4 interfaces found; skipping mDNS advertisement");
        return Ok(None);
    }

    let daemon = ServiceDaemon::new().context("create mdns daemon")?;

    // Whitelist approach: disable EVERY interface, then re-enable
    // only the IPs we picked out as real LAN addresses. The earlier
    // blacklist approach (disable docker, br-, veth, …) was leaky —
    // any virtualisation tool whose interface naming we hadn't
    // catalogued slipped through, and we kept seeing 3 Kino records
    // in `avahi-browse -art` on hosts with docker installed
    // (one with the LAN IP, one with the 172.x.x.x bridge IP, and
    // an "orphan" empty-addr entry from a third broadcast path).
    //
    // `IfKind::All` removes every announce socket; per-IP enables
    // restore exactly the addresses we want. mdns-sd processes the
    // `disable_interface` and `enable_interface` calls via its
    // command channel, in order, so by the time `register()` fires
    // the daemon has only the addresses we whitelisted.
    if let Err(e) = daemon.disable_interface(IfKind::All) {
        tracing::warn!(error = %e, "mDNS: couldn't disable all interfaces (whitelist setup)");
    }
    for ip in &ips {
        if let Err(e) = daemon.enable_interface(IfKind::Addr(*ip)) {
            tracing::warn!(ip = %ip, error = %e, "mDNS: couldn't enable LAN interface");
        }
    }

    // Resolve to a non-colliding hostname before publishing.
    // Probes `<hostname>.local` for ~600ms; if another host on the
    // LAN already owns it (response IP isn't one of ours), bumps
    // to `<hostname>-2`, `<hostname>-3`, … up to `-9`. Same pattern
    // Avahi uses for its own service-name conflicts.
    //
    // The user can always set a specific name via Settings →
    // General → Networking; this is the safety net for the case
    // where they didn't (e.g. a host install + a dev container,
    // or two hosts on the same WiFi after a clone).
    let resolved_hostname = pick_available_hostname(&daemon, &settings.hostname, &ips).await;
    if resolved_hostname != settings.hostname {
        tracing::warn!(
            requested = %settings.hostname,
            resolved = %resolved_hostname,
            "mDNS: '{}.local' was already claimed by another host on the LAN — \
             publishing as '{}.local' instead. Change `mdns_hostname` in Settings \
             or stop the conflicting instance to reclaim.",
            settings.hostname,
            resolved_hostname,
        );
    }

    // Hostnames in the `_http._tcp.local.` record need the trailing
    // dot to be FQDN-shaped per the mDNS spec; the library accepts
    // either form but the dot is the explicit one.
    let host_fqdn = format!("{resolved_hostname}.local.");

    let mut props = std::collections::HashMap::new();
    props.insert("path".to_owned(), "/".to_owned());
    props.insert("version".to_owned(), env!("CARGO_PKG_VERSION").to_owned());

    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &settings.service_name,
        &host_fqdn,
        ips.as_slice(),
        port,
        Some(props),
    )
    .context("build service info")?;
    let fullname = info.get_fullname().to_owned();
    daemon.register(info).context("register mdns service")?;

    tracing::info!(
        host = %host_fqdn,
        ips = ?ips,
        port,
        service = %settings.service_name,
        "mDNS responder live — kino reachable at http://{}.local:{}",
        resolved_hostname,
        port,
    );
    Ok(Some(Handle {
        daemon,
        fullname,
        resolved_hostname,
    }))
}

/// Probe the LAN for `<hostname>.local`; if anyone else owns it,
/// bump a numeric suffix and try again. Returns the first free
/// hostname found.
///
/// "Owns it" = the resolution returned at least one IP that isn't
/// in `own_ips`. Stale-self records (Avahi's local cache from a
/// previous run on this host) match `own_ips` and are NOT treated
/// as collisions — we'd just be racing ourselves.
///
/// Probe budget: 600ms per candidate, max 9 candidates. Worst-case
/// ~5s on startup if every name from `kino` to `kino-9` is taken,
/// which would be an unusual environment.
async fn pick_available_hostname(
    daemon: &ServiceDaemon,
    preferred: &str,
    own_ips: &[IpAddr],
) -> String {
    for suffix in 0..10u32 {
        let candidate = if suffix == 0 {
            preferred.to_owned()
        } else {
            // Suffix starts at -2 (skip -1; reads as "the second one"
            // rather than "the minus-one-th one"). Matches Avahi /
            // Bonjour conventions.
            format!("{preferred}-{}", suffix + 1)
        };
        let target = format!("{candidate}.local.");
        match daemon.resolve_hostname(&target, Some(600)) {
            Ok(rx) => {
                let mut conflict = false;
                let deadline = std::time::Instant::now() + Duration::from_millis(700);
                while std::time::Instant::now() < deadline {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    let recv_result = tokio::time::timeout(remaining, rx.recv_async()).await;
                    // Outer Err = tokio timeout fired. Inner Err = the
                    // probe channel closed (daemon stopped resolving
                    // for any reason). Either way, no more events
                    // coming on this candidate — break.
                    let Ok(Ok(event)) = recv_result else {
                        break;
                    };
                    match event {
                        HostnameResolutionEvent::AddressesFound(_, addrs) => {
                            // Only treat as conflict if AT LEAST ONE address
                            // isn't ours. Avahi's local cache for our previous
                            // run shows our own IP — that's not someone else's
                            // claim, just a stale record.
                            let foreign: Vec<IpAddr> = addrs
                                .iter()
                                .filter(|ip| !own_ips.contains(ip) && !ip.is_loopback())
                                .copied()
                                .collect();
                            if !foreign.is_empty() {
                                tracing::debug!(
                                    candidate = %candidate,
                                    foreign_ips = ?foreign,
                                    "mDNS: collision probe — name taken by another host"
                                );
                                conflict = true;
                                break;
                            }
                        }
                        HostnameResolutionEvent::SearchTimeout(_)
                        | HostnameResolutionEvent::SearchStopped(_) => break,
                        _ => {}
                    }
                }
                let _ = daemon.stop_resolve_hostname(&target);
                if !conflict {
                    return candidate;
                }
            }
            Err(e) => {
                // Probe failed — pessimistic: assume free, return.
                // Worst case we still publish, and the network races
                // it out per RFC 6762 §8 (responders MUST handle
                // post-publish conflicts).
                tracing::debug!(
                    candidate = %candidate,
                    error = %e,
                    "mDNS: collision probe failed; proceeding"
                );
                return candidate;
            }
        }
    }
    // All 10 candidates collided. Fall through to a UUID-suffixed
    // name so we still publish something rather than failing the
    // whole responder. Practically unreachable — would mean 10
    // kino instances on the same LAN, which is its own discussion.
    let suffix: String = uuid::Uuid::new_v4().to_string().chars().take(6).collect();
    format!("{preferred}-{suffix}")
}

/// Enumerate the host's non-loopback, non-link-local IPv4 addresses,
/// excluding virtual / bridge interfaces (Docker, `libvirt`, VPN tunnels,
/// `VirtualBox`, …). Returns the filtered IP set plus the names of the
/// virtual interfaces we excluded — the caller passes those names to
/// `ServiceDaemon::disable_interface` so the daemon also skips opening
/// announce sockets on them.
///
/// Without this filter, kino announces on every interface with an
/// IPv4 address, which on any host with Docker installed produces
/// ~10 entries in the A record set (one per docker bridge). Avahi
/// then registers the service multiple times under auto-renamed
/// `Kino (2)` / `Kino (3)` names, and `kino.local` resolves to a
/// 172.x.x.x bridge address that's only routable from this machine —
/// breaking the headline "open kino.local from any device" feature.
///
/// IPv6 records would be ideal too but most home routers don't forward
/// IPv6 link-local mDNS reliably; the IPv4 A record is the path that
/// works everywhere.
pub(crate) fn lan_ipv4s_with_virtual_filtered() -> (Vec<IpAddr>, Vec<String>) {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return (Vec::new(), Vec::new());
    };
    let mut ips = Vec::new();
    let mut virt = Vec::new();
    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }
        let IpAddr::V4(v4) = iface.ip() else { continue };
        if v4.is_link_local() || v4.is_unspecified() {
            continue;
        }
        if is_virtual_interface_name(&iface.name) {
            virt.push(iface.name);
            continue;
        }
        ips.push(IpAddr::V4(v4));
    }
    virt.sort();
    virt.dedup();
    (ips, virt)
}

/// Heuristic: does this interface name belong to a virtual / bridge /
/// VPN interface that mDNS should NOT announce on? We match by name
/// prefix because every virtualisation tool follows a stable naming
/// convention. Covers the cases that bite homelab users:
///
/// - `docker0`, `br-<id>` — Docker bridges
/// - `veth*` — Docker / Podman / Kubernetes container vNICs
/// - `virbr*`, `vnet*` — `libvirt` / KVM
/// - `vboxnet*` — `VirtualBox`
/// - `tun*`, `tap*` — `OpenVPN`, generic tunnels
/// - `wg*` — `WireGuard`
/// - `cni*`, `flannel*`, `cilium*`, `cali*` — Kubernetes CNI
/// - `lxc*`, `lxd*` — LXC / LXD
/// - `zt*` — `ZeroTier`
/// - `tailscale*`, `ts*` — `Tailscale` (these CAN reach a real LAN
///   in some topologies, but advertising `kino.local` over `Tailscale`
///   produces a duplicate alongside `MagicDNS` that confuses clients;
///   the right way to use kino over `Tailscale` is the `Tailscale`
///   hostname, not mDNS.)
fn is_virtual_interface_name(name: &str) -> bool {
    const VIRTUAL_PREFIXES: &[&str] = &[
        "docker",
        "br-",
        "veth",
        "virbr",
        "vnet",
        "vboxnet",
        "tun",
        "tap",
        "wg",
        "cni",
        "flannel",
        "cilium",
        "cali",
        "lxc",
        "lxd",
        "zt",
        "tailscale",
        "ts",
    ];
    VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_ipv4s_skips_loopback_and_link_local() {
        let (ips, _virt) = lan_ipv4s_with_virtual_filtered();
        for ip in ips {
            assert!(!ip.is_loopback(), "loopback leaked: {ip}");
            if let IpAddr::V4(v4) = ip {
                assert!(!v4.is_link_local(), "link-local leaked: {v4}");
            }
        }
    }

    #[test]
    fn virtual_interface_name_filter_catches_docker_libvirt_vpn() {
        // The bug we're fixing: every docker bridge ends up in the
        // A record set, kino.local resolves to 172.x.x.x.
        assert!(is_virtual_interface_name("docker0"));
        assert!(is_virtual_interface_name("br-0b7f7d848ace"));
        assert!(is_virtual_interface_name("veth123abc"));
        assert!(is_virtual_interface_name("virbr0"));
        assert!(is_virtual_interface_name("vboxnet0"));
        assert!(is_virtual_interface_name("tun0"));
        assert!(is_virtual_interface_name("wg0"));
        assert!(is_virtual_interface_name("flannel.1"));
        assert!(is_virtual_interface_name("zt5u4yz3kf"));
        // Real LAN interfaces stay visible — these are what we want
        // kino.local to actually point at.
        assert!(!is_virtual_interface_name("eth0"));
        assert!(!is_virtual_interface_name("eth1"));
        assert!(!is_virtual_interface_name("enp6s0"));
        assert!(!is_virtual_interface_name("wlan0"));
        assert!(!is_virtual_interface_name("wlp3s0"));
        assert!(!is_virtual_interface_name("en0"));
    }

    #[tokio::test]
    async fn load_settings_default_when_no_config_row() {
        // Pool with the schema but no seeded config row triggers the
        // None branch in load_settings.
        let pool = crate::db::create_test_pool().await;
        let settings = load_settings(&pool).await;
        assert!(settings.enabled);
        assert_eq!(settings.hostname, "kino");
        assert_eq!(settings.service_name, "Kino");
    }

    #[tokio::test]
    async fn load_settings_reads_seeded_values() {
        let pool = crate::db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();
        let settings = load_settings(&pool).await;
        // ensure_defaults seeds the schema-default row, so toggle and
        // hostname round-trip the migration's defaults.
        assert!(settings.enabled);
        assert_eq!(settings.hostname, "kino");
    }
}
