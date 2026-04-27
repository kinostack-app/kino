//! `GET /api/v1/diagnostics/export` — single-shot triage bundle.
//!
//! Returns an `application/zip` containing everything an operator
//! (or kino's own bug-report flow) needs to diagnose an issue
//! without asking the user to dig through `SQLite` logs by hand.
//! Sensitive fields (API keys, VPN secrets, indexer credentials)
//! are redacted before they leave the binary.
//!
//! Bundle layout:
//! ```text
//!   meta.json                — kino version, OS, ffmpeg, schema version, generation timestamp
//!   config-redacted.json     — config row with sensitive fields replaced by "[REDACTED]"
//!   indexers-redacted.json   — indexer list with api_key + settings_json redacted
//!   backups.json             — backup metadata (kinds, timestamps, sizes — not archive blobs)
//!   tasks.json               — scheduler registry + last_error / last_run_at
//!   service-descriptor.txt   — systemd unit / launchd plist text (Linux + macOS only)
//!   logs/last-7d.jsonl       — log_entry rows from the last 7 days, one per line
//! ```
//!
//! Bundle is built in-memory then returned as a single response body
//! — typical size is a handful of MB even with 7d of debug logs, so
//! streaming isn't worth the complexity.

use std::io::Write as _;

use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
// `Column` brings the `.name()` accessor onto each column ref;
// `Row` brings `.try_get` and `.columns()` onto SqliteRow.
use sqlx::{Column as _, Row as _};
use utoipa::ToSchema;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Window of `log_entry` rows included in the bundle. 7 days covers
/// every common bug-report timeframe ("it broke yesterday", "this
/// has been happening all week") without inflating the archive past
/// what's reasonable to upload to a GitHub issue.
const LOG_WINDOW_DAYS: i64 = 7;

/// Hard cap on the number of log rows we'll serialise into the
/// bundle. Even at 7 days, a chatty install (DEBUG level on a busy
/// scheduler) can produce a hundred thousand rows; capping at 50k
/// keeps the archive small + fast to generate. The most recent rows
/// are kept (`ORDER BY ts_us DESC`).
const LOG_ROW_CAP: i64 = 50_000;

/// Marker used wherever we replace a sensitive value. Keeps the JSON
/// shape stable so the operator reading the bundle can still tell at
/// a glance which fields were *set* (vs absent / null).
const REDACTED: &str = "[REDACTED]";

/// `GET /api/v1/diagnostics/export` — build + return a ZIP bundle.
///
/// Probe-style endpoint: doesn't mutate state, doesn't emit
/// `AppEvent`. The frontend offers this as a "Download diagnostic
/// bundle" button under Settings → Diagnostics.
#[utoipa::path(
    get, path = "/api/v1/diagnostics/export",
    responses(
        (status = 200, description = "ZIP bundle", content_type = "application/zip"),
    ),
    tag = "system", security(("api_key" = []))
)]
pub async fn export_bundle(State(state): State<AppState>) -> AppResult<Response> {
    let bytes = build_bundle(&state)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("build diagnostic bundle: {e:#}")))?;

    let filename = format!(
        "kino-diagnostics-{}.zip",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("response build: {e}")))?;
    Ok(response)
}

async fn build_bundle(state: &AppState) -> anyhow::Result<Vec<u8>> {
    // Collect the data first, off the zip writer's borrow.
    let meta = collect_meta(state).await;
    let config_json = collect_config_redacted(&state.db).await?;
    let indexers_json = collect_indexers_redacted(&state.db).await?;
    let backups_json = collect_backups(&state.db).await?;
    let tasks_json = collect_tasks(state).await;
    let service_descriptor = render_service_descriptor();
    let log_jsonl = collect_logs_jsonl(&state.db).await?;

    // Build the archive in memory. `zip` requires `Write + Seek`;
    // `std::io::Cursor<Vec<u8>>` is the standard in-memory adapter.
    let mut buf: std::io::Cursor<Vec<u8>> = std::io::Cursor::new(Vec::with_capacity(64 * 1024));
    {
        let mut writer = ZipWriter::new(&mut buf);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        write_entry(
            &mut writer,
            "meta.json",
            &serde_json::to_vec_pretty(&meta)?,
            opts,
        )?;
        write_entry(
            &mut writer,
            "config-redacted.json",
            config_json.as_bytes(),
            opts,
        )?;
        write_entry(
            &mut writer,
            "indexers-redacted.json",
            indexers_json.as_bytes(),
            opts,
        )?;
        write_entry(&mut writer, "backups.json", backups_json.as_bytes(), opts)?;
        write_entry(&mut writer, "tasks.json", tasks_json.as_bytes(), opts)?;
        if let Some(descriptor) = service_descriptor {
            write_entry(
                &mut writer,
                "service-descriptor.txt",
                descriptor.as_bytes(),
                opts,
            )?;
        }
        write_entry(
            &mut writer,
            "logs/last-7d.jsonl",
            log_jsonl.as_bytes(),
            opts,
        )?;

        writer.finish()?;
    }
    Ok(buf.into_inner())
}

