//! Byte-source resolution for the unified play API. Library file vs
//! in-flight torrent vs nothing-grabbed — every request runs through
//! [`resolve_byte_source`] before the handler decides which bytes to
//! serve.
//!
//! Library wins when both a complete `media` row + a readable file on
//! disk exist (steady state post-import). The torrent path is the
//! transitional source: streaming bytes from librqbit while the
//! download is still active. See `playback/handlers.rs` module docs
//! for the per-request dispatch rationale and the
//! `architecture/operations.md` Play sections for the lifecycle
//! diagram.

use crate::download::DownloadPhase;
use crate::error::AppError;
use crate::playback::PlayKind;
use crate::state::AppState;

/// Resolved byte source for a given entity. The dispatcher picks one
/// of these per request; the handler then serves bytes from it.
#[derive(Debug, Clone)]
pub(crate) enum ByteSource {
    /// Imported `media` row + readable file on disk. Authoritative.
    Library {
        media_id: i64,
        file_path: String,
        container: Option<String>,
        video_codec: Option<String>,
        audio_codec: Option<String>,
        runtime_ticks: Option<i64>,
        trickplay_generated: bool,
    },
    /// Active (non-failed) torrent download. Pieces served via
    /// librqbit's piece-prioritised file stream.
    Stream {
        download_id: i64,
        torrent_hash: String,
        /// Resolved once the torrent's metadata is available. None
        /// until then — caller treats as "not ready yet" (202-ish).
        file_idx: Option<usize>,
        /// Total byte size of the picked file. None until metadata.
        file_size: Option<u64>,
        /// Raw download row state — `searching`, `queued`,
        /// `grabbing`, `downloading`, `paused`, `seeding`,
        /// `finished`, `failed`.
        state: String,
        error_message: Option<String>,
        downloaded: i64,
        download_speed: i64,
    },
}

/// Error cases the dispatcher returns when neither byte source is
/// usable. Each variant carries the info the `/prepare` handler
/// needs to render an actionable error to the user.
#[derive(Debug)]
pub(crate) enum ResolveError {
    /// No entity row exists in our DB. For an arbitrary deep link,
    /// the frontend's first step is "add this movie/episode via
    /// watch-now," which creates the row + triggers a search.
    EntityNotFound,
    /// Entity exists and has an imported `media` row, but the file
    /// on disk is gone (external tamper, bad mount, disk swap).
    LibraryFileMissing { media_id: i64, file_path: String },
    /// Entity has an active download but it's in a pre-bytes state
    /// (searching / queued / grabbing — no `file_idx` yet). The
    /// frontend renders the progress stepper.
    DownloadNotReady {
        download_id: i64,
        state: String,
        error_message: Option<String>,
    },
    /// Download failed terminally — no release found, magnet error,
    /// disk full, etc. Surface `error_message` to the user.
    DownloadFailed {
        download_id: i64,
        error_message: Option<String>,
    },
    /// Entity exists but nothing has been grabbed yet. Frontend
    /// offers "Start watching" → fires `/api/v1/watch_now` which
    /// creates a download and returns the same entity URL.
    NoSource,
}

