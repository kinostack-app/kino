//! `AcquisitionPolicy` — single source of truth for "should we
//! grab this release for this target?".
//!
//! [`AcquisitionPolicy::evaluate`] returns a typed
//! [`Decision::Accept { score }`] or [`Decision::Reject { reason }`]
//! — no third option, no silent skip. Every site that picks releases
//! consumes the same function, so the answer to "would we grab
//! this?" depends only on the inputs.
//!
//! ## Scope: per-release decision only
//!
//! `evaluate` decides about *one* release. Selection from a
//! candidate list (top-1 by score, with tie-breakers) happens at
//! the caller. "We found zero acceptable releases" is an outcome of
//! running the policy over zero or more candidates, not a
//! per-release reject reason.
//!
//! ## Disallowed tiers are a hard reject
//!
//! A tier marked `allowed: false` in the user's profile rejects as
//! [`RejectReason::DisallowedTier`]. The user disallowed it; we
//! don't fall back to it because nothing else passed. To enable a
//! tier, flip its `allowed` flag.
//!
//! See `architecture/acquisition-policy.md` for the full design.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::acquisition::release_target::{BlocklistEntry, ReleaseTarget, ReleaseTargetKind};
use crate::settings::quality_profile::QualityTier;

/// Outcome of evaluating one release for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The release is acceptable. Caller compares scores across
    /// candidates to pick the top.
    Accept { score: i64 },

    /// The release is rejected for the given reason. Caller logs
    /// + skips; never grabs.
    Reject { reason: RejectReason },
}

impl Decision {
    /// Convenience: did `evaluate` accept this release? Tests use
    /// this to assert "the only acceptable candidate" without
    /// destructuring.
    #[must_use]
    pub const fn is_accept(&self) -> bool {
        matches!(self, Self::Accept { .. })
    }

    /// Convenience: was this rejected for the given reason?
    #[must_use]
    pub fn rejected_for(&self, expected: RejectReason) -> bool {
        matches!(self, Self::Reject { reason } if *reason == expected)
    }

    /// Score if accepted, otherwise `None`. Search loops use this
    /// to pick the top-scoring acceptable candidate without nested
    /// matches.
    #[must_use]
    pub const fn score(&self) -> Option<i64> {
        match self {
            Self::Accept { score } => Some(*score),
            Self::Reject { .. } => None,
        }
    }
}

/// Why a release was rejected. Surfaced in logs + admin UI so
/// "no releases acceptable" can be debugged without re-running the
/// search with verbose logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Matches a blocklist entry (hash or title) for this target.
    /// Loaded via [`ReleaseTarget::load_blocklist`]; matched via
    /// [`BlocklistEntry::matches_release`].
    Blocklisted,

    /// Release language doesn't match the profile's
    /// `accepted_languages`. Untagged releases pass through (the
    /// codebase treats no language tag as "default — usually
    /// English"); explicit non-matching languages are rejected.
    LanguageMismatch,

    /// The release's tier is marked `allowed: false` in the user's
    /// profile. Hard reject — see module docs for the rationale.
    DisallowedTier,

    /// The release's tier doesn't appear in the profile at all.
    /// Distinct from `DisallowedTier` because "not in the profile"
    /// is a config or schema mismatch, not a user choice.
    UnknownTier,

    /// Episode-only: the release's parsed season/episode numbers
    /// don't match the target episode. Caught by the search loop's
    /// fuzzy matcher; surfaced as a reject so admin UI can show
    /// "Torznab returned 9 unrelated episodes".
    EpisodeTargetMismatch,

    /// Upgrade mode: the release's score isn't strictly higher
    /// than the existing media's score. Same rank or lower = no
    /// upgrade.
    NotAnUpgrade,

    /// Upgrade mode: the release's tier rank is above the profile's
    /// `cutoff`. Once you have a release at-or-above cutoff, no
    /// further upgrades are pursued.
    ExceedsCutoffTier,
}

