//! Movie search — finds, scores, and (optionally) grabs the best
//! release for a single movie. Two flavours:
//!
//! * `search_movie` — the public, auto-grab variant the wanted-sweep
//!   and `MovieAdded` event handler use.
//! * `search_movie_with` — same flow, but the caller controls
//!   `auto_grab`. Watch-now passes `false` because it pre-creates a
//!   `searching` download row that `fulfill_searching_with_release`
//!   resolves; the search must not race that with its own grab.
//!
//! First-time vs upgrade is decided per-call: if the movie already
//! has media, the policy gate runs in upgrade mode (existing tier +
//! score is the floor a candidate has to beat).

// ReleaseTarget is used implicitly via `movie.load_blocklist(...)` and
// `movie.current_active_download(...)`; the trait must be in scope
// for method resolution to find them, but clippy can't see that and
// flags it as unused.
#[allow(unused_imports)]
use crate::acquisition::ReleaseTarget;
use crate::acquisition::{
    AcquisitionPolicy, BlocklistEntry, Decision, ExistingPick, PolicyContext, ReleaseCandidate,
    policy::DEFAULT_SAME_TIER_UPGRADE_DELTA,
};
use crate::events::AppEvent;
use crate::indexers::model::Indexer;
use crate::parser;
use crate::state::AppState;
use crate::torznab::client::{TorznabClient, TorznabQuery};
use crate::torznab::parse::TorznabRelease;

use super::{
    existing_media_pick, fanout_search, indexer_caps, narrow_query, search_cardigann_indexer,
};

/// Search all enabled indexers for a specific movie and store/grab the best release.
///
/// Covers two cases, derived from (`media` / `watched_at`) rather
/// than a persisted status column:
///   - no media, not watched → first-time acquisition, grab the best release.
///   - has media, not watched → upgrade check — only grab if the best
///     new release is a tier-level upgrade over the existing media.
/// Watched movies are skipped: the user's done.
#[tracing::instrument(skip(state), fields(movie_id))]
pub async fn search_movie(state: &AppState, movie_id: i64) -> anyhow::Result<()> {
    search_movie_with(state, movie_id, true).await
}