/// Resolve the byte source for a `(kind, entity_id)`. Library wins
/// if both a complete library file and an active download exist
/// (post-import steady state). See module docs for the pick rules.
pub(crate) async fn resolve_byte_source(
    state: &AppState,
    kind: PlayKind,
    entity_id: i64,
) -> Result<ByteSource, ResolveError> {
    // Guard: does the entity exist?
    let entity_exists: bool = match kind {
        PlayKind::Movie => {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM movie WHERE id = ?")
                .bind(entity_id)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0)
                > 0
        }
        PlayKind::Episode => {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM episode WHERE id = ?")
                .bind(entity_id)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0)
                > 0
        }
    };
    if !entity_exists {
        return Err(ResolveError::EntityNotFound);
    }

    // 1) Library source. Latest-imported media row for the entity.
    //    `date_added DESC LIMIT 1` gives us the authoritative file —
    //    a re-import/upgrade picks the newer row automatically.
    let library = lookup_library_media(state, kind, entity_id).await;
    if let Some(lib) = library {
        if tokio::fs::try_exists(&lib.file_path).await.unwrap_or(false) {
            return Ok(ByteSource::Library {
                media_id: lib.media_id,
                file_path: lib.file_path,
                container: lib.container,
                video_codec: lib.video_codec,
                audio_codec: lib.audio_codec,
                runtime_ticks: lib.runtime_ticks,
                trickplay_generated: lib.trickplay_generated,
            });
        }
        // Row says there's a file, but disk says otherwise. Surface
        // so the user can re-import or fix their storage — don't
        // silently fall through to the torrent source (which may not
        // even match the library's re-encoded / renamed version).
        return Err(ResolveError::LibraryFileMissing {
            media_id: lib.media_id,
            file_path: lib.file_path,
        });
    }

    // 2) Active download source. Most recent non-failed download
    //    linked to the entity.
    let dl = lookup_active_download(state, kind, entity_id).await;
    let Some(dl) = dl else {
        return Err(ResolveError::NoSource);
    };
    let phase = DownloadPhase::parse(&dl.state);
    if matches!(
        phase,
        Some(DownloadPhase::Failed | DownloadPhase::Cancelled)
    ) {
        return Err(ResolveError::DownloadFailed {
            download_id: dl.id,
            error_message: dl.error_message,
        });
    }

    // Pre-bytes states: no file_idx, no torrent handle yet.
    if matches!(
        phase,
        Some(DownloadPhase::Searching | DownloadPhase::Queued | DownloadPhase::Grabbing)
    ) || dl.torrent_hash.is_none()
    {
        return Err(ResolveError::DownloadNotReady {
            download_id: dl.id,
            state: dl.state,
            error_message: dl.error_message,
        });
    }

    // Metadata-ready: pick the file_idx that matches this entity
    // (episode-match in a season pack; largest video for a movie).
    let hash = dl.torrent_hash.clone().unwrap_or_default();
    let files = state
        .torrent
        .as_ref()
        .filter(|_| !hash.is_empty())
        .and_then(|t| t.files(&hash));
    let (file_idx, file_size) = match files {
        Some(files) => match pick_file_for_entity(state, kind, entity_id, &files).await {
            Some((idx, size)) => (Some(idx), Some(size)),
            None => (None, None),
        },
        None => (None, None),
    };

    Ok(ByteSource::Stream {
        download_id: dl.id,
        torrent_hash: hash,
        file_idx,
        file_size,
        state: dl.state,
        error_message: dl.error_message,
        downloaded: dl.downloaded,
        download_speed: dl.download_speed,
    })
}

/// Library-media lookup row. One field per schema column we care
/// about; flattened from the JOIN so the caller doesn't have to
/// know whether movie or episode.
#[derive(Debug)]
struct LibraryMedia {
    media_id: i64,
    file_path: String,
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    runtime_ticks: Option<i64>,
    trickplay_generated: bool,
}