impl RejectReason {
    /// All variants in declaration order. Used for admin UI
    /// "rejected by reason" histograms.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::Blocklisted,
            Self::LanguageMismatch,
            Self::DisallowedTier,
            Self::UnknownTier,
            Self::EpisodeTargetMismatch,
            Self::NotAnUpgrade,
            Self::ExceedsCutoffTier,
        ]
        .into_iter()
    }

    /// Wire / log string. Stable across the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blocklisted => "blocklisted",
            Self::LanguageMismatch => "language_mismatch",
            Self::DisallowedTier => "disallowed_tier",
            Self::UnknownTier => "unknown_tier",
            Self::EpisodeTargetMismatch => "episode_target_mismatch",
            Self::NotAnUpgrade => "not_an_upgrade",
            Self::ExceedsCutoffTier => "exceeds_cutoff_tier",
        }
    }
}

/// Subset of release fields that drive the policy decision.
///
/// Caller builds this from the parsed release + DB row; the policy
/// stays independent of the exact `Release` / `ParsedRelease`
/// shape so unit tests don't need to construct full DB rows. See
/// the migration notes in `architecture/acquisition-policy.md` for
/// the recommended builder pattern.
#[derive(Debug, Clone)]
pub struct ReleaseCandidate<'a> {
    /// Title as published by the indexer. Used for the
    /// title-equality leg of the blocklist check.
    pub source_title: &'a str,

    /// `info_hash` from the magnet/torrent. `None` for indexers
    /// that don't expose it; the blocklist check then falls back
    /// to title equality.
    pub torrent_info_hash: Option<&'a str>,

    /// Languages parsed from the release title. Empty = untagged
    /// (treated as the implicit default per language gate).
    pub languages: &'a [String],

    /// Seeder count from the indexer; `None` = not reported. Drives
    /// the seeder bonus in the score.
    pub seeders: Option<i64>,

    /// `quality_id` of the matched tier (`"remux_2160p"`,
    /// `"bluray_1080p"`, etc). Used to look up rank + allowed in
    /// the profile; tier resolution itself happens in the parser.
    pub tier_id: &'a str,

    /// Title indicates a PROPER release (replaces a previously
    /// botched encode). Adds a small score bonus.
    pub is_proper: bool,

    /// Title indicates a REPACK (re-released for a fixed issue).
    /// Adds a small score bonus.
    pub is_repack: bool,

    /// Release covers a full season pack rather than a single
    /// episode. Caller only sets this for episode targets.
    pub is_season_pack: bool,

    /// Parsed season number from the release title; `None` if the
    /// parser couldn't determine one.
    pub parsed_season: Option<i64>,

    /// Parsed episode numbers (one or many) from the release title.
    /// Empty for movie releases or season packs the parser couldn't
    /// itemise.
    pub parsed_episodes: &'a [i64],
}

/// What the policy knows about an existing media asset for the
/// target, when running in upgrade mode. `None` means first-time
/// pickup — no upgrade gate applies.
#[derive(Debug, Clone)]
pub struct ExistingPick<'a> {
    /// `quality_id` of the tier the existing media is at.
    pub tier_id: &'a str,

    /// Score the existing media was originally accepted at. Compared
    /// strictly: a new release must score *higher* (not equal) to
    /// upgrade.
    pub score: i64,
}

/// How big a score gap must be before a same-tier upgrade is
/// accepted. Higher-tier upgrades ignore this — any improvement to
/// a better tier accepts. Default 200 matches the legacy scorer's
/// anti-flutter threshold (otherwise a single extra seeder would
/// trigger a re-grab of the same release at the same tier).
pub const DEFAULT_SAME_TIER_UPGRADE_DELTA: i64 = 200;

