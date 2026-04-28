//! mDNS publisher — shells out to the OS-native mDNS responder
//! rather than running an in-process Rust implementation.
//!
//! ## Why shell-out
//!
//! The previous in-process responder (built on `mdns-sd`) entered a
//! "broken announce" mode on some host configurations where it
//! broadcast empty DNS packets — 12-byte header with zero records —
//! continuously. Local clients with a populated cache still
//! resolved `kino.local` (because Avahi served from cache), but
//! fresh clients (a phone joining the LAN, an Android device with a
//! cleared resolver cache) saw only the empty answers and treated
//! them as `NXDOMAIN`.
//!
//! `avahi-publish` (Linux) and `dns-sd` (macOS / Windows) are the
//! battle-tested system tools that ship with their respective mDNS
//! responders. They handle probe / announce / conflict-rename /
//! goodbye / interface filtering / TTL correctly, integrate with
//! every mDNS client out there (Android, iOS, `AppleTV`, printers),
//! and disappear cleanly the instant we kill the child process.
//!
//! ## Per-OS strategy
//!
//! - **Linux**: `avahi-publish -s` for the service entry,
//!   `avahi-publish -a` for the custom A record (`<hostname>.local`
//!   → LAN IP). Two long-lived child processes, killed on `Drop`.
//!
//! - **`macOS`**: `dns-sd -P` (proxy mode) — single command
//!   publishes the service entry AND a custom hostname A record
//!   together.
//!
//! - **Windows**: `dns-sd -R` for the service entry only. Bonjour
//!   for Windows doesn't reliably support proxy mode for arbitrary
//!   hostnames — users hit kino by IP, or via Bonjour discovery
//!   in apps that browse for `_http._tcp`.
//!
//! - **Container / no responder**: `detect_provider()` returns
//!   `MdnsProvider::None` with per-OS remediation guidance. The
//!   Settings UI surfaces this; postinst banners nudge users.
//!
//! ## Cast discovery
//!
//! `cast_sender::discovery` still uses `mdns-sd` as a *client*
//! (browsing for `_googlecast._tcp.local.`). The publish-side bug
//! that drove this rewrite doesn't affect the browse-side, so we
//! keep that crate in tree for that single purpose.

use std::net::IpAddr;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use anyhow::Context as _;
use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

/// Standard service type for HTTP. Bonjour-aware browsers all know
/// this name. We're not advertising any kino-specific protocol —
/// the goal is "discoverable HTTP server".
const SERVICE_TYPE: &str = "_http._tcp";
const KINO_VERSION: &str = env!("CARGO_PKG_VERSION");

/// mDNS settings as read from the `config` row.
#[derive(Debug, Clone)]
pub struct Settings {
    pub enabled: bool,
    pub hostname: String,
    pub service_name: String,
}

/// Live publish handle. Wraps the spawned `avahi-publish` /
/// `dns-sd` child processes. `Drop` kills them so the system
/// responder sends the unregister goodbye and clients drop the
/// records from their cache without waiting on TTL.
#[derive(Debug)]
pub struct Handle {
    children: Mutex<Vec<Child>>,
    /// The hostname kino is published under. Reflects what's
    /// actually being advertised; same as `Settings::hostname`
    /// today (the system responder handles its own conflict
    /// renaming, but the name we requested is what we report
    /// back to the SPA + tray).
    pub resolved_hostname: String,
}

