//! `GET /api/v1/health` — composed dashboard snapshot across every
//! subsystem with live state worth surfacing. Each panel is built by
//! its own collector; collectors run in parallel and any that fails
//! or times out is returned with `status = unknown` instead of
//! failing the whole response. See `docs/subsystems/20-health-dashboard.md`.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::error::AppResult;
use crate::state::AppState;

/// Cap on how long any single panel collector may run. Beyond this
/// the whole panel is marked `unknown` so a slow subsystem can't
/// stall the entire page.
const PANEL_TIMEOUT: Duration = Duration::from_millis(500);

/// Shared panel status. `unknown` means "we couldn't collect" and is
/// rendered as a neutral "Couldn't check" card — it never escalates
/// `overall` above `operational`.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Operational,
    Degraded,
    Critical,
    Unknown,
}

impl HealthStatus {
    fn rank(self) -> u8 {
        match self {
            // Unknown reaches this function only via direct calls from
            // tests / debugging. In production flow Unknown is filtered
            // out before ranking — see `worst_of`.
            Self::Operational | Self::Unknown => 0,
            Self::Degraded => 1,
            Self::Critical => 2,
        }
    }

    /// Aggregate multiple panel statuses into a single overall state.
    /// `Unknown` is deliberately filtered before ranking so a panel we
    /// couldn't reach doesn't degrade `overall` — we don't want a
    /// slow collector making the banner go amber.
    fn worst_of(states: &[Self]) -> Self {
        states
            .iter()
            .copied()
            .filter(|s| !matches!(s, Self::Unknown))
            .max_by_key(|s| s.rank())
            .unwrap_or(Self::Operational)
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub overall: HealthStatus,
    pub checked_at: String,
    pub panels: HealthPanels,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthPanels {
    pub storage: StoragePanel,
    /// Hidden when VPN is disabled in config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vpn: Option<VpnPanel>,
    pub indexers: IndexersPanel,
    pub downloads: DownloadsPanel,
    pub transcoder: TranscoderPanel,
    pub scheduler: SchedulerPanel,
    pub metadata: MetadataPanel,
}

// ── Storage ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct StoragePanel {
    pub status: HealthStatus,
    pub summary: String,
    pub paths: Vec<StoragePath>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StoragePath {
    /// Human label ("Media library", "Downloads", "Kino data").
    pub label: String,
    pub path: String,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// Rounded percentage 0-100. None when we couldn't stat the path.
    pub free_pct: Option<u8>,
    /// Bytes this folder is responsible for, derived from the DB
    /// (`SUM(media.size)` for the library, `SUM(downloaded)` for the
    /// downloads). None for the data path or any folder we don't
    /// track per-byte. Lets the UI show "kino is using X of Y" per
    /// folder without walking the filesystem.
    pub used_bytes: Option<u64>,
    /// Filesystem device id (Unix `dev()`) so the UI can group folders
    /// that share a drive. None on platforms / errors where we can't
    /// stat. Two paths with the same `device_id` live on the same
    /// physical (or logical) disk and share `free_bytes`/`total_bytes`.
    pub device_id: Option<u64>,
}

// ── VPN ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct VpnPanel {
    pub status: HealthStatus,
    pub summary: String,
    /// "connected" | "connecting" | "disconnected" | "error"
    pub connection_state: String,
    pub interface: Option<String>,
    pub forwarded_port: Option<u16>,
    pub last_handshake_ago_secs: Option<u64>,
    /// Subsystem 33 phase B: most recent IP-leak self-test result.
    /// `Some(true)` = observed egress matches VPN endpoint. `Some(false)`
    /// = leak detected (downloads were paused). `None` = inconclusive
    /// or test hasn't run yet.
    pub protected: Option<bool>,
    pub leak_observed_ip: Option<String>,
    pub leak_expected_ip: Option<String>,
    pub last_leak_check_ago_secs: Option<u64>,
    pub last_leak_check_error: Option<String>,
}

// ── Indexers ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct IndexersPanel {
    pub status: HealthStatus,
    pub summary: String,
    pub total: i64,
    pub healthy: i64,
    pub failing: i64,
    pub disabled: i64,
    pub items: Vec<IndexerItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IndexerItem {
    pub id: i64,
    pub name: String,
    /// "healthy" | "failing" | "disabled"
    pub state: String,
    pub escalation_level: i64,
    pub last_failure_at: Option<String>,
}

// ── Downloads ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadsPanel {
    pub status: HealthStatus,
    pub summary: String,
    pub active: i64,
    pub importing: i64,
    pub failed_last_24h: i64,
    pub stuck_importing: i64,
}