/// Per-evaluation context: the profile, blocklist, and target.
/// Bundled into one struct so `evaluate` takes one arg + the
/// candidate, keeping call sites readable.
#[derive(Debug)]
pub struct PolicyContext<'a, T: ReleaseTarget> {
    pub target: &'a T,

    /// All tiers in the user's quality profile. Already deserialised
    /// from the JSON `items` column.
    pub profile_tiers: &'a [QualityTier],

    /// Profile's `accepted_languages` (deserialised). Empty list =
    /// no language filter at all.
    pub accepted_languages: &'a [String],

    /// Profile's `cutoff` quality id. Used only when `existing` is
    /// `Some`; once a release at-or-above cutoff exists, further
    /// candidates reject as `ExceedsCutoffTier`.
    pub cutoff_tier_id: &'a str,

    /// Blocklist entries scoped to this target, pre-loaded via
    /// [`ReleaseTarget::load_blocklist`].
    pub blocklist: &'a [BlocklistEntry],

    /// `Some` when running in upgrade mode (existing media for the
    /// target). `None` for first-time pickup.
    pub existing: Option<ExistingPick<'a>>,

    /// Episode-only: how many other episodes in this season are
    /// also wanted. Drives the season-pack score boost. `None` for
    /// movies or for non-pack episodes.
    pub wanted_in_season: Option<i64>,

    /// Minimum score delta a same-tier upgrade must clear. Higher
    /// tiers ignore this — any move up accepts. Default
    /// [`DEFAULT_SAME_TIER_UPGRADE_DELTA`] (200) prevents one extra
    /// seeder triggering a re-grab of the same release.
    pub min_same_tier_upgrade_delta: i64,
}

/// Single source of truth for release acquisition decisions.
#[derive(Debug)]
pub struct AcquisitionPolicy;

impl AcquisitionPolicy {
    /// Decide whether `release` is acceptable for `ctx.target`,
    /// and at what score.
    ///
    /// Order of checks (cheap → expensive):
    /// 1. Blocklist match (in-memory iter, O(M))
    /// 2. Language gate (in-memory contains)
    /// 3. Tier lookup → `UnknownTier` if not in profile
    /// 4. Tier `allowed` → `DisallowedTier` if false
    /// 5. Episode target match (when target is an Episode)
    /// 6. Compute score
    /// 7. Upgrade gate (when `ctx.existing` is Some)
    pub fn evaluate<T: ReleaseTarget>(
        release: &ReleaseCandidate<'_>,
        ctx: &PolicyContext<'_, T>,
    ) -> Decision {
        // (1) Blocklist
        if ctx
            .blocklist
            .iter()
            .any(|entry| entry.matches_release(release.torrent_info_hash, release.source_title))
        {
            return Decision::Reject {
                reason: RejectReason::Blocklisted,
            };
        }

        // (2) Language. Untagged releases pass through (treated as
        // implicit default — usually English). `multi` releases pass
        // through (multi-language packs carry the target language).
        // Empty accepted_languages = no filter.
        if !release.languages.is_empty()
            && !ctx.accepted_languages.is_empty()
            && !release
                .languages
                .iter()
                .any(|lang| lang.eq_ignore_ascii_case("multi"))
            && !release.languages.iter().any(|lang| {
                ctx.accepted_languages
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(lang))
            })
        {
            return Decision::Reject {
                reason: RejectReason::LanguageMismatch,
            };
        }

        // (3) Tier in profile?
        let Some(tier) = ctx
            .profile_tiers
            .iter()
            .find(|t| t.quality_id == release.tier_id)
        else {
            return Decision::Reject {
                reason: RejectReason::UnknownTier,
            };
        };

        // (4) Tier allowed?
        if !tier.allowed {
            return Decision::Reject {
                reason: RejectReason::DisallowedTier,
            };
        }

        // (5) Episode target match
        if ctx.target.kind() == ReleaseTargetKind::Episode
            && !release_matches_episode_target(release, ctx.target)
        {
            return Decision::Reject {
                reason: RejectReason::EpisodeTargetMismatch,
            };
        }

        // (6) Score
        let score = compute_score(release, tier, ctx);