fn write_entry<W: std::io::Write + std::io::Seek>(
    writer: &mut ZipWriter<W>,
    name: &str,
    data: &[u8],
    opts: SimpleFileOptions,
) -> anyhow::Result<()> {
    writer.start_file(name, opts)?;
    writer.write_all(data)?;
    Ok(())
}

// ── meta.json ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
struct Meta {
    kino_version: String,
    /// Compile-time profile (`debug` / `release`). Helps when a bug
    /// only shows up in optimised builds (e.g. inlining differences).
    build_profile: String,
    target_triple: String,
    os_family: String,
    os: String,
    arch: String,
    schema_version: i64,
    /// `ffmpeg -version`'s first line (parsed) when ffmpeg is on
    /// `PATH` or the bundled binary is reachable; None when neither
    /// works.
    ffmpeg_version: Option<String>,
    generated_at: String,
}

async fn collect_meta(state: &AppState) -> Meta {
    let ffmpeg_version = ffmpeg_version(state.transcode.as_ref()).await;
    Meta {
        kino_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_profile: if cfg!(debug_assertions) {
            "debug".into()
        } else {
            "release".into()
        },
        target_triple: target_triple(),
        os_family: std::env::consts::FAMILY.to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        schema_version: crate::backup::archive::CURRENT_SCHEMA_VERSION,
        ffmpeg_version,
        generated_at: Utc::now().to_rfc3339(),
    }
}

fn target_triple() -> String {
    // `std::env::consts::OS` etc. give the runtime view; the build
    // triple is what `cargo build` was told to target. Absent a
    // build-script that bakes this in, derive a useful approximation.
    format!(
        "{arch}-{os}",
        arch = std::env::consts::ARCH,
        os = std::env::consts::OS,
    )
}

async fn ffmpeg_version(
    transcode: Option<&crate::playback::transcode::TranscodeManager>,
) -> Option<String> {
    let path = transcode.map_or_else(|| "ffmpeg".to_owned(), |t| t.ffmpeg_path().clone());
    let output = tokio::process::Command::new(&path)
        .arg("-version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // First line of `ffmpeg -version` looks like:
    //   ffmpeg version 6.1.1 Copyright (c) 2000-2023 the FFmpeg developers
    let first = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .to_owned();
    Some(first)
}

// ── config-redacted.json ──────────────────────────────────────────

/// Field names whose values get replaced with `[REDACTED]` before
/// the config row hits the bundle. Anything storing a credential,
/// secret, or per-install token belongs here. Add to the list when
/// a new sensitive column lands; the SQL `SELECT *` strategy means
/// new columns surface in the bundle automatically — only the
/// redaction list needs maintenance.
const REDACTED_CONFIG_FIELDS: &[&str] = &[
    "api_key",
    "vpn_private_key",
    "vpn_port_forward_api_key",
    "tmdb_api_key",
    "opensubtitles_api_key",
    "opensubtitles_password",
    "trakt_client_id",
    "trakt_client_secret",
    "mdblist_api_key",
    "session_signing_key",
];

async fn collect_config_redacted(db: &SqlitePool) -> anyhow::Result<String> {
    let row = sqlx::query("SELECT * FROM config WHERE id = 1")
        .fetch_optional(db)
        .await?;
    let Some(row) = row else {
        return Ok("{}".to_owned());
    };
    let mut map = serde_json::Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name();
        let value = if REDACTED_CONFIG_FIELDS.contains(&name) {
            // Differentiate "field is set" from "field is null" so
            // the operator can still tell which knobs are configured
            // — we surface "[REDACTED]" only when the column has a
            // non-null, non-empty value.
            match row.try_get::<Option<String>, _>(i) {
                Ok(Some(s)) if !s.is_empty() => serde_json::Value::String(REDACTED.to_owned()),
                _ => serde_json::Value::Null,
            }
        } else {
            sqlite_value_to_json(&row, i)
        };
        map.insert(name.to_owned(), value);
    }
    Ok(serde_json::to_string_pretty(&map)?)
}

