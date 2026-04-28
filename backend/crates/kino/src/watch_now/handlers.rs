//! Watch-now orchestrator — one endpoint that takes a movie (`tmdb_id`)
//! or episode (library id) and kicks off the full acquire-to-stream
//! flow: auto-add the movie if missing, create a placeholder
//! `searching` download row, return the `download_id` immediately,
//! then run search + grab + start in a background task. The UI
//! navigates to the player on the returned id and uses the download
//! row's state + events to drive its loading stepper.
//!
//! Returning early lets the player load instantly — the user sees
//! "Finding a release → Starting the download → …" as real backend
//! events happen, instead of staring at a pre-navigation toast for
//! 5–30 seconds while search runs.
//!
//! The only cases that still resolve synchronously:
//!   - already-imported content (short-circuit to `/play/$mediaId`)
//!   - already-active download for this entity (reuse its id)
//!   - no enabled indexers (409 before we create a row — the user's
//!     setup is broken, not in-flight, so a player that just errors
//!     would add a round-trip for no gain)
//!
//! Failures inside phase 2 (no releases, timeout, grab error) land on
//! the download row as `state = 'failed'` + `error_message`; the
//! player's `/stream/:id/info` polls pick them up and render.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::acquisition::release::GrabAndWatchReply;
use crate::content::movie::model::CreateMovie;
use crate::content::show::model::CreateShow;
use crate::download::DownloadPhase;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::watch_now::WatchNowPhase;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(45);

/// Request body — discriminated on `kind`. Movies are identified by
/// TMDB id (so we can auto-add missing entries); episodes by their
/// internal library id (the show must already be followed, which
/// ensures the episode row exists).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchNowRequest {
    Movie {
        tmdb_id: i64,
    },
    Episode {
        episode_id: i64,
    },
    /// Episode identified by TMDB show + season + episode numbers —
    /// lets the UI's episode rows (which come from TMDB, not the
    /// library) trigger Play even when the show isn't followed yet.
    /// We auto-follow the show and resolve the matching episode row.
    EpisodeByTmdb {
        show_tmdb_id: i64,
        season: i64,
        episode: i64,
    },
    /// Show-level Play — the user clicked Play on a show card or
    /// hero without picking an episode. We resolve to the right
    /// episode automatically: first unwatched aired (Next Up) if
    /// the user is mid-series, the pilot otherwise. Auto-follows
    /// the show with a minimal-commitment policy (future episodes
    /// monitored; no bulk back-catalog beyond what the scheduler
    /// naturally picks up from the default monitor flags).
    ShowSmartPlay {
        show_tmdb_id: i64,
    },
}

/// `POST /api/v1/watch-now` — see module docs.
#[utoipa::path(
    post, path = "/api/v1/watch-now",
    request_body = WatchNowRequest,
    responses(
        (status = 200, body = GrabAndWatchReply),
        (status = 404, description = "Episode not found (show isn't followed)"),
        (status = 409, description = "No releases found after search"),
        (status = 504, description = "Search timed out")
    ),
    tag = "playback", security(("api_key" = []))
)]
pub async fn watch_now(
    State(state): State<AppState>,
    Json(input): Json<WatchNowRequest>,
) -> AppResult<Json<GrabAndWatchReply>> {
    // Serialize the whole handler so a fast double-click on Play
    // doesn't race through the `find_active_download` check twice
    // and INSERT two placeholder rows for the same release. Critical
    // section is small (a few SELECTs + one INSERT per call) so the
    // process-wide mutex is a cheap insurance policy.
    let _guard = state.watch_now_lock.lock().await;
    tracing::info!(request = ?input, "watch-now requested");
    let result = match input {
        WatchNowRequest::Movie { tmdb_id } => watch_now_movie(&state, tmdb_id).await,
        WatchNowRequest::Episode { episode_id } => watch_now_episode(&state, episode_id).await,
        WatchNowRequest::EpisodeByTmdb {
            show_tmdb_id,
            season,
            episode,
        } => {
            let episode_id = find_or_create_episode(&state, show_tmdb_id, season, episode).await?;
            watch_now_episode(&state, episode_id).await
        }
        WatchNowRequest::ShowSmartPlay { show_tmdb_id } => {
            watch_now_show_smart(&state, show_tmdb_id).await
        }
    };
    match &result {
        Ok(json) => tracing::info!(outcome = ?json.0, "watch-now resolved"),
        Err(e) => tracing::warn!(error = %e, "watch-now rejected"),
    }
    result
}