// ── Transcoder ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct TranscoderPanel {
    pub status: HealthStatus,
    pub summary: String,
    pub active_sessions: usize,
    pub session_cap: i64,
    pub ffmpeg_available: bool,
    pub hw_acceleration: String,
    /// True when a hardware backend is available but config is "none".
    /// Hint for the UI to prompt the user; not an error state on its own.
    pub hw_available_but_off: bool,
}

// ── Scheduler ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct SchedulerPanel {
    pub status: HealthStatus,
    pub summary: String,
    pub failing_tasks: Vec<FailingTask>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FailingTask {
    pub name: String,
    pub last_error: String,
    pub last_run_at: Option<String>,
}

// ── Metadata (TMDB) ────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct MetadataPanel {
    pub status: HealthStatus,
    pub summary: String,
    pub tmdb_configured: bool,
}

// ── Handler ───────────────────────────────────────────────────────

/// `GET /api/v1/health` — one-shot snapshot of every dashboard panel.
/// Panels collect in parallel; a stuck collector times out at 500ms
/// and is marked `unknown` so the rest of the page still loads.
#[utoipa::path(
    get, path = "/api/v1/health",
    responses((status = 200, body = HealthResponse)),
    tag = "system", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn get_health(State(state): State<AppState>) -> AppResult<Json<HealthResponse>> {
    let (storage, vpn, indexers, downloads, transcoder, scheduler, metadata) = tokio::join!(
        with_timeout("storage", collect_storage(&state)),
        with_timeout("vpn", collect_vpn(&state)),
        with_timeout("indexers", collect_indexers(&state.db)),
        with_timeout("downloads", collect_downloads(&state.db)),
        with_timeout("transcoder", collect_transcoder(&state)),
        with_timeout("scheduler", collect_scheduler(&state)),
        with_timeout("metadata", collect_metadata(&state)),
    );

    let storage = storage.unwrap_or_else(|| StoragePanel {
        status: HealthStatus::Unknown,
        summary: "Couldn't check storage".into(),
        paths: vec![],
    });
    let indexers = indexers.unwrap_or_else(|| IndexersPanel {
        status: HealthStatus::Unknown,
        summary: "Couldn't check indexers".into(),
        total: 0,
        healthy: 0,
        failing: 0,
        disabled: 0,
        items: vec![],
    });
    let downloads = downloads.unwrap_or_else(|| DownloadsPanel {
        status: HealthStatus::Unknown,
        summary: "Couldn't check downloads".into(),
        active: 0,
        importing: 0,
        failed_last_24h: 0,
        stuck_importing: 0,
    });
    let transcoder = transcoder.unwrap_or_else(|| TranscoderPanel {
        status: HealthStatus::Unknown,
        summary: "Couldn't check transcoder".into(),
        active_sessions: 0,
        session_cap: 0,
        ffmpeg_available: false,
        hw_acceleration: "none".into(),
        hw_available_but_off: false,
    });
    let scheduler = scheduler.unwrap_or_else(|| SchedulerPanel {
        status: HealthStatus::Unknown,
        summary: "Couldn't check scheduler".into(),
        failing_tasks: vec![],
    });
    let metadata = metadata.unwrap_or_else(|| MetadataPanel {
        status: HealthStatus::Unknown,
        summary: "Couldn't check metadata".into(),
        tmdb_configured: false,
    });
    // `vpn` is already Option<VpnPanel>; `with_timeout` wraps that in
    // another Option so a None from the timeout and a None from "VPN
    // disabled" both collapse to the same hidden-panel state.
    let vpn = vpn.flatten();

    let mut states = vec![
        storage.status,
        indexers.status,
        downloads.status,
        transcoder.status,
        scheduler.status,
        metadata.status,
    ];
    if let Some(ref v) = vpn {
        states.push(v.status);
    }
    let overall = HealthStatus::worst_of(&states);

    Ok(Json(HealthResponse {
        overall,
        checked_at: Utc::now().to_rfc3339(),
        panels: HealthPanels {
            storage,
            vpn,
            indexers,
            downloads,
            transcoder,
            scheduler,
            metadata,
        },
    }))
}

/// Wrap a collector future in `PANEL_TIMEOUT`. On timeout, logs a
/// warn + returns None so the handler falls back to an `unknown` panel.
async fn with_timeout<T, F: std::future::Future<Output = T>>(name: &str, fut: F) -> Option<T> {
    if let Ok(v) = tokio::time::timeout(PANEL_TIMEOUT, fut).await {
        Some(v)
    } else {
        tracing::warn!(panel = name, "health panel collector timed out");
        None
    }
}

