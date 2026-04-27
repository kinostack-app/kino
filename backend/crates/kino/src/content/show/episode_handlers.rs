//! Episode-scoped handlers — split out of `content/show/handlers.rs`
//! to keep that file under the 800-line target. These endpoints all
//! mutate state on a single `episode` row (mark/unmark watched,
//! analyse intro, redownload, acquire, discard) and need none of the
//! show-listing helpers their sibling owns.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use serde::Deserialize;
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::events::display::episode_display_title;
use crate::state::AppState;

/// `POST /api/v1/episodes/{id}/watched` — mark a single episode as
/// watched. Used by the episode-row overflow menu when the user has
/// already seen it elsewhere and wants Next Up to move on.
#[utoipa::path(
    post, path = "/api/v1/episodes/{id}/watched",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn mark_episode_watched(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE episode
         SET watched_at = ?, play_count = play_count + 1, playback_position_ticks = 0
         WHERE id = ?",
    )
    .bind(&now)
    .bind(id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("episode {id} not found")));
    }
    let title = episode_display_title(&state.db, id).await;
    state.emit(crate::events::AppEvent::Watched {
        movie_id: None,
        episode_id: Some(id),
        title,
    });
    push_episode_watched_to_trakt(&state, id, Some(now)).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/episodes/{id}/watched` — undo a "mark as watched"
/// so the episode reappears in Next Up.
#[utoipa::path(
    delete, path = "/api/v1/episodes/{id}/watched",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn unmark_episode_watched(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Clearing `last_searched_at` here matches `redownload_episode`'s
    // behaviour: un-marking watched reverts an episode to wanted, so
    // a stale timestamp shouldn't gate the next sweep. Without this,
    // an episode that was searched and found nothing → manually
    // watched → un-watched would sit in the wrong backoff tier
    // for up to 30 days.
    let result = sqlx::query(
        "UPDATE episode SET watched_at = NULL, playback_position_ticks = 0, last_searched_at = NULL WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("episode {id} not found")));
    }
    let title = episode_display_title(&state.db, id).await;
    state.emit(crate::events::AppEvent::Unwatched {
        movie_id: None,
        episode_id: Some(id),
        title,
    });
    push_episode_unwatch_to_trakt(&state, id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Manually kick intro/credits analysis for a season. Fire-and-forget —
/// returns 202 Accepted immediately, work happens on a background task.
/// Used by the scheduled catch-up and any future admin tool.
#[utoipa::path(
    post, path = "/api/v1/shows/{id}/seasons/{season_number}/analyse-intro",
    params(("id" = i64, Path), ("season_number" = i64, Path)),
    responses((status = 202), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn analyse_season_intro(
    State(state): State<AppState>,
    Path((show_id, season_number)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM show WHERE id = ?")
        .bind(show_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound(format!("show {show_id} not found")));
    }
    let s = state.clone();
    tokio::spawn(async move {
        if let Err(e) =
            crate::playback::intro_skipper::analyse_season(&s, show_id, season_number).await
        {
            tracing::warn!(error = %e, show_id, season_number, "manual intro analysis failed");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

async fn push_episode_watched_to_trakt(
    state: &AppState,
    episode_id: i64,
    watched_at: Option<String>,
) {
    use crate::integrations::trakt;
    if let Ok(client) = trakt::TraktClient::from_db(state.db.clone()).await
        && let Err(e) = trakt::sync::push_watched(&client, None, Some(episode_id), watched_at).await
    {
        tracing::warn!(error = %e, episode_id, "trakt episode watched push failed");
    }
}

async fn push_episode_unwatch_to_trakt(state: &AppState, episode_id: i64) {
    use crate::integrations::trakt;
    if let Ok(client) = trakt::TraktClient::from_db(state.db.clone()).await
        && let Err(e) = trakt::sync::push_unwatch(&client, None, Some(episode_id)).await
    {
        tracing::warn!(error = %e, episode_id, "trakt episode unwatch push failed");
    }
}

/// `POST /api/v1/episodes/{id}/redownload` — reset an imported
/// episode so the scheduler (or next Play click) re-searches for a
/// new release. Unlinks the current media but doesn't delete the
/// file on disk — user can remove it manually if desired; cleanup
/// is conservative because accidental redownload shouldn't torch the
/// existing copy while the new one is being fetched.
#[utoipa::path(
    post, path = "/api/v1/episodes/{id}/redownload",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn redownload_episode(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM episode WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("episode {id} not found")));
    }

    // Unlink imported media so the UI stops showing the episode as
    // available. `media_episode` rows disappear; orphan `media` rows
    // (no remaining episode/movie link) get pruned to avoid leaking
    // rows, but their file_path is left on disk — user can reclaim
    // space manually once the redownload completes.
    sqlx::query("DELETE FROM media_episode WHERE episode_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query(
        "DELETE FROM media
         WHERE movie_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.media_id = media.id)",
    )
    .execute(&state.db)
    .await?;

    // Also detach any in-flight downloads so a retry actually goes
    // back through search instead of reusing the previous torrent.
    sqlx::query(
        "DELETE FROM download_content WHERE episode_id = ?
           AND download_id IN (SELECT id FROM download WHERE state != 'imported')",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    sqlx::query(
        // Clearing media + downloads above already gets us to a
        // derived 'wanted' phase. This UPDATE just re-enables the
        // two monitoring axes + resets the search debounce so the
        // sweep / next Play click triggers immediately.
        "UPDATE episode SET acquire = 1, in_scope = 1, last_searched_at = NULL WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/episodes/{id}/acquire` — flip an episode's
