//! Periodic VPN health check.
//!
//! Strategy: a live `WireGuard` tunnel MUST see a handshake roughly every
//! 3 minutes (rekey interval + 30s grace). If we haven't seen one for
//! longer than that, the tunnel is dead — tear down and reconnect.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use super::{VpnConfig, VpnManager, killswitch};
use crate::events::AppEvent;

/// Threshold for "the tunnel is alive". 3 minutes matches the `WireGuard`
/// rekey interval + a small grace buffer.
const HANDSHAKE_MAX_AGE_SECS: u64 = 180;

/// Run one health-check pass. Returns true if the tunnel is healthy or
/// was successfully reconnected.
///
/// When `vpn_killswitch_enabled` is on, a stale-handshake event causes
/// every active download to pause cleanly *before* the disconnect, and
/// to resume after the next successful reconnect. Without the
/// killswitch the disconnect still happens; downloads just churn until
/// `bind_device_name` fails them at the kernel layer.
pub async fn check_once(
    pool: &SqlitePool,
    vpn: Arc<VpnManager>,
    torrent: Option<&dyn crate::download::session::TorrentSession>,
    event_tx: &broadcast::Sender<AppEvent>,
) -> anyhow::Result<bool> {
    if !vpn.is_connected() {
        tracing::debug!("VPN health: not connected, skipping");
        return Ok(false);
    }

    let age = match vpn.last_handshake().await {
        Some(t) => t.elapsed(),
        None => Duration::from_secs(HANDSHAKE_MAX_AGE_SECS * 2),
    };
    tracing::debug!(
        age_secs = age.as_secs(),
        threshold_secs = HANDSHAKE_MAX_AGE_SECS,
        "VPN health tick",
    );

    if age.as_secs() <= HANDSHAKE_MAX_AGE_SECS {
        // Process-restart recovery: if the prior process was killed
        // mid-pause-cycle, downloads still carry the killswitch
        // marker. A healthy tunnel means it's safe to flip them back
        // on. Cheap when there's nothing flagged.
        if killswitch::is_enabled(pool).await {
            let resumed = killswitch::resume_killswitch_paused(pool, torrent, event_tx).await?;
            if resumed > 0 {
                tracing::info!(
                    count = resumed,
                    "killswitch: resumed downloads on healthy tick"
                );
            }
        }
        return Ok(true);
    }

    tracing::warn!(
        age_secs = age.as_secs(),
        "VPN handshake stale, reconnecting"
    );

    // Pause-all *before* disconnect so peer sockets stop trying to
    // route over the failing tunnel. Skipped when the user has
    // disabled the killswitch — they explicitly opted for the
    // bind-device-name fail-closed path only.
    let killswitch_on = killswitch::is_enabled(pool).await;
    if killswitch_on {
        match killswitch::pause_all_active(pool, torrent, event_tx).await {
            Ok(n) if n > 0 => {
                tracing::info!(count = n, "killswitch: paused downloads pre-reconnect");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "killswitch pause sweep failed; continuing"),
        }
    }

    // Re-read config from db (may have been updated by user).
    let cfg = load_config(pool).await?;
    if let Err(e) = vpn.disconnect().await {
        // Disconnect failure isn't fatal — connect() will force-rebuild.
        // Worth surfacing because it hints the previous tunnel leaked.
        tracing::warn!(error = %e, "VPN disconnect failed before reconnect");
    }
    vpn.connect(&cfg).await?;

    if killswitch_on {
        match killswitch::resume_killswitch_paused(pool, torrent, event_tx).await {
            Ok(n) if n > 0 => {
                tracing::info!(count = n, "killswitch: resumed downloads post-reconnect");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "killswitch resume sweep failed; continuing"),
        }
    }

    Ok(true)
}

pub async fn load_config(pool: &SqlitePool) -> anyhow::Result<VpnConfig> {
    type Row = (
        Option<String>, // private_key
        Option<String>, // address
        Option<String>, // server_public_key
        Option<String>, // server_endpoint
        Option<String>, // dns
        String,         // port_forward_provider (NOT NULL with default)
        Option<String>, // port_forward_api_key
    );

    let row: Row = sqlx::query_as(
        "SELECT vpn_private_key, vpn_address, vpn_server_public_key, vpn_server_endpoint,
                vpn_dns, vpn_port_forward_provider, vpn_port_forward_api_key
         FROM config WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;

    Ok(VpnConfig {
        private_key: row
            .0
            .ok_or_else(|| anyhow::anyhow!("vpn_private_key not configured"))?,
        address: row
            .1
            .ok_or_else(|| anyhow::anyhow!("vpn_address not configured"))?,
        server_public_key: row
            .2
            .ok_or_else(|| anyhow::anyhow!("vpn_server_public_key not configured"))?,
        server_endpoint: row
            .3
            .ok_or_else(|| anyhow::anyhow!("vpn_server_endpoint not configured"))?,
        dns: row.4,
        port_forward_provider: row.5,
        port_forward_api_key: row.6,
    })
}