/// Sentinel prefix the frontend pattern-matches to render a
/// "Go to Indexers" action in the error state. Kept in one place
/// so renames surface as frontend/backend sync mismatches in PR review.
const NO_INDEXERS_SENTINEL: &str =
    "No indexers configured. Add one in Settings \u{2192} Indexers to start searching.";

/// Return 409 with the no-indexers sentinel before kicking a search
/// — avoids the 45s timeout + generic "no releases found" path when
/// the user simply hasn't set Prowlarr up yet.
async fn ensure_indexers_configured(state: &AppState) -> AppResult<()> {
    let enabled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM indexer
         WHERE enabled = 1
           AND (disabled_until IS NULL OR disabled_until < ?)",
    )
    .bind(crate::time::Timestamp::now().to_rfc3339())
    .fetch_one(&state.db)
    .await?;
    if enabled == 0 {
        return Err(AppError::Conflict(NO_INDEXERS_SENTINEL.into()));
    }
    Ok(())
}

pub(crate) async fn find_or_create_episode(
    state: &AppState,
    show_tmdb_id: i64,
    season: i64,
    episode: i64,
) -> AppResult<i64> {
    // Resolve or create the parent show.
    let existing_show: Option<i64> = sqlx::query_scalar("SELECT id FROM show WHERE tmdb_id = ?")
        .bind(show_tmdb_id)
        .fetch_optional(&state.db)
        .await?;
    let show_id = if let Some(id) = existing_show {
        id
    } else {
        // Light-commitment auto-follow: streaming a specific episode
        // is exploration, not a bulk-grab commitment. Follow the show,
        // seed future-only monitoring, and leave every existing
        // episode at acquire=0 so the scheduler doesn't hammer
        // indexers for the entire back-catalog. The specific episode
        // the user is streaming still gets grabbed directly by the
        // watch-now flow right after this function returns.
        //
        // `create_show_inner` (not the public handler) so the
        // `wanted_search` trigger doesn't race the scheduler ahead
        // of our own searching-placeholder row, duplicating the
        // download.
        let created = crate::content::show::handlers::create_show_inner(
            state,
            CreateShow {
                tmdb_id: show_tmdb_id,
                quality_profile_id: None,
                monitored: Some(true),
                monitor_new_items: Some("future".into()),
                seasons_to_monitor: Some(vec![]),
                follow_intent: Some("adhoc".into()),
                monitor_specials: None,
            },
        )
        .await?;
        created.id
    };

    let ep_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM episode WHERE show_id = ? AND season_number = ? AND episode_number = ?",
    )
    .bind(show_id)
    .bind(season)
    .bind(episode)
    .fetch_optional(&state.db)
    .await?;
    ep_id.ok_or_else(|| {
        AppError::NotFound(format!(
            "episode S{season:02}E{episode:02} not populated for show"
        ))
    })
}