/// Best-effort type-narrowing from a sqlx `Row` column to JSON.
/// `SQLite` is dynamically typed but our schema uses `STRICT` and
/// per-column affinities, so trying `TEXT → INTEGER → REAL → fall
/// through` covers the columns we actually have.
fn sqlite_value_to_json(row: &sqlx::sqlite::SqliteRow, i: usize) -> serde_json::Value {
    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(i) {
        return serde_json::Value::String(s);
    }
    if let Ok(Some(n)) = row.try_get::<Option<i64>, _>(i) {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(Some(f)) = row.try_get::<Option<f64>, _>(i)
        && let Some(n) = serde_json::Number::from_f64(f)
    {
        return serde_json::Value::Number(n);
    }
    if let Ok(Some(b)) = row.try_get::<Option<bool>, _>(i) {
        return serde_json::Value::Bool(b);
    }
    serde_json::Value::Null
}

// ── indexers-redacted.json ────────────────────────────────────────

#[derive(Debug, Serialize)]
struct IndexerSnapshot {
    id: i64,
    name: String,
    url: String,
    api_key_set: bool,
    enabled: bool,
    priority: i64,
    indexer_type: String,
    definition_id: Option<String>,
    /// Whether `settings_json` was populated. Settings rows usually
    /// hold per-indexer credentials (cookies, tokens, login pairs)
    /// so we don't include the JSON itself — just whether it's set.
    settings_json_set: bool,
    escalation_level: i64,
    initial_failure_time: Option<String>,
    most_recent_failure_time: Option<String>,
    disabled_until: Option<String>,
}

async fn collect_indexers_redacted(db: &SqlitePool) -> anyhow::Result<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        name: String,
        url: String,
        api_key: Option<String>,
        enabled: bool,
        priority: i64,
        indexer_type: String,
        definition_id: Option<String>,
        settings_json: Option<String>,
        escalation_level: i64,
        initial_failure_time: Option<String>,
        most_recent_failure_time: Option<String>,
        disabled_until: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, url, api_key, enabled, priority, indexer_type, definition_id,
                settings_json, escalation_level, initial_failure_time,
                most_recent_failure_time, disabled_until
         FROM indexer ORDER BY priority, id",
    )
    .fetch_all(db)
    .await?;
    let snaps: Vec<IndexerSnapshot> = rows
        .into_iter()
        .map(|r| IndexerSnapshot {
            id: r.id,
            name: r.name,
            url: r.url,
            api_key_set: r.api_key.is_some_and(|s| !s.is_empty()),
            enabled: r.enabled,
            priority: r.priority,
            indexer_type: r.indexer_type,
            definition_id: r.definition_id,
            settings_json_set: r.settings_json.is_some_and(|s| !s.is_empty()),
            escalation_level: r.escalation_level,
            initial_failure_time: r.initial_failure_time,
            most_recent_failure_time: r.most_recent_failure_time,
            disabled_until: r.disabled_until,
        })
        .collect();
    Ok(serde_json::to_string_pretty(&snaps)?)
}

// ── backups.json ──────────────────────────────────────────────────