/// `acquire` bit on so the scheduler's wanted-sweep picks it up.
/// Used by the "Get" button on episode cards: when a show was
/// followed with "future only" (or seasons were unmonitored via the
/// Manage dialog), the user can still explicitly request a single
/// episode without changing season-wide settings.
///
/// Minimal side effects: no media / download cleanup (nothing to
/// clean for an un-imported episode), just flip the bit and clear
/// the search debounce so the next sweep tick considers it
/// immediately. Scheduler sweep cadence is a few seconds — users
/// perceive the card flip to "Searching" as near-instant.
#[utoipa::path(
    post, path = "/api/v1/episodes/{id}/acquire",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
pub async fn acquire_episode(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM episode WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound(format!("episode {id} not found")));
    }
    sqlx::query(
        "UPDATE episode SET acquire = 1, in_scope = 1, last_searched_at = NULL WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    // Kick an immediate search in the background — otherwise the
    // episode sits in "searching" state until the next wanted-sweep
    // tick, which is `auto_search_interval` (default 15 min). That
    // made the Get button feel broken ("it never leaves searching").
    // `search_episode` emits `SearchStarted` + downstream
    // `ReleaseGrabbed` / `DownloadStarted` events, so the UI flows
    // through searching → queued → downloading without waiting for
    // the scheduler. Fire-and-forget: the handler returns 204 as
    // soon as the DB write lands.
    let task_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::acquisition::search::episode::search_episode(&task_state, id).await {
            tracing::warn!(episode_id = id, error = %e, "acquire_episode: immediate search failed");
        }
    });

    Ok(StatusCode::NO_CONTENT)
}

/// Body for the by-TMDB acquire endpoint — supports the case where
/// the user wants to grab a single episode of a show they don't
/// follow yet. Mirrors `watch_now_episode_by_tmdb`'s inputs.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AcquireEpisodeByTmdb {
    pub show_tmdb_id: i64,
    pub season_number: i64,
    pub episode_number: i64,
}