/// Show-level Play: resolve the "right" episode (next unwatched aired,
/// pilot fallback) and delegate to the episode watch-now flow. Auto-
/// follows the show if it isn't already in the library.
async fn watch_now_show_smart(
    state: &AppState,
    show_tmdb_id: i64,
) -> AppResult<Json<GrabAndWatchReply>> {
    // Auto-follow on first Play with *minimal commitment*: the show
    // is tracked, future TMDB-new episodes will be monitored, but
    // every existing episode goes in at monitored=0 so the scheduler's
    // wanted-sweep doesn't bulk-hammer indexers for 98 back-catalog
    // episodes the user never asked for. Users who want a real bulk
    // pull use the Follow dialog instead of Play. (`show_watch_state`
    // knows how to compute Next Up / progress without the monitored
    // filter when zero episodes are monitored, so Next Up still works
    // for the play-auto-follow case.)
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM show WHERE tmdb_id = ?")
        .bind(show_tmdb_id)
        .fetch_optional(&state.db)
        .await?;
    let show_id = if let Some(id) = existing {
        id
    } else {
        // `create_show_inner` — same reasoning as
        // `find_or_create_episode`: skip the `wanted_search` trigger
        // so it doesn't race our inline search.
        let created = crate::content::show::handlers::create_show_inner(
            state,
            CreateShow {
                tmdb_id: show_tmdb_id,
                quality_profile_id: None,
                monitored: Some(true),
                monitor_new_items: Some("future".into()),
                seasons_to_monitor: Some(vec![]),
                follow_intent: Some("adhoc".into()),
                monitor_specials: None,
            },
        )
        .await?;
        created.id
    };

    // Resolve target episode. Priority order:
    //   1. Earliest unwatched episode that's already imported — if
    //      any season is downloaded, start there instead of kicking
    //      off a fresh grab for the pilot. This matches what the
    //      user sees on the detail page's Next Up.
    //   2. Earliest unwatched aired episode (Sonarr-style pilot
    //      fallback — this is where a cold library starts).
    //   3. Earliest aired (fully-watched show → replay the pilot).
    // Season 0 is excluded: Specials rarely have clean scene releases,
    // so auto-targeting them means the search returns N results and
    // we reject them all as off-target.
    // `monitored` is ignored on purpose — Play is explicit user
    // intent, not a scheduler-queue membership check.
    let target: Option<i64> = sqlx::query_scalar(
        "SELECT e.id FROM episode e
         WHERE e.show_id = ?
           AND e.season_number >= 1
           AND e.watched_at IS NULL
           AND EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
         ORDER BY e.season_number, e.episode_number
         LIMIT 1",
    )
    .bind(show_id)
    .fetch_optional(&state.db)
    .await?;
    let target = if let Some(id) = target {
        id
    } else {
        let aired_unwatched: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM episode
             WHERE show_id = ?
               AND season_number >= 1
               AND watched_at IS NULL
               AND (air_date_utc IS NULL OR air_date_utc <= datetime('now'))
             ORDER BY season_number, episode_number
             LIMIT 1",
        )
        .bind(show_id)
        .fetch_optional(&state.db)
        .await?;
        match aired_unwatched {
            Some(id) => id,
            None => sqlx::query_scalar(
                "SELECT id FROM episode
                 WHERE show_id = ?
                   AND season_number >= 1
                   AND (air_date_utc IS NULL OR air_date_utc <= datetime('now'))
                 ORDER BY season_number, episode_number
                 LIMIT 1",
            )
            .bind(show_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| {
                AppError::Conflict("This show has no aired episodes yet — nothing to play.".into())
            })?,
        }
    };

    watch_now_episode(state, target).await
}

async fn watch_now_movie(state: &AppState, tmdb_id: i64) -> AppResult<Json<GrabAndWatchReply>> {
    // 1. Resolve or create the movie.
    let movie_id = find_or_create_movie(state, tmdb_id).await?;

    // 2. Already imported? Short-circuit to the library URL.
    if find_imported_media_for_movie(state, movie_id)
        .await?
        .is_some()
    {
        return Ok(Json(GrabAndWatchReply {
            kind: crate::playback::PlayKind::Movie,
            entity_id: movie_id,
        }));
    }

    // 3. Active download? Reuse it (the user is probably re-clicking
    //    Play on a movie whose torrent is still running, including
    //    one of our own placeholder `searching` rows).
    if find_active_download_for_movie(state, movie_id)
        .await?
        .is_some()
    {
        return Ok(Json(GrabAndWatchReply {
            kind: crate::playback::PlayKind::Movie,
            entity_id: movie_id,
        }));
    }

    // 4. Pre-flight: fail fast if no indexers are configured. The
    //    stepper can't meaningfully represent this — it's a setup
    //    issue, not an in-flight one — so surface the 409 inline
    //    with the "Go to Indexers" sentinel intact.
    ensure_indexers_configured(state).await?;

    // 5. Create placeholder `searching` download + content link.
    //    Anchoring the download row here gives the frontend a stable
    //    id before any search/grab work — the player loads and drives
    //    its stepper off this row's state transitions.
    let title = find_movie_title(state, movie_id).await?;
    let download_id = create_searching_download_for_movie(state, movie_id, &title).await?;

    // 6. Spawn phase 2: search → pick best → fulfill → kick.
    spawn_movie_phase_two(state.clone(), download_id, movie_id);

    Ok(Json(GrabAndWatchReply {
        kind: crate::playback::PlayKind::Movie,
        entity_id: movie_id,
    }))
}