/// As [`search_movie`] but with explicit control over whether the
/// best release gets auto-grabbed when found. The scheduler's wanted-
/// sweep passes `true`; the two-phase watch-now flow passes `false`
/// because it fulfills the caller's pre-created `searching` download
/// row instead of letting this function INSERT a second row.
#[allow(clippy::too_many_lines)]
pub async fn search_movie_with(
    state: &AppState,
    movie_id: i64,
    auto_grab: bool,
) -> anyhow::Result<()> {
    let pool = &state.db;
    let event_tx = &state.event_tx;
    let started = std::time::Instant::now();

    // Load the full Movie row so the trait methods (load_blocklist,
    // current_active_download) and the policy can operate on a single
    // typed target. Status is derived; we discard it here.
    let movie: Option<crate::content::movie::model::Movie> =
        sqlx::query_as::<_, crate::content::movie::model::Movie>(
            "SELECT *, '' AS status FROM movie WHERE id = ? AND monitored = 1",
        )
        .bind(movie_id)
        .fetch_optional(pool)
        .await?;
    let Some(movie) = movie else {
        return Ok(());
    };
    // Watched movies are "done" — neither first-time nor upgrade.
    if movie.watched_at.as_ref().is_some_and(|s| !s.is_empty()) {
        return Ok(());
    }
    let id = movie.id;
    let title = movie.title.clone();
    let imdb_id = movie.imdb_id.clone();
    let year = movie.year;
    let has_media: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media WHERE movie_id = ?)")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap_or(false);
    let is_upgrade_search = has_media;

    let _ = event_tx.send(AppEvent::SearchStarted {
        movie_id: Some(id),
        episode_id: None,
        title: title.clone(),
    });

    // Get enabled indexers. `disabled_until` is stored RFC3339 so
    // we bind Rust-side `now` rather than using SQLite's
    // `datetime('now')` (different format, lexicographic compare
    // was silently wrong).
    let indexers = sqlx::query_as::<_, Indexer>(
        "SELECT * FROM indexer WHERE enabled = 1 AND (disabled_until IS NULL OR disabled_until < ?) ORDER BY priority",
    )
    .bind(crate::time::Timestamp::now().to_rfc3339())
    .fetch_all(pool)
    .await?;

    if indexers.is_empty() {
        tracing::warn!(movie_id = id, "no enabled indexers for search");
        return Ok(());
    }

    let torznab_client = TorznabClient::new();
    let now = crate::time::Timestamp::now().to_rfc3339();
    // (release_id, score, tier_id) for the highest-scoring release we see.
    let mut best_score: Option<(i64, i64, String)> = None;

    // Load blocklist for this movie via the trait — single source of
    // truth for "which entries scope to this target".
    let blocklist: Vec<BlocklistEntry> = movie.load_blocklist(pool).await?;

    let profile_id = movie.quality_profile_id;
    let (profile_items_json, accepted_langs_json, profile_cutoff): (String, String, String) =
        sqlx::query_as(
            "SELECT items, accepted_languages, cutoff FROM quality_profile WHERE id = ?",
        )
        .bind(profile_id)
        .fetch_one(pool)
        .await?;

    let profile_items: Vec<crate::settings::quality_profile::QualityTier> =
        serde_json::from_str(&profile_items_json).unwrap_or_default();
    let accepted_languages: Vec<String> =
        serde_json::from_str(&accepted_langs_json).unwrap_or_default();

    // Pre-compute existing media's (tier, score) when in upgrade mode
    // so PolicyContext.existing is set once for the whole loop.
    let existing_pick = if is_upgrade_search {
        existing_media_pick(pool, id, &profile_items).await?
    } else {
        None
    };

    // Fan out the indexer queries in parallel — the slowest indexer
    // no longer gates the whole search. Each future logs its own
    // failure path; we only see successful result sets here.
    let fanout = fanout_search(&indexers, "movie", |indexer| {
        let client = torznab_client.clone();
        let state = state.clone();
        let title = title.clone();
        let imdb_id = imdb_id.clone();
        async move {
            let started = std::time::Instant::now();
            let results: Vec<TorznabRelease> = if indexer.indexer_type.as_str() == "cardigann" {
                search_cardigann_indexer(&state, &indexer, &title, year, imdb_id.as_deref()).await
            } else {
                let caps = indexer_caps(&indexer);
                // Skip indexers that explicitly declared
                // movie-search unavailable. Unprobed indexers
                // fall through: `movie_available()` is strict
                // false when mode is None, but we only gate when
                // we've actually seen a `<movie-search available="no">`.
                if caps.movie_search.as_ref().is_some_and(|m| !m.available) {
                    tracing::debug!(
                        indexer = %indexer.name,
                        context = "movie",
                        "skipping — movie-search declared unavailable"
                    );
                    return (indexer, Vec::new());
                }
                let query = narrow_query(
                    TorznabQuery {
                        q: Some(format!("{} {}", title, year.unwrap_or(0))),
                        imdbid: imdb_id.clone(),
                        ..Default::default()
                    },
                    &caps,
                    "movie",
                );
                match client
                    .search(&indexer.url, indexer.api_key.as_deref(), &query)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            indexer = %indexer.name,
                            context = "movie",
                            error = %e,
                            "indexer search failed"
                        );
                        Vec::new()
                    }
                }
            };
            tracing::debug!(
                indexer = %indexer.name,
                context = "movie",
                results = results.len(),
                duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "indexer search complete"
            );
            (indexer, results)
        }
    })
    .await;

    // Cross-indexer dedup by info_hash. Fanout is already sorted by
    // priority (lowest number first), so the first time we see a
    // given hash it's from the highest-priority indexer carrying it;
    // we keep that one and skip subsequent duplicates. Releases
    // without an info_hash pass through — can't dedup what we can't
    // identify, and the DB's unique index on (movie_id, indexer_id,
    // guid) still prevents double-inserting the same row.
    let mut seen_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut dup_count = 0_usize;
    for (indexer, results) in &fanout {
        tracing::info!(
            indexer = %indexer.name,
            movie = %title,
            results = results.len(),
            "search results"
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

            // Parse the title up front; the policy and the INSERT
            // both consume the parsed shape.
            let parsed = parser::parse(&result.title);
            let tier_id = parser::determine_quality_tier(&parsed);

            // Single decision gate: blocklist + language + tier-allowed
            // + upgrade are all `AcquisitionPolicy::evaluate`'s job.
            // The hash dedup above stays inline because it spans the
            // cross-indexer fanout, not a per-release predicate.
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
                parsed_episodes: &[],
            };
            let ctx = PolicyContext {
                target: &movie,
                profile_tiers: &profile_items,
                accepted_languages: &accepted_languages,
                cutoff_tier_id: &profile_cutoff,
                blocklist: &blocklist,
                existing: existing_pick.as_ref().map(|(t, s)| ExistingPick {
                    tier_id: t,
                    score: *s,
                }),
                wanted_in_season: None,
                min_same_tier_upgrade_delta: DEFAULT_SAME_TIER_UPGRADE_DELTA,
            };
            let score = match AcquisitionPolicy::evaluate(&candidate, &ctx) {
                Decision::Accept { score } => score,
                Decision::Reject { reason } => {
                    tracing::debug!(
                        indexer = %indexer.name,
                        release = %result.title,
                        ?reason,
                        "policy rejected release",
                    );
                    continue;
                }
            };

            // Store release — INSERT OR IGNORE + SELECT makes this
            // safe under concurrent searches for the same content
            // (two Play clicks racing), backed by the unique index
            // on (movie_id, indexer_id, guid).
            sqlx::query(
                "INSERT OR IGNORE INTO release (guid, indexer_id, movie_id, title, size, download_url, magnet_url, info_hash, publish_date, seeders, leechers, grabs, resolution, source, video_codec, audio_codec, hdr_format, is_remux, is_proper, is_repack, release_group, quality_score, status, first_seen_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'available', ?)",
            )
            .bind(&result.guid)
            .bind(indexer.id)
            .bind(id)
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
                "SELECT id FROM release WHERE movie_id = ? AND indexer_id = ? AND guid = ?",
            )
            .bind(id)
            .bind(indexer.id)
            .bind(&result.guid)
            .fetch_one(pool)
            .await?;

            // Track best
            if best_score.as_ref().is_none_or(|(_, s, _)| score > *s) {
                best_score = Some((release_id, score, tier_id.clone()));
            }
        }
    }

    if dup_count > 0 {
        tracing::info!(
            movie_id = id,
            duplicates = dup_count,
            "cross-indexer dedup dropped duplicate releases"
        );
    }

    // The policy already gated each release on the upgrade check
    // (PolicyContext.existing was set when `is_upgrade_search`), so
    // anything that reached `best_score` is by definition acceptable.
    // No post-loop upgrade re-evaluation needed.
    let grabbed = best_score.is_some();
    if let Some((release_id, _, _)) = best_score {
        if auto_grab {
            crate::acquisition::grab::grab_release(state, release_id, id).await?;
        } else {
            tracing::debug!(
                movie_id = id,
                release_id,
                "search_movie_with(auto_grab=false): leaving grab to caller"
            );
        }
    }

    sqlx::query("UPDATE movie SET last_searched_at = ? WHERE id = ?")
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

    tracing::info!(
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        grabbed,
        "search_movie finished",
    );

    Ok(())
}
