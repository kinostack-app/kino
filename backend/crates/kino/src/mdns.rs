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

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceInfo};
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
pub fn start(settings: &Settings, port: u16) -> anyhow::Result<Option<Handle>> {
    if !settings.enabled {
        tracing::info!("mDNS disabled in config");
        return Ok(None);
    }
    let ips = lan_ipv4s();
    if ips.is_empty() {
        tracing::warn!("no LAN IPv4 interfaces found; skipping mDNS advertisement");
        return Ok(None);
    }

    let daemon = ServiceDaemon::new().context("create mdns daemon")?;

    // Hostnames in the `_http._tcp.local.` record need the trailing
    // dot to be FQDN-shaped per the mDNS spec; the library accepts
    // either form but the dot is the explicit one.
    let host_fqdn = format!("{}.local.", settings.hostname);

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
        settings.hostname,
        port,
    );
    Ok(Some(Handle { daemon, fullname }))
}

/// Enumerate the host's non-loopback, non-link-local IPv4 addresses.
/// IPv6 records would be ideal too but most home routers don't forward
/// IPv6 link-local mDNS reliably; the IPv4 A record is the path that
/// works everywhere.
fn lan_ipv4s() -> Vec<IpAddr> {
    if_addrs::get_if_addrs()
        .ok()
        .into_iter()
        .flatten()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_link_local() && !v4.is_unspecified() => Some(IpAddr::V4(v4)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_ipv4s_skips_loopback_and_link_local() {
        for ip in lan_ipv4s() {
            assert!(!ip.is_loopback(), "loopback leaked: {ip}");
            if let IpAddr::V4(v4) = ip {
                assert!(!v4.is_link_local(), "link-local leaked: {v4}");
            }
        }
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
