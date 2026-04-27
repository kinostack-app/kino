//! Search service — finds and scores releases for wanted content.
//!
//! Called by both:
//! - Event handler (`MovieAdded` → search immediately)
//! - Scheduler (`wanted_search` periodic sweep)

pub mod episode;
pub mod movie;
pub mod wanted_sweep;

use futures::stream::{FuturesUnordered, StreamExt};

// `ReleaseTarget` brought into scope so `episode.load_blocklist(...)`
// trait-method calls (in the still-in-place episode search) resolve.
// Clippy can't see method-call uses of trait imports, hence the allow.
#[allow(unused_imports)]
use crate::acquisition::ReleaseTarget;
use crate::acquisition::{PolicyContext, ReleaseCandidate};
use crate::indexers::model::Indexer;
use crate::parser;
use crate::state::AppState;
use crate::torznab::caps::{SearchMode, TorznabCapabilities};
use crate::torznab::client::TorznabQuery;
use crate::torznab::parse::TorznabRelease;

/// Read the cached capabilities JSON off an indexer row. Returns
/// the default (all-open, fall-through `tv_supports` / `movie_supports`
/// return true) when the indexer hasn't been probed yet or the JSON
/// failed to parse — both cases shouldn't silently strip params.
pub(super) fn indexer_caps(indexer: &Indexer) -> TorznabCapabilities {
    let mut caps = TorznabCapabilities::default();
    if let Some(raw) = indexer.supported_search_params.as_deref() {
        #[derive(serde::Deserialize, Default)]
        struct StoredSearch {
            tv_search: Option<SearchMode>,
            movie_search: Option<SearchMode>,
        }
        match serde_json::from_str::<StoredSearch>(raw) {
            Ok(s) => {
                caps.tv_search = s.tv_search;
                caps.movie_search = s.movie_search;
            }
            Err(e) => {
                tracing::debug!(
                    indexer = %indexer.name,
                    error = %e,
                    "supported_search_params JSON parse failed; treating as unprobed"
                );
            }
        }
    }
    if let Some(raw) = indexer.supported_categories.as_deref()
        && let Ok(cats) = serde_json::from_str::<Vec<i64>>(raw)
    {
        caps.categories = cats;
    }
    caps
}

/// Keep only the query fields the indexer declared for the given
/// mode. No-op for indexers we've never probed (they keep the
/// current "send everything" behaviour). Always preserves `q`, `cat`,
/// `season`, and `ep` — those are either universal or already gated
/// by the caller.
pub(super) fn narrow_query(
    mut q: TorznabQuery,
    caps: &TorznabCapabilities,
    mode: &str,
) -> TorznabQuery {
    let supports = |param: &str| match mode {
        "tv" => caps.tv_supports(param),
        _ => caps.movie_supports(param),
    };
    if q.imdbid.is_some() && !supports("imdbid") {
        q.imdbid = None;
    }
    if q.tvdbid.is_some() && !supports("tvdbid") {
        q.tvdbid = None;
    }
    if q.tmdbid.is_some() && !supports("tmdbid") {
        q.tmdbid = None;
    }
    q
}

/// Fan out a per-indexer async search across every indexer in
/// parallel, collect each indexer's results, and return them in
/// priority order so downstream processing is deterministic. Errors
/// inside `per_indexer` are the caller's concern — the futures are
/// expected to log + return `Vec::new()` rather than propagate. This
/// keeps one slow indexer from blocking the others.
pub(super) async fn fanout_search<F, Fut>(
    indexers: &[Indexer],
    context: &'static str,
    per_indexer: F,
) -> Vec<(Indexer, Vec<TorznabRelease>)>
where
    F: Fn(Indexer) -> Fut,
    Fut: std::future::Future<Output = (Indexer, Vec<TorznabRelease>)>,
{
    let started = std::time::Instant::now();
    let mut inflight = FuturesUnordered::new();
    for indexer in indexers {
        inflight.push(per_indexer(indexer.clone()));
    }
    let mut collected = Vec::with_capacity(indexers.len());
    while let Some(entry) = inflight.next().await {
        collected.push(entry);
    }
    // Restore priority order — FuturesUnordered yields by completion.
    collected.sort_by_key(|(ind, _)| ind.priority);
    tracing::info!(
        context,
        indexers = indexers.len(),
        total_duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "indexer fanout complete"
    );
    collected
}

