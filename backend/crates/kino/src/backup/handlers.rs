//! HTTP routes for `/api/v1/backups/*`.
//!
//! Phase 1 surface:
//!
//! - `GET    /api/v1/backups`              — list rows (newest first)
//! - `POST   /api/v1/backups`              — create one (manual)
//! - `GET    /api/v1/backups/{id}/download` — stream the archive
//! - `DELETE /api/v1/backups/{id}`         — delete row + file
//! - `POST   /api/v1/backups/{id}/restore` — restore from a known row
//! - `POST   /api/v1/backups/restore-upload` — restore from uploaded file

use axum::Json;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Serialize;
use utoipa::ToSchema;

use super::archive;
use super::model::{Backup, BackupKind};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Cap on uploaded restore archives (1 GiB). The on-disk shape is
/// usually a handful of MB; the cap is just a safety net so a
/// runaway request can't OOM the host.
pub const RESTORE_UPLOAD_LIMIT: usize = 1024 * 1024 * 1024;

/// Env var that opts a process into "exit-after-restore" behaviour.
/// When set (or when the in-process `AtomicBool` marker is true), a
/// successful restore schedules `std::process::exit(75)`
/// (`EX_TEMPFAIL`) after a brief delay so the HTTP response can flush.
/// systemd / launchd / Windows SCM all treat non-zero exit as a
/// failure for restart-policy purposes, so the supervisor brings the
/// process back up against the freshly-restored database with no
/// user intervention.
///
/// Native systemd / launchd unit files set this env var in the
/// service descriptor; the Windows SCM dispatcher sets the in-
/// process marker instead (workspace forbids `unsafe_code` and
/// `std::env::set_var` is unsafe under Rust 2024). Tests never see
/// either set, so `cargo test` doesn't kill itself mid-suite.
pub const RESTART_AFTER_RESTORE_ENV: &str = "KINO_RESTART_AFTER_RESTORE";

/// Process-wide marker for "exit-after-restore" set by the Windows
/// SCM dispatcher (`service_runner.rs`) — see
/// `RESTART_AFTER_RESTORE_ENV` for the parallel env-var path used
/// by systemd / launchd.
static RESTART_AFTER_RESTORE_MARKER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set the in-process "exit-after-restore" marker. Called from the
/// Windows SCM dispatcher before tokio spins up.
pub fn set_restart_after_restore_marker() {
    RESTART_AFTER_RESTORE_MARKER.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Spawn a delayed `exit(75)` if either the env var or the in-process
/// marker says we should. Lets the calling handler return its 200 OK
/// before the process goes down — without this gap the client sees a
/// connection-reset instead of the success response.
fn maybe_schedule_restart_after_restore() -> bool {
    let env_set = std::env::var_os(RESTART_AFTER_RESTORE_ENV).is_some();
    let marker_set = RESTART_AFTER_RESTORE_MARKER.load(std::sync::atomic::Ordering::Relaxed);
    if !env_set && !marker_set {
        return false;
    }
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        tracing::info!(
            "exiting after successful restore — service supervisor will restart with restored DB"
        );
        // Exit code 75 (`EX_TEMPFAIL`) reads as a non-zero exit to
        // every supervisor we target (systemd Restart=on-failure,
        // launchd KeepAlive on non-zero, Windows SCM Recovery
        // actions). Distinguishable from a real crash if anyone
        // greps logs, but functionally treated the same way.
        std::process::exit(75);
    });
    true
}

/// `GET /api/v1/backups`
#[utoipa::path(
    get, path = "/api/v1/backups",
    responses((status = 200, body = Vec<Backup>)),
    tag = "backups", security(("api_key" = []))
)]
pub async fn list_backups(State(state): State<AppState>) -> AppResult<Json<Vec<Backup>>> {
    let rows = sqlx::query_as::<_, Backup>("SELECT * FROM backup ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

/// `POST /api/v1/backups` — manual create.
#[utoipa::path(
    post, path = "/api/v1/backups",
    responses((status = 201, body = Backup)),
    tag = "backups", security(("api_key" = []))
)]
pub async fn create_backup(State(state): State<AppState>) -> AppResult<(StatusCode, Json<Backup>)> {
    let id = archive::create(
        &state.db,
        &state.data_path,
        BackupKind::Manual,
        &state.event_tx,
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("{e:#}")))?;
    let row = sqlx::query_as::<_, Backup>("SELECT * FROM backup WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `GET /api/v1/backups/{id}/download` — stream the archive file.
#[utoipa::path(
    get, path = "/api/v1/backups/{id}/download",
    params(("id" = i64, Path)),
    responses((status = 200, content_type = "application/gzip"), (status = 404)),
    tag = "backups", security(("api_key" = []))
)]
pub async fn download_backup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT filename, size_bytes FROM backup WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (filename, size_bytes) =
        row.ok_or_else(|| AppError::NotFound(format!("backup {id} not found")))?;
    let location = archive::ensure_location(&state.db, &state.data_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e:#}")))?;
    let path = location.join(&filename);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound(format!("backup file missing on disk: {filename}")))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let response = Response::builder()
        .header(header::CONTENT_TYPE, "application/gzip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, size_bytes.to_string())
        .body(body)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("build download response: {e}")))?;
    Ok(response)
}

