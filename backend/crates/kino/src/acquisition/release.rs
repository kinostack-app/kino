//! Release — one parsed-and-scored result from an indexer search,
//! optionally followed by a grab. The Release row carries everything
//! the policy gate consults at decision time (tier, languages,
//! seeders, etc.) plus the lifecycle (`pending` → `grabbed`). This
//! module owns the row model + the HTTP CRUD that surfaces releases
//! to the frontend's manual-pick drawer.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Release {
    pub id: i64,
    pub guid: String,
    pub indexer_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub show_id: Option<i64>,
    pub season_number: Option<i64>,
    pub episode_id: Option<i64>,
    pub title: String,
    pub size: Option<i64>,
    pub download_url: Option<String>,
    pub magnet_url: Option<String>,
    pub info_url: Option<String>,
    pub info_hash: Option<String>,
    pub publish_date: Option<String>,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub grabs: Option<i64>,
    pub resolution: Option<i64>,
    pub source: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hdr_format: Option<String>,
    pub is_remux: bool,
    pub is_proper: bool,
    pub is_repack: bool,
    pub release_group: Option<String>,
    pub languages: Option<String>,
    pub indexer_flags: Option<String>,
    pub quality_score: Option<i64>,
    pub status: String,
    pub pending_until: Option<String>,
    pub first_seen_at: String,
    pub grabbed_at: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ReleaseFilter {
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
}

/// List releases, optionally filtered by movie or episode.
#[utoipa::path(
    get, path = "/api/v1/releases",
    params(ReleaseFilter),
    responses((status = 200, body = Vec<Release>)),
    tag = "releases", security(("api_key" = []))
)]
pub async fn list_releases(
    State(state): State<AppState>,
    Query(filter): Query<ReleaseFilter>,
) -> AppResult<Json<Vec<Release>>> {
    let releases = if let Some(movie_id) = filter.movie_id {
        sqlx::query_as::<_, Release>(
            "SELECT * FROM release WHERE movie_id = ? ORDER BY quality_score DESC NULLS LAST, first_seen_at DESC",
        )
        .bind(movie_id)
        .fetch_all(&state.db)
        .await?
    } else if let Some(episode_id) = filter.episode_id {
        sqlx::query_as::<_, Release>(
            "SELECT * FROM release WHERE episode_id = ? ORDER BY quality_score DESC NULLS LAST, first_seen_at DESC",
        )
        .bind(episode_id)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, Release>("SELECT * FROM release ORDER BY first_seen_at DESC LIMIT 100")
            .fetch_all(&state.db)
            .await?
    };
    Ok(Json(releases))
}

/// Grab a specific release — the manual-picker action behind the
/// "Releases" drawer. Routes through the same service helpers as
/// automatic grabs so the download gets a proper `download_content`
/// link (movie or episode), URL resolution via `resolve_release_url`,
/// release-status update, and the `ReleaseGrabbed` event.
#[utoipa::path(
    post, path = "/api/v1/releases/{id}/grab",
    params(("id" = i64, Path)),
    responses((status = 200, description = "Release grabbed"), (status = 404), (status = 400)),
    tag = "releases", security(("api_key" = []))
)]
pub async fn grab_release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let release = sqlx::query_as::<_, Release>("SELECT * FROM release WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("release {id} not found")))?;

    // Re-check the blocklist at grab time. Other policy rejects
    // (DisallowedTier, NotAnUpgrade, etc.) represent user intent
    // when they clicked grab — pass through. Blocklist is the one
    // explicit "never grab this" signal that overrides intent;
    // surfacing the rejection here is the right behaviour.
    let scoped: Vec<crate::acquisition::BlocklistEntry> = if let Some(movie_id) = release.movie_id {
        sqlx::query_as("SELECT torrent_info_hash, source_title FROM blocklist WHERE movie_id = ?")
            .bind(movie_id)
            .fetch_all(&state.db)
            .await?
    } else if let Some(episode_id) = release.episode_id {
        sqlx::query_as("SELECT torrent_info_hash, source_title FROM blocklist WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_all(&state.db)
            .await?
    } else {
        Vec::new()
    };
    if scoped
        .iter()
        .any(|entry| entry.matches_release(release.info_hash.as_deref(), &release.title))
    {
        return Err(AppError::BadRequest(format!(
            "release {id} is on the blocklist; remove it first if you want to grab it"
        )));
    }

    // Dispatch to the movie- or episode-scoped grab helper so the
    // download row gets a real download_content link. A release tied
    // to neither is bad data; reject with 400 rather than silently
    // creating an orphan download that'd fail at import time.
    if let Some(movie_id) = release.movie_id {
        crate::acquisition::grab::grab_release(&state, id, movie_id)
            .await
            .map_err(AppError::Internal)?;
    } else if let Some(episode_id) = release.episode_id {
        crate::acquisition::grab::grab_episode_release(&state, id, episode_id)
            .await
            .map_err(AppError::Internal)?;
    } else {
        return Err(AppError::BadRequest(format!(
            "release {id} has no linked movie or episode"
        )));
    }
    Ok(StatusCode::OK)
}

/// Response for `POST /api/v1/releases/{id}/grab-and-watch`. The
/// unified play identity: `{kind, entity_id}` — enough for the
/// client to navigate to `/play/{kind}/{entity_id}`. Whether the
/// byte source is the library or a still-in-progress torrent is
/// the dispatcher's concern, not the caller's.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrabAndWatchReply {
    pub kind: crate::playback::PlayKind,
    pub entity_id: i64,
}