/// (resolution, source, `video_codec`, `audio_codec`, `is_remux`, `is_proper`)
type ExistingMediaQuality = (
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    bool,
);

/// Look up the existing media's `(tier_id, score)` for upgrade-mode
/// scoring. Returns `None` if no media row exists.
pub(super) async fn existing_media_pick(
    pool: &sqlx::SqlitePool,
    movie_id: i64,
    profile_items: &[crate::settings::quality_profile::QualityTier],
) -> anyhow::Result<Option<(String, i64)>> {
    let existing: Option<ExistingMediaQuality> = sqlx::query_as(
        "SELECT resolution, source, video_codec, audio_codec, is_remux, is_proper FROM media
         WHERE movie_id = ? ORDER BY date_added DESC LIMIT 1",
    )
    .bind(movie_id)
    .fetch_optional(pool)
    .await?;
    let Some(e) = existing else {
        return Ok(None);
    };
    let parsed = parser::ParsedRelease {
        resolution: e.0.map(|r| r.to_string()),
        source: e.1,
        video_codec: e.2,
        audio_codec: e.3,
        is_remux: e.4,
        is_proper: e.5,
        ..Default::default()
    };
    let tier_id = parser::determine_quality_tier(&parsed);
    let Some(tier) = profile_items.iter().find(|t| t.quality_id == tier_id) else {
        // Existing media at an unknown tier — treat as score 0 so any
        // recognised tier upgrades over it. Logged once for diagnosis.
        tracing::warn!(
            movie_id,
            tier = %tier_id,
            "existing media at tier not in user's profile; treating as score 0"
        );
        return Ok(Some((tier_id, 0)));
    };
    let candidate = ReleaseCandidate {
        source_title: "",
        torrent_info_hash: None,
        languages: &[],
        seeders: None,
        tier_id: &tier_id,
        is_proper: parsed.is_proper,
        is_repack: parsed.is_repack,
        is_season_pack: false,
        parsed_season: None,
        parsed_episodes: &[],
    };
    // Compute the score with the same formula `evaluate` uses
    // internally; we don't need a full PolicyContext for upgrade
    // mode here because the ctx is only consumed for `wanted_in_season`.
    let throwaway_ctx = PolicyContext {
        target: &crate::content::movie::model::Movie {
            id: movie_id,
            ..stub_movie()
        },
        profile_tiers: profile_items,
        accepted_languages: &[],
        cutoff_tier_id: "",
        blocklist: &[],
        existing: None,
        wanted_in_season: None,
        min_same_tier_upgrade_delta: 0,
    };
    let score = crate::acquisition::policy::compute_score(&candidate, tier, &throwaway_ctx);
    Ok(Some((tier_id, score)))
}