/// `POST /api/v1/episodes/acquire-by-tmdb` — acquire a single
/// episode identified by `(show_tmdb_id, season, episode)`. When the
/// show isn't in the library yet, auto-follows it with "future only"
/// monitoring (same light-commitment pattern `watch_now_episode_by_tmdb`
/// uses) so the user's intent to grab this one episode doesn't
/// accidentally bulk-grab the entire back-catalog.
///
/// Symmetric with `/episodes/{id}/acquire` for the in-library case
/// but also handles the cold-start: user on a TMDB show detail
/// clicks "Get" on S03E07 without ever following the show.
#[utoipa::path(
    post, path = "/api/v1/episodes/acquire-by-tmdb",
    request_body = AcquireEpisodeByTmdb,
    responses(
        (status = 204),
        (status = 404, description = "Show / season / episode not found on TMDB"),
    ),
    tag = "shows", security(("api_key" = []))
)]
pub async fn acquire_episode_by_tmdb(
    State(state): State<AppState>,
    Json(input): Json<AcquireEpisodeByTmdb>,
) -> AppResult<StatusCode> {
    // Reuse watch-now's auto-follow + episode-lookup helper so the
    // cold-start semantics stay consistent with Play.
    let episode_id = crate::watch_now::handlers::find_or_create_episode(
        &state,
        input.show_tmdb_id,
        input.season_number,
        input.episode_number,
    )
    .await?;
    acquire_episode(State(state), Path(episode_id)).await
}

