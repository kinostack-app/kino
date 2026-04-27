use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use base64::Engine as _;
use serde::Serialize;
use utoipa::ToSchema;

use crate::download::DownloadPhase;
use crate::download::model::{Download, DownloadWithContent};
use crate::error::{AppError, AppResult};
use crate::pagination::{Cursor, PaginatedResponse, PaginationParams};
use crate::state::AppState;

/// List downloads (queue view).
///
/// Paginated per the `docs/subsystems/09-api.md` contract —
/// previous implementation capped at a silent `LIMIT 100` which
/// truncated bigger queues without telling the caller. Rows are
/// sorted by a phase-priority CASE expression (active first,
/// then pending, then terminal), breaking ties on `added_at DESC`.
/// The cursor carries both the phase-bucket ordinal and the `id`
/// so pagination is stable across state flips happening between
/// pages.
#[utoipa::path(
    get, path = "/api/v1/downloads",
    params(PaginationParams),
    responses((status = 200, body = PaginatedResponse<DownloadWithContent>)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn list_downloads(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> AppResult<Json<PaginatedResponse<DownloadWithContent>>> {
    let limit = params.limit();
    let fetch_limit = limit + 1;
    let cursor = params.cursor.as_deref().and_then(Cursor::decode);

    // Phase-priority ordering: active first, then pending, then
    // terminal. `added_at DESC` breaks ties within a phase so the
    // most recent shows at the top of its bucket.
    let phase_case = "CASE d.state
        WHEN 'downloading' THEN 0
        WHEN 'grabbing' THEN 1
        WHEN 'stalled' THEN 2
        WHEN 'queued' THEN 3
        WHEN 'paused' THEN 4
        WHEN 'seeding' THEN 5
        WHEN 'completed' THEN 6
        WHEN 'importing' THEN 7
        WHEN 'imported' THEN 8
        WHEN 'failed' THEN 9
        ELSE 10
    END";

    let base = format!(
        "SELECT d.*, {phase_case} AS phase_rank,
                dc.movie_id AS content_movie_id, dc.episode_id AS content_episode_id
         FROM download d
         LEFT JOIN download_content dc ON d.id = dc.download_id",
    );

    let downloads = if let Some(c) = cursor {
        // The cursor encodes "(phase_rank, added_at, id)" as a
        // composite; we decode it and resume from just past that
        // tuple. `sort_value` holds "RANK|added_at" so a single
        // comparison works.
        let sql = format!(
            "{base}
             WHERE (
               {phase_case} > ?
               OR ({phase_case} = ? AND datetime(d.added_at) < datetime(?))
               OR ({phase_case} = ? AND d.added_at = ? AND d.id > ?)
             )
             ORDER BY {phase_case} ASC, d.added_at DESC, d.id ASC
             LIMIT ?",
        );
        let (rank, added_at) = parse_download_sort_value(c.sort_value.as_deref());
        sqlx::query_as::<_, DownloadWithContent>(&sql)
            .bind(rank)
            .bind(rank)
            .bind(&added_at)
            .bind(rank)
            .bind(&added_at)
            .bind(c.id)
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!(
            "{base}
             ORDER BY {phase_case} ASC, d.added_at DESC, d.id ASC
             LIMIT ?",
        );
        sqlx::query_as::<_, DownloadWithContent>(&sql)
            .bind(fetch_limit)
            .fetch_all(&state.db)
            .await?
    };

    Ok(Json(PaginatedResponse::new(downloads, limit, |d| Cursor {
        id: d.id,
        sort_value: Some(format!("{}|{}", download_phase_rank(&d.state), d.added_at,)),
    })))
}

/// Map a download state string to the same phase rank the SQL
/// `CASE` expression uses. Kept next to the SQL so drift is
/// obvious in review — any new state needs entries in both.
fn download_phase_rank(state: &str) -> i64 {
    match DownloadPhase::parse(state) {
        Some(DownloadPhase::Downloading) => 0,
        Some(DownloadPhase::Grabbing) => 1,
        Some(DownloadPhase::Stalled) => 2,
        Some(DownloadPhase::Queued) => 3,
        Some(DownloadPhase::Paused) => 4,
        Some(DownloadPhase::Seeding) => 5,
        Some(DownloadPhase::Completed) => 6,
        Some(DownloadPhase::Importing) => 7,
        Some(DownloadPhase::Imported) => 8,
        Some(DownloadPhase::Failed) => 9,
        Some(DownloadPhase::Searching | DownloadPhase::CleanedUp | DownloadPhase::Cancelled)
        | None => 10,
    }
}

fn parse_download_sort_value(s: Option<&str>) -> (i64, String) {
    let Some(raw) = s else {
        return (0, String::new());
    };
    match raw.split_once('|') {
        Some((rank, added)) => (rank.parse().unwrap_or(0), added.to_owned()),
        None => (0, raw.to_owned()),
    }
}

/// Get a download by ID.
#[utoipa::path(
    get, path = "/api/v1/downloads/{id}",
    params(("id" = i64, Path)),
    responses((status = 200, body = Download), (status = 404)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn get_download(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Download>> {
    let download = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;
    Ok(Json(download))
}

/// Cancel and remove a download.
#[utoipa::path(
    delete, path = "/api/v1/downloads/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn cancel_download(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let download = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    // Stop any running streaming-trickplay task for this download.
    // Safe to call for terminal and active states — idempotent when
    // nothing's running.
    state.stream_trickplay.stop(id).await;

    // Also stop any running HLS transcode session started by watch-now
    // on a browser-incompatible source. Idempotent.
    if let Some(ref transcode) = state.transcode {
        let session_id = format!("stream-{id}");
        if let Err(e) = transcode.stop_session(&session_id).await {
            tracing::warn!(download_id = id, %session_id, error = %e, "failed to stop stream transcode on cancel");
        }
    }

    // Terminal states — delete the record. Content phase derives
    // from (media, active-download, watched_at), so removing the
    // download row automatically makes the phase revert to wanted
    // on the next read. No UPDATE to status needed.
    if matches!(
        DownloadPhase::parse(&download.state),
        Some(DownloadPhase::Failed | DownloadPhase::Imported | DownloadPhase::Completed)
    ) {
        sqlx::query("DELETE FROM download_content WHERE download_id = ?")
            .bind(id)
            .execute(&state.db)
            .await?;
        sqlx::query("DELETE FROM download WHERE id = ?")
            .bind(id)
            .execute(&state.db)
            .await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    // Active downloads — cancel: remove from librqbit + reset content status.
    // Failed removals queue for retry on the cleanup_retry tick.
    if let (Some(client), Some(hash)) = (&state.torrent, &download.torrent_hash) {
        let outcome = state
            .cleanup_tracker
            .try_remove(crate::cleanup::ResourceKind::Torrent, hash, || async {
                client.remove(hash, true).await
            })
            .await?;
        if !outcome.is_removed() {
            tracing::warn!(
                download_id = id,
                torrent_hash = %hash,
                ?outcome,
                "torrent removal queued for retry",
            );
        }
    }

    sqlx::query(
        "UPDATE download SET state = 'failed', error_message = 'cancelled by user' WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    // Status derives from the (now-terminal) download row — content
    // phase is automatically 'wanted' again on the next read. Emit
    // DownloadCancelled (not DownloadFailed) — cancellation is user
    // intent, not a failure. The UI renders it as a quiet "Cancelled"
    // instead of the red failure card with "Pick alternate". Cache
    // invalidation parity is handled by the WS handler.
    let _ = state
        .event_tx
        .send(crate::events::AppEvent::DownloadCancelled {
            download_id: id,
            title: crate::events::display::download_display_title(&state.db, id, &download.title)
                .await,
        });

    Ok(StatusCode::NO_CONTENT)
}

/// Pause a download.
#[utoipa::path(
    post, path = "/api/v1/downloads/{id}/pause",
    params(("id" = i64, Path)),
    responses((status = 200), (status = 404), (status = 400)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn pause_download(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let download = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    if !matches!(
        DownloadPhase::parse(&download.state),
        Some(DownloadPhase::Downloading | DownloadPhase::Stalled)
    ) {
        return Err(AppError::BadRequest(format!(
            "cannot pause download in state '{}'",
            download.state
        )));
    }

    // Pause in librqbit
    if let (Some(client), Some(hash)) = (&state.torrent, &download.torrent_hash) {
        client
            .pause(hash)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("pause torrent: {e}")))?;
    }

    // Zero out live-activity fields so the UI doesn't show stale
    // download/upload speed / peers / eta frozen at whatever they
    // were the moment the user hit pause. poll_real_progress skips
    // paused rows, so without this reset those fields keep their
    // last values indefinitely, which reads as a stuck-UI bug.
    sqlx::query(
        "UPDATE download
            SET state = 'paused',
                download_speed = 0,
                upload_speed = 0,
                seeders = NULL,
                leechers = NULL,
                eta = NULL
          WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    // Fan out so other windows flip their UI immediately. Without
    // this, a paused state only reaches other tabs when the next
    // `download_progress` tick runs — and progress events don't
    // carry state, so they wouldn't propagate at all without a
    // separate pause event.
    state.emit(crate::events::AppEvent::DownloadPaused {
        download_id: id,
        title: crate::events::display::download_display_title(&state.db, id, &download.title).await,
    });

    Ok(StatusCode::OK)
}

/// Resume a paused download.
#[utoipa::path(
    post, path = "/api/v1/downloads/{id}/resume",
    params(("id" = i64, Path)),
    responses((status = 200), (status = 404), (status = 400)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn resume_download(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let download = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    if DownloadPhase::parse(&download.state) != Some(DownloadPhase::Paused) {
        return Err(AppError::BadRequest(format!(
            "cannot resume download in state '{}'",
            download.state
        )));
    }

    // Resume in librqbit when a hash exists. Previously this was
    // silent-fall-through: if the torrent client was unavailable,
    // the DB flipped to `downloading` anyway and the UI lied about
    // state. New contract: no hash → DB-only (nothing to talk to,
    // fine); hash present but client gone → error so the user sees
    // the real problem.
    if let Some(hash) = &download.torrent_hash {
        let Some(client) = &state.torrent else {
            return Err(AppError::Internal(anyhow::anyhow!(
                "torrent client unavailable — cannot resume"
            )));
        };
        client
            .resume(hash)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("resume torrent: {e}")))?;
    }

    // Same zeroing on resume — between flipping state and the first
    // progress tick landing, the UI would otherwise read the
    // zero'd-out pause values, then the first progress event
    // repopulates them. Explicit so the resumed state starts from
    // "—" rather than whatever was persisted.
    sqlx::query(
        "UPDATE download
            SET state = 'downloading',
                download_speed = 0,
                upload_speed = 0,
                seeders = NULL,
                leechers = NULL,
                eta = NULL
          WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    state.emit(crate::events::AppEvent::DownloadResumed {
        download_id: id,
        title: crate::events::display::download_display_title(&state.db, id, &download.title).await,
    });

    Ok(StatusCode::OK)
}

/// Retry a failed download.
#[utoipa::path(
    post, path = "/api/v1/downloads/{id}/retry",
    params(("id" = i64, Path)),
    responses((status = 200), (status = 404), (status = 400)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn retry_download(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let download = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    if DownloadPhase::parse(&download.state) != Some(DownloadPhase::Failed) {
        return Err(AppError::BadRequest(format!(
            "cannot retry download in state '{}'",
            download.state
        )));
    }

    // Reset to queued for re-processing by the monitor
    sqlx::query(
        "UPDATE download SET state = 'queued', error_message = NULL, torrent_hash = NULL WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::OK)
}

/// Blocklist this download's release and search for another.
#[utoipa::path(
    post, path = "/api/v1/downloads/{id}/blocklist-and-search",
    params(("id" = i64, Path)),
    responses((status = 200), (status = 404)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn blocklist_and_search(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let download = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    // Cancel torrent if active. Failed removals queue for retry.
    if let (Some(client), Some(hash)) = (&state.torrent, &download.torrent_hash) {
        let outcome = state
            .cleanup_tracker
            .try_remove(crate::cleanup::ResourceKind::Torrent, hash, || async {
                client.remove(hash, true).await
            })
            .await?;
        if !outcome.is_removed() {
            tracing::warn!(
                download_id = id,
                torrent_hash = %hash,
                ?outcome,
                "torrent removal queued for retry (blocklist+search path)",
            );
        }
    }

    // Get linked movie/episode
    let movie_id: Option<i64> = sqlx::query_scalar(
        "SELECT movie_id FROM download_content WHERE download_id = ? AND movie_id IS NOT NULL LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .flatten();

    let episode_id: Option<i64> = sqlx::query_scalar(
        "SELECT episode_id FROM download_content WHERE download_id = ? AND episode_id IS NOT NULL LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .flatten();

    // Look up the release's indexer_id (download row carries
    // release_id, not indexer_id).
    let indexer_id: Option<i64> = if let Some(rid) = download.release_id {
        sqlx::query_scalar("SELECT indexer_id FROM release WHERE id = ?")
            .bind(rid)
            .fetch_optional(&state.db)
            .await?
            .flatten()
    } else {
        None
    };

    // Hash stored lowercase so the matches_release helper +
    // blocklist_hashes_normalized invariant agree on canonical form.
    let normalized_hash = download.torrent_hash.as_ref().map(|h| h.to_lowercase());

    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO blocklist (movie_id, episode_id, source_title, torrent_info_hash, indexer_id, size, message, date) VALUES (?, ?, ?, ?, ?, ?, 'Blocked by user', ?)",
    )
    .bind(movie_id)
    .bind(episode_id)
    .bind(&download.title)
    .bind(normalized_hash.as_deref())
    .bind(indexer_id)
    .bind(download.size)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // Delete download
    sqlx::query("DELETE FROM download_content WHERE download_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM download WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    // Delete-above already unblocks re-search: the content phase
    // derives back to 'wanted' with no download + no media. Kick a
    // fresh search now.
    if let Some(mid) = movie_id {
        let search_state = state.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::acquisition::search::movie::search_movie(&search_state, mid).await
            {
                tracing::error!(movie_id = mid, error = %e, "re-search after blocklist failed");
            }
        });
    }

    Ok(StatusCode::OK)
}

/// One file within a multi-file torrent. Used by the Files tab so the
/// user can pick which episodes of a season pack to grab, or see how
/// much of each file has downloaded.
#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadFileEntry {
    pub index: i64,
    pub path: String,
    pub size: i64,
    /// Whether this file is in the "only-files" selection — i.e. will
    /// be downloaded. False when the user has unchecked it.
    pub selected: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadFilesReply {
    pub files: Vec<DownloadFileEntry>,
    /// True while librqbit is still fetching the info-dict from peers
    /// and doesn't yet know the file list. Frontend polls when true.
    pub metadata_pending: bool,
}

/// `GET /api/v1/downloads/{id}/files` — enumerate the torrent's files
/// with their current selection state. Returns `metadata_pending` = true
/// when librqbit is still resolving the info-dict (magnet just added).
#[utoipa::path(
    get, path = "/api/v1/downloads/{id}/files",
    params(("id" = i64, Path)),
    responses((status = 200, body = DownloadFilesReply), (status = 404)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn download_files(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<DownloadFilesReply>> {
    let dl = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    let Some(client) = &state.torrent else {
        return Ok(Json(DownloadFilesReply {
            files: Vec::new(),
            metadata_pending: true,
        }));
    };
    let Some(hash) = &dl.torrent_hash else {
        return Ok(Json(DownloadFilesReply {
            files: Vec::new(),
            metadata_pending: true,
        }));
    };

    let Some(files) = client.files(hash) else {
        return Ok(Json(DownloadFilesReply {
            files: Vec::new(),
            metadata_pending: true,
        }));
    };
    let selected: std::collections::HashSet<usize> = client
        .selected_files(hash)
        .unwrap_or_default()
        .into_iter()
        .collect();

    let entries: Vec<DownloadFileEntry> = files
        .into_iter()
        .map(|(idx, path, size)| DownloadFileEntry {
            index: i64::try_from(idx).unwrap_or(i64::MAX),
            path: path.to_string_lossy().into_owned(),
            size: i64::try_from(size).unwrap_or(i64::MAX),
            selected: selected.contains(&idx),
        })
        .collect();

    Ok(Json(DownloadFilesReply {
        files: entries,
        metadata_pending: false,
    }))
}

#[derive(Debug, serde::Deserialize, ToSchema)]
pub struct UpdateFileSelection {
    pub file_indices: Vec<i64>,
}

/// `POST /api/v1/downloads/{id}/files/select` — update which files in
/// a multi-file torrent are actively downloaded. Empty list means
/// "nothing" (effectively pauses the torrent); passing every index
/// means "all files" (default on grab).
#[utoipa::path(
    post, path = "/api/v1/downloads/{id}/files/select",
    params(("id" = i64, Path)),
    request_body = UpdateFileSelection,
    responses((status = 204), (status = 404), (status = 400)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn update_download_files(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateFileSelection>,
) -> AppResult<StatusCode> {
    let dl = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    let Some(client) = &state.torrent else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "torrent client unavailable"
        )));
    };
    let Some(hash) = &dl.torrent_hash else {
        return Err(AppError::BadRequest(
            "download has no torrent hash yet — wait for metadata".to_owned(),
        ));
    };

    let indices: Vec<usize> = body
        .file_indices
        .into_iter()
        .filter_map(|i| usize::try_from(i).ok())
        .collect();

    client
        .update_file_selection(hash, indices)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("update file selection: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Per-peer snapshot for the Peers tab. Flattened from librqbit's
/// internal `PeerStats` — we only surface fields the UI actually uses.
#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadPeer {
    pub addr: String,
    /// One of: `queued`, `connecting`, `live`, `dead`, `not_needed`.
    pub state: String,
    /// Cumulative bytes fetched from this peer — lets the UI order
    /// peers by "contribution" without needing rate info.
    pub fetched_bytes: i64,
    pub uploaded_bytes: i64,
    /// Connection medium. None for not-yet-connected peers.
    pub conn_kind: Option<String>,
    pub errors: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadPeersReply {
    pub peers: Vec<DownloadPeer>,
    /// `true` when the torrent isn't in the `live` state (paused,
    /// initializing, or finished) — no peer connections exist.
    pub not_live: bool,
}

/// `GET /api/v1/downloads/{id}/peers` — snapshot of peer connections
/// for this torrent. Returns `not_live = true` instead of an error
/// when the torrent is paused so the UI can render "(paused)" cleanly.
#[utoipa::path(
    get, path = "/api/v1/downloads/{id}/peers",
    params(("id" = i64, Path)),
    responses((status = 200, body = DownloadPeersReply), (status = 404)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn download_peers(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<DownloadPeersReply>> {
    let dl = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    let not_live_reply = Json(DownloadPeersReply {
        peers: Vec::new(),
        not_live: true,
    });
    let Some(client) = &state.torrent else {
        return Ok(not_live_reply);
    };
    let Some(hash) = &dl.torrent_hash else {
        return Ok(not_live_reply);
    };
    let Some(snapshot) = client.peer_stats(hash, true) else {
        return Ok(not_live_reply);
    };

    let peers: Vec<DownloadPeer> = snapshot
        .peers
        .into_iter()
        .map(|(addr, s)| {
            let conn_kind = s
                .conn_kind
                .as_ref()
                .map(|k| format!("{k:?}").to_lowercase());
            DownloadPeer {
                addr,
                state: s.state.to_owned(),
                fetched_bytes: i64::try_from(s.counters.fetched_bytes).unwrap_or(i64::MAX),
                uploaded_bytes: i64::try_from(s.counters.uploaded_bytes).unwrap_or(i64::MAX),
                conn_kind,
                errors: i64::from(s.counters.errors),
            }
        })
        .collect();

    Ok(Json(DownloadPeersReply {
        peers,
        not_live: false,
    }))
}

/// Piece-bitmap response. `bitmap_b64` is the raw librqbit "have"
/// bitmap, base64-encoded; each bit is one piece in MSB0 order, packed
/// into bytes. `total_pieces` is the logical bit count — trailing bits
/// in the last byte are padding the frontend must ignore.
#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadPiecesReply {
    pub bitmap_b64: String,
    pub total_pieces: u32,
    /// True when the torrent isn't yet managed by librqbit (metadata
    /// still resolving, torrent client not started, etc.). UI renders
    /// an empty placeholder instead of misleading "all missing" bars.
    pub not_available: bool,
}

/// `GET /api/v1/downloads/{id}/pieces` — per-piece have bitmap for the
/// Pieces tab's canvas rendering. Transport is base64 over JSON rather
/// than a raw binary endpoint because our OpenAPI-driven SDK codegen
/// only understands JSON responses; the bitmap is small (thousands of
/// bytes at most — ten megabits per thousand pieces).
#[utoipa::path(
    get, path = "/api/v1/downloads/{id}/pieces",
    params(("id" = i64, Path)),
    responses((status = 200, body = DownloadPiecesReply), (status = 404)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn download_pieces(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<DownloadPiecesReply>> {
    let dl = sqlx::query_as::<_, Download>("SELECT * FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("download {id} not found")))?;

    let empty = Json(DownloadPiecesReply {
        bitmap_b64: String::new(),
        total_pieces: 0,
        not_available: true,
    });
    let Some(client) = &state.torrent else {
        return Ok(empty);
    };
    let Some(hash) = &dl.torrent_hash else {
        return Ok(empty);
    };
    let Some((bytes, total)) = client.pieces(hash) else {
        return Ok(empty);
    };

    let bitmap_b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Json(DownloadPiecesReply {
        bitmap_b64,
        total_pieces: total,
        not_available: false,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SpeedTestResult {
    pub ok: bool,
    pub message: String,
    /// Measured throughput in bytes/s. None on failure.
    pub bytes_per_sec: Option<u64>,
    /// Bytes transferred before we stopped. Helps explain short
    /// responses when the CDN truncates.
    pub bytes: u64,
    /// Observed download duration in milliseconds.
    pub duration_ms: u64,
}

/// `POST /api/v1/downloads/speed-test` — measures egress throughput by
/// downloading a known-size payload. Useful for setting realistic
/// Download Limit values and diagnosing slow downloads. Not routed
/// through the `BitTorrent` client, so it reflects the server's raw
/// network capability.
///
/// Source priority: Cloudflare's speedtest (global anycast) first,
/// with a Hetzner fallback when Cloudflare bot-blocks us. We set a
/// browser User-Agent because the default reqwest UA gets a 403.
#[utoipa::path(
    post, path = "/api/v1/downloads/speed-test",
    responses((status = 200, body = SpeedTestResult)),
    tag = "downloads", security(("api_key" = []))
)]
pub async fn speed_test() -> AppResult<Json<SpeedTestResult>> {
    // 100 MB — enough to converge on a stable estimate on a fast link
    // without wedging a slow one for minutes. We try a few sources and
    // pick the first that succeeds:
    //   - Cachefly: long-lived CDN, rarely blocks bots, HTTPS.
    //   - Tele2: plain HTTP — bypasses TLS cert validation entirely,
    //     useful on hosts with clock skew or broken trust stores.
    //   - OVH / Cloudflare: mostly-reliable mainstream CDNs. Cloudflare's
    //     JA3/JA4 bot detection still rejects rustls occasionally.
    let sources: [&str; 4] = [
        "https://cachefly.cachefly.net/100mb.test",
        "http://speedtest.tele2.net/100MB.zip",
        "https://proof.ovh.net/files/100Mb.dat",
        "https://speed.cloudflare.com/__down?bytes=104857600",
    ];

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        // Force HTTP/1.1 — Cloudflare's JA3/JA4 bot detection is more
        // lenient on h1 than h2 (default) for non-Chrome TLS stacks.
        .http1_only()
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/124.0.0.0 Safari/537.36",
        )
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(SpeedTestResult {
                ok: false,
                message: format!("client init: {e}"),
                bytes_per_sec: None,
                bytes: 0,
                duration_ms: 0,
            }));
        }
    };

    let mut last_error = String::from("no sources tried");
    for url in sources {
        match run_speed_test(&client, url).await {
            Ok(result) => return Ok(Json(result)),
            Err(e) => {
                tracing::warn!(url, error = %e, "speed test source failed, trying next");
                last_error = format!("{url}: {e}");
            }
        }
    }
    Ok(Json(SpeedTestResult {
        ok: false,
        message: format!("all sources failed — last: {last_error}"),
        bytes_per_sec: None,
        bytes: 0,
        duration_ms: 0,
    }))
}

/// Hit a single speed-test source and return the measured throughput
/// or an error message describing why we gave up on it.
async fn run_speed_test(client: &reqwest::Client, url: &str) -> Result<SpeedTestResult, String> {
    let start = std::time::Instant::now();
    let resp = client
        .get(url)
        // Browser-like headers — makes the request look less like a
        // scripted bot to Cloudflare's edge challenge.
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Dest", "empty")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "same-origin")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }

    // 100 MB in RAM is fine for a one-shot on a home server; avoids
    // pulling in reqwest's optional stream feature just for counting.
    let body = resp.bytes().await.map_err(|e| format!("body read: {e}"))?;
    let total = body.len() as u64;
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let bps = if elapsed_ms == 0 {
        0
    } else {
        total * 1000 / elapsed_ms
    };

    tracing::info!(
        url,
        bytes = total,
        duration_ms = elapsed_ms,
        bytes_per_sec = bps,
        "speed test complete",
    );

    Ok(SpeedTestResult {
        ok: true,
        message: format!("Downloaded {} MB in {} ms", total / 1024 / 1024, elapsed_ms),
        bytes_per_sec: Some(bps),
        bytes: total,
        duration_ms: elapsed_ms,
    })
}