// ── Collectors ────────────────────────────────────────────────────

/// Return the underlying filesystem device id for a path, used by the
/// UI to group paths that live on the same drive. Unix-only — on
/// Windows we'd want `GetVolumePathName` + drive-letter comparison;
/// since kino's deployment target is Linux containers + macOS dev,
/// returning None on Windows is fine.
fn device_id_for(path: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(path).ok().map(|m| m.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

async fn collect_storage(state: &AppState) -> StoragePanel {
    // Query the three paths we care about from config. Data path
    // comes from AppState since that's already known at boot; library
    // + download paths come from the config row so we reflect any
    // post-setup changes.
    #[derive(sqlx::FromRow)]
    struct Row {
        media_library_path: Option<String>,
        download_path: Option<String>,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT media_library_path, download_path FROM config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let data_path = state.data_path.to_string_lossy().to_string();
    // (label, path, kind) — kind drives the per-folder used_bytes
    // lookup below. "data" has no DB-tracked size; "library" sums
    // media.size; "downloads" sums currently-downloaded bytes.
    let mut paths: Vec<(&'static str, String, &'static str)> =
        vec![("Kino data", data_path, "data")];
    if let Some(r) = row {
        if let Some(p) = r.media_library_path.filter(|s| !s.is_empty()) {
            paths.push(("Media library", p, "library"));
        }
        if let Some(p) = r.download_path.filter(|s| !s.is_empty()) {
            paths.push(("Downloads", p, "downloads"));
        }
    }

    // Cheap DB sums — much faster than walking the filesystem and
    // accurate for kino-managed bytes (the user could have other files
    // in the same folders, which `total - free` on the volume would
    // capture but those aren't kino's to attribute).
    let library_used: Option<u64> = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size), 0) FROM media WHERE size IS NOT NULL",
    )
    .fetch_one(&state.db)
    .await
    .ok()
    .and_then(|n| u64::try_from(n).ok());
    let downloads_used: Option<u64> = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(downloaded), 0) FROM download
         WHERE state IN ('searching','queued','grabbing','downloading','paused','stalled','completed','importing','seeding')",
    )
    .fetch_one(&state.db)
    .await
    .ok()
    .and_then(|n| u64::try_from(n).ok());

    let mut entries: Vec<StoragePath> = Vec::with_capacity(paths.len());
    for (label, path, kind) in paths {
        let p = std::path::Path::new(&path);
        let free = fs4::available_space(p).ok();
        let total = fs4::total_space(p).ok();
        let pct = free.zip(total).and_then(|(f, t)| {
            if t == 0 {
                None
            } else {
                // Safe cast: percentage always fits in a u8.
                #[allow(clippy::cast_possible_truncation)]
                Some(((f * 100) / t) as u8)
            }
        });
        let device_id = device_id_for(p);
        let used_bytes = match kind {
            "library" => library_used,
            "downloads" => downloads_used,
            _ => None,
        };
        entries.push(StoragePath {
            label: label.to_owned(),
            path,
            free_bytes: free,
            total_bytes: total,
            free_pct: pct,
            used_bytes,
            device_id,
        });
    }

    // Spec: degraded if any path < 10% free, critical if any < 2%.
    let min_pct = entries.iter().filter_map(|e| e.free_pct).min();
    // Nothing-to-stat (fresh install, no paths set) falls through to
    // the `Some(_) | None` arm as operational — benign default.
    let status = match min_pct {
        Some(pct) if pct < 2 => HealthStatus::Critical,
        Some(pct) if pct < 10 => HealthStatus::Degraded,
        Some(_) | None => HealthStatus::Operational,
    };

    let summary = match status {
        HealthStatus::Critical => format!("Disk critically low ({} % free)", min_pct.unwrap_or(0)),
        HealthStatus::Degraded => format!("Disk getting low ({} % free)", min_pct.unwrap_or(0)),
        _ => "All drives healthy".into(),
    };

    StoragePanel {
        status,
        summary,
        paths: entries,
    }
}