impl Drop for Handle {
    fn drop(&mut self) {
        let Ok(mut children) = self.children.lock() else {
            return;
        };
        for mut child in children.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// What's available on this host to publish mDNS records?
/// Surfaced via `/api/v1/network/lan-probe` so the Settings UI can
/// render a status indicator + remediation when nothing is found.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MdnsProvider {
    /// Linux Avahi — `avahi-daemon` running and `avahi-publish` on
    /// PATH. The happy path on every desktop Linux distro.
    Avahi,
    /// `macOS` Bonjour, or Bonjour for Windows. `dns-sd` is on PATH.
    Bonjour,
    /// No working responder detected. Publish skipped; LAN clients
    /// can't reach kino by name.
    None,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProviderStatus {
    pub provider: MdnsProvider,
    /// Per-OS guidance for what to install / enable when
    /// `provider == None`. Empty string when working.
    pub remediation: String,
}

pub fn detect_provider() -> ProviderStatus {
    #[cfg(target_os = "linux")]
    {
        let working = has_binary("avahi-publish") && avahi_daemon_running();
        if working {
            return ProviderStatus {
                provider: MdnsProvider::Avahi,
                remediation: String::new(),
            };
        }
        ProviderStatus {
            provider: MdnsProvider::None,
            remediation: "Install + start Avahi:\n  \
                 Debian/Ubuntu/Pop!_OS: sudo apt install avahi-daemon avahi-utils\n  \
                 Fedora/RHEL:           sudo dnf install avahi avahi-tools\n  \
                 Arch:                  sudo pacman -S avahi\n  \
                 then: sudo systemctl enable --now avahi-daemon"
                .to_owned(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        if has_binary("dns-sd") {
            return ProviderStatus {
                provider: MdnsProvider::Bonjour,
                remediation: String::new(),
            };
        }
        ProviderStatus {
            provider: MdnsProvider::None,
            remediation: "macOS should ship `dns-sd` and the Bonjour mDNSResponder by default. \
                 If this message appears, $PATH is missing /usr/bin or the system is corrupted."
                .to_owned(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        if has_binary("dns-sd") {
            return ProviderStatus {
                provider: MdnsProvider::Bonjour,
                remediation: String::new(),
            };
        }
        ProviderStatus {
            provider: MdnsProvider::None,
            remediation: "Install Bonjour for Windows from https://support.apple.com/kb/DL999, \
                 or use kino's IP address directly. mDNS .local discovery is \
                 unavailable on Windows without it."
                .to_owned(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    ProviderStatus {
        provider: MdnsProvider::None,
        remediation: "mDNS publishing isn't implemented on this OS.".to_owned(),
    }
}

fn has_binary(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .is_some_and(|s| s.success())
}

#[cfg(target_os = "linux")]
fn avahi_daemon_running() -> bool {
    // `avahi-browse -t` exits 0 when it can talk to the daemon.
    // Cheaper + more direct than asking systemctl, which doesn't
    // exist on every Linux setup (musl distros, runit, …).
    use std::time::Duration;
    let Ok(mut child) = Command::new("avahi-browse")
        .args(["-t", "_kino-probe-noexist._tcp"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    // Cap at 500ms so a hung daemon doesn't block boot.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return status.success();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

/// Read mDNS settings from config. Defaults applied on missing-row
/// (only happens in pre-init test paths).
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

/// Bring the publisher up. Returns `Ok(None)` when mDNS is disabled
/// in config or no system responder is available — both are
/// "not an error, just nothing to advertise" cases.
pub fn start(settings: &Settings, port: u16) -> anyhow::Result<Option<Handle>> {
    if !settings.enabled {
        tracing::info!("mDNS disabled in config");
        return Ok(None);
    }
    let provider = detect_provider();
    match provider.provider {
        MdnsProvider::Avahi => start_avahi(settings, port),
        MdnsProvider::Bonjour => start_bonjour(settings, port),
        MdnsProvider::None => {
            tracing::warn!(
                remediation = %provider.remediation,
                "mDNS publishing skipped — no system responder detected on this host"
            );
            Ok(None)
        }
    }
}

#[cfg(target_os = "linux")]
fn start_avahi(settings: &Settings, port: u16) -> anyhow::Result<Option<Handle>> {
    let lan_ip = first_lan_ipv4();
    let mut children: Vec<Child> = Vec::new();

    // Service entry — discoverable via avahi-browse, Finder, Android
    // network discovery, AirPlay-style browsers, etc. Survives if
    // the address publish fails or no LAN IP is found.
    let svc = Command::new("avahi-publish")
        .args([
            "-s",
            &settings.service_name,
            SERVICE_TYPE,
            &port.to_string(),
            "path=/",
            &format!("version={KINO_VERSION}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn avahi-publish for service entry")?;
    children.push(svc);

    // Custom A record — what makes `http://kino.local` work in a
    // browser. avahi-daemon publishes the SYSTEM hostname's A
    // record automatically (e.g. `pop-os.local`), but not arbitrary
    // names — that's what `-a` is for.
    if let Some(ip) = lan_ip {
        let host_fqdn = format!("{}.local", settings.hostname);
        let addr = Command::new("avahi-publish")
            .args(["-a", &host_fqdn, &ip.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn avahi-publish for A record")?;
        children.push(addr);
    } else {
        tracing::warn!("no LAN IPv4 found — skipping A-record publish; service entry still active");
    }

    tracing::info!(
        host = %settings.hostname,
        port,
        ip = ?lan_ip,
        "mDNS responder live (via Avahi) — kino reachable at http://{}.local:{}",
        settings.hostname,
        port,
    );
    Ok(Some(Handle {
        children: Mutex::new(children),
        resolved_hostname: settings.hostname.clone(),
    }))
}

#[cfg(target_os = "macos")]
fn start_bonjour(settings: &Settings, port: u16) -> anyhow::Result<Option<Handle>> {
    let lan_ip = first_lan_ipv4().context("no LAN IPv4 found")?;
    let host_fqdn = format!("{}.local.", settings.hostname);
    // dns-sd -P (proxy mode): registers a service entry AND a
    // custom hostname A record together.
    // Args: name, type, domain, port, host, address, [txt records]
    let child = Command::new("dns-sd")
        .args([
            "-P",
            &settings.service_name,
            SERVICE_TYPE,
            "local.",
            &port.to_string(),
            &host_fqdn,
            &lan_ip.to_string(),
            "path=/",
            &format!("version={KINO_VERSION}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn dns-sd -P for proxy publish")?;
    tracing::info!(
        host = %settings.hostname,
        port,
        ip = %lan_ip,
        "mDNS responder live (via Bonjour) — kino reachable at http://{}.local:{}",
        settings.hostname,
        port,
    );
    Ok(Some(Handle {
        children: Mutex::new(vec![child]),
        resolved_hostname: settings.hostname.clone(),
    }))
}

#[cfg(target_os = "windows")]
fn start_bonjour(settings: &Settings, port: u16) -> anyhow::Result<Option<Handle>> {
    // Bonjour for Windows's dns-sd doesn't reliably support proxy-
    // mode publishing of arbitrary hostnames. We register only the
    // service entry — clients that browse for _http._tcp see kino
    // and connect by service+port; they don't need to know the
    // hostname. Direct typing of `kino.local` won't resolve, but
    // most discovery-aware clients (AirPlay, DLNA, modern Android
    // intents) work fine.
    let child = Command::new("dns-sd")
        .args([
            "-R",
            &settings.service_name,
            SERVICE_TYPE,
            ".",
            &port.to_string(),
            "path=/",
            &format!("version={KINO_VERSION}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn dns-sd -R for service entry")?;
    tracing::info!(
        host = %settings.hostname,
        port,
        "mDNS responder live (via Bonjour, service-only mode on Windows)"
    );
    Ok(Some(Handle {
        children: Mutex::new(vec![child]),
        resolved_hostname: settings.hostname.clone(),
    }))
}

// Cross-cfg stubs so the dispatch in `start()` compiles on every
// target. `detect_provider()` only ever returns `Avahi` on Linux
// and `Bonjour` on macOS/Windows, so the "wrong" branch is
// unreachable in practice — but the compiler doesn't know that.
#[cfg(not(target_os = "linux"))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "stub satisfies cross-cfg dispatch"
)]
fn start_avahi(_: &Settings, _: u16) -> anyhow::Result<Option<Handle>> {
    Ok(None)
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "stub satisfies cross-cfg dispatch"
)]
fn start_bonjour(_: &Settings, _: u16) -> anyhow::Result<Option<Handle>> {
    Ok(None)
}

fn first_lan_ipv4() -> Option<IpAddr> {
    let (ips, _) = lan_ipv4s_with_virtual_filtered();
    ips.into_iter().next()
}

/// Enumerate the host's non-loopback, non-link-local IPv4 addresses,
/// excluding virtual / bridge interfaces (Docker, `libvirt`, VPN
/// tunnels, `VirtualBox`, …). Returns the filtered IP set plus the
/// names of the virtual interfaces we excluded — kept as part of
/// the public(crate) surface because `cast_sender::handlers` and
/// `api::network` use it for the receiver-facing URL builder + the
/// LAN probe respectively.
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

/// Heuristic: does this interface name belong to a virtual / bridge
/// / VPN interface that mDNS announce shouldn't touch? Kept on the
/// IP-enumeration side for the cast receiver URL builder so a TV
/// never gets handed a 172.x.x.x docker bridge address. The mDNS
/// publish path itself defers interface filtering to Avahi /
/// Bonjour.
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
        assert!(is_virtual_interface_name("docker0"));
        assert!(is_virtual_interface_name("br-0b7f7d848ace"));
        assert!(is_virtual_interface_name("veth123abc"));
        assert!(is_virtual_interface_name("virbr0"));
        assert!(is_virtual_interface_name("vboxnet0"));
        assert!(is_virtual_interface_name("tun0"));
        assert!(is_virtual_interface_name("wg0"));
        assert!(is_virtual_interface_name("flannel.1"));
        assert!(is_virtual_interface_name("zt5u4yz3kf"));
        assert!(!is_virtual_interface_name("eth0"));
        assert!(!is_virtual_interface_name("eth1"));
        assert!(!is_virtual_interface_name("enp6s0"));
        assert!(!is_virtual_interface_name("wlan0"));
        assert!(!is_virtual_interface_name("wlp3s0"));
        assert!(!is_virtual_interface_name("en0"));
    }

    #[tokio::test]
    async fn load_settings_default_when_no_config_row() {
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
        assert!(settings.enabled);
    }

    #[test]
    fn detect_provider_returns_a_status() {
        // Doesn't assert anything specific about the host (CI / dev
        // container may or may not have avahi installed); just
        // confirms the function doesn't panic and the variant
        // serialises cleanly.
        let status = detect_provider();
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("provider"));
    }
}