async fn collect_backups(db: &SqlitePool) -> anyhow::Result<String> {
    #[derive(sqlx::FromRow, Serialize)]
    struct BackupRow {
        id: i64,
        kind: String,
        filename: String,
        size_bytes: i64,
        kino_version: String,
        schema_version: i64,
        created_at: String,
    }
    let rows: Vec<BackupRow> = sqlx::query_as(
        "SELECT id, kind, filename, size_bytes, kino_version, schema_version, created_at
         FROM backup ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(db)
    .await?;
    Ok(serde_json::to_string_pretty(&rows)?)
}

// ── tasks.json ────────────────────────────────────────────────────

#[derive(Serialize)]
struct TaskSnap {
    name: String,
    interval_secs: u64,
    last_run_at: Option<String>,
    last_error: Option<String>,
}

async fn collect_tasks(state: &AppState) -> String {
    let Some(scheduler) = state.scheduler.as_ref() else {
        return "[]".to_owned();
    };
    let tasks = scheduler.list_tasks().await;
    let snaps: Vec<TaskSnap> = tasks
        .into_iter()
        .map(|t| TaskSnap {
            name: t.name,
            interval_secs: t.interval_seconds,
            last_run_at: t.last_run_at,
            last_error: t.last_error,
        })
        .collect();
    serde_json::to_string_pretty(&snaps).unwrap_or_else(|_| "[]".to_owned())
}

// ── service-descriptor.txt ────────────────────────────────────────

/// Render the platform-specific service descriptor text (or None if
/// we don't bundle one for this OS). Uses the same template the
/// `kino install-service` command writes, so the bundle reflects
/// what the production service descriptor looks like for diagnosis
/// of supervisor-related issues. Linux + macOS only; Windows SCM
/// services are described via registry entries that don't have a
/// neat textual form to bundle.
// Each cfg branch picks a single Some/None at compile time, so
// clippy's per-target view sees an "unnecessarily wrapped" Option.
// The wrap is load-bearing at the cross-platform level — keep it.
#[allow(clippy::unnecessary_wraps)]
fn render_service_descriptor() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // Embed both the .deb / .rpm packaged unit (the canonical
        // production shape) and a header pointing at it.
        let pkg_unit = include_str!("../../debian/service");
        Some(format!(
            "# Production systemd unit (from debian/service in the kino source tree).\n\
             # Native packages install this verbatim to /lib/systemd/system/kino.service.\n\
             # Tarball / cargo install users get the user-mode equivalent rendered at\n\
             # `kino install-service` time — see service_install/linux.rs::render_unit.\n\
             \n\
             {pkg_unit}"
        ))
    }
    #[cfg(target_os = "macos")]
    {
        Some(
            "# macOS LaunchDaemon plist is generated at install time —\n\
             # see service_install/macos.rs::render_plist for the template.\n\
             # Run `sudo launchctl print system/tv.kino.daemon` to dump the\n\
             # currently-loaded descriptor.\n"
                .to_owned(),
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

// ── logs/last-7d.jsonl ────────────────────────────────────────────

async fn collect_logs_jsonl(db: &SqlitePool) -> anyhow::Result<String> {
    #[derive(sqlx::FromRow, Serialize)]
    struct LogRow {
        id: i64,
        ts_us: i64,
        level: i64,
        target: String,
        subsystem: Option<String>,
        trace_id: Option<String>,
        span_id: Option<String>,
        message: String,
        fields_json: Option<String>,
        source: String,
    }
    // `unixepoch('now', '-7 days') * 1000000` would be cleaner but
    // SQLite doesn't have integer multiply on the function result
    // until 3.42. Compute the cutoff in Rust to stay portable.
    let cutoff_us = (Utc::now() - chrono::Duration::days(LOG_WINDOW_DAYS)).timestamp_micros();
    let rows: Vec<LogRow> = sqlx::query_as(
        "SELECT id, ts_us, level, target, subsystem, trace_id, span_id, message,
                fields_json, source
         FROM log_entry WHERE ts_us > ?
         ORDER BY ts_us DESC
         LIMIT ?",
    )
    .bind(cutoff_us)
    .bind(LOG_ROW_CAP)
    .fetch_all(db)
    .await?;
    let mut out = String::with_capacity(rows.len() * 256);
    for row in rows {
        out.push_str(&serde_json::to_string(&row)?);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_field_list_includes_known_secrets() {
        // Sanity check — if a contributor adds a new credential
        // column to config without updating REDACTED_CONFIG_FIELDS,
        // the diagnostic bundle would leak it. This isn't an
        // exhaustive guard (no schema reflection yet) but catches
        // typos in the existing list.
        for f in [
            "api_key",
            "vpn_private_key",
            "tmdb_api_key",
            "session_signing_key",
        ] {
            assert!(
                REDACTED_CONFIG_FIELDS.contains(&f),
                "{f} should be in REDACTED_CONFIG_FIELDS"
            );
        }
    }

    #[tokio::test]
    async fn bundle_builds_against_fresh_db() {
        // Smoke test: a fresh test pool has the schema + an empty
        // config; the bundle builder shouldn't panic and should
        // produce a non-empty zip.
        use crate::test_support::TestAppBuilder;
        let app = TestAppBuilder::new().build().await;
        let bytes = build_bundle(&app.state).await.expect("bundle builds");
        assert!(!bytes.is_empty(), "bundle has bytes");
        // ZIP magic ("PK\x03\x04") at the start — minimal sanity check
        // that we actually produced an archive vs a plain JSON blob.
        assert_eq!(
            &bytes[0..4],
            b"PK\x03\x04",
            "bundle starts with the ZIP file magic"
        );
    }
}