async fn watch_now_episode(
    state: &AppState,
    episode_id: i64,
) -> AppResult<Json<GrabAndWatchReply>> {
    // Episode must already exist — shows must be followed before
    // their episodes can be played. 404 is the correct signal for
    // the UI to prompt "follow this show first".
    let exists: bool = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM episode WHERE id = ?")
        .bind(episode_id)
        .fetch_one(&state.db)
        .await?
        > 0;
    if !exists {
        return Err(AppError::NotFound(format!(
            "episode {episode_id} not in library — follow the show first"
        )));
    }

    // Declare intent: the user is actively asking for this episode,
    // so flip `acquire = 1`. Without this, a show auto-followed via
    // Play (which seeds every episode at acquire=0) leaves the
    // streamed episode invisible to the monitored-seasons stats until
    // import completes — the Manage dialog's tri-state checkbox renders
    // blank instead of partial. Also ensures the scheduler will retry
    // search if this grab fails before the file is imported.
    sqlx::query("UPDATE episode SET acquire = 1, in_scope = 1 WHERE id = ?")
        .bind(episode_id)
        .execute(&state.db)
        .await?;

    if find_imported_media_for_episode(state, episode_id)
        .await?
        .is_some()
    {
        return Ok(Json(GrabAndWatchReply {
            kind: crate::playback::PlayKind::Episode,
            entity_id: episode_id,
        }));
    }

    if find_active_download_for_episode(state, episode_id)
        .await?
        .is_some()
    {
        return Ok(Json(GrabAndWatchReply {
            kind: crate::playback::PlayKind::Episode,
            entity_id: episode_id,
        }));
    }

    ensure_indexers_configured(state).await?;

    let title = find_episode_title(state, episode_id).await?;
    let download_id = create_searching_download_for_episode(state, episode_id, &title).await?;

    spawn_episode_phase_two(state.clone(), download_id, episode_id);

    Ok(Json(GrabAndWatchReply {
        kind: crate::playback::PlayKind::Episode,
        entity_id: episode_id,
    }))
}

// ── Helpers ─────────────────────────────────────────────────────

/// Find the library `movie_id` for this `tmdb_id`, creating it via the
/// existing create-movie pipeline if absent. Uses the HTTP handler
/// directly because it already contains the full TMDB-fetch +
/// default-profile + INSERT logic we'd otherwise duplicate.
async fn find_or_create_movie(state: &AppState, tmdb_id: i64) -> AppResult<i64> {
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM movie WHERE tmdb_id = ?")
        .bind(tmdb_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    // Call the create-movie helper (not the public handler) so the
    // `MovieAdded` event + `wanted_search` trigger don't fire before
    // we've had a chance to create our own `searching` download row.
    // Firing the trigger now would let the scheduler's wanted-sweep
    // race ahead, find "movie exists with no active download," and
    // auto-grab — producing a second download row for the same
    // release we're about to handle inline.
    let created = crate::content::movie::handlers::create_movie_inner(
        state,
        CreateMovie {
            tmdb_id,
            quality_profile_id: None,
            monitored: Some(true),
        },
    )
    .await?;
    Ok(created.id)
}

async fn find_imported_media_for_movie(state: &AppState, movie_id: i64) -> AppResult<Option<i64>> {
    let r: Option<i64> = sqlx::query_scalar("SELECT id FROM media WHERE movie_id = ? LIMIT 1")
        .bind(movie_id)
        .fetch_optional(&state.db)
        .await?;
    Ok(r)
}

async fn find_imported_media_for_episode(
    state: &AppState,
    episode_id: i64,
) -> AppResult<Option<i64>> {
    let r: Option<i64> = sqlx::query_scalar(
        "SELECT me.media_id FROM media_episode me
         WHERE me.episode_id = ? LIMIT 1",
    )
    .bind(episode_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(r)
}

/// Active download linked to this movie via `download_content`. "Active"
/// means any state that the streamer can still serve bytes from —
/// everything except fully-failed or fully-imported. (Imported is
/// handled by `find_imported_media_for_movie` above, so we include
/// those states here but callers will never reach this path after the
/// imported-short-circuit.)
async fn find_active_download_for_movie(
    state: &AppState,
    movie_id: i64,
) -> AppResult<Option<(i64, Option<String>)>> {
    let r: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT d.id, d.torrent_hash FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         WHERE dc.movie_id = ?
           AND d.state IN ('searching', 'queued', 'grabbing', 'downloading', 'paused', 'stalled', 'seeding', 'importing')
         ORDER BY d.id DESC LIMIT 1",
    )
    .bind(movie_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(r)
}

async fn find_active_download_for_episode(
    state: &AppState,
    episode_id: i64,
) -> AppResult<Option<(i64, Option<String>)>> {
    let r: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT d.id, d.torrent_hash FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         WHERE dc.episode_id = ?
           AND d.state IN ('searching', 'queued', 'grabbing', 'downloading', 'paused', 'stalled', 'seeding', 'importing')
         ORDER BY d.id DESC LIMIT 1",
    )
    .bind(episode_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(r)
}

/// Wrap a search future with our 45s ceiling. Returns an error-flavoured
/// anyhow result (the phase-2 task handles either by stamping the
/// download row as `failed` with the message).
async fn run_search_with_timeout<F, T>(fut: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    match tokio::time::timeout(SEARCH_TIMEOUT, fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow::anyhow!("Search timed out — try again in a moment.")),
    }
}

async fn kick_download(state: &AppState, download_id: i64) {
    #[derive(sqlx::FromRow)]
    struct DlRow {
        title: String,
        magnet_url: Option<String>,
    }
    let row = sqlx::query_as::<_, DlRow>(
        "SELECT title, magnet_url FROM download WHERE id = ? AND state = ?",
    )
    .bind(download_id)
    .bind(DownloadPhase::Queued)
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
        tracing::warn!(download_id, error = %e, "kick_download failed — scheduler will retry");
    }
}