/// Grab a release AND start the torrent synchronously so the client
/// can stream it immediately. Idempotent: if a download for this
/// release already exists, returns its id (or redirects to the
/// imported media if it's already landed in the library).
#[utoipa::path(
    post, path = "/api/v1/releases/{id}/grab-and-watch",
    params(("id" = i64, Path)),
    responses((status = 200, body = GrabAndWatchReply), (status = 404)),
    tag = "releases", security(("api_key" = []))
)]
pub async fn grab_and_watch(
    State(state): State<AppState>,
    Path(release_id): Path<i64>,
) -> AppResult<Json<GrabAndWatchReply>> {
    // Idempotency: has this release already been grabbed?
    #[derive(sqlx::FromRow)]
    struct Existing {
        id: i64,
        state: String,
    }
    let existing = sqlx::query_as::<_, Existing>(
        "SELECT id, state FROM download WHERE release_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(release_id)
    .fetch_optional(&state.db)
    .await?;

    if let Some(d) = existing {
        // If the import already ran, short-circuit to the library
        // media_id so the client can navigate to the canonical play
        // URL instead of re-opening a streaming session against a
        // completed torrent.
        let imported_media_id: Option<i64> = sqlx::query_scalar(
            "SELECT m.id FROM media m
             JOIN download_content dc ON m.movie_id = dc.movie_id
             WHERE dc.download_id = ? AND dc.movie_id IS NOT NULL
             UNION ALL
             SELECT m.id FROM media m
             JOIN media_episode me ON me.media_id = m.id
             JOIN download_content dc ON me.episode_id = dc.episode_id
             WHERE dc.download_id = ? AND dc.episode_id IS NOT NULL
             LIMIT 1",
        )
        .bind(d.id)
        .bind(d.id)
        .fetch_optional(&state.db)
        .await?;
        let _ = imported_media_id; // presence doesn't change the reply shape
        // Reply is entity-keyed regardless of import state — the
        // dispatcher picks library vs torrent source per byte
        // request, so the client just needs to navigate to the
        // canonical URL.
        if crate::download::DownloadPhase::parse(&d.state)
            == Some(crate::download::DownloadPhase::Queued)
        {
            kick_queued(&state, d.id).await;
        }
        let (kind, entity_id) = resolve_entity_for_download(&state, d.id).await?;
        return Ok(Json(GrabAndWatchReply { kind, entity_id }));
    }

    // New grab — resolve movie_id from the release, then call the
    // full grab_release flow in acquisition::search which handles URL
    // resolution, status updates, and event emission.
    let movie_id: Option<i64> = sqlx::query_scalar("SELECT movie_id FROM release WHERE id = ?")
        .bind(release_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let Some(movie_id) = movie_id else {
        return Err(AppError::BadRequest(
            "release has no associated movie — TV episodes not yet supported".into(),
        ));
    };

    let download_id = crate::acquisition::grab::grab_release(&state, release_id, movie_id)
        .await
        .map_err(AppError::Internal)?;

    // Start the torrent NOW rather than waiting for the scheduler's
    // next tick — the user is staring at a loading spinner.
    kick_queued(&state, download_id).await;

    Ok(Json(GrabAndWatchReply {
        kind: crate::playback::PlayKind::Movie,
        entity_id: movie_id,
    }))
}

/// Look up the `(kind, entity_id)` a download is linked to via the
/// `download_content` table. Used by `grab_and_watch` when the
/// reply needs to cover a download that was created for either a
/// movie or a pack episode.
async fn resolve_entity_for_download(
    state: &AppState,
    download_id: i64,
) -> AppResult<(crate::playback::PlayKind, i64)> {
    let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT movie_id, episode_id FROM download_content WHERE download_id = ? LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(&state.db)
    .await?;
    let (movie_id, episode_id) = row.ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "download {download_id} has no content link"
        ))
    })?;
    if let Some(m) = movie_id {
        return Ok((crate::playback::PlayKind::Movie, m));
    }
    if let Some(e) = episode_id {
        return Ok((crate::playback::PlayKind::Episode, e));
    }
    Err(AppError::Internal(anyhow::anyhow!(
        "download_content has neither movie_id nor episode_id"
    )))
}

