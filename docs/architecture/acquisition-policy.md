# AcquisitionPolicy

`AcquisitionPolicy::evaluate` is the single source of truth for
"should we grab this release for this target?". It returns a typed
`Decision::{Accept{score}, Reject{reason: RejectReason}}` — never a
silent skip, never a magic score sentinel.

## The bug class it closes

The codex review caught two flavours of bug repeatedly:

1. **Asymmetric filters.** Movies route every release through
   `language_acceptable()`; episodes had a near-identical inline
   check that diverged on the "untagged-counts-as-English" edge
   case. A release tagged only as `multi` was rejected by movies
   but accepted by episodes.
2. **Silent skips.** `if blocklisted { continue; }` swallows the
   reason; admin UIs and logs both lose visibility into *why* a
   search returned no usable releases. The user sees "no releases
   found" when in fact 12 were rejected by blocklist.

A typed `Decision` collapses both: every site that picks releases
calls `evaluate`, and every reject carries a structured reason that
admin UI / logs / future histograms can read.

## Surface

```rust
pub enum Decision {
    Accept { score: i64 },
    Reject { reason: RejectReason },
}

pub enum RejectReason {
    Blocklisted,
    LanguageMismatch,
    DisallowedTier,
    UnknownTier,
    EpisodeTargetMismatch,
    NotAnUpgrade,
    ExceedsCutoffTier,
}

pub struct ReleaseCandidate<'a> { /* parsed metadata + DB fields */ }
pub struct PolicyContext<'a, T: ReleaseTarget> { /* target + profile + blocklist + upgrade */ }

impl AcquisitionPolicy {
    pub fn evaluate<T: ReleaseTarget>(
        release: &ReleaseCandidate<'_>,
        ctx: &PolicyContext<'_, T>,
    ) -> Decision { ... }
}
```

`ReleaseCandidate` is a narrow view: caller builds it from the
parsed release + DB row. The policy stays independent of the exact
`Release` / `ParsedRelease` shape, which keeps unit tests cheap
(no DB row construction) and lets the parser shape evolve without
touching the policy.

## Order of checks (cheap → expensive)

1. **Blocklist** — in-memory iter over already-loaded entries.
2. **Language gate** — empty profile filter or empty release
   languages = pass; otherwise must intersect.
3. **Tier in profile** — `UnknownTier` if missing.
4. **Tier allowed** — `DisallowedTier` if the user disabled it.
5. **Episode target match** — only when `target.kind() == Episode`.
6. **Compute score** — base `tier.rank * 1000`, plus PROPER/REPACK
   bonuses, log10-shaped seeder bonus, season-pack boost.
7. **Upgrade gate** — only when `ctx.existing` is `Some`.
   `ExceedsCutoffTier` if existing already at-or-above cutoff,
   else `NotAnUpgrade` if score isn't strictly higher.

The order matters: blocklist is the most opinionated user signal
("never grab this thing"), so it short-circuits before any
score-derived decision.

## Two deliberate behaviour changes vs the legacy scorer

### 1. Disallowed tiers are a hard reject

Legacy scorer applied a `-500` penalty to `allowed: false` tiers
but still accepted them if they were the only candidate. Edge case
that bit users: with a remux-only profile, if every higher tier
was rejected by language, a `cam` release would land "as a
fallback". The user marked it disallowed for a reason; accepting
it anyway because nothing else was available is a footgun.

New behaviour: `allowed: false` → `RejectReason::DisallowedTier`,
period. If the user wants the tier, they can flip its `allowed`
flag.

### 2. Score equality is not an upgrade

Legacy code had a couple of paths that compared `>` and others
that compared `>=`. The `>=` paths led to a flutter where the same
search would re-grab the existing release on every sweep
(accepted as "upgrade", but byte-identical to what we already
have). `evaluate` is uniformly strict-greater.

## Scope: per-release decision only

`evaluate` decides about *one* release. Selection from a candidate
list (top-1 by score, with tie-breakers) happens at the caller —
that's the search loop's responsibility, not the policy's.

Same for "we found zero acceptable releases" — that's an outcome
of running the policy over zero or more candidates, not a
per-release reject reason.

## Call sites

`evaluate` is the single gate every grab-decision path runs through:

- `acquisition::search::movie` — inner scoring loop.
- `acquisition::search::episode` — inner scoring loop + episode-mismatch gate.
- `watch_now` phase-2 release picker reads `release.quality_score`,
  populated upstream by the search path's `evaluate` call.

## Known limitation: episode target match

The `EpisodeTargetMismatch` check currently only verifies the
parser found *any* season/episode info — the actual season/episode
comparison still runs in the search loop because `ReleaseTarget`
deliberately doesn't expose `season_number` / `episode_number`
(see `release-target.md`). Phase B will either:

- widen the trait with an `episode_target_match()` helper that
  takes the parsed numbers, or
- move the comparison entirely upstream so `evaluate` sees only
  pre-filtered candidates.

Documented here because the test `EpisodeTargetMismatch` reject
won't actually fire from `evaluate` until that gap closes.

## Cross-references

- [`release-target.md`](./release-target.md) — `ReleaseTarget`
  trait that `evaluate` consumes; `BlocklistEntry::matches_release`
  is the centralised blocklist check.
- [`state-machines.md`](./state-machines.md) — `RejectReason` is a
  state-machine-style enum: classification methods, pinned via
  serialise-round-trip tests; adding a variant forces every match
  site to react.
- [`operations.md`](./operations.md) — `evaluate` is a pure
  function; it sits inside the "validate preconditions" step of
  the search/grab operation. No DB writes, no side effects.