async fn find_movie_title(state: &AppState, movie_id: i64) -> AppResult<String> {
    let t: String = sqlx::query_scalar("SELECT title FROM movie WHERE id = ?")
        .bind(movie_id)
        .fetch_one(&state.db)
        .await?;
    Ok(t)
}

async fn find_episode_title(state: &AppState, episode_id: i64) -> AppResult<String> {
    // Prefer the episode's own title ("Pilot"); fall back to the show's
    // title for episodes with no name yet (TMDB data drift). The
    // download row's title drives the pre-grab player identity, so
    // empty is worse than a slightly generic fallback.
    let row: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT e.title, s.title FROM episode e
         JOIN show s ON s.id = e.show_id
         WHERE e.id = ?",
    )
    .bind(episode_id)
    .fetch_one(&state.db)
    .await?;
    Ok(match row {
        (Some(ref t), _) if !t.is_empty() => t.clone(),
        (_, Some(s)) => s,
        _ => String::new(),
    })
}

/// Create a placeholder download row for an in-flight watch-now search.
/// The row has `state = 'searching'`, no `release_id` / `magnet_url`,
/// and is linked to the movie via `download_content`. Phase 2 fulfills
/// it via [`crate::acquisition::grab::fulfill_searching_with_release`].
async fn create_searching_download_for_movie(
    state: &AppState,
    movie_id: i64,
    title: &str,
) -> AppResult<i64> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let download_id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, wn_phase, added_at) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(title)
    .bind(DownloadPhase::Searching)
    .bind(WatchNowPhase::PhaseOne)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, ?)")
        .bind(download_id)
        .bind(movie_id)
        .execute(&state.db)
        .await?;
    Ok(download_id)
}

async fn create_searching_download_for_episode(
    state: &AppState,
    episode_id: i64,
    title: &str,
) -> AppResult<i64> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    let download_id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, wn_phase, added_at) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(title)
    .bind(DownloadPhase::Searching)
    .bind(WatchNowPhase::PhaseOne)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("INSERT INTO download_content (download_id, episode_id) VALUES (?, ?)")
        .bind(download_id)
        .bind(episode_id)
        .execute(&state.db)
        .await?;
    Ok(download_id)
}

