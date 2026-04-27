//! Grab — the side-effect after `search_*` picks a release. Insert
//! the `download` row, link to its target (movie or episode),
//! resolve the release URL into something librqbit can consume,
//! and emit `ReleaseGrabbed`. The two grab paths (movie vs episode)
//! are sibling functions because the `download_content` link target
//! is the only real difference.
//!
//! Free-space + URL-resolve helpers live here too — they're only
//! used at grab time, so colocating them keeps the pre-grab checks
//! next to the grab.

use crate::download::DownloadPhase;
use crate::events::AppEvent;
use crate::state::AppState;

/// Grab a specific release — create download, update status.
///
/// Resolves the stored URL into something librqbit can actually consume.
/// Many cardigann definitions (`LimeTorrents`, 1337x, etc.) store a
/// details-page URL on the release; we fetch it here and run the
/// definition's `download:` selectors to extract the real magnet.
/// Grab a release: create the download row, link it to the movie,
/// and emit the `ReleaseGrabbed` event. Returns the created
/// `download_id` so callers (like the Watch-now endpoint) can
/// immediately trigger a start + open a stream against it without a
/// second lookup.
#[tracing::instrument(skip(state), fields(release_id, movie_id))]
#[tracing::instrument(skip(state), fields(release_id, movie_id))]
#[allow(clippy::too_many_lines)]
pub async fn grab_release(state: &AppState, release_id: i64, movie_id: i64) -> anyhow::Result<i64> {
    let pool = &state.db;
    let event_tx = &state.event_tx;
    let now = crate::time::Timestamp::now().to_rfc3339();

    // Re-read at decision time: the user may have watched the movie
    // (manually, via Trakt sync, or via the player's watched
    // threshold) between the wanted-search picking this id and the
    // grab landing. The eligibility query at sweep time is a
    // snapshot; this re-read makes the decision authoritative.
    let watched_at: Option<String> =
        sqlx::query_scalar("SELECT watched_at FROM movie WHERE id = ?")
            .bind(movie_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    if watched_at.is_some_and(|s| !s.is_empty()) {
        tracing::debug!(
            movie_id,
            release_id,
            "grab_release: skipping — movie was watched since search picked this release"
        );
        anyhow::bail!("movie {movie_id} watched since search began");
    }

    // Dedup: if a non-terminal download already exists for this
    // release, return its id instead of creating a second row. Covers
    // the double-click race and the case where two scheduler ticks
    // both pick the same best release before the first commits.
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM download
         WHERE release_id = ?
           AND state NOT IN ('failed', 'imported', 'completed', 'cleaned_up')
         ORDER BY id DESC
         LIMIT 1",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        tracing::debug!(
            release_id,
            download_id = id,
            "grab dedup: reusing existing download"
        );
        return Ok(id);
    }

    // Load release + the indexer that produced it. We also pull
    // resolution/source for a human-readable quality string and the
    // indexer's display name so the History UI can render
    // "Grabbed 8.4 GB via NZBGeek · Bluray-1080p" without a second
    // round-trip.
    #[derive(sqlx::FromRow)]
    struct ReleaseRow {
        title: String,
        magnet_url: Option<String>,
        download_url: Option<String>,
        size: Option<i64>,
        indexer_id: Option<i64>,
        resolution: Option<i64>,
        source: Option<String>,
        indexer_name: Option<String>,
    }
    let row = sqlx::query_as::<_, ReleaseRow>(
        "SELECT r.title, r.magnet_url, r.download_url, r.size, r.indexer_id,
                r.resolution, r.source, i.name as indexer_name
         FROM release r
         LEFT JOIN indexer i ON i.id = r.indexer_id
         WHERE r.id = ?",
    )
    .bind(release_id)
    .fetch_one(pool)
    .await?;
    let ReleaseRow {
        title,
        magnet_url: release_magnet,
        download_url: release_download,
        size,
        indexer_id,
        resolution,
        source,
        indexer_name,
    } = row;

    ensure_free_space_for_grab(state, size).await?;

    // Prefer stored magnet; fall back to download_url (typically a details page).
    let raw_url = release_magnet
        .clone()
        .or(release_download.clone())
        .ok_or_else(|| anyhow::anyhow!("release {release_id} has no download URL"))?;

    // Resolve to a librqbit-consumable URL (magnet or direct .torrent).
    let magnet = resolve_release_url(state, indexer_id, &raw_url)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                release_id,
                url = %raw_url,
                error = %e,
                "download URL resolution failed; passing raw URL to torrent client"
            );
            raw_url.clone()
        });

    // Mark release as grabbed
    sqlx::query("UPDATE release SET status = 'grabbed', grabbed_at = ? WHERE id = ?")
        .bind(&now)
        .bind(release_id)
        .execute(pool)
        .await?;

    // Create download
    let download_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO download (release_id, title, state, size, added_at, magnet_url) VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(release_id)
    .bind(&title)
    .bind(DownloadPhase::Queued)
    .bind(size)
    .bind(&now)
    .bind(&magnet)
    .fetch_one(pool)
    .await?;

    // Link download to movie
    sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, ?)")
        .bind(download_id)
        .bind(movie_id)
        .execute(pool)
        .await?;

    // Status derives from the download row we just created — no
    // explicit UPDATE needed.

    // Assemble a display-friendly quality string from the release
    // columns — matches the format Imported already uses
    // ("source-resolution-p" or just "resolutionp"), so History rows
    // look consistent regardless of which stage emitted them.
    let quality = match (resolution, source.as_deref()) {
        (Some(r), Some(s)) => Some(format!("{s}-{r}p")),
        (Some(r), None) => Some(format!("{r}p")),
        _ => None,
    };

    let _ = event_tx.send(AppEvent::ReleaseGrabbed {
        download_id,
        title,
        quality,
        indexer: indexer_name,
        size,
    });

    tracing::info!(download_id, movie_id, release_id, "release grabbed");

    Ok(download_id)
}