async fn lookup_library_media(
    state: &AppState,
    kind: PlayKind,
    entity_id: i64,
) -> Option<LibraryMedia> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        file_path: String,
        container: Option<String>,
        video_codec: Option<String>,
        audio_codec: Option<String>,
        runtime_ticks: Option<i64>,
        trickplay_generated: bool,
    }
    let row: Option<Row> = match kind {
        PlayKind::Movie => sqlx::query_as(
            "SELECT id, file_path, container, video_codec, audio_codec,
                    runtime_ticks, trickplay_generated
             FROM media
             WHERE movie_id = ?
             ORDER BY date_added DESC LIMIT 1",
        )
        .bind(entity_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten(),
        PlayKind::Episode => sqlx::query_as(
            "SELECT m.id, m.file_path, m.container, m.video_codec, m.audio_codec,
                    m.runtime_ticks, m.trickplay_generated
             FROM media m
             JOIN media_episode me ON me.media_id = m.id
             WHERE me.episode_id = ?
             ORDER BY m.date_added DESC LIMIT 1",
        )
        .bind(entity_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten(),
    };
    row.map(|r| LibraryMedia {
        media_id: r.id,
        file_path: r.file_path,
        container: r.container,
        video_codec: r.video_codec,
        audio_codec: r.audio_codec,
        runtime_ticks: r.runtime_ticks,
        trickplay_generated: r.trickplay_generated,
    })
}

#[derive(Debug)]
struct ActiveDownload {
    id: i64,
    torrent_hash: Option<String>,
    state: String,
    error_message: Option<String>,
    downloaded: i64,
    download_speed: i64,
}

async fn lookup_active_download(
    state: &AppState,
    kind: PlayKind,
    entity_id: i64,
) -> Option<ActiveDownload> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        torrent_hash: Option<String>,
        state: String,
        error_message: Option<String>,
        downloaded: i64,
        download_speed: i64,
    }
    let sql = match kind {
        PlayKind::Movie => {
            "SELECT d.id, d.torrent_hash, d.state, d.error_message,
                    d.downloaded, d.download_speed
             FROM download d
             JOIN download_content dc ON dc.download_id = d.id
             WHERE dc.movie_id = ?
             ORDER BY d.id DESC LIMIT 1"
        }
        PlayKind::Episode => {
            "SELECT d.id, d.torrent_hash, d.state, d.error_message,
                    d.downloaded, d.download_speed
             FROM download d
             JOIN download_content dc ON dc.download_id = d.id
             WHERE dc.episode_id = ?
             ORDER BY d.id DESC LIMIT 1"
        }
    };
    let row: Option<Row> = sqlx::query_as(sql)
        .bind(entity_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    row.map(|r| ActiveDownload {
        id: r.id,
        torrent_hash: r.torrent_hash,
        state: r.state,
        error_message: r.error_message,
        downloaded: r.downloaded,
        download_speed: r.download_speed,
    })
}

/// Pick the file inside the torrent that corresponds to this
/// entity. For episodes we match by `SxxExx` in the filename; for
/// movies (and as a fallback) we take the largest video file.
///
/// Episode-pack guard: when the torrent has more than one playable
/// video file and no filename matches the requested
/// `(season, episode)`, return `None` instead of guessing. The old
/// behaviour fell through to "largest video file" which, for a season
/// pack with non-parseable filenames, would stream the same wrong
/// episode (whichever file was largest) for every unmatched request.
/// The largest-fallback only applies to single-video torrents where
/// "the only file" is unambiguous.
async fn pick_file_for_entity(
    state: &AppState,
    kind: PlayKind,
    entity_id: i64,
    files: &[(usize, std::path::PathBuf, u64)],
) -> Option<(usize, u64)> {
    if let PlayKind::Episode = kind {
        let season_episode: Option<(i64, i64)> =
            sqlx::query_as("SELECT season_number, episode_number FROM episode WHERE id = ?")
                .bind(entity_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        if let Some((s, e)) = season_episode
            && let Some(pick) = crate::playback::file_pick::pick_episode(files, s, e)
        {
            return Some((pick.0, pick.2));
        }
        let video_count = crate::playback::file_pick::video_file_count(files);
        if video_count > 1 {
            tracing::warn!(
                entity_id,
                video_count,
                "episode pick: torrent has multiple video files but none matched SxxExx — \
                 refusing to guess (would stream the wrong episode)",
            );
            return None;
        }
    }
    crate::playback::file_pick::pick_largest(files).map(|(idx, _, size)| (idx, size))
}

/// Map `ResolveError` onto the corresponding HTTP status. Byte
/// endpoints that can't serve anything need a concrete status for
/// the client; `/prepare` returns richer payloads.
pub(crate) fn resolve_error_to_app_error(e: ResolveError) -> AppError {
    match e {
        ResolveError::EntityNotFound => AppError::NotFound("entity not tracked".into()),
        ResolveError::LibraryFileMissing { file_path, .. } => {
            AppError::NotFound(format!("library file missing on disk: {file_path}"))
        }
        ResolveError::DownloadNotReady { state, .. } => AppError::BadRequest(format!(
            "download not ready (state={state}) — poll /prepare"
        )),
        ResolveError::DownloadFailed { error_message, .. } => {
            AppError::NotFound(error_message.unwrap_or_else(|| "download failed".to_owned()))
        }
        ResolveError::NoSource => AppError::NotFound("no playable source — start watch-now".into()),
    }
}