/// Look up an episode's existing media `(tier_id, score)` for
/// upgrade-mode scoring. Mirrors `existing_media_pick` for the
/// episode shape (joined through `media_episode`).
pub(super) async fn existing_episode_media_pick(
    pool: &sqlx::SqlitePool,
    episode_id: i64,
    profile_items: &[crate::settings::quality_profile::QualityTier],
) -> anyhow::Result<Option<(String, i64)>> {
    let existing: Option<ExistingMediaQuality> = sqlx::query_as(
        "SELECT m.resolution, m.source, m.video_codec, m.audio_codec, m.is_remux, m.is_proper
         FROM media m
         JOIN media_episode me ON me.media_id = m.id
         WHERE me.episode_id = ?
         ORDER BY m.date_added DESC
         LIMIT 1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;
    let Some(e) = existing else {
        return Ok(None);
    };
    let parsed = parser::ParsedRelease {
        resolution: e.0.map(|r| r.to_string()),
        source: e.1,
        video_codec: e.2,
        audio_codec: e.3,
        is_remux: e.4,
        is_proper: e.5,
        ..Default::default()
    };
    let tier_id = parser::determine_quality_tier(&parsed);
    let Some(tier) = profile_items.iter().find(|t| t.quality_id == tier_id) else {
        tracing::warn!(
            episode_id,
            tier = %tier_id,
            "existing episode media at tier not in user's profile; treating as score 0"
        );
        return Ok(Some((tier_id, 0)));
    };
    let candidate = ReleaseCandidate {
        source_title: "",
        torrent_info_hash: None,
        languages: &[],
        seeders: None,
        tier_id: &tier_id,
        is_proper: parsed.is_proper,
        is_repack: parsed.is_repack,
        is_season_pack: false,
        parsed_season: None,
        parsed_episodes: &[],
    };
    let target = stub_episode(episode_id);
    let throwaway_ctx = PolicyContext {
        target: &target,
        profile_tiers: profile_items,
        accepted_languages: &[],
        cutoff_tier_id: "",
        blocklist: &[],
        existing: None,
        wanted_in_season: None,
        min_same_tier_upgrade_delta: 0,
    };
    let score = crate::acquisition::policy::compute_score(&candidate, tier, &throwaway_ctx);
    Ok(Some((tier_id, score)))
}

pub(super) fn stub_episode(id: i64) -> crate::content::show::episode::Episode {
    crate::content::show::episode::Episode {
        id,
        series_id: 0,
        show_id: 0,
        season_number: 0,
        tmdb_id: None,
        tvdb_id: None,
        episode_number: 0,
        title: None,
        overview: None,
        air_date_utc: None,
        runtime: None,
        still_path: None,
        tmdb_rating: None,
        status: String::new(),
        acquire: false,
        in_scope: false,
        playback_position_ticks: 0,
        play_count: 0,
        last_played_at: None,
        watched_at: None,
        preferred_audio_stream_index: None,
        preferred_subtitle_stream_index: None,
        last_searched_at: None,
        intro_start_ms: None,
        intro_end_ms: None,
        credits_start_ms: None,
        credits_end_ms: None,
        intro_analysis_at: None,
    }
}

/// Construct a `Movie` value with all fields zeroed/empty. Used as
/// a stand-in target for score computations that don't actually
/// touch the target.
pub(super) fn stub_movie() -> crate::content::movie::model::Movie {
    crate::content::movie::model::Movie {
        id: 0,
        tmdb_id: 0,
        imdb_id: None,
        tvdb_id: None,
        title: String::new(),
        original_title: None,
        overview: None,
        tagline: None,
        year: None,
        runtime: None,
        release_date: None,
        physical_release_date: None,
        digital_release_date: None,
        certification: None,
        poster_path: None,
        backdrop_path: None,
        genres: None,
        tmdb_rating: None,
        tmdb_vote_count: None,
        popularity: None,
        original_language: None,
        collection_tmdb_id: None,
        collection_name: None,
        youtube_trailer_id: None,
        quality_profile_id: 0,
        status: String::new(),
        monitored: false,
        added_at: String::new(),
        last_searched_at: None,
        blurhash_poster: None,
        blurhash_backdrop: None,
        playback_position_ticks: 0,
        play_count: 0,
        last_played_at: None,
        watched_at: None,
        preferred_audio_stream_index: None,
        preferred_subtitle_stream_index: None,
        last_metadata_refresh: None,
        user_rating: None,
        logo_path: None,
        logo_palette: None,
    }
}

pub(super) async fn search_cardigann_indexer(
    state: &AppState,
    indexer: &Indexer,
    title: &str,
    year: Option<i64>,
    imdb_id: Option<&str>,
) -> Vec<TorznabRelease> {
    let Some(ref definitions) = state.definitions else {
        tracing::warn!(indexer = %indexer.name, "cardigann indexer but no definitions loaded");
        return Vec::new();
    };

    let Some(definition_id) = indexer.definition_id.as_deref() else {
        tracing::warn!(indexer = %indexer.name, "cardigann indexer has no definition_id");
        return Vec::new();
    };

    let Some(definition) = definitions.get(definition_id) else {
        tracing::warn!(indexer = %indexer.name, definition_id, "definition not found");
        return Vec::new();
    };

    let settings: std::collections::HashMap<String, String> = indexer
        .settings_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let query = crate::indexers::template::SearchQuery {
        q: format!("{} {}", title, year.unwrap_or(0)),
        keywords: format!("{} {}", title, year.unwrap_or(0)),
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
                "cardigann search failed — dropping cached client"
            );
            state.invalidate_indexer_client(indexer.id).await;
            Vec::new()
        }
    }
}