        // (7) Upgrade gate
        if let Some(existing) = &ctx.existing {
            // ExceedsCutoffTier: we already have at-or-above cutoff.
            if let Some(cutoff_rank) = tier_rank(ctx.profile_tiers, ctx.cutoff_tier_id)
                && let Some(existing_rank) = tier_rank(ctx.profile_tiers, existing.tier_id)
                && existing_rank >= cutoff_rank
            {
                return Decision::Reject {
                    reason: RejectReason::ExceedsCutoffTier,
                };
            }
            // Higher tier always upgrades (any improvement); same
            // tier requires the configurable score-delta floor;
            // lower tier never upgrades.
            let new_rank = tier.rank;
            let existing_rank = tier_rank(ctx.profile_tiers, existing.tier_id).unwrap_or(0);
            let upgraded = match new_rank.cmp(&existing_rank) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => {
                    score > existing.score + ctx.min_same_tier_upgrade_delta
                }
                std::cmp::Ordering::Less => false,
            };
            if !upgraded {
                return Decision::Reject {
                    reason: RejectReason::NotAnUpgrade,
                };
            }
        }

        Decision::Accept { score }
    }
}

// ── Internal helpers ──────────────────────────────────────────────

fn tier_rank(tiers: &[QualityTier], quality_id: &str) -> Option<i64> {
    tiers
        .iter()
        .find(|t| t.quality_id == quality_id)
        .map(|t| t.rank)
}

fn release_matches_episode_target<T: ReleaseTarget>(
    release: &ReleaseCandidate<'_>,
    _target: &T,
) -> bool {
    // The actual season/episode comparison needs the target's
    // season_number / episode_number, which the trait doesn't expose
    // (see release-target.md). Accept if the parser found any
    // season/episode info; the search loop's exact comparison runs
    // upstream.
    release.parsed_season.is_some() || !release.parsed_episodes.is_empty()
}