/// Resolve a raw release URL into something librqbit can consume.
///
/// Looks up the indexer, finds its cardigann definition, and delegates
/// to the downloader's `resolve_download_url` to follow any details-page
/// redirect and extract a magnet via CSS selectors.
async fn resolve_release_url(
    state: &AppState,
    indexer_id: Option<i64>,
    url: &str,
) -> anyhow::Result<String> {
    if url.starts_with("magnet:") {
        return Ok(url.to_owned());
    }

    let Some(indexer_id) = indexer_id else {
        return Ok(url.to_owned());
    };
    let Some(ref definitions) = state.definitions else {
        return Ok(url.to_owned());
    };

    // Load indexer + its definition_id + settings.
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT definition_id, settings_json FROM indexer WHERE id = ?")
            .bind(indexer_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((Some(definition_id), settings_json)) = row else {
        return Ok(url.to_owned());
    };
    let Some(definition) = definitions.get(&definition_id) else {
        return Ok(url.to_owned());
    };

    let settings: std::collections::HashMap<String, String> = settings_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Reuse the session-scoped indexer client so cookies / login
    // state carry across the download-url resolve; `downloader`
    // sometimes needs an authenticated page fetch to scrape the
    // magnet out.
    let client = state.indexer_client(indexer_id).await;
    match crate::indexers::downloader::resolve_download_url(&definition, &client, &settings, url)
        .await
    {
        Ok(u) => Ok(u),
        Err(e) => {
            // Dropping the cached client on any error is the cheapest
            // way to handle auth expiry — the next search or resolve
            // rebuilds it and re-logs in.
            state.invalidate_indexer_client(indexer_id).await;
            Err(e)
        }
    }
}

async fn link_episode_to_download(
    pool: &sqlx::SqlitePool,
    download_id: i64,
    episode_id: i64,
) -> anyhow::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO download_content (download_id, episode_id) VALUES (?, ?)")
        .bind(download_id)
        .bind(episode_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Safety margin over the release size required before we'll start a
/// new download — `SQLite` WAL, tracker announces, partial resume files,
/// plus typical hardlink copies at import-time all want headroom.
/// Only consumed by the production branch of `ensure_free_space_for_grab`;
/// tests bypass the check entirely.
#[cfg(not(test))]
const GRAB_FREE_SPACE_BUFFER_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// Reject a grab when the download volume can't fit `size + buffer`.
/// Unknown size (indexer didn't return one) falls through — we'd
/// rather risk a disk-full than refuse every grab from a terse indexer.
///
/// Skipped entirely under `cfg(test)`: CI runners' `/tmp` (where flow
/// tests put their tempdirs) can dip below the 2 GB buffer during
/// heavy parallel test load. When that happens `grab_release` silently
/// fails and downstream "no download row" assertions look like logic
/// bugs. The check is a production guardrail; flow tests have no
/// real disk to protect.
#[cfg(test)]
#[allow(clippy::unused_async)]
pub async fn ensure_free_space_for_grab(
    _state: &AppState,
    _release_size: Option<i64>,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(test))]
pub async fn ensure_free_space_for_grab(
    state: &AppState,
    release_size: Option<i64>,
) -> anyhow::Result<()> {
    let Some(size) = release_size else {
        return Ok(());
    };
    if size <= 0 {
        return Ok(());
    }
    let Ok(Some(path)) =
        sqlx::query_scalar::<_, Option<String>>("SELECT download_path FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await
    else {
        return Ok(());
    };
    let path = path.unwrap_or_default();
    if path.is_empty() {
        return Ok(());
    }
    let Ok(free) = fs4::available_space(std::path::Path::new(&path)) else {
        // Free-space lookup failed (FS doesn't report, path issue we
        // already flag elsewhere). Don't block the grab on this.
        return Ok(());
    };
    let required = size.saturating_add(GRAB_FREE_SPACE_BUFFER_BYTES);
    let required_u64 = u64::try_from(required).unwrap_or(u64::MAX);
    if free < required_u64 {
        anyhow::bail!(
            "not enough free space at download_path: need {} (release + 2 GB buffer), have {}",
            human_bytes(required_u64),
            human_bytes(free),
        );
    }
    Ok(())
}

// Only consumed by the production branch of `ensure_free_space_for_grab`;
// tests bypass the disk-space check entirely so the helper is dead in
// `cfg(test)` builds.
#[cfg(not(test))]
#[allow(clippy::cast_precision_loss)]
fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.0} MB", n / MB)
    } else {
        format!("{n:.0} B")
    }
}

