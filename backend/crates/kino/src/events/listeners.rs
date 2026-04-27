//! Background event listeners — consume `AppEvent` and dispatch to
//! history logging, WebSocket broadcast, and webhook delivery.

use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::AppEvent;
use crate::state::AppState;

/// Spawn all event listeners. Returns when cancelled.
pub async fn run_event_listeners(state: AppState, cancel: CancellationToken) {
    let pool = state.db.clone();
    let event_tx = state.event_tx.clone();

    let history_handle = tokio::spawn(history_listener(
        pool.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    let ws_handle = tokio::spawn(websocket_listener(
        event_tx.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    let webhook_handle = tokio::spawn(webhook_listener(
        pool.clone(),
        event_tx.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    // Trakt collection sync — adds imported items to the user's
    // Trakt collection and removes them on cleanup. Cheap when
    // Trakt isn't configured/connected (the listener short-circuits
    // before any HTTP).
    let trakt_collection_handle = tokio::spawn(trakt_collection_listener(
        pool.clone(),
        event_tx.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    // Intro-skipper post-import hook (subsystem 15). On each
    // `Imported` with an `episode_id`, kick `analyse_season` for
    // that episode's season. Handles its own concurrency via the
    // shared `media_processing_sem`.
    let intro_handle = tokio::spawn(intro_skipper_listener(
        state.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    // Trickplay post-import hook. Subsystem #12 spec: "Background
    // task after import completes". Without this the user has to
    // wait up to 5 min for the next `trickplay_generation` tick,
    // which is painful after a bulk Trakt import or season grab.
    let trickplay_handle = tokio::spawn(trickplay_post_import_listener(
        state.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    // Auto-retry with alternate release on automatic download failure.
    // On `DownloadFailed`, blocklist the failed release so the next
    // search skips it, then trigger a fresh `wanted_search`. Skips
    // user-initiated cancels (error string starts with "cancelled").
    let retry_handle = tokio::spawn(retry_failed_listener(
        state.clone(),
        event_tx.subscribe(),
        cancel.child_token(),
    ));

    // Wait for cancellation, then wait for all listeners
    cancel.cancelled().await;
    let _ = tokio::join!(
        history_handle,
        ws_handle,
        webhook_handle,
        trakt_collection_handle,
        intro_handle,
        trickplay_handle,
        retry_handle,
    );
}

/// On each `AppEvent::Imported`, nudge the scheduler to run
/// `trickplay_generation` immediately. The task's own `SELECT ...
/// WHERE trickplay_generated = 0 LIMIT 1` shape already limits to
/// one file per run; a burst of imports queues one trigger per
/// import and the scheduler's coalesce-if-running logic keeps the
/// workload bounded. Best-effort — `try_send` on a full channel
/// means the next sweep tick catches up; nothing blocks the event
/// bus.
async fn trickplay_post_import_listener(
    state: AppState,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(AppEvent::Imported { .. } | AppEvent::Upgraded { .. }) => {
                        if let Err(e) = state
                            .trigger_tx
                            .try_send(crate::scheduler::TaskTrigger::fire("trickplay_generation"))
                        {
                            // Channel full or closed — log at debug
                            // because the 5-min sweep will eventually
                            // pick up the backlog regardless.
                            tracing::debug!(
                                error = %e,
                                "trickplay trigger dropped (queue full or closed)"
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "trickplay post-import listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn retry_failed_listener(
    state: AppState,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(AppEvent::DownloadFailed { download_id, error, .. }) => {
                        // User-initiated cancels don't loop back into
                        // search — the user just said "no, don't want
                        // this" and kicking off a new search would
                        // contradict that intent.
                        if error.to_lowercase().starts_with("cancelled") {
                            continue;
                        }
                        // Watch-now phase-2 tasks manage their own
                        // alternate-release loop inline so the frontend
                        // stays bound to the same download_id. If this
                        // failure belongs to one of those, skip — the
                        // task is already handling blocklist + retry.
                        // Without this the listener would kick
                        // wanted_search in parallel and spawn a fresh
                        // download row on a different id.
                        let wn_phase: Option<String> = sqlx::query_scalar(
                            "SELECT wn_phase FROM download WHERE id = ?",
                        )
                        .bind(download_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                        if wn_phase.as_deref().and_then(crate::watch_now::WatchNowPhase::parse)
                            == Some(crate::watch_now::WatchNowPhase::PhaseTwo)
                        {
                            continue;
                        }
                        crate::acquisition::blocklist::blocklist_and_retry(
                            &state, download_id, &error,
                        )
                        .await;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "retry-failed listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn intro_skipper_listener(
    state: AppState,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(AppEvent::Imported { episode_id: Some(eid), .. }) => {
                        if let Err(e) = handle_episode_imported(&state, eid).await {
                            tracing::debug!(error = %e, episode_id = eid, "intro analysis trigger failed");
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "intro-skipper listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

/// Per-session set of `(show_id, season_number)` keys that either
/// have an analysis in flight or one queued inside the debounce
/// window. Importing a 10-episode season pack previously spawned
/// 10 analyses; the set coalesces those to exactly one.
static INTRO_ANALYSES_IN_FLIGHT: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashSet<(i64, i64)>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

/// Debounce window — a season-pack import loop inserts 10+
/// `Imported` events in quick succession. Waiting a beat lets
/// them all coalesce before the first task starts, so the fingerprint
/// pass sees the full set of episodes on its first attempt.
const INTRO_ANALYSIS_DEBOUNCE_MS: u64 = 3_000;

async fn handle_episode_imported(state: &AppState, episode_id: i64) -> anyhow::Result<()> {
    // Fetch (show_id, season_number) for the just-imported episode.
    let row: Option<(i64, i64)> =
        sqlx::query_as("SELECT show_id, season_number FROM episode WHERE id = ?")
            .bind(episode_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((show_id, season_number)) = row else {
        return Ok(());
    };

    // Coalesce: if a task for this (show, season) already exists,
    // skip. Otherwise claim the slot, then spawn the debounced task.
    {
        let mut guard = INTRO_ANALYSES_IN_FLIGHT.lock().await;
        if !guard.insert((show_id, season_number)) {
            tracing::debug!(
                show_id,
                season_number,
                episode_id,
                "intro analysis already queued for season — coalescing"
            );
            return Ok(());
        }
    }

    let state = state.clone();
    tokio::spawn(async move {
        // Debounce window — catches the rest of the import burst.
        tokio::time::sleep(std::time::Duration::from_millis(INTRO_ANALYSIS_DEBOUNCE_MS)).await;
        let run_result =
            crate::playback::intro_skipper::analyse_season(&state, show_id, season_number).await;
        // Release the slot *after* the task completes so overlapping
        // imports that arrive during analysis don't spawn a duplicate;
        // the daily catch-up picks up anything that landed late.
        {
            let mut guard = INTRO_ANALYSES_IN_FLIGHT.lock().await;
            guard.remove(&(show_id, season_number));
        }
        if let Err(e) = run_result {
            tracing::warn!(
                error = %e,
                show_id,
                season_number,
                "intro analysis failed"
            );
        }
    });
    Ok(())
}

/// Log every event to the History table.
async fn history_listener(
    pool: SqlitePool,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(e) => {
                        // Skip high-frequency events
                        if matches!(e, AppEvent::DownloadProgress { .. }) {
                            continue;
                        }
                        if let Err(err) = log_history(&pool, &e).await {
                            tracing::error!(error = %err, "history logging failed");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "history listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn log_history(pool: &SqlitePool, event: &AppEvent) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let event_type = event.event_type();
    let title = event.title();
    let data = serde_json::to_string(event).unwrap_or_default();

    // Extract IDs from event. Every variant that carries a
    // resolvable content id populates the top-level
    // `movie_id` / `episode_id` columns so the per-item history
    // query (`WHERE movie_id = ?` / `WHERE episode_id = ?`) sees
    // every event the user cares about. Previously only three
    // variants were extracted, leaving 10+ events stored but
    // unqueryable without JSON unpacking.
    let (movie_id, episode_id) = resolve_history_ids(pool, event).await;

    sqlx::query(
        "INSERT INTO history (movie_id, episode_id, event_type, date, source_title, quality, data) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(movie_id)
    .bind(episode_id)
    .bind(event_type)
    .bind(&now)
    .bind(title)
    .bind(event.quality())
    .bind(&data)
    .execute(pool)
    .await?;

    Ok(())
}

/// Resolve `(movie_id, episode_id)` for a history row. Variants
/// that carry the IDs directly are extracted inline; download-
/// lifecycle variants only carry `download_id` on the wire, so we
/// join `download_content` to find the linked content. One extra
/// query per download-lifecycle event — acceptable for history-
/// write cadence (not hot-path).
async fn resolve_history_ids(pool: &SqlitePool, event: &AppEvent) -> (Option<i64>, Option<i64>) {
    match event {
        AppEvent::MovieAdded { movie_id, .. } => (Some(*movie_id), None),
        AppEvent::Imported {
            movie_id,
            episode_id,
            ..
        }
        | AppEvent::Watched {
            movie_id,
            episode_id,
            ..
        }
        | AppEvent::Unwatched {
            movie_id,
            episode_id,
            ..
        }
        | AppEvent::SearchStarted {
            movie_id,
            episode_id,
            ..
        } => (*movie_id, *episode_id),
        AppEvent::Upgraded { movie_id, .. } | AppEvent::ContentRemoved { movie_id, .. } => {
            (*movie_id, None)
        }
        AppEvent::NewEpisode { episode_id, .. } => (None, Some(*episode_id)),
        AppEvent::Rated { kind, id, .. } => match kind.as_str() {
            "movie" => (Some(*id), None),
            "episode" => (None, Some(*id)),
            _ => (None, None),
        },
        // Download lifecycle only carries download_id on the wire.
        // Resolve to the linked content so per-item history picks
        // these up. Season-pack downloads link to N episodes; we
        // take the first — the history row's `data` blob still
        // carries the whole AppEvent for UIs that want the full
        // picture. None on DB error so a transient blip doesn't
        // lose the history row entirely.
        AppEvent::ReleaseGrabbed { download_id, .. }
        | AppEvent::DownloadStarted { download_id, .. }
        | AppEvent::DownloadComplete { download_id, .. }
        | AppEvent::DownloadFailed { download_id, .. }
        | AppEvent::DownloadCancelled { download_id, .. }
        | AppEvent::DownloadPaused { download_id, .. }
        | AppEvent::DownloadResumed { download_id, .. }
        | AppEvent::DownloadMetadataReady { download_id, .. } => {
            let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
                "SELECT movie_id, episode_id FROM download_content
                 WHERE download_id = ? LIMIT 1",
            )
            .bind(download_id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
            row.unwrap_or((None, None))
        }
        _ => (None, None),
    }
}

/// Forward events to WebSocket clients via the broadcast channel.
/// Events are serialized to JSON and sent as the existing String broadcast.
async fn websocket_listener(
    _event_tx: broadcast::Sender<AppEvent>,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    // The WebSocket handler in api/ws.rs subscribes to AppEvent directly.
    // This listener exists to log/monitor WebSocket throughput.
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(_) => {} // WS clients get events directly from their own subscription
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "ws listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

/// Mirror local imports + removals into the user's Trakt collection.
/// Listens for `Imported` (new file landed) and `ContentRemoved`
/// (movie/show deleted) and POSTs to /sync/collection or
/// /sync/collection/remove. Each event is a fire-and-forget — failures
/// log and continue rather than retrying, since the next full sync
/// reconciles. Cheap when Trakt is disconnected: the helper short-
/// circuits inside without making an HTTP call.
async fn trakt_collection_listener(
    pool: SqlitePool,
    event_tx: broadcast::Sender<AppEvent>,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(e) => {
                        if let Err(err) = handle_collection_event(&pool, &event_tx, &e).await {
                            tracing::debug!(error = %err, "trakt collection sync skipped");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "trakt collection listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn handle_collection_event(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    event: &AppEvent,
) -> anyhow::Result<()> {
    if !crate::integrations::trakt::is_connected(pool).await {
        return Ok(());
    }
    match event {
        AppEvent::Imported {
            movie_id,
            episode_id,
            ..
        } => {
            let client = crate::integrations::trakt::TraktClient::from_db(pool.clone())
                .await?
                .with_event_tx(event_tx.clone());
            crate::integrations::trakt::sync::push_collection_imported(
                &client,
                *movie_id,
                *episode_id,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        AppEvent::ContentRemoved {
            movie_id, show_id, ..
        } => {
            let client = crate::integrations::trakt::TraktClient::from_db(pool.clone())
                .await?
                .with_event_tx(event_tx.clone());
            crate::integrations::trakt::sync::push_collection_removed(&client, *movie_id, *show_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        // On add: (a) pull existing rating/watched/watchlist state for
        // that one item from Trakt — closes the gap where the periodic
        // incremental sweep ignores the bucket because the remote
        // watermark hasn't moved since we last synced — and (b) push
        // it onto the user's Trakt watchlist so the two surfaces stay
        // aligned ("library on kino" == "watchlist on Trakt").
        AppEvent::MovieAdded { movie_id, .. } => {
            let client = crate::integrations::trakt::TraktClient::from_db(pool.clone())
                .await?
                .with_event_tx(event_tx.clone());
            crate::integrations::trakt::sync::pull_one_movie_state(&client, *movie_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let _ = crate::integrations::trakt::sync::push_watchlist_add(
                &client,
                crate::integrations::trakt::sync::WatchlistKind::Movie,
                *movie_id,
            )
            .await;
        }
        AppEvent::ShowAdded { show_id, .. } => {
            let client = crate::integrations::trakt::TraktClient::from_db(pool.clone())
                .await?
                .with_event_tx(event_tx.clone());
            crate::integrations::trakt::sync::pull_one_show_state(&client, *show_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let _ = crate::integrations::trakt::sync::push_watchlist_add(
                &client,
                crate::integrations::trakt::sync::WatchlistKind::Show,
                *show_id,
            )
            .await;
        }
        _ => {}
    }
    Ok(())
}

/// Dispatch events to webhook targets.
async fn webhook_listener(
    pool: SqlitePool,
    event_tx: broadcast::Sender<AppEvent>,
    mut rx: broadcast::Receiver<AppEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = rx.recv() => {
                match event {
                    Ok(e) => {
                        // Skip progress events (too frequent for webhooks)
                        if matches!(e, AppEvent::DownloadProgress { .. }) {
                            continue;
                        }
                        let notif = build_notification_event(&pool, &e).await;
                        crate::notification::webhook::deliver(&pool, &notif, Some(&event_tx)).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "webhook listener lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

/// Turn an `AppEvent` into the `notification::Event` struct the
/// webhook templates read. Every placeholder documented in
/// `08-notification.md` (`{{show}}`, `{{season}}`, `{{episode}}`,
/// `{{quality}}`, `{{year}}`, `{{size}}`, `{{indexer}}`,
/// `{{movie_id}}`, `{{episode_id}}`, `{{message}}`) must actually
/// be populated here. Previously the listener copied only
/// `event_type` + `title`, so webhooks rendered mostly-empty in
/// production even though `test_webhook` populated everything.
#[allow(clippy::too_many_lines)]
async fn build_notification_event(
    pool: &SqlitePool,
    event: &AppEvent,
) -> crate::notification::Event {
    use crate::notification::Event as N;

    // Start with the universal fields, then fill per-variant.
    let mut n = N {
        event_type: event.event_type().to_owned(),
        movie_id: None,
        episode_id: None,
        title: Some(event.title().to_owned()),
        show: None,
        season: None,
        episode: None,
        quality: None,
        year: None,
        size: None,
        indexer: None,
        message: None,
    };

    match event {
        AppEvent::MovieAdded { movie_id, .. } => {
            n.movie_id = Some(*movie_id);
            n.year = movie_year(pool, *movie_id).await;
        }
        AppEvent::ShowAdded { show_id, .. } => {
            n.show = Some(event.title().to_owned());
            n.year = show_year(pool, *show_id).await;
        }
        AppEvent::SearchStarted {
            movie_id,
            episode_id,
            ..
        } => {
            n.movie_id = *movie_id;
            n.episode_id = *episode_id;
            if let Some(ep_id) = episode_id {
                populate_episode_context(pool, *ep_id, &mut n).await;
            }
        }
        AppEvent::ReleaseGrabbed {
            download_id,
            quality,
            indexer,
            size,
            ..
        } => {
            n.quality.clone_from(quality);
            n.indexer.clone_from(indexer);
            n.size = size.map(format_bytes);
            populate_download_context(pool, *download_id, &mut n).await;
        }
        AppEvent::DownloadStarted { download_id, .. }
        | AppEvent::DownloadPaused { download_id, .. }
        | AppEvent::DownloadResumed { download_id, .. }
        | AppEvent::DownloadCancelled { download_id, .. } => {
            populate_download_context(pool, *download_id, &mut n).await;
        }
        AppEvent::DownloadComplete {
            download_id, size, ..
        } => {
            n.size = size.map(format_bytes);
            populate_download_context(pool, *download_id, &mut n).await;
        }
        AppEvent::DownloadFailed {
            download_id, error, ..
        } => {
            n.message = Some(error.clone());
            populate_download_context(pool, *download_id, &mut n).await;
        }
        AppEvent::Imported {
            movie_id,
            episode_id,
            quality,
            ..
        } => {
            n.movie_id = *movie_id;
            n.episode_id = *episode_id;
            n.quality.clone_from(quality);
            if let Some(ep_id) = episode_id {
                populate_episode_context(pool, *ep_id, &mut n).await;
            } else if let Some(mid) = movie_id {
                n.year = movie_year(pool, *mid).await;
            }
        }
        AppEvent::Upgraded {
            movie_id,
            new_quality,
            old_quality,
            ..
        } => {
            n.movie_id = *movie_id;
            n.quality.clone_from(new_quality);
            // Use the message slot to surface "old → new" so templates
            // can render the transition without needing a second field.
            if let (Some(o), Some(q)) = (old_quality, new_quality) {
                n.message = Some(format!("{o} → {q}"));
            }
            if let Some(mid) = movie_id {
                n.year = movie_year(pool, *mid).await;
            }
        }
        AppEvent::Watched {
            movie_id,
            episode_id,
            ..
        }
        | AppEvent::Unwatched {
            movie_id,
            episode_id,
            ..
        } => {
            n.movie_id = *movie_id;
            n.episode_id = *episode_id;
            if let Some(ep_id) = episode_id {
                populate_episode_context(pool, *ep_id, &mut n).await;
            } else if let Some(mid) = movie_id {
                n.year = movie_year(pool, *mid).await;
            }
        }
        AppEvent::NewEpisode {
            show_id,
            episode_id,
            show_title,
            season,
            episode,
            episode_title,
        } => {
            n.episode_id = Some(*episode_id);
            n.show = Some(show_title.clone());
            n.season = Some(*season);
            n.episode = Some(*episode);
            n.year = show_year(pool, *show_id).await;
            if let Some(t) = episode_title {
                n.title = Some(format!("{show_title} · S{season:02}E{episode:02} · {t}"));
            }
        }
        AppEvent::ContentRemoved {
            movie_id, show_id, ..
        } => {
            n.movie_id = *movie_id;
            if let Some(mid) = movie_id {
                n.year = movie_year(pool, *mid).await;
            } else if let Some(sid) = show_id {
                n.year = show_year(pool, *sid).await;
            }
        }
        AppEvent::HealthWarning { message } | AppEvent::HealthRecovered { message } => {
            n.message = Some(message.clone());
            // Health events don't have a content title — clear the
            // default we seeded from event.title().
            n.title = None;
        }
        _ => {
            // Fallthrough for events with no extra context to hydrate
            // (MetadataRefreshed, TrickplayStreamUpdated, etc.) — the
            // event_type + title already carried by `n` is enough.
        }
    }
    n
}

/// Populate `show` / `season` / `episode` / `year` for an event
/// that references a single episode. One joined query, None-
/// tolerant: if the episode was deleted between emit and consume,
/// leave the fields blank rather than returning an error.
async fn populate_episode_context(
    pool: &SqlitePool,
    episode_id: i64,
    n: &mut crate::notification::Event,
) {
    let row: Option<(String, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT s.title, s.year, e.season_number, e.episode_number
         FROM episode e JOIN show s ON s.id = e.show_id
         WHERE e.id = ?",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    if let Some((show, year, season, episode)) = row {
        n.show = Some(show);
        n.year = year;
        n.season = Some(season);
        n.episode = Some(episode);
    }
}

#[derive(sqlx::FromRow)]
struct DownloadLink {
    movie_id: Option<i64>,
    episode_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct DownloadReleaseRow {
    size: Option<i64>,
    quality: Option<String>,
    indexer: Option<String>,
}

/// Populate movie/episode + indexer + quality from a download row
/// when the event only carried the `download_id`. Most download-
/// lifecycle events skip quality/indexer on the wire; this fills
/// them in from `release` + `download_content` + parent content.
async fn populate_download_context(
    pool: &SqlitePool,
    download_id: i64,
    n: &mut crate::notification::Event,
) {
    // First: which content does this download serve?
    let link: Option<DownloadLink> = sqlx::query_as(
        "SELECT movie_id, episode_id FROM download_content
         WHERE download_id = ? LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    if let Some(l) = link {
        n.movie_id = l.movie_id;
        n.episode_id = l.episode_id;
        if let Some(ep_id) = l.episode_id {
            populate_episode_context(pool, ep_id, n).await;
        } else if let Some(mid) = l.movie_id {
            n.year = movie_year(pool, mid).await;
        }
    }

    // Second: size / indexer / quality from the release row the
    // download was grabbed for.
    let rel: Option<DownloadReleaseRow> = sqlx::query_as(
        "SELECT r.size,
                CASE
                  WHEN r.source IS NOT NULL AND r.resolution IS NOT NULL
                    THEN r.source || '-' || r.resolution || 'p'
                  WHEN r.resolution IS NOT NULL
                    THEN r.resolution || 'p'
                  ELSE r.source
                END AS quality,
                i.name AS indexer
         FROM download d
         LEFT JOIN release r ON r.id = d.release_id
         LEFT JOIN indexer i ON i.id = r.indexer_id
         WHERE d.id = ?",
    )
    .bind(download_id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    if let Some(r) = rel {
        if n.size.is_none() {
            n.size = r.size.map(format_bytes);
        }
        if n.quality.is_none() {
            n.quality = r.quality;
        }
        if n.indexer.is_none() {
            n.indexer = r.indexer;
        }
    }
}

async fn movie_year(pool: &SqlitePool, movie_id: i64) -> Option<i64> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT year FROM movie WHERE id = ?")
        .bind(movie_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
}

async fn show_year(pool: &SqlitePool, show_id: i64) -> Option<i64> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT year FROM show WHERE id = ?")
        .bind(show_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
}

/// Human-readable byte size for webhook payloads. Keeps the
/// granularity low (one decimal for GB/MB, integer for KB/B) —
/// good enough for "downloaded 8.4 GB" without overcooking it.
fn format_bytes(bytes: i64) -> String {
    const GB: f64 = 1_073_741_824.0;
    const MB: f64 = 1_048_576.0;
    const KB: f64 = 1024.0;
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    let b = bytes.max(0) as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}