/// Mark a `searching` (or `queued`) download as failed and expose the
/// error message on the row so the player's `/stream/:id/info` surfaces
/// it directly. No-op if the row has already transitioned past those
/// states (raced with a successful fulfill).
async fn mark_search_failed(state: &AppState, download_id: i64, message: &str) {
    // Keep the message short-ish — the frontend surfaces it inline.
    let msg = if message.len() > 500 {
        &message[..500]
    } else {
        message
    };
    if let Err(e) = sqlx::query(
        "UPDATE download
            SET state = ?, error_message = ?
          WHERE id = ? AND state IN (?, ?)",
    )
    .bind(DownloadPhase::Failed)
    .bind(msg)
    .bind(download_id)
    .bind(DownloadPhase::Searching)
    .bind(DownloadPhase::Queued)
    .execute(&state.db)
    .await
    {
        tracing::warn!(download_id, error = %e, "mark_search_failed: UPDATE failed");
    }
    let _ = state
        .event_tx
        .send(crate::events::AppEvent::DownloadFailed {
            download_id,
            title: String::new(),
            error: msg.to_owned(),
        });
}

/// How many alternate releases we try inside phase 2 before giving up
/// and stamping the placeholder row `failed`. Each attempt picks the
/// next-best release that isn't blocklisted, resets the row, fulfils,
/// and kicks — blocklisting the release on any failure.
const PHASE_TWO_MAX_ATTEMPTS: u32 = 3;

fn spawn_movie_phase_two(state: AppState, download_id: i64, movie_id: i64) {
    tokio::spawn(async move {
        // Claim ownership via the wn_phase column so the
        // DownloadFailed listener doesn't double-dip. Persisted
        // (vs the previous in-memory HashSet) so a restart
        // mid-loop preserves the resume + cancel decisions.
        if let Err(e) = set_wn_phase(&state, download_id, WatchNowPhase::PhaseTwo).await {
            tracing::warn!(download_id, error = %e, "couldn't set wn_phase=phase_two");
        }
        let result = Box::pin(run_movie_phase_two(&state, download_id, movie_id)).await;
        if let Err(e) = set_wn_phase(&state, download_id, WatchNowPhase::Settled).await {
            tracing::warn!(download_id, error = %e, "couldn't set wn_phase=settled");
        }
        if let Err(e) = result {
            tracing::warn!(
                download_id,
                movie_id,
                error = %e,
                "watch-now phase 2 (movie) failed"
            );
            mark_search_failed(&state, download_id, &e.to_string()).await;
        }
    });
}

async fn run_movie_phase_two(
    state: &AppState,
    download_id: i64,
    movie_id: i64,
) -> anyhow::Result<()> {
    let has_releases: bool =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM release WHERE movie_id = ?")
            .bind(movie_id)
            .fetch_one(&state.db)
            .await?
            > 0;
    if !has_releases {
        Box::pin(run_search_with_timeout(
            crate::acquisition::search::movie::search_movie_with(state, movie_id, false),
        ))
        .await?;
    }

    for attempt in 1..=PHASE_TWO_MAX_ATTEMPTS {
        // Pick the best release that isn't already blocklisted for
        // this movie. On retries, this naturally advances to the
        // next candidate because the prior attempt's release was
        // blocklisted at the end of the last iteration.
        let best: Option<i64> = sqlx::query_scalar(
            "SELECT r.id FROM release r
             WHERE r.movie_id = ?
               AND NOT EXISTS (
                 SELECT 1 FROM blocklist b
                 WHERE b.movie_id = r.movie_id
                   AND (
                     (b.torrent_info_hash IS NOT NULL AND b.torrent_info_hash = r.info_hash)
                     OR b.source_title = r.title
                   )
               )
             ORDER BY r.quality_score DESC LIMIT 1",
        )
        .bind(movie_id)
        .fetch_optional(&state.db)
        .await?;
        let Some(release_id) = best else {
            anyhow::bail!("No releases found for this movie yet — check back in a minute.");
        };

        if attempt_fulfill_and_kick(state, download_id, release_id, attempt).await? {
            return Ok(());
        }
    }
    anyhow::bail!(
        "Couldn't start a working torrent after {PHASE_TWO_MAX_ATTEMPTS} attempts — latest error is on the download row."
    );
}

fn spawn_episode_phase_two(state: AppState, download_id: i64, episode_id: i64) {
    tokio::spawn(async move {
        if let Err(e) = set_wn_phase(&state, download_id, WatchNowPhase::PhaseTwo).await {
            tracing::warn!(download_id, error = %e, "couldn't set wn_phase=phase_two");
        }
        let result = Box::pin(run_episode_phase_two(&state, download_id, episode_id)).await;
        if let Err(e) = set_wn_phase(&state, download_id, WatchNowPhase::Settled).await {
            tracing::warn!(download_id, error = %e, "couldn't set wn_phase=settled");
        }
        if let Err(e) = result {
            tracing::warn!(
                download_id,
                episode_id,
                error = %e,
                "watch-now phase 2 (episode) failed"
            );
            mark_search_failed(&state, download_id, &e.to_string()).await;
        }
    });
}

