//! mDNS-based Chromecast discovery.
//!
//! Browses the standard `_googlecast._tcp.local.` service type and
//! upserts each discovered device into `cast_device` with
//! `source = 'mdns'`. The mDNS daemon itself owns the announce /
//! goodbye / TTL machinery — we just react to `ServiceResolved` /
//! `ServiceRemoved` events.
//!
//! ## Networking caveat
//!
//! mDNS is link-local multicast (224.0.0.251:5353). Inside Docker's
//! default bridge network, multicast doesn't cross from the host
//! LAN into the container — so the browse loop returns nothing in
//! the dev container. Users on a Docker install (recommended:
//! `network_mode: host`) or running the native binary directly see
//! their Chromecasts immediately. The manual-add-by-IP handler is
//! the workaround for the bridge case.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use chrono::Utc;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use sqlx::SqlitePool;

static LAST_LOGGED: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The Cast-protocol mDNS service type. Every Chromecast announces
/// under this, regardless of model / generation.
const SERVICE_TYPE: &str = "_googlecast._tcp.local.";

/// Spawn the long-running mDNS browser. Returns immediately; the
/// daemon runs in a dedicated tokio task that drives an internal
/// `mdns-sd` background thread.
///
/// Errors only on `ServiceDaemon::new()` failure — once the daemon
/// is up, individual browse-event errors are logged and skipped
/// rather than killing the loop.
pub fn spawn(pool: SqlitePool) -> anyhow::Result<()> {
    let daemon = ServiceDaemon::new()?;
    let receiver = daemon.browse(SERVICE_TYPE)?;

    tokio::spawn(async move {
        // Keep the daemon owned by the task so it lives as long as
        // the loop. Dropping it would kill the underlying browse
        // thread.
        let _daemon_guard = daemon;
        loop {
            match receiver.recv_async().await {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    if let Err(e) = upsert_resolved(&pool, &info).await {
                        tracing::warn!(
                            error = %e,
                            fullname = %info.get_fullname(),
                            "cast: failed to upsert mDNS-resolved device",
                        );
                    }
                }
                Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                    if let Err(e) = mark_removed(&pool, &fullname).await {
                        tracing::warn!(
                            error = %e,
                            fullname = %fullname,
                            "cast: failed to remove mDNS-vanished device",
                        );
                    } else {
                        tracing::info!(fullname = %fullname, "cast: device left LAN");
                    }
                }
                Ok(_) => {
                    // Other variants (SearchStarted, ServiceFound
                    // without resolved hostname yet, etc.) — ignore;
                    // we only act on the fully-resolved + removed
                    // events.
                }
                Err(e) => {
                    tracing::error!(error = %e, "cast: mDNS browse channel closed");
                    // Channel closed = daemon died. Backoff briefly
                    // and break — the supervisor (main.rs) will
                    // restart on next boot. We don't try to
                    // resurrect inside the loop because failures
                    // here usually mean the OS dropped multicast
                    // permission or similar non-transient state.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    break;
                }
            }
        }
    });

    Ok(())
}

/// Upsert an mDNS-resolved device. The `fullname` is stable across
/// re-announcements for the same device, so it doubles as the row
/// id. `last_seen` updates every announcement, letting a future
/// stale-row sweep prune anything not seen in N minutes.
async fn upsert_resolved(pool: &SqlitePool, info: &mdns_sd::ServiceInfo) -> sqlx::Result<()> {
    let id = info.get_fullname().to_owned();
    let name = info
        .get_property_val_str("fn")
        .map(str::to_owned)
        .or_else(|| Some(strip_service_suffix(&id).to_owned()))
        .unwrap_or_else(|| id.clone());
    let model = info.get_property_val_str("md").map(str::to_owned);
    // Pick the first IPv4. mdns-sd returns a HashSet of IpAddr —
    // Chromecasts mostly publish v4 + v6; v4 is the reliable path
    // for the TCP control socket.
    let ip = info
        .get_addresses_v4()
        .iter()
        .next()
        .map_or_else(|| "0.0.0.0".to_owned(), std::string::ToString::to_string);
    let port = i64::from(info.get_port());
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO cast_device
            (id, name, ip, port, model, source, last_seen, created_at)
         VALUES (?, ?, ?, ?, ?, 'mdns', ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            name      = excluded.name,
            ip        = excluded.ip,
            port      = excluded.port,
            model     = excluded.model,
            last_seen = excluded.last_seen",
    )
    .bind(&id)
    .bind(&name)
    .bind(&ip)
    .bind(port)
    .bind(model.as_deref())
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    if ip == "0.0.0.0" {
        tracing::debug!(id = %id, name = %name, "cast: mDNS announce with no IPv4 yet (will resolve)");
    } else {
        let mut seen = LAST_LOGGED.lock().expect("LAST_LOGGED mutex poisoned");
        match seen.get(&id) {
            Some(prev) if prev == &ip => {
                tracing::debug!(id = %id, ip = %ip, "cast: duplicate mDNS resolution");
            }
            _ => {
                tracing::info!(id = %id, name = %name, ip = %ip, "cast: device discovered via mDNS");
                seen.insert(id.clone(), ip.clone());
            }
        }
    }
    Ok(())
}

/// Drop an mDNS row when the device leaves. Manually-added rows
/// (`source = 'manual'`) survive — the user added them on purpose.
async fn mark_removed(pool: &SqlitePool, fullname: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM cast_device WHERE id = ? AND source = 'mdns'")
        .bind(fullname)
        .execute(pool)
        .await?;
    Ok(())
}

/// `_googlecast._tcp.local.` suffix → strip → "Living-Room-TV".
/// Fallback display name when the device omits its `fn` TXT entry.
fn strip_service_suffix(fullname: &str) -> &str {
    fullname
        .strip_suffix(&format!(".{SERVICE_TYPE}"))
        .unwrap_or(fullname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_service_suffix_handles_typical_chromecast_name() {
        assert_eq!(
            strip_service_suffix("Chromecast-abc123._googlecast._tcp.local."),
            "Chromecast-abc123"
        );
        // Already-stripped fallback returns the input unchanged.
        assert_eq!(strip_service_suffix("plain-name"), "plain-name");
    }
}