#[allow(clippy::too_many_lines, clippy::items_after_statements)]
pub async fn grab_episode_release(
    state: &AppState,
    release_id: i64,
    episode_id: i64,
) -> anyhow::Result<i64> {
    let pool = &state.db;
    let event_tx = &state.event_tx;
    let now = crate::time::Timestamp::now().to_rfc3339();

    // Re-read at decision time: if the user marked the episode
    // watched (manually or via Trakt sync) between the wanted-search
    // pick and the grab landing, bail. The eligibility query at
    // sweep time was a snapshot.
    let watched_at: Option<String> =
        sqlx::query_scalar("SELECT watched_at FROM episode WHERE id = ?")
            .bind(episode_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    if watched_at.is_some_and(|s| !s.is_empty()) {
        tracing::debug!(
            episode_id,
            release_id,
            "grab_episode_release: skipping — episode was watched since search picked this release"
        );
        anyhow::bail!("episode {episode_id} watched since search began");
    }

    // Dedup: reuse an existing non-terminal download for this release
    // instead of creating a duplicate row (double-click / racing
    // ticks).
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM download
         WHERE release_id = ?
           AND state NOT IN ('failed', 'imported', 'completed', 'cleaned_up')
         ORDER BY id DESC
         LIMIT 1",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        tracing::debug!(
            release_id,
            download_id = id,
            "grab_episode dedup: reusing existing download"
        );
        link_episode_to_download(pool, id, episode_id).await?;
        return Ok(id);
    }

    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        magnet_url: Option<String>,
        download_url: Option<String>,
        size: Option<i64>,
        indexer_id: Option<i64>,
        resolution: Option<i64>,
        source: Option<String>,
        indexer_name: Option<String>,
        info_hash: Option<String>,
        season_number: Option<i64>,
        show_id: Option<i64>,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT r.title, r.magnet_url, r.download_url, r.size, r.indexer_id,
                r.resolution, r.source, i.name as indexer_name,
                r.info_hash, r.season_number, r.show_id
         FROM release r
         LEFT JOIN indexer i ON i.id = r.indexer_id
         WHERE r.id = ?",
    )
    .bind(release_id)
    .fetch_one(pool)
    .await?;

    ensure_free_space_for_grab(state, row.size).await?;

    // Info-hash dedup: a season pack shows up as a separate release
    // row per episode search (unique index is on (episode_id,
    // indexer_id, guid)), but corresponds to one torrent. Reuse the
    // existing download instead of kicking off a second copy; just
    // add a download_content link for this episode.
    if let Some(ref hash) = row.info_hash {
        let existing_by_hash: Option<i64> = sqlx::query_scalar(
            "SELECT d.id FROM download d
             JOIN release r ON r.id = d.release_id
             WHERE r.info_hash = ?
               AND d.state NOT IN ('failed', 'imported', 'completed', 'cleaned_up')
             ORDER BY d.id DESC
             LIMIT 1",
        )
        .bind(hash)
        .fetch_optional(pool)
        .await?;
        if let Some(id) = existing_by_hash {
            tracing::debug!(
                release_id,
                download_id = id,
                info_hash = %hash,
                "grab_episode dedup: reusing existing download via info_hash"
            );
            link_episode_to_download(pool, id, episode_id).await?;
            return Ok(id);
        }
    }

    let raw_url = row
        .magnet_url
        .clone()
        .or(row.download_url.clone())
        .ok_or_else(|| anyhow::anyhow!("release {release_id} has no download URL"))?;

    let magnet = resolve_release_url(state, row.indexer_id, &raw_url)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                release_id,
                url = %raw_url,
                error = %e,
                "download URL resolution failed; passing raw URL to torrent client"
            );
            raw_url.clone()
        });

    sqlx::query("UPDATE release SET status = 'grabbed', grabbed_at = ? WHERE id = ?")
        .bind(&now)
        .bind(release_id)
        .execute(pool)
        .await?;

    let download_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO download (release_id, title, state, size, added_at, magnet_url)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(release_id)
    .bind(&row.title)
    .bind(DownloadPhase::Queued)
    .bind(row.size)
    .bind(&now)
    .bind(&magnet)
    .fetch_one(pool)
    .await?;

    link_episode_to_download(pool, download_id, episode_id).await?;

    // Season-pack multi-link: if this release is a pack covering an
    // entire season, proactively link every *other* wanted episode in
    // that season to this same download. Two wins: (a) the wanted-
    // sweep's in-flight check sees those episodes covered, so it won't
    // kick off duplicate pack grabs from each episode's own search;
    // (b) import can see the full set of target episodes and match
    // each torrent file to its episode (see `do_import`).
    let parsed = crate::parser::parse(&row.title);
    if parsed.is_season_pack
        && let (Some(show_id), Some(season_number)) = (row.show_id, row.season_number)
    {
        let linked = sqlx::query(
            "INSERT OR IGNORE INTO download_content (download_id, episode_id)
             SELECT ?, e.id
             FROM episode e
             WHERE e.show_id = ?
               AND e.season_number = ?
               AND e.acquire = 1
               AND e.watched_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
               AND e.id != ?",
        )
        .bind(download_id)
        .bind(show_id)
        .bind(season_number)
        .bind(episode_id)
        .execute(pool)
        .await?;
        tracing::info!(
            download_id,
            season = season_number,
            extra_linked = linked.rows_affected(),
            "season pack grab — linked remaining wanted episodes to this download"
        );
    }

    // Status ('downloading') derives from the just-created download row.
    let quality = match (row.resolution, row.source.as_deref()) {
        (Some(r), Some(s)) => Some(format!("{s}-{r}p")),
        (Some(r), None) => Some(format!("{r}p")),
        _ => None,
    };
    // Event title uses the composed episode form so toasts / history
    // don't show the raw release filename. `row.title` (the release
    // name) is still stored on the download row for its own detail
    // surfaces.
    let _ = event_tx.send(AppEvent::ReleaseGrabbed {
        download_id,
        title: crate::events::display::episode_display_title(pool, episode_id).await,
        quality,
        indexer: row.indexer_name,
        size: row.size,
    });
    tracing::info!(
        download_id,
        episode_id,
        release_id,
        "episode release grabbed"
    );

    Ok(download_id)
}