async fn collect_vpn(state: &AppState) -> Option<VpnPanel> {
    let vpn = state.vpn.as_ref()?;
    let conn = match vpn.status() {
        crate::download::vpn::VpnStatus::Connected => "connected",
        crate::download::vpn::VpnStatus::Connecting => "connecting",
        crate::download::vpn::VpnStatus::Disconnected => "disconnected",
        crate::download::vpn::VpnStatus::Error => "error",
    };
    let last_handshake_ago_secs = vpn.last_handshake().await.map(|t| t.elapsed().as_secs());

    // Operational while the tunnel is up. Degraded for a transient
    // reconnect. Critical once we've been disconnected/errored for
    // more than two minutes or never completed a handshake.
    let status = match (conn, last_handshake_ago_secs) {
        ("connected", _) => HealthStatus::Operational,
        ("connecting", _) => HealthStatus::Degraded,
        (_, Some(secs)) if secs > 120 => HealthStatus::Critical,
        (_, None) => HealthStatus::Critical,
        _ => HealthStatus::Degraded,
    };

    let summary = match status {
        HealthStatus::Operational => match vpn.forwarded_port() {
            Some(port) => format!("Connected · forwarded port {port}"),
            None => "Connected".into(),
        },
        HealthStatus::Degraded => "Reconnecting…".into(),
        HealthStatus::Critical => "Tunnel down".into(),
        HealthStatus::Unknown => "Unknown".into(),
    };

    let leak = vpn.leak_status();
    let last_leak_check_ago_secs = leak
        .as_ref()
        .and_then(|s| s.checked_at.elapsed().ok().map(|d| d.as_secs()));
    let (protected, leak_observed_ip, leak_expected_ip, last_leak_check_error) =
        leak.as_ref().map_or((None, None, None, None), |s| {
            (
                s.protected,
                s.observed_ip.map(|i| i.to_string()),
                s.expected_ip.map(|i| i.to_string()),
                s.last_error.clone(),
            )
        });

    Some(VpnPanel {
        status,
        summary,
        connection_state: conn.to_owned(),
        interface: Some(vpn.interface_name().to_string()),
        forwarded_port: vpn.forwarded_port(),
        last_handshake_ago_secs,
        protected,
        leak_observed_ip,
        leak_expected_ip,
        last_leak_check_ago_secs,
        last_leak_check_error,
    })
}

async fn collect_indexers(db: &SqlitePool) -> IndexersPanel {
    // Pull the minimum set of columns we need — name, health columns.
    // Ordered by escalation descending so the UI naturally surfaces
    // the worst offenders at the top of the list.
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        name: String,
        enabled: bool,
        escalation_level: i64,
        most_recent_failure_time: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, enabled, escalation_level, most_recent_failure_time
         FROM indexer ORDER BY escalation_level DESC, name ASC",
    )
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let total = i64::try_from(rows.len()).unwrap_or(0);
    let mut healthy = 0i64;
    let mut failing = 0i64;
    let mut disabled = 0i64;
    let mut items: Vec<IndexerItem> = Vec::with_capacity(rows.len());
    for r in rows {
        let state = if !r.enabled {
            disabled += 1;
            "disabled"
        } else if r.escalation_level > 0 {
            failing += 1;
            "failing"
        } else {
            healthy += 1;
            "healthy"
        };
        items.push(IndexerItem {
            id: r.id,
            name: r.name,
            state: state.to_owned(),
            escalation_level: r.escalation_level,
            last_failure_at: r.most_recent_failure_time,
        });
    }

    // Spec: degraded if any indexer at escalation >= 2; critical if
    // any indexer disabled by the health sweep. An operator-disabled
    // indexer is still "disabled" and worth flagging — we don't
    // distinguish operator vs auto-disable yet.
    let any_escalated = items
        .iter()
        .any(|i| i.state == "failing" && i.escalation_level >= 2);
    let any_failing = failing > 0;
    let none_configured = total == 0;

    let (status, summary) = if none_configured {
        (HealthStatus::Degraded, "No indexers configured".to_owned())
    } else if disabled > 0 && healthy == 0 {
        (
            HealthStatus::Critical,
            format!("{disabled} indexer(s) disabled; none healthy"),
        )
    } else if any_escalated {
        (
            HealthStatus::Degraded,
            format!("{failing} indexer(s) failing"),
        )
    } else if any_failing {
        (
            HealthStatus::Degraded,
            format!("{failing} indexer(s) in backoff"),
        )
    } else {
        (
            HealthStatus::Operational,
            format!("{healthy} indexer(s) healthy"),
        )
    };

    IndexersPanel {
        status,
        summary,
        total,
        healthy,
        failing,
        disabled,
        items,
    }
}

