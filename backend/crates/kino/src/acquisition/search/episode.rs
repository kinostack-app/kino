//! Episode search — finds, scores, and (optionally) grabs the best
//! release for a single episode. Mirrors movie search but operates on
//! the (`show_id`, `season_number`, `episode_number`) shape and binds
//! results to the episode (not movie) side of `download_content` /
//! `media_episode`.
//!
//! Two flavours:
//!
//! * `search_episode` — auto-grab variant the wanted-sweep and the
//!   episode-level acquire endpoint use.
//! * `search_episode_with` — same flow with explicit auto-grab, used
//!   by watch-now's two-phase orchestrator.

// ReleaseTarget is brought into scope so `episode.load_blocklist(...)`
// and `episode.current_active_download(...)` resolve. Clippy can't see
// trait-method uses of an import.
#[allow(unused_imports)]
use crate::acquisition::ReleaseTarget;
use crate::acquisition::{
    AcquisitionPolicy, Decision, ExistingPick, PolicyContext, ReleaseCandidate,
    policy::DEFAULT_SAME_TIER_UPGRADE_DELTA,
};
use crate::events::AppEvent;
use crate::indexers::model::Indexer;
use crate::parser;
use crate::state::AppState;
use crate::torznab::client::{TorznabClient, TorznabQuery};
use crate::torznab::parse::TorznabRelease;

use super::{
    existing_episode_media_pick, fanout_search, indexer_caps, narrow_query, release_matches_target,
    stub_episode,
};

// ──────────────────────────── Episode search ────────────────────────────
//
// TV-episode search is structurally identical to movie search: per-indexer
// query, parse + score results, store releases, grab best. Differences:
//
//   * Query is `"{show title} S{ss}E{ee}"` (e.g. "Breaking Bad S05E14");
//     Torznab & Cardigann both accept season/ep discriminators too.
//   * Releases bind `show_id` + `episode_id` (not movie_id). The rest of
//     the download pipeline already handles this shape — see the
//     `download_content` + `media_episode` tables.
//   * Quality profile comes from the parent show, not the episode.

/// Search all enabled indexers for a single episode and store/grab
/// the best release.
#[tracing::instrument(skip(state), fields(episode_id))]
pub async fn search_episode(state: &AppState, episode_id: i64) -> anyhow::Result<()> {
    search_episode_with(state, episode_id, true).await
}