async fn set_wn_phase(
    state: &AppState,
    download_id: i64,
    phase: WatchNowPhase,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE download SET wn_phase = ? WHERE id = ?")
        .bind(phase)
        .bind(download_id)
        .execute(&state.db)
        .await
        .map(|_| ())
}

async fn run_episode_phase_two(
    state: &AppState,
    download_id: i64,
    episode_id: i64,
) -> anyhow::Result<()> {
    let has_releases: bool =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM release WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_one(&state.db)
            .await?
            > 0;
    if !has_releases {
        Box::pin(run_search_with_timeout(
            crate::acquisition::search::episode::search_episode_with(state, episode_id, false),
        ))
        .await?;
    }

    for attempt in 1..=PHASE_TWO_MAX_ATTEMPTS {
        let best: Option<i64> = sqlx::query_scalar(
            "SELECT r.id FROM release r
             WHERE r.episode_id = ?
               AND NOT EXISTS (
                 SELECT 1 FROM blocklist b
                 WHERE b.episode_id = r.episode_id
                   AND (
                     (b.torrent_info_hash IS NOT NULL AND b.torrent_info_hash = r.info_hash)
                     OR b.source_title = r.title
                   )
               )
             ORDER BY r.quality_score DESC LIMIT 1",
        )
        .bind(episode_id)
        .fetch_optional(&state.db)
        .await?;
        let Some(release_id) = best else {
            // Three distinguishable outcomes when search produced no
            // grab-able release for this episode:
            //   (a) indexers returned hits but for *other* episodes
            //       of the same show — wider coverage helps,
            //   (b) indexers returned zero hits at all — the show
            //       might just not be on these indexers,
            //   (c) all indexer requests errored (DNS / TCP /
            //       cloudflare) — the indexer is broken, adding
            //       more won't help if this one is the problem.
            //
            // We can't see (c) from this layer (the cardigann
            // adapter logs the network error but doesn't surface
            // it back here yet — tracked alongside the indexer-
            // health surface). What we *can* distinguish is (a)
            // vs (b)/(c) by checking whether any release exists
            // for the show. The frontend pattern-matches on the
            // distinct phrasing to surface an "Add Indexer" CTA,
            // so we keep "No matching release found" as the lead.
            let show_id: Option<i64> =
                sqlx::query_scalar("SELECT show_id FROM episode WHERE id = ?")
                    .bind(episode_id)
                    .fetch_optional(&state.db)
                    .await?;
            let other_releases: i64 = if let Some(sid) = show_id {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM release WHERE show_id = ? AND episode_id IS NOT ?",
                )
                .bind(sid)
                .bind(episode_id)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0)
            } else {
                0
            };
            if other_releases > 0 {
                anyhow::bail!(
                    "No matching release found for this episode. Your indexers returned results for other episodes only — add another indexer in Settings for wider coverage."
                );
            }
            anyhow::bail!(
                "No matching release found for this episode. Indexers returned no results — the show may not be on these indexers, or one of them may be unreachable. Check Settings → Indexers for failures."
            );
        };

        if attempt_fulfill_and_kick(state, download_id, release_id, attempt).await? {
            return Ok(());
        }
    }
    anyhow::bail!(
        "Couldn't start a working torrent after {PHASE_TWO_MAX_ATTEMPTS} attempts — latest error is on the download row."
    );
}