/// Fill in a pre-created `searching` download row with the details of
/// a picked release. The two-phase watch-now flow calls this from its
/// background task once the search has completed: the `download_id`
/// the frontend is already bound to gets its `release_id`, `magnet_url`,
/// `title`, and `size` populated, and the state transitions to `queued`
/// so the download monitor picks it up on the next tick.
///
/// Emits `ReleaseGrabbed` just like [`grab_release`] / [`grab_episode_release`].
/// For season-pack episode releases, also fans the download link out to
/// every other wanted episode in that season — same as the episode-grab
/// path.
#[tracing::instrument(skip(state), fields(release_id, download_id))]
#[allow(clippy::too_many_lines)]
pub async fn fulfill_searching_with_release(
    state: &AppState,
    release_id: i64,
    download_id: i64,
) -> anyhow::Result<()> {
    let pool = &state.db;
    let event_tx = &state.event_tx;
    let now = crate::time::Timestamp::now().to_rfc3339();

    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        magnet_url: Option<String>,
        download_url: Option<String>,
        size: Option<i64>,
        indexer_id: Option<i64>,
        resolution: Option<i64>,
        source: Option<String>,
        indexer_name: Option<String>,
        season_number: Option<i64>,
        show_id: Option<i64>,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT r.title, r.magnet_url, r.download_url, r.size, r.indexer_id,
                r.resolution, r.source, i.name as indexer_name,
                r.season_number, r.show_id
         FROM release r
         LEFT JOIN indexer i ON i.id = r.indexer_id
         WHERE r.id = ?",
    )
    .bind(release_id)
    .fetch_one(pool)
    .await?;

    ensure_free_space_for_grab(state, row.size).await?;

    let raw_url = row
        .magnet_url
        .clone()
        .or(row.download_url.clone())
        .ok_or_else(|| anyhow::anyhow!("release {release_id} has no download URL"))?;

    let magnet = resolve_release_url(state, row.indexer_id, &raw_url)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                release_id,
                url = %raw_url,
                error = %e,
                "download URL resolution failed; passing raw URL to torrent client"
            );
            raw_url.clone()
        });

    sqlx::query("UPDATE release SET status = 'grabbed', grabbed_at = ? WHERE id = ?")
        .bind(&now)
        .bind(release_id)
        .execute(pool)
        .await?;

    // Flip the searching row into a queued download. The monitor's
    // poll query picks it up on the next tick.
    let updated = sqlx::query(
        "UPDATE download
            SET release_id = ?, title = ?, size = ?, magnet_url = ?,
                state = ?, error_message = NULL
          WHERE id = ? AND state = ?",
    )
    .bind(release_id)
    .bind(&row.title)
    .bind(row.size)
    .bind(&magnet)
    .bind(DownloadPhase::Queued)
    .bind(download_id)
    .bind(DownloadPhase::Searching)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(
            "download {download_id} not in 'searching' state — can't fulfill (cancelled or concurrently grabbed?)"
        );
    }

    // Season-pack multi-link for episode grabs. Looks up whether the
    // originally-linked content was an episode, then fans out. Skipped
    // for movie grabs (no season concept) and non-pack releases.
    let episode_id: Option<i64> = sqlx::query_scalar(
        "SELECT episode_id FROM download_content
         WHERE download_id = ? AND episode_id IS NOT NULL
         LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    if let Some(episode_id) = episode_id {
        let parsed = crate::parser::parse(&row.title);
        if parsed.is_season_pack
            && let (Some(show_id), Some(season_number)) = (row.show_id, row.season_number)
        {
            let linked = sqlx::query(
                "INSERT OR IGNORE INTO download_content (download_id, episode_id)
                 SELECT ?, e.id
                 FROM episode e
                 WHERE e.show_id = ?
                   AND e.season_number = ?
                   AND e.acquire = 1
                   AND e.watched_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)
                   AND e.id != ?",
            )
            .bind(download_id)
            .bind(show_id)
            .bind(season_number)
            .bind(episode_id)
            .execute(pool)
            .await?;
            tracing::info!(
                download_id,
                season = season_number,
                extra_linked = linked.rows_affected(),
                "season pack fulfill — linked remaining wanted episodes to this download"
            );
        }
    }

    let quality = match (row.resolution, row.source.as_deref()) {
        (Some(r), Some(s)) => Some(format!("{s}-{r}p")),
        (Some(r), None) => Some(format!("{r}p")),
        _ => None,
    };
    // For the searching-download fulfill path the download might be
    // a movie or an episode; `download_display_title` picks the
    // right composition.
    let _ = event_tx.send(AppEvent::ReleaseGrabbed {
        download_id,
        title: crate::events::display::download_display_title(pool, download_id, &row.title).await,
        quality,
        indexer: row.indexer_name,
        size: row.size,
    });
    tracing::info!(download_id, release_id, "searching download fulfilled");
    Ok(())
}