/// As [`search_episode`] but with explicit control over whether the
/// best release gets auto-grabbed. See [`search_movie_with`] for
/// rationale.
#[allow(clippy::too_many_lines, clippy::items_after_statements)]
pub async fn search_episode_with(
    state: &AppState,
    episode_id: i64,
    auto_grab: bool,
) -> anyhow::Result<()> {
    let pool = &state.db;
    let event_tx = &state.event_tx;
    let started = std::time::Instant::now();

    // Episode + parent show lookup in one query. The show's quality
    // profile is what we score against.
    #[derive(sqlx::FromRow)]
    #[allow(dead_code)]
    struct Row {
        ep_id: i64,
        show_id: i64,
        season_number: i64,
        episode_number: i64,
        has_media: bool,
        watched_at: Option<String>,
        show_title: String,
        show_imdb_id: Option<String>,
        show_tvdb_id: Option<i64>,
        show_year: Option<i64>,
        show_quality_profile_id: i64,
    }
    // No `e.acquire = 1` gate here on purpose: search_episode is
    // called from both the scheduler's wanted-sweep (which has already
    // filtered on acquire=1) *and* the user-triggered watch-now flow,
    // where a Play click is explicit intent to acquire this specific
    // episode regardless of whether it was flagged for bulk pickup.
    // Show-level `monitored` still matters — a removed show shouldn't
    // be searchable. Has-media / watched-at drive the skip rule
    // instead of a persisted status column.
    let row: Option<Row> = sqlx::query_as(
        "SELECT e.id as ep_id, e.show_id, e.season_number, e.episode_number,
                EXISTS(SELECT 1 FROM media_episode me WHERE me.episode_id = e.id) AS has_media,
                e.watched_at as watched_at,
                s.title as show_title, s.imdb_id as show_imdb_id,
                s.tvdb_id as show_tvdb_id, s.year as show_year,
                s.quality_profile_id as show_quality_profile_id
         FROM episode e
         JOIN show s ON s.id = e.show_id
         WHERE e.id = ? AND s.monitored = 1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(());
    };
    // Watched episodes are done — neither first-time nor upgrade.
    // An episode with media goes into upgrade mode (grab only if the
    // best release is a tier-level upgrade over the existing file);
    // without media, first-time grab.
    if row.watched_at.is_some() {
        return Ok(());
    }
    let is_upgrade_search = row.has_media;

    let episode_query = format!(
        "{} S{:02}E{:02}",
        row.show_title, row.season_number, row.episode_number
    );
    // User-facing event title uses the "Show · SxxExx · Title"
    // composition; the `episode_query` stays as the indexer search
    // string (no title, so quote-matching doesn't reject hits).
    let _ = event_tx.send(AppEvent::SearchStarted {
        movie_id: None,
        episode_id: Some(row.ep_id),
        title: crate::events::display::episode_display_title(pool, row.ep_id).await,
    });

    // Stamp last_searched_at *now* to debounce concurrent callers.
    // The wanted-sweep's eligibility clause respects this timestamp
    // so it won't re-fire us while we're in flight. User-triggered
    // watch-now bypasses the sweep entirely.
    let now_ts = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query("UPDATE episode SET last_searched_at = ? WHERE id = ?")
        .bind(&now_ts)
        .bind(row.ep_id)
        .execute(pool)
        .await?;

    let indexers = sqlx::query_as::<_, Indexer>(
        "SELECT * FROM indexer WHERE enabled = 1 AND (disabled_until IS NULL OR disabled_until < ?) ORDER BY priority",
    )
    .bind(crate::time::Timestamp::now().to_rfc3339())
    .fetch_all(pool)
    .await?;
    if indexers.is_empty() {
        // No indexers — leave last_searched_at so we don't spin,
        // but the sweep will retry after the backoff tier ticks.
        return Ok(());
    }

    let torznab_client = TorznabClient::new();
    let now = crate::time::Timestamp::now().to_rfc3339();

    // Blocklist for this episode (hash or title) — same shape as
    // the movie path, BlocklistEntry::matches_release is the
    // single check.
    let blocklist: Vec<crate::acquisition::BlocklistEntry> = sqlx::query_as(
        "SELECT torrent_info_hash, source_title FROM blocklist WHERE episode_id = ?",
    )
    .bind(row.ep_id)
    .fetch_all(pool)
    .await?;

    // Quality profile items (tiers) for scoring + language filter.
    let (profile_items_json, accepted_langs_json, profile_cutoff): (String, String, String) =
        sqlx::query_as(
            "SELECT items, accepted_languages, cutoff FROM quality_profile WHERE id = ?",
        )
        .bind(row.show_quality_profile_id)
        .fetch_one(pool)
        .await?;
    let profile_items: Vec<crate::settings::quality_profile::QualityTier> =
        serde_json::from_str(&profile_items_json).unwrap_or_default();
    let accepted_languages: Vec<String> =
        serde_json::from_str(&accepted_langs_json).unwrap_or_default();

    // Pre-compute existing media's (tier, score) for upgrade mode so
    // PolicyContext.existing is set once for the whole loop.
    let existing_pick = if is_upgrade_search {
        existing_episode_media_pick(pool, row.ep_id, &profile_items).await?
    } else {
        None
    };
    // Stand-in Episode value for the trait; the policy's
    // target-kind branch needs `kind() == Episode` to gate the
    // (currently stub) EpisodeTargetMismatch check.
    let episode_target = stub_episode(row.ep_id);

    // Wanted-in-season count drives the season-pack score boost. A
    // single episode wanted → pack is overkill, don't boost. Two or
    // more → boost proportional to coverage so the pack gets grabbed
    // once instead of N individual torrents. Boost is ~500 per covered
    // episode, roughly half a tier per episode, so a 10-ep pack beats
    // an individual release one tier up (1000 pts → 5000 pts boost).
    let wanted_in_season: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episode e
         WHERE e.show_id = ? AND e.season_number = ?
           AND e.acquire = 1
           AND e.watched_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM media_episode me WHERE me.episode_id = e.id)",
    )
    .bind(row.show_id)
    .bind(row.season_number)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // (release_id, score, tier_id) — tier_id needed for upgrade compare.
    let mut best_score: Option<(i64, i64, String)> = None;

    let mut kept_total = 0_usize;
    let mut skipped_total = 0_usize;
    // Parallel fanout across indexers — same pattern as
    // search_movie_with. Each per-indexer future logs its own
    // failure and returns an empty Vec rather than bail, so one slow
    // / broken indexer doesn't block the rest.
    let show_title = row.show_title.clone();
    let show_imdb_id = row.show_imdb_id.clone();
    let show_tvdb_id = row.show_tvdb_id;
    let season_number = row.season_number;
    let episode_number = row.episode_number;
    let episode_q = episode_query.clone();
    let fanout = fanout_search(&indexers, "episode", |indexer| {
        let client = torznab_client.clone();
        let state = state.clone();
        let show_title = show_title.clone();
        let show_imdb_id = show_imdb_id.clone();
        let episode_q = episode_q.clone();
        async move {
            let started = std::time::Instant::now();
            let results: Vec<TorznabRelease> = if indexer.indexer_type.as_str() == "cardigann" {
                search_cardigann_episode(
                    &state,
                    &indexer,
                    &show_title,
                    season_number,
                    episode_number,
                    show_imdb_id.as_deref(),
                )
                .await
            } else {
                let caps = indexer_caps(&indexer);
                if caps.tv_search.as_ref().is_some_and(|m| !m.available) {
                    tracing::debug!(
                        indexer = %indexer.name,
                        context = "episode",
                        "skipping — tv-search declared unavailable"
                    );
                    return (indexer, Vec::new());
                }
                let query = narrow_query(
                    TorznabQuery {
                        q: Some(episode_q.clone()),
                        imdbid: show_imdb_id.clone(),
                        tvdbid: show_tvdb_id,
                        season: Some(season_number),
                        ep: Some(episode_number),
                        cat: Some("5000".into()), // TV category
                        ..Default::default()
                    },
                    &caps,
                    "tv",
                );
                match client
                    .search(&indexer.url, indexer.api_key.as_deref(), &query)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            indexer = %indexer.name,
                            context = "episode",
                            error = %e,
                            "indexer search failed"
                        );
                        Vec::new()
                    }
                }
            };
            tracing::debug!(
                indexer = %indexer.name,
                context = "episode",
                results = results.len(),
                duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "indexer search complete"
            );
            (indexer, results)
        }
    })
    .await;

    // Cross-indexer dedup by info_hash. Same policy as the movie
    // search path — fanout arrives priority-sorted, first seen is
    // the keeper, subsequent duplicates drop out of the pipeline
    // before they're scored or inserted.
    let mut seen_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut dup_count = 0_usize;
    for (indexer, results) in &fanout {
        tracing::info!(
            indexer = %indexer.name,
            episode = %episode_query,
            results = results.len(),
            "episode search results"
        );

        for result in results {
            if let Some(hash) = result.info_hash.as_deref() {
                let key = hash.to_ascii_lowercase();
                if !seen_hashes.insert(key) {
                    dup_count += 1;
                    tracing::debug!(
                        indexer = %indexer.name,
                        hash = %hash,
                        release = %result.title,
                        "skipping duplicate info_hash — already kept from higher-priority indexer"
                    );
                    continue;
                }
            }

            if blocklist
                .iter()
                .any(|entry| entry.matches_release(result.info_hash.as_deref(), &result.title))
            {
                continue;
            }

            let parsed = parser::parse(&result.title);

            // Episode-target match (hard gate, lives outside the
            // policy because the policy's EpisodeTargetMismatch check
            // is intentionally a stub — see release/policy.rs docs).
            // Torznab fuzzy matches return unrelated S05E03 in a
            // S01E01 search; without this gate the wrong episode wins.
            if !release_matches_target(
                &parsed,
                &row.show_title,
                row.season_number,
                row.episode_number,
            ) {
                skipped_total += 1;
                tracing::debug!(
                    target_show = %row.show_title,
                    target_s = row.season_number,
                    target_e = row.episode_number,
                    release_title = %result.title,
                    parsed_title = %parsed.title,
                    parsed_season = ?parsed.season,
                    parsed_episodes = ?parsed.episodes,
                    "skipping release — doesn't match target",
                );
                continue;
            }
            kept_total += 1;

            let tier_id = parser::determine_quality_tier(&parsed);
            let parsed_episodes: Vec<i64> = parsed.episodes.iter().map(|n| i64::from(*n)).collect();
            let candidate = ReleaseCandidate {
                source_title: &result.title,
                torrent_info_hash: result.info_hash.as_deref(),
                languages: &parsed.languages,
                seeders: result.seeders,
                tier_id: &tier_id,
                is_proper: parsed.is_proper,
                is_repack: parsed.is_repack,
                is_season_pack: parsed.is_season_pack,
                parsed_season: parsed.season.map(i64::from),
                parsed_episodes: &parsed_episodes,
            };
            let ctx = PolicyContext {
                target: &episode_target,
                profile_tiers: &profile_items,
                accepted_languages: &accepted_languages,
                cutoff_tier_id: &profile_cutoff,
                blocklist: &blocklist,
                existing: existing_pick.as_ref().map(|(t, s)| ExistingPick {
                    tier_id: t,
                    score: *s,
                }),
                wanted_in_season: Some(wanted_in_season),
                min_same_tier_upgrade_delta: DEFAULT_SAME_TIER_UPGRADE_DELTA,
            };
            let score = match AcquisitionPolicy::evaluate(&candidate, &ctx) {
                Decision::Accept { score } => score,
                Decision::Reject { reason } => {
                    tracing::debug!(
                        indexer = %indexer.name,
                        release = %result.title,
                        ?reason,
                        "policy rejected episode release",
                    );
                    continue;
                }
            };

            // INSERT OR IGNORE + SELECT: dedup races where two
            // concurrent Play clicks race past the has_releases? check.
            // Unique index on (episode_id, indexer_id, guid) enforces.
            sqlx::query(
                "INSERT OR IGNORE INTO release (guid, indexer_id, show_id, episode_id, season_number,
                                      title, size, download_url, magnet_url, info_hash,
                                      publish_date, seeders, leechers, grabs,
                                      resolution, source, video_codec, audio_codec, hdr_format,
                                      is_remux, is_proper, is_repack, release_group,
                                      quality_score, status, first_seen_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'available', ?)",
            )
            .bind(&result.guid)
            .bind(indexer.id)
            .bind(row.show_id)
            .bind(row.ep_id)
            .bind(row.season_number)
            .bind(&result.title)
            .bind(result.size)
            .bind(&result.download_url)
            .bind(&result.magnet_url)
            .bind(&result.info_hash)
            .bind(&result.publish_date)
            .bind(result.seeders)
            .bind(result.leechers)
            .bind(result.grabs)
            .bind(parsed.resolution.as_deref().and_then(|r| r.parse::<i64>().ok()))
            .bind(parsed.source.as_deref())
            .bind(parsed.video_codec.as_deref())
            .bind(parsed.audio_codec.as_deref())
            .bind(parsed.hdr_format.as_deref())
            .bind(parsed.is_remux)
            .bind(parsed.is_proper)
            .bind(parsed.is_repack)
            .bind(parsed.release_group.as_deref())
            .bind(score)
            .bind(&now)
            .execute(pool)
            .await?;
            let release_id: i64 = sqlx::query_scalar(
                "SELECT id FROM release WHERE episode_id = ? AND indexer_id = ? AND guid = ?",
            )
            .bind(row.ep_id)
            .bind(indexer.id)
            .bind(&result.guid)
            .fetch_one(pool)
            .await?;

            if best_score.as_ref().is_none_or(|(_, s, _)| score > *s) {
                best_score = Some((release_id, score, tier_id.clone()));
            }
        }
    }

    tracing::info!(
        episode_id = row.ep_id,
        kept = kept_total,
        skipped = skipped_total,
        duplicates = dup_count,
        "episode-search filter summary"
    );
    let mut grabbed_any = false;
    if let Some((release_id, _, _)) = best_score {
        // Policy already gated each release on the upgrade check
        // (PolicyContext.existing was set when in upgrade mode), so
        // anything that reached `best_score` is already acceptable.
        if auto_grab {
            crate::acquisition::grab::grab_episode_release(state, release_id, row.ep_id).await?;
            grabbed_any = true;
        } else {
            tracing::debug!(
                episode_id = row.ep_id,
                release_id,
                "search_episode_with(auto_grab=false): leaving grab to caller"
            );
        }
    }
    // Nothing-found case: no status revert needed — status is
    // derived, and the sweep's tiered backoff uses last_searched_at
    // (stamped at the top of this function) to decide when to retry.

    // Re-stamp last_searched_at with a fresh timestamp so the
    // tiered-backoff schedule reflects *this* search's end time.
    sqlx::query("UPDATE episode SET last_searched_at = ? WHERE id = ?")
        .bind(&now)
        .bind(row.ep_id)
        .execute(pool)
        .await?;

    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(
        episode_id = row.ep_id,
        elapsed_ms,
        grabbed = grabbed_any,
        "search_episode finished"
    );

    Ok(())
}