/// `POST /api/v1/episodes/{id}/discard` — inverse of `acquire`.
/// Symmetric with the episode card's "X" button: the user no longer
/// wants this episode. Walks back the acquisition completely:
///   1. Cancel any in-flight download for the episode (stops librqbit
///      + emits `DownloadCancelled` so the UI snaps across tabs).
///   2. Delete linked `media_episode` rows + prune orphan `media`
///      (file left on disk; user reclaims manually if desired).
///   3. Set `acquire = 0` so the scheduler stops trying.
#[utoipa::path(
    post, path = "/api/v1/episodes/{id}/discard",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "shows", security(("api_key" = []))
)]
#[allow(clippy::too_many_lines)] // one linear cleanup pipeline; splitting scatters it
pub async fn discard_episode(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Grab the show_id + a readable label up-front so the
    // ContentRemoved event has the show-scope the WS handler needs to
    // invalidate `show-episodes` / `LIBRARY_SHOWS_KEY` on other tabs.
    #[derive(sqlx::FromRow)]
    struct EpRow {
        show_id: i64,
        season_number: i64,
        episode_number: i64,
        title: Option<String>,
        show_title: String,
    }
    let row: Option<EpRow> = sqlx::query_as::<_, EpRow>(
        "SELECT e.show_id, e.season_number, e.episode_number, e.title,
                s.title AS show_title
         FROM episode e
         JOIN show s ON s.id = e.show_id
         WHERE e.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else {
        return Err(AppError::NotFound(format!("episode {id} not found")));
    };

    // Cancel in-flight downloads for this episode. Reuse the same
    // cancel_download path so librqbit is stopped + DownloadCancelled
    // fires (keeps cross-window cards in lockstep via the existing
    // invalidate pattern).
    let download_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT d.id FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         WHERE dc.episode_id = ?
           AND d.state NOT IN ('failed','imported','completed','cleaned_up')",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    for dl_id in download_ids {
        if let Err(e) =
            crate::download::handlers::cancel_download(State(state.clone()), Path(dl_id)).await
        {
            tracing::warn!(episode_id = id, download_id = dl_id, error = %e, "discard_episode: cancel failed");
        }
    }

    // Collect files that are about to become orphans so we can
    // unlink them from disk after the DB rows are gone. Narrow to
    // media that *only* this episode references — a pack-release
    // linked to several episodes must survive until the last
    // episode is discarded.
    let media_paths: Vec<(i64, String)> = sqlx::query_as(
        "SELECT m.id, m.file_path
         FROM media m
         JOIN media_episode me ON me.media_id = m.id
         WHERE me.episode_id = ?
           AND m.movie_id IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM media_episode me2
               WHERE me2.media_id = m.id AND me2.episode_id != ?
           )",
    )
    .bind(id)
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    // Drop the media link and prune the now-orphan media row. The
    // file on disk is deleted below so discarding an episode leaves
    // no residue — this used to leave the file behind (the comment
    // that this "mirrors redownload_episode" was wrong; that one
    // *intentionally* keeps the file so the user can reclaim space
    // after the new download lands).
    sqlx::query("DELETE FROM media_episode WHERE episode_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query(
        "DELETE FROM media
         WHERE movie_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.media_id = media.id)",
    )
    .execute(&state.db)
    .await?;
    let library_root = crate::content::movie::handlers::fetch_library_root(&state.db).await;
    let library_root_path = library_root.as_deref().map(std::path::Path::new);
    for (media_id, path) in &media_paths {
        crate::content::movie::handlers::remove_library_file(*media_id, path, library_root_path)
            .await;
    }

    // Drop both axes: acquire=0 stops the scheduler, in_scope=0 keeps
    // the episode out of Next Up / progress rollups. Without the
    // in_scope drop, a discarded episode would still surface as "next
    // up" on the show detail page — a `show_watch_state` aired-
    // fallback picks any in_scope unwatched row, regardless of
    // acquire.
    sqlx::query("UPDATE episode SET acquire = 0, in_scope = 0 WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    // Purge `download_content` links for this episode. `cancel_
    // download` above only touches *active-state* downloads (queued/
    // downloading/etc.); terminal rows (imported / failed / completed)
    // keep their `download_content` rows, and those would otherwise
    // linger pointing at an episode that's about to be dropped out of
    // scope. Not a FK violation (the column is nullable / cascade-
    // less), but an invariant worth holding: "every `download_content`
    // row points at a still-in-scope entity." Scoped to `episode_id =
    // id` so season packs linked to OTHER episodes aren't affected.
    sqlx::query("DELETE FROM download_content WHERE episode_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    // Emit so other tabs invalidate their caches without polling.
    // `ContentRemoved` is silent (no toast — it's in the frontend's
    // SILENT_EVENTS list) and its handler invalidates exactly the
    // caches an episode discard affects: LIBRARY_SHOWS_KEY (active_
    // download / next_episode projections), show-episodes,
    // showWatchState, continueWatching, calendar. `cancel_download`
    // already fired DownloadCancelled for any in-flight download, but
    // episodes with only imported media don't go through that path —
    // this event closes that hole.
    let ep_label = format!(
        "S{:02}E{:02}{}",
        row.season_number,
        row.episode_number,
        match row.title {
            Some(t) if !t.is_empty() => format!(" · {t}"),
            _ => String::new(),
        }
    );
    let _ = state
        .event_tx
        .send(crate::events::AppEvent::ContentRemoved {
            movie_id: None,
            show_id: Some(row.show_id),
            title: format!("{} — {}", row.show_title, ep_label),
        });

    // Cascade cleanup for adhoc shows that now have nothing to
    // track. A show auto-followed by Play / Get / acquire-by-tmdb
    // exists solely to service acquisition; once every episode it
    // acquired is gone and no media or active downloads remain,
    // the show row is a ghost — delete it. `Explicit` shows are
    // untouched: the user deliberately followed, they keep the
    // empty-but-followed state until they Remove it themselves.
    let should_cascade: bool = sqlx::query_scalar(
        "SELECT (
            s.follow_intent = 'adhoc'
            AND NOT EXISTS (SELECT 1 FROM episode WHERE show_id = s.id AND acquire = 1)
            AND NOT EXISTS (
                SELECT 1 FROM media_episode me
                JOIN episode e ON e.id = me.episode_id
                WHERE e.show_id = s.id
            )
            AND NOT EXISTS (
                SELECT 1 FROM download_content dc
                JOIN download d ON d.id = dc.download_id
                JOIN episode e ON e.id = dc.episode_id
                WHERE e.show_id = s.id
                  AND d.state NOT IN ('failed','imported','completed','cleaned_up')
            )
         ) AS cascade
         FROM show s WHERE s.id = ?",
    )
    .bind(row.show_id)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or(false);
    if should_cascade
        && let Err(e) = super::handlers::delete_show(State(state.clone()), Path(row.show_id)).await
    {
        tracing::warn!(show_id = row.show_id, error = %e, "discard_episode: cascade delete failed");
    }

    Ok(StatusCode::NO_CONTENT)
}