/// Release row enriched with download + blocklist state, for the
/// "Release history / manual picker" UI. Flat shape so the frontend
/// table can render each column without nested lookups.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct ReleaseWithStatus {
    pub id: i64,
    pub title: String,
    pub size: Option<i64>,
    pub indexer_name: Option<String>,
    pub resolution: Option<i64>,
    pub source: Option<String>,
    pub video_codec: Option<String>,
    pub release_group: Option<String>,
    pub quality_score: Option<i64>,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub publish_date: Option<String>,
    pub is_proper: bool,
    pub is_repack: bool,
    pub is_remux: bool,
    pub first_seen_at: String,
    pub grabbed_at: Option<String>,
    /// Latest non-terminal download state for this release. None when
    /// never grabbed. Frontend uses this to tag the row:
    /// `queued`/`grabbing`/`downloading`/`stalled`/`paused`/`importing`
    /// /`completed`/`imported`/`seeding`/`failed`.
    pub download_state: Option<String>,
    /// `error_message` from the most recent failed download of this
    /// release — explains why a grab didn't stick so the user can
    /// pick a different release intelligently.
    pub download_error: Option<String>,
    /// True when this release is on the content's blocklist (either
    /// user-added or auto-tombstoned by `blocklist_and_retry`). The UI
    /// greys out the row and swaps the Grab button for "Unblock".
    pub is_blocklisted: bool,
}

/// Per-episode release history — every release we've seen from any
/// indexer for this episode, with download + blocklist state joined
/// so the UI can render a one-glance "what did we try, what's pending"
/// view. Powers the manual-picker drawer and "why isn't this episode
/// downloading?" diagnostics.
#[utoipa::path(
    get, path = "/api/v1/episodes/{id}/releases",
    params(("id" = i64, Path)),
    responses((status = 200, body = Vec<ReleaseWithStatus>), (status = 404)),
    tag = "releases", security(("api_key" = []))
)]
pub async fn episode_releases(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<ReleaseWithStatus>>> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM episode WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("episode {id} not found")));
    }
    let releases: Vec<ReleaseWithStatus> = sqlx::query_as(
        "SELECT r.id, r.title, r.size, i.name AS indexer_name,
                r.resolution, r.source, r.video_codec, r.release_group,
                r.quality_score, r.seeders, r.leechers,
                r.publish_date, r.is_proper, r.is_repack, r.is_remux,
                r.first_seen_at, r.grabbed_at,
                (SELECT state FROM download
                 WHERE release_id = r.id
                 ORDER BY id DESC LIMIT 1) AS download_state,
                (SELECT error_message FROM download
                 WHERE release_id = r.id AND state = 'failed'
                 ORDER BY id DESC LIMIT 1) AS download_error,
                EXISTS(SELECT 1 FROM blocklist b
                       WHERE b.episode_id = r.episode_id
                         AND (b.torrent_info_hash = r.info_hash
                              OR b.source_title = r.title)) AS is_blocklisted
         FROM release r
         LEFT JOIN indexer i ON i.id = r.indexer_id
         WHERE r.episode_id = ?
         ORDER BY r.quality_score DESC NULLS LAST, r.first_seen_at DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(releases))
}

/// Per-movie release history — same shape as `episode_releases` but
/// scoped to a `movie_id`.
#[utoipa::path(
    get, path = "/api/v1/movies/{id}/releases",
    params(("id" = i64, Path)),
    responses((status = 200, body = Vec<ReleaseWithStatus>), (status = 404)),
    tag = "releases", security(("api_key" = []))
)]
pub async fn movie_releases(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<ReleaseWithStatus>>> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM movie WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("movie {id} not found")));
    }
    let releases: Vec<ReleaseWithStatus> = sqlx::query_as(
        "SELECT r.id, r.title, r.size, i.name AS indexer_name,
                r.resolution, r.source, r.video_codec, r.release_group,
                r.quality_score, r.seeders, r.leechers,
                r.publish_date, r.is_proper, r.is_repack, r.is_remux,
                r.first_seen_at, r.grabbed_at,
                (SELECT state FROM download
                 WHERE release_id = r.id
                 ORDER BY id DESC LIMIT 1) AS download_state,
                (SELECT error_message FROM download
                 WHERE release_id = r.id AND state = 'failed'
                 ORDER BY id DESC LIMIT 1) AS download_error,
                EXISTS(SELECT 1 FROM blocklist b
                       WHERE b.movie_id = r.movie_id
                         AND (b.torrent_info_hash = r.info_hash
                              OR b.source_title = r.title)) AS is_blocklisted
         FROM release r
         LEFT JOIN indexer i ON i.id = r.indexer_id
         WHERE r.movie_id = ?
         ORDER BY r.quality_score DESC NULLS LAST, r.first_seen_at DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(releases))
}

/// Best-effort synchronous start of a queued download. Errors are
/// logged but not surfaced — the scheduler's next tick will retry.
async fn kick_queued(state: &AppState, download_id: i64) {
    #[derive(sqlx::FromRow)]
    struct DlRow {
        title: String,
        magnet_url: Option<String>,
    }
    let row = sqlx::query_as::<_, DlRow>(
        "SELECT title, magnet_url FROM download WHERE id = ? AND state = 'queued'",
    )
    .bind(download_id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(dl)) = row else {
        return;
    };
    if let Err(e) = crate::download::monitor::start_download(
        &state.db,
        &state.event_tx,
        state.torrent.as_deref(),
        download_id,
        &dl.title,
        dl.magnet_url.as_deref(),
    )
    .await
    {
        tracing::warn!(download_id, error = %e, "kick_queued failed — scheduler will retry");
    }
}