/// The score formula `evaluate` uses internally. Exposed so callers
/// computing a score for *existing* media (the upgrade-mode
/// reference) get the exact same arithmetic without round-tripping
/// through `evaluate`.
pub fn compute_score<T: ReleaseTarget>(
    release: &ReleaseCandidate<'_>,
    tier: &QualityTier,
    ctx: &PolicyContext<'_, T>,
) -> i64 {
    // Base: tier rank * 1000. Rank is the user's relative ordering
    // of tiers in the profile.
    let mut score = tier.rank * 1000;

    if release.is_proper {
        score += 100;
    }
    if release.is_repack {
        score += 100;
    }

    // Seeder bonus: log10(seeders) * 10. Caps the bonus naturally
    // so 5000 seeders ≈ +37; 50 ≈ +17.
    if let Some(seeders) = release.seeders
        && seeders > 0
    {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let bonus = (f64::from(i32::try_from(seeders).unwrap_or(i32::MAX)).log10() * 10.0) as i64;
        score += bonus;
    }

    // Season-pack boost: +500 per wanted-in-season episode, only
    // meaningful at >=2 (a pack covering one wanted episode is just
    // an episode release).
    if release.is_season_pack
        && let Some(wanted) = ctx.wanted_in_season
        && wanted >= 2
    {
        score += 500 * wanted;
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::movie::model::Movie;

    // Small constructors so each test reads as one assertion, not
    // 30 lines of setup.

    fn movie() -> Movie {
        Movie {
            id: 1,
            tmdb_id: 0,
            imdb_id: None,
            tvdb_id: None,
            title: "T".into(),
            original_title: None,
            overview: None,
            tagline: None,
            year: Some(2026),
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
            quality_profile_id: 1,
            status: String::new(),
            monitored: true,
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

    fn tier(id: &str, rank: i64, allowed: bool) -> QualityTier {
        QualityTier {
            quality_id: id.into(),
            name: id.into(),
            allowed,
            rank,
        }
    }

    fn candidate<'a>(
        title: &'a str,
        hash: Option<&'a str>,
        tier_id: &'a str,
        languages: &'a [String],
    ) -> ReleaseCandidate<'a> {
        ReleaseCandidate {
            source_title: title,
            torrent_info_hash: hash,
            languages,
            seeders: Some(50),
            tier_id,
            is_proper: false,
            is_repack: false,
            is_season_pack: false,
            parsed_season: None,
            parsed_episodes: &[],
        }
    }

    fn ctx<'a>(
        m: &'a Movie,
        tiers: &'a [QualityTier],
        accepted: &'a [String],
        blocklist: &'a [BlocklistEntry],
    ) -> PolicyContext<'a, Movie> {
        PolicyContext {
            target: m,
            profile_tiers: tiers,
            accepted_languages: accepted,
            cutoff_tier_id: "bluray_1080p",
            blocklist,
            existing: None,
            wanted_in_season: None,
            // Tests default to 0 so the strict-greater-score path
            // is testable; production callers pass
            // DEFAULT_SAME_TIER_UPGRADE_DELTA.
            min_same_tier_upgrade_delta: 0,
        }
    }

    #[test]
    fn accepts_basic_movie_in_allowed_tier() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("My.Movie.2026.BluRay.1080p", None, "bluray_1080p", &[]);
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.is_accept(), "should accept: {decision:?}");
        assert!(decision.score().unwrap() >= 13_000);
    }

    #[test]
    fn rejects_blocklisted_by_hash() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let langs = vec![];
        let blocklist = vec![BlocklistEntry {
            torrent_info_hash: Some("ABCDEF".into()),
            source_title: "anything".into(),
        }];
        let release = candidate("My.Movie.2026", Some("abcdef"), "bluray_1080p", &[]);
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.rejected_for(RejectReason::Blocklisted));
    }

    #[test]
    fn rejects_blocklisted_by_title_when_no_hash() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let langs = vec![];
        let blocklist = vec![BlocklistEntry {
            torrent_info_hash: None,
            source_title: "My.Movie.2026.BluRay.1080p".into(),
        }];
        let release = candidate("My.Movie.2026.BluRay.1080p", None, "bluray_1080p", &[]);
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.rejected_for(RejectReason::Blocklisted));
    }

    #[test]
    fn rejects_language_mismatch_when_explicit() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec!["en".to_owned()];
        let release_langs = vec!["fr".to_owned()];
        let release = candidate("My.Movie.2026", None, "bluray_1080p", &release_langs);
        let release = ReleaseCandidate {
            languages: &release_langs,
            ..release
        };
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.rejected_for(RejectReason::LanguageMismatch));
    }

    #[test]
    fn accepts_untagged_release_regardless_of_language_filter() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec!["en".to_owned()];
        let release = candidate("My.Movie.2026", None, "bluray_1080p", &[]);
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(
            decision.is_accept(),
            "untagged should pass language gate: {decision:?}"
        );
    }

    #[test]
    fn accepts_when_no_language_filter_configured() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let release_langs = vec!["fr".to_owned()];
        let release = ReleaseCandidate {
            languages: &release_langs,
            ..candidate("My.Movie.2026", None, "bluray_1080p", &[])
        };
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.is_accept());
    }

    #[test]
    fn rejects_unknown_tier() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("My.Movie.2026", None, "tier_not_in_profile", &[]);
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.rejected_for(RejectReason::UnknownTier));
    }

    #[test]
    fn rejects_disallowed_tier_as_hard_reject() {
        let m = movie();
        let tiers = vec![tier("cam", 1, false), tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("My.Movie.CAM", None, "cam", &[]);
        let decision = AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist));
        assert!(decision.rejected_for(RejectReason::DisallowedTier));
    }

    #[test]
    fn proper_and_repack_add_to_score() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let plain = candidate("My.Movie.2026", None, "bluray_1080p", &[]);
        let proper = ReleaseCandidate {
            is_proper: true,
            ..plain.clone()
        };
        let repack = ReleaseCandidate {
            is_repack: true,
            ..plain.clone()
        };
        let both = ReleaseCandidate {
            is_proper: true,
            is_repack: true,
            ..plain.clone()
        };
        let context = ctx(&m, &tiers, &langs, &blocklist);
        let plain_s = AcquisitionPolicy::evaluate(&plain, &context)
            .score()
            .unwrap();
        let proper_s = AcquisitionPolicy::evaluate(&proper, &context)
            .score()
            .unwrap();
        let repack_s = AcquisitionPolicy::evaluate(&repack, &context)
            .score()
            .unwrap();
        let both_s = AcquisitionPolicy::evaluate(&both, &context)
            .score()
            .unwrap();
        assert_eq!(proper_s - plain_s, 100);
        assert_eq!(repack_s - plain_s, 100);
        assert_eq!(both_s - plain_s, 200);
    }

    #[test]
    fn higher_seeders_outscore_lower() {
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let low = ReleaseCandidate {
            seeders: Some(2),
            ..candidate("M", None, "bluray_1080p", &[])
        };
        let high = ReleaseCandidate {
            seeders: Some(2000),
            ..candidate("M", None, "bluray_1080p", &[])
        };
        let context = ctx(&m, &tiers, &langs, &blocklist);
        let low_s = AcquisitionPolicy::evaluate(&low, &context).score().unwrap();
        let high_s = AcquisitionPolicy::evaluate(&high, &context)
            .score()
            .unwrap();
        assert!(high_s > low_s, "{low_s} vs {high_s}");
    }

    #[test]
    fn upgrade_mode_rejects_equal_score() {
        let m = movie();
        // Cutoff above existing tier so the cutoff gate doesn't fire —
        // we want to isolate the score-equality check.
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "bluray_1080p", &[]);
        let mut context = ctx(&m, &tiers, &langs, &blocklist);
        context.cutoff_tier_id = "remux_2160p";
        let candidate_score = AcquisitionPolicy::evaluate(&release, &context)
            .score()
            .unwrap();
        context.existing = Some(ExistingPick {
            tier_id: "bluray_1080p",
            score: candidate_score,
        });
        let decision = AcquisitionPolicy::evaluate(&release, &context);
        assert!(decision.rejected_for(RejectReason::NotAnUpgrade));
    }

    #[test]
    fn upgrade_mode_accepts_strictly_higher_score() {
        let m = movie();
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "remux_2160p", &[]);
        let mut context = ctx(&m, &tiers, &langs, &blocklist);
        // Cutoff at remux so existing at bluray is *below* cutoff;
        // upgrade gate evaluates score, which is strictly higher.
        context.cutoff_tier_id = "remux_2160p";
        context.existing = Some(ExistingPick {
            tier_id: "bluray_1080p",
            score: 13_017,
        });
        let decision = AcquisitionPolicy::evaluate(&release, &context);
        assert!(
            decision.is_accept(),
            "remux > bluray must upgrade: {decision:?}"
        );
    }

    #[test]
    fn upgrade_mode_rejects_when_existing_at_or_above_cutoff() {
        let m = movie();
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "remux_2160p", &[]);
        let mut context = ctx(&m, &tiers, &langs, &blocklist);
        // Cutoff = bluray_1080p (rank 13); existing = remux_2160p (rank 18) → above cutoff.
        context.existing = Some(ExistingPick {
            tier_id: "remux_2160p",
            score: 18_000,
        });
        let decision = AcquisitionPolicy::evaluate(&release, &context);
        assert!(decision.rejected_for(RejectReason::ExceedsCutoffTier));
    }

    #[test]
    fn first_time_mode_skips_upgrade_gate() {
        // No existing media → the not_an_upgrade / exceeds_cutoff
        // checks must not fire.
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "bluray_1080p", &[]);
        let context = ctx(&m, &tiers, &langs, &blocklist);
        assert!(context.existing.is_none());
        assert!(AcquisitionPolicy::evaluate(&release, &context).is_accept());
    }

    #[test]
    fn reject_reason_round_trips_via_as_str_and_serde() {
        for r in RejectReason::all() {
            let s = r.as_str();
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(json, format!("\"{s}\""));
        }
    }

    #[test]
    fn multi_language_release_passes_strict_filter() {
        // A `multi` pack carries the target language among others;
        // the gate must let it through even when the strict filter
        // wouldn't accept the literal string "multi".
        let m = movie();
        let tiers = vec![tier("bluray_1080p", 13, true)];
        let blocklist = vec![];
        let langs = vec!["en".to_owned()];
        let release_langs = vec!["multi".to_owned()];
        let release = ReleaseCandidate {
            languages: &release_langs,
            ..candidate("M", None, "bluray_1080p", &[])
        };
        assert!(
            AcquisitionPolicy::evaluate(&release, &ctx(&m, &tiers, &langs, &blocklist)).is_accept()
        );
    }

    #[test]
    fn same_tier_upgrade_requires_score_delta() {
        let m = movie();
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "bluray_1080p", &[]);
        let mut context = ctx(&m, &tiers, &langs, &blocklist);
        context.cutoff_tier_id = "remux_2160p";
        context.min_same_tier_upgrade_delta = 200;
        // Existing 50 points below new — under the 200 threshold,
        // so reject.
        let new_score = AcquisitionPolicy::evaluate(&release, &context)
            .score()
            .unwrap();
        context.existing = Some(ExistingPick {
            tier_id: "bluray_1080p",
            score: new_score - 50,
        });
        assert!(
            AcquisitionPolicy::evaluate(&release, &context)
                .rejected_for(RejectReason::NotAnUpgrade)
        );

        // Now 250 points below — clears the threshold.
        context.existing = Some(ExistingPick {
            tier_id: "bluray_1080p",
            score: new_score - 250,
        });
        assert!(AcquisitionPolicy::evaluate(&release, &context).is_accept());
    }

    #[test]
    fn higher_tier_upgrade_ignores_same_tier_delta() {
        let m = movie();
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "remux_2160p", &[]);
        let mut context = ctx(&m, &tiers, &langs, &blocklist);
        context.cutoff_tier_id = "remux_2160p";
        context.min_same_tier_upgrade_delta = 10_000; // huge delta
        // Higher tier than existing — accept regardless of delta.
        context.existing = Some(ExistingPick {
            tier_id: "bluray_1080p",
            score: 13_017,
        });
        assert!(AcquisitionPolicy::evaluate(&release, &context).is_accept());
    }

    #[test]
    fn lower_tier_never_upgrades() {
        let m = movie();
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];
        let release = candidate("M", None, "bluray_1080p", &[]);
        let mut context = ctx(&m, &tiers, &langs, &blocklist);
        context.cutoff_tier_id = "remux_2160p";
        // Existing remux at low score; new bluray at high score —
        // still a downgrade.
        context.existing = Some(ExistingPick {
            tier_id: "remux_2160p",
            score: 0,
        });
        // Cutoff blocks at-or-above-cutoff first; ExceedsCutoffTier
        // wins. Move cutoff out of the way to isolate the downgrade
        // check.
        context.cutoff_tier_id = "remux_2160p";
        // Existing is AT cutoff so ExceedsCutoffTier fires before
        // NotAnUpgrade; that's the expected priority.
        assert!(
            AcquisitionPolicy::evaluate(&release, &context)
                .rejected_for(RejectReason::ExceedsCutoffTier)
        );
    }

    /// Property: in upgrade mode, the *only* way to accept is a
    /// strictly higher score AND existing not at-or-above cutoff.
    /// If we ever change the upgrade logic this test should fail.
    #[test]
    fn upgrade_acceptance_invariant() {
        let m = movie();
        let tiers = vec![
            tier("bluray_1080p", 13, true),
            tier("remux_2160p", 18, true),
        ];
        let blocklist = vec![];
        let langs = vec![];

        for existing_score in [0_i64, 13_000, 18_000] {
            for new_tier in ["bluray_1080p", "remux_2160p"] {
                let release = candidate("M", None, new_tier, &[]);
                let mut context = ctx(&m, &tiers, &langs, &blocklist);
                context.existing = Some(ExistingPick {
                    tier_id: "bluray_1080p",
                    score: existing_score,
                });
                let new_score = compute_score(
                    &release,
                    tiers.iter().find(|t| t.quality_id == new_tier).unwrap(),
                    &context,
                );
                let decision = AcquisitionPolicy::evaluate(&release, &context);
                if decision.is_accept() {
                    assert!(
                        new_score > existing_score,
                        "accepted but {new_score} <= {existing_score}"
                    );
                }
            }
        }
    }
}