/// Grab a specific release for an episode. Mirrors `grab_release`
/// for movies: resolve magnet, insert download row, link to episode
/// via `download_content`, update episode status, emit event.
/// Returns the created `download_id`.
#[tracing::instrument(skip(state), fields(release_id, episode_id))]
#[allow(clippy::too_many_lines)]
async fn search_cardigann_episode(
    state: &AppState,
    indexer: &Indexer,
    show_title: &str,
    season: i64,
    episode: i64,
    imdb_id: Option<&str>,
) -> Vec<TorznabRelease> {
    let Some(ref definitions) = state.definitions else {
        return Vec::new();
    };
    let Some(definition_id) = indexer.definition_id.as_deref() else {
        return Vec::new();
    };
    let Some(definition) = definitions.get(definition_id) else {
        return Vec::new();
    };
    let settings: std::collections::HashMap<String, String> = indexer
        .settings_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Cardigann search paths template `{{ .Keywords }}` into the
    // request URL — so `keywords` is what actually reaches the
    // indexer. Setting it to `show_title` alone (as we did previously)
    // meant LimeTorrents-style "latest uploads" listings ignored the
    // episode entirely and returned recent episodes of the show.
    // Include the SxxExx tag here; most Cardigann definitions carry a
    // `keywordsfilters` entry that either keeps or normalises it into
    // whatever shape the site expects.
    let kw = format!("{show_title} S{season:02}E{episode:02}");
    let query = crate::indexers::template::SearchQuery {
        q: kw.clone(),
        keywords: kw,
        season: Some(format!("{season:02}")),
        ep: Some(format!("{episode:02}")),
        imdbid: imdb_id.map(String::from),
        ..Default::default()
    };

    let client = state.indexer_client(indexer.id).await;
    match crate::indexers::search(&client, &definition, &settings, &query).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                indexer = %indexer.name,
                indexer_id = indexer.id,
                error = %e,
                "cardigann episode search failed — dropping cached client"
            );
            state.invalidate_indexer_client(indexer.id).await;
            Vec::new()
        }
    }
}