/// Does `parsed` plausibly refer to the episode at `(target_season,
/// target_episode)` of the show named `target_title`? Acceptance
/// rules:
///
///   - **Title** must match after normalisation (lowercase, punctuation
///     stripped, whitespace collapsed). Catches "Mrs Browns Boys" slipping
///     into a search for "The Boys" — same season/episode numbers, wrong
///     show. Normalised-exact, not fuzzy: loose matching is exactly
///     what lets imposters through.
///   - **Season / episode** must match: accept exact `(season, episode)`
///     hits, matching season packs, or titles that carry no `SxxExx`
///     (indexer's season/ep search params did the filtering for us).
///
/// Without this filter, Torznab's fuzzy search lets wrong-show /
/// off-target releases slip into `release` bound to the target
/// `episode_id`, and a high-scoring imposter wins the grab.
pub(super) fn release_matches_target(
    parsed: &crate::parser::ParsedRelease,
    target_title: &str,
    target_season: i64,
    target_episode: i64,
) -> bool {
    // Title gate first — cheap, decisive.
    if !titles_match(&parsed.title, target_title) {
        return false;
    }

    // Season / episode gate.
    let Some(season) = parsed.season else {
        return true;
    };
    if i64::from(season) != target_season {
        return false;
    }
    if parsed.is_season_pack || parsed.episodes.is_empty() {
        return true;
    }
    parsed
        .episodes
        .iter()
        .any(|e| i64::from(*e) == target_episode)
}