/// `DELETE /api/v1/backups/{id}`
#[utoipa::path(
    delete, path = "/api/v1/backups/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "backups", security(("api_key" = []))
)]
pub async fn delete_backup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let removed = archive::delete_one(&state.db, &state.data_path, id, &state.event_tx)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e:#}")))?;
    if !removed {
        return Err(AppError::NotFound(format!("backup {id} not found")));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RestoreReply {
    pub ok: bool,
    /// Operator-facing message: tells the user kino must be
    /// restarted to load the restored database. The frontend
    /// surfaces this verbatim alongside per-platform restart
    /// commands.
    pub message: String,
}

/// `POST /api/v1/backups/{id}/restore` — restore from a known row.
/// Auto-creates a pre-restore snapshot first.
#[utoipa::path(
    post, path = "/api/v1/backups/{id}/restore",
    params(("id" = i64, Path)),
    responses((status = 200, body = RestoreReply), (status = 404), (status = 409)),
    tag = "backups", security(("api_key" = []))
)]
pub async fn restore_backup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<RestoreReply>> {
    archive::restore_backup_id(&state.db, &state.data_path, id, &state.event_tx)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e:#}")))?;
    let auto_restart = maybe_schedule_restart_after_restore();
    Ok(Json(RestoreReply {
        ok: true,
        message: if auto_restart {
            "Restore staged. kino is restarting now — the service supervisor will bring it \
             back up against the restored database in a few seconds."
                .to_owned()
        } else {
            "Restore staged. Restart kino to load the restored database — the next process \
             boot will pick up the new state automatically."
                .to_owned()
        },
    }))
}

/// `POST /api/v1/backups/restore-upload` — multipart upload for a
/// backup made on another machine. Single field, name `archive`.
#[utoipa::path(
    post, path = "/api/v1/backups/restore-upload",
    request_body(content = String, content_type = "multipart/form-data"),
    responses((status = 200, body = RestoreReply), (status = 400), (status = 409)),
    tag = "backups", security(("api_key" = []))
)]
pub async fn restore_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<RestoreReply>> {
    // Save the upload to a tempfile next to data_path so the
    // restore extractor can stream-read it without an in-memory
    // copy of the whole archive.
    let tmp = tempfile::Builder::new()
        .prefix("kino-restore-upload-")
        .suffix(".tar.gz")
        .tempfile_in(&state.data_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("create tempfile: {e}")))?;
    let tmp_path = tmp.path().to_path_buf();

    let mut found = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart parse: {e}")))?
    {
        if field.name() != Some("archive") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("read upload: {e}")))?;
        tokio::fs::write(&tmp_path, &bytes)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("write upload: {e}")))?;
        found = true;
        break;
    }
    if !found {
        return Err(AppError::BadRequest(
            "upload missing `archive` field".into(),
        ));
    }
    // Trust the uploaded archive's manifest schema_version up to
    // the current; checksum check is skipped on uploads since we
    // have no pre-shared expectation.
    archive::restore_path(
        &state.db,
        &state.data_path,
        &tmp_path,
        None,
        archive::CURRENT_SCHEMA_VERSION,
        &state.event_tx,
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("{e:#}")))?;
    // Hold tmp until after restore so its drop doesn't yank the
    // file mid-extract.
    drop(tmp);
    let auto_restart = maybe_schedule_restart_after_restore();
    Ok(Json(RestoreReply {
        ok: true,
        message: if auto_restart {
            "Restore staged. kino is restarting now — the service supervisor will bring it \
             back up against the restored database in a few seconds."
                .to_owned()
        } else {
            "Restore staged. Restart kino to load the restored database — the next process \
             boot will pick up the new state automatically."
                .to_owned()
        },
    }))
}

/// Body-size limit layer for the restore-upload endpoint. Applied
/// at the route registration site in `main.rs`.
pub fn upload_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(RESTORE_UPLOAD_LIMIT)
}