async fn collect_downloads(db: &SqlitePool) -> DownloadsPanel {
    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE state IN ('queued','downloading')")
            .fetch_one(db)
            .await
            .unwrap_or(0);
    let importing: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE state = 'importing'")
            .fetch_one(db)
            .await
            .unwrap_or(0);
    let stuck_importing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM download WHERE state = 'importing'
         AND datetime(added_at) < datetime('now', '-10 minutes')",
    )
    .fetch_one(db)
    .await
    .unwrap_or(0);
    let failed_last_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM download WHERE state = 'failed'
         AND datetime(added_at) > datetime('now', '-1 day')",
    )
    .fetch_one(db)
    .await
    .unwrap_or(0);

    let (status, summary) = if stuck_importing > 0 {
        (
            HealthStatus::Critical,
            format!("{stuck_importing} import(s) stuck"),
        )
    } else if failed_last_24h > 2 {
        (
            HealthStatus::Degraded,
            format!("{failed_last_24h} failure(s) in last 24h"),
        )
    } else if active > 0 {
        (
            HealthStatus::Operational,
            format!("{active} active download(s)"),
        )
    } else {
        (HealthStatus::Operational, "Idle".to_owned())
    };

    DownloadsPanel {
        status,
        summary,
        active,
        importing,
        failed_last_24h,
        stuck_importing,
    }
}

async fn collect_transcoder(state: &AppState) -> TranscoderPanel {
    let ffmpeg_available = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());

    let active_sessions = match state.transcode.as_ref() {
        Some(t) => t.active_session_count().await,
        None => 0,
    };
    let session_cap: i64 =
        sqlx::query_scalar("SELECT max_concurrent_transcodes FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or(2);

    let hw_acceleration: String =
        sqlx::query_scalar("SELECT hw_acceleration FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "none".into());
    let hw_available_but_off = crate::playback::hw_probe_cache::cached()
        .is_some_and(|caps| caps.any_available() && hw_acceleration == "none");

    let session_cap_usize = usize::try_from(session_cap).unwrap_or(usize::MAX);
    let (status, summary) = if !ffmpeg_available {
        (HealthStatus::Critical, "FFmpeg not found".to_owned())
    } else if active_sessions >= session_cap_usize && session_cap > 0 {
        (
            HealthStatus::Degraded,
            format!("At cap ({active_sessions}/{session_cap})"),
        )
    } else if active_sessions == 0 {
        (HealthStatus::Operational, "Idle".to_owned())
    } else {
        (
            HealthStatus::Operational,
            format!("{active_sessions} session(s) active"),
        )
    };

    TranscoderPanel {
        status,
        summary,
        active_sessions,
        session_cap,
        ffmpeg_available,
        hw_acceleration,
        hw_available_but_off,
    }
}

async fn collect_scheduler(state: &AppState) -> SchedulerPanel {
    let Some(scheduler) = state.scheduler.as_ref() else {
        return SchedulerPanel {
            status: HealthStatus::Unknown,
            summary: "Scheduler not running".into(),
            failing_tasks: vec![],
        };
    };
    let tasks = scheduler.list_tasks().await;
    let failing: Vec<FailingTask> = tasks
        .into_iter()
        .filter_map(|t| {
            t.last_error.map(|e| FailingTask {
                name: t.name,
                last_error: e,
                last_run_at: t.last_run_at,
            })
        })
        .collect();

    let (status, summary) = if failing.is_empty() {
        (HealthStatus::Operational, "All tasks healthy".to_owned())
    } else {
        (
            HealthStatus::Degraded,
            format!("{} task(s) failing", failing.len()),
        )
    };

    SchedulerPanel {
        status,
        summary,
        failing_tasks: failing,
    }
}

// No awaits inside — metadata panel is a cheap Option check — but
// `with_timeout` below expects a future so we keep the async fn
// signature. The allow is local rather than file-wide.
#[allow(clippy::unused_async)]
async fn collect_metadata(state: &AppState) -> MetadataPanel {
    let tmdb_configured = state.tmdb_snapshot().is_some();
    let (status, summary) = if tmdb_configured {
        (HealthStatus::Operational, "TMDB connected".to_owned())
    } else {
        (
            HealthStatus::Degraded,
            "TMDB not configured — browse and metadata disabled".to_owned(),
        )
    };
    MetadataPanel {
        status,
        summary,
        tmdb_configured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_of_ranks_critical_above_degraded() {
        assert!(matches!(
            HealthStatus::worst_of(&[HealthStatus::Operational, HealthStatus::Degraded]),
            HealthStatus::Degraded
        ));
        assert!(matches!(
            HealthStatus::worst_of(&[HealthStatus::Degraded, HealthStatus::Critical]),
            HealthStatus::Critical
        ));
        // Unknown doesn't promote the overall status.
        assert!(matches!(
            HealthStatus::worst_of(&[HealthStatus::Operational, HealthStatus::Unknown]),
            HealthStatus::Operational
        ));
    }

    #[test]
    fn worst_of_empty_is_operational() {
        assert!(matches!(
            HealthStatus::worst_of(&[]),
            HealthStatus::Operational
        ));
    }
}