/// Normalise a show title for comparison: lowercase, strip punctuation,
/// collapse whitespace. "The Boys" == "the.boys" == "The-Boys". Does
/// NOT do fuzzy matching (no edit distance, no subset) — that's
/// deliberately strict, because loose matching is exactly what lets
/// "Mrs Browns Boys" through on a search for "The Boys".
pub(super) fn titles_match(parsed_title: &str, target_title: &str) -> bool {
    fn normalise(s: &str) -> String {
        s.chars()
            .filter_map(|c| {
                if c.is_alphanumeric() {
                    Some(c.to_ascii_lowercase())
                } else if c.is_whitespace() || matches!(c, '.' | '_' | '-' | ':' | '\'') {
                    Some(' ')
                } else {
                    None
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    let a = normalise(parsed_title);
    let b = normalise(target_title);
    !a.is_empty() && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(title: &str, s: u16, eps: &[u16], is_pack: bool) -> crate::parser::ParsedRelease {
        crate::parser::ParsedRelease {
            title: title.to_owned(),
            season: Some(s),
            episodes: eps.to_vec(),
            is_season_pack: is_pack,
            ..Default::default()
        }
    }

    #[test]
    fn matches_exact_episode() {
        assert!(release_matches_target(
            &parsed("The Boys", 1, &[1], false),
            "The Boys",
            1,
            1
        ));
    }

    #[test]
    fn rejects_wrong_episode() {
        assert!(!release_matches_target(
            &parsed("The Boys", 1, &[2], false),
            "The Boys",
            1,
            1
        ));
    }

    #[test]
    fn rejects_wrong_season_even_with_matching_ep_number() {
        assert!(!release_matches_target(
            &parsed("The Boys", 5, &[3], false),
            "The Boys",
            1,
            3
        ));
    }

    #[test]
    fn accepts_season_pack_of_target_season() {
        assert!(release_matches_target(
            &parsed("The Boys", 1, &[], true),
            "The Boys",
            1,
            7
        ));
    }

    #[test]
    fn rejects_season_pack_of_wrong_season() {
        assert!(!release_matches_target(
            &parsed("The Boys", 2, &[], true),
            "The Boys",
            1,
            1
        ));
    }

    #[test]
    fn accepts_multi_episode_pack_containing_target() {
        assert!(release_matches_target(
            &parsed("The Boys", 1, &[5, 6, 7, 8], false),
            "The Boys",
            1,
            7
        ));
    }

    #[test]
    fn accepts_release_with_no_parsed_season() {
        // Title still needs to match when present.
        let p = crate::parser::ParsedRelease {
            title: "The Boys".into(),
            ..Default::default()
        };
        assert!(release_matches_target(&p, "The Boys", 3, 2));
    }

    #[test]
    fn rejects_different_show_with_matching_episode_numbers() {
        // The motivating case: search for "The Boys" S01E01 on an
        // indexer that returns "Mrs Browns Boys S01E01" because the
        // word "Boys" overlaps. Season/ep numbers match — *only* the
        // title check saves us.
        assert!(!release_matches_target(
            &parsed("Mrs Browns Boys", 1, &[1], false),
            "The Boys",
            1,
            1,
        ));
    }

    #[test]
    fn title_match_is_punctuation_insensitive() {
        // Indexers use various separators in release names.
        assert!(release_matches_target(
            &parsed("The.Boys", 1, &[1], false),
            "The Boys",
            1,
            1
        ));
        assert!(release_matches_target(
            &parsed("the-boys", 1, &[1], false),
            "The Boys",
            1,
            1
        ));
        assert!(release_matches_target(
            &parsed("Doctor Who", 1, &[1], false),
            "Doctor Who",
            1,
            1
        ));
    }

    #[test]
    fn title_match_rejects_spinoffs_and_suffixes() {
        // Avoid accidentally grabbing "The Boys Presents: Diabolical"
        // when the user clicked Play on "The Boys". Spinoffs have
        // their own TMDB entry.
        assert!(!release_matches_target(
            &parsed("The Boys Presents Diabolical", 1, &[1], false),
            "The Boys",
            1,
            1,
        ));
    }

    fn test_indexer(id: i64, name: &str, priority: i64) -> Indexer {
        Indexer {
            id,
            name: name.into(),
            url: String::new(),
            api_key: None,
            priority,
            enabled: true,
            supports_rss: true,
            supports_search: true,
            supported_categories: None,
            supported_search_params: None,
            initial_failure_time: None,
            most_recent_failure_time: None,
            escalation_level: 0,
            disabled_until: None,
            indexer_type: "torznab".into(),
            definition_id: None,
            settings_json: None,
        }
    }

    #[test]
    fn narrow_query_strips_params_indexer_doesnt_support() {
        // LimeTorrents-shape: q-only for both modes.
        let caps = TorznabCapabilities {
            tv_search: Some(SearchMode {
                available: true,
                supported_params: vec!["q".into(), "season".into(), "ep".into()],
            }),
            movie_search: Some(SearchMode {
                available: true,
                supported_params: vec!["q".into()],
            }),
            categories: vec![],
        };
        let q = TorznabQuery {
            q: Some("the boys s01e01".into()),
            imdbid: Some("tt1190634".into()),
            tvdbid: Some(305_288),
            tmdbid: Some(76_479),
            season: Some(1),
            ep: Some(1),
            cat: Some("5000".into()),
        };
        let narrowed = narrow_query(q, &caps, "tv");
        assert!(narrowed.imdbid.is_none(), "imdbid should be stripped");
        assert!(narrowed.tvdbid.is_none(), "tvdbid should be stripped");
        assert!(narrowed.tmdbid.is_none(), "tmdbid should be stripped");
        assert!(narrowed.q.is_some(), "q must survive");
        assert_eq!(narrowed.season, Some(1), "season must survive");
        assert_eq!(narrowed.ep, Some(1), "ep must survive");
    }

    #[test]
    fn narrow_query_passthrough_when_unprobed() {
        let caps = TorznabCapabilities::default();
        let q = TorznabQuery {
            q: Some("title".into()),
            imdbid: Some("tt1".into()),
            tmdbid: Some(1),
            ..Default::default()
        };
        let narrowed = narrow_query(q, &caps, "movie");
        assert!(
            narrowed.imdbid.is_some(),
            "unprobed indexers must pass IDs through"
        );
        assert!(narrowed.tmdbid.is_some());
    }

    /// Fanout helper: each future fires in parallel (total ≈ max
    /// per-future time, not sum), output preserves priority order
    /// regardless of the order futures complete in.
    #[tokio::test]
    async fn fanout_runs_in_parallel_and_sorts_by_priority() {
        let indexers = vec![
            test_indexer(1, "slow-high-priority", 1),
            test_indexer(2, "fast-low-priority", 50),
        ];

        let start = std::time::Instant::now();
        let out = fanout_search(&indexers, "test", |indexer| async move {
            // Slow indexer takes 200ms, fast takes 10ms. Sequential
            // would run 210+ms; parallel runs the slow one alone
            // (~200ms) with some scheduler jitter on top.
            let delay = if indexer.id == 1 { 200 } else { 10 };
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            (indexer, Vec::new())
        })
        .await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(210),
            "fanout should be parallel (<210ms), took {elapsed:?}"
        );
        // Priority 1 first, 50 second — not fastest-first.
        assert_eq!(out[0].0.id, 1, "priority 1 must come first");
        assert_eq!(out[1].0.id, 2, "priority 50 must come second");
    }
}