/// One attempt at fulfilling the placeholder and kicking the torrent.
/// Resets the row to `searching` first so [`fulfill_searching_with_release`]
/// can UPDATE it; on fulfill or `start_download` failure, blocklists the
/// release (so the next attempt picks a different one) and returns
/// `Ok(false)` to tell the caller to loop again. Returns `Ok(true)`
/// when the torrent is running (state transitioned past `searching`
/// without landing on `failed`).
async fn attempt_fulfill_and_kick(
    state: &AppState,
    download_id: i64,
    release_id: i64,
    attempt: u32,
) -> anyhow::Result<bool> {
    // Return the row to `searching` so fulfill can UPDATE it (its
    // WHERE clause gates on that state). Clearing release_id +
    // magnet_url + error_message avoids stale carry-over from the
    // previous attempt.
    sqlx::query(
        "UPDATE download
            SET state = ?,
                release_id = NULL,
                magnet_url = NULL,
                error_message = NULL
          WHERE id = ?",
    )
    .bind(DownloadPhase::Searching)
    .bind(download_id)
    .execute(&state.db)
    .await?;

    if let Err(e) =
        crate::acquisition::grab::fulfill_searching_with_release(state, release_id, download_id)
            .await
    {
        tracing::warn!(
            download_id,
            release_id,
            attempt,
            error = %e,
            "phase-2 fulfill failed — blocklisting release and retrying"
        );
        // The fulfill may or may not have stamped release_id onto
        // the row before failing; call the blocklist helper against
        // the release_id we know we were trying.
        blocklist_by_release_id(state, download_id, release_id, &e.to_string()).await;
        return Ok(false);
    }

    kick_download(state, download_id).await;

    // kick_download writes state='failed' internally on add_torrent
    // errors and returns Ok either way; inspect the row to decide.
    let (state_after, err_msg): (String, Option<String>) =
        sqlx::query_as("SELECT state, error_message FROM download WHERE id = ?")
            .bind(download_id)
            .fetch_one(&state.db)
            .await?;
    if state_after == "failed" {
        let reason = err_msg.unwrap_or_else(|| "start_download failed".to_owned());
        // Classify before blocklisting. System-level errors (download
        // path missing / EACCES / disk full / no torrent client) say
        // NOTHING about the release — every release would fail the
        // same way. Blocklisting them burns through every working
        // release for the season before the user has a chance to
        // fix the config. Halt phase-2 with a clear pointer instead.
        if is_config_error(&reason) {
            tracing::warn!(
                download_id,
                release_id,
                attempt,
                error = %reason,
                "phase-2: config-level failure — halting (not blocklisting any releases)"
            );
            anyhow::bail!(
                "kino can't start torrents — looks like a configuration issue, \
                 not the release. Check Settings → Library: download path must \
                 exist and be writable by the kino user. Underlying error: {reason}"
            );
        }
        tracing::warn!(
            download_id,
            release_id,
            attempt,
            error = %reason,
            "phase-2 kick failed — blocklisting release and retrying"
        );
        blocklist_by_release_id(state, download_id, release_id, &reason).await;
        return Ok(false);
    }
    Ok(true)
}

/// Heuristic: is this a SYSTEM-level error (every release would fail
/// the same way) or a RELEASE-level error (some releases would work)?
/// Used by phase-2 to avoid blocklisting otherwise-good releases when
/// the actual problem is on kino's side. Keep generous: false-positive
/// here means we halt instead of blocklist, which is recoverable;
/// false-negative blocklists releases the user has to un-blocklist
/// by hand (per the user's prior incident with this exact pattern).
fn is_config_error(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("error opening")
        || r.contains("permission denied")
        || r.contains("no such file or directory")
        || r.contains("eacces")
        || r.contains("enoent")
        || r.contains("read-only file system")
        || r.contains("erofs")
        || r.contains("no space left")
        || r.contains("enospc")
        || r.contains("disk quota exceeded")
        || r.contains("edquot")
        || r.contains("torrent client not available")
        || r.contains("download path")
}

/// Blocklist a specific release for the movie/episode linked to
/// `download_id`. Used from the retry loop which knows the release it
/// was trying even when the download row's current `release_id` is
/// null (post-reset) or stale (post-fulfill-partial-failure). Falls
/// back to the pooled `blocklist_release_for_download` via a
/// temporary UPDATE so we reuse the same blocklist-write code path.
async fn blocklist_by_release_id(
    state: &AppState,
    download_id: i64,
    release_id: i64,
    reason: &str,
) {
    // Point the row at this release_id briefly so the shared
    // blocklist helper's "SELECT release_id FROM download" read picks
    // it up. The retry loop will reset the row again on its next
    // iteration, so this is a transient write, not a state change
    // the frontend would observe.
    let _ = sqlx::query("UPDATE download SET release_id = ? WHERE id = ?")
        .bind(release_id)
        .bind(download_id)
        .execute(&state.db)
        .await;
    crate::acquisition::blocklist::blocklist_release_for_download(state, download_id, reason).await;
}
