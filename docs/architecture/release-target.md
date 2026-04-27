# ReleaseTarget

The `ReleaseTarget` trait lets release-pickup, blocklist checks,
search-debounce stamps, and active-download lookup work uniformly
against `Movie` and `Episode`. It exists to collapse the symmetric
pairs of code that the codex review repeatedly flagged as a bug
source — the kind of duplication where one half gets fixed and the
other rots silently.

## The bug class it closes

A representative case from the codex review:

> "On a manual blocklist event for a movie, we clear
> `last_searched_at` so the wanted-search picks it up immediately
> on the next sweep. The episode path doesn't do this — episodes
> stay stuck on the 24h debounce, so a re-blocklisted episode
> keeps trying the same blocklisted release for a day."

The fix isn't to copy the behaviour to the episode path: the next
shared step will land the same way. The fix is to make the two
paths *the same path*, expressed once.

## Surface

```rust
pub trait ReleaseTarget: Send + Sync {
    fn kind(&self) -> ReleaseTargetKind;
    fn id(&self) -> i64;
    fn target_title(&self) -> &str;
    fn target_year(&self) -> Option<u16>;

    async fn load_blocklist(&self, pool: &SqlitePool)
        -> sqlx::Result<Vec<BlocklistEntry>>;
    async fn stamp_searched(&self, tx: &mut Transaction<'_, Sqlite>)
        -> sqlx::Result<()>;
    async fn clear_search_stamp(&self, tx: &mut Transaction<'_, Sqlite>)
        -> sqlx::Result<()>;
    async fn current_active_download(&self, pool: &SqlitePool)
        -> sqlx::Result<Option<Download>>;
}
```

Five methods carry actual polymorphism; `kind`/`id`/`target_title`/
`target_year` are sync getters that exist so generic code can log /
branch without an extra trait.

`BlocklistEntry::matches_release(hash, title)` is the single source
of truth for "is this release blocklisted" — both paths used to
inline the same hash-or-title check, sometimes diverging on
case-sensitivity. Centralised here.

## What's intentionally NOT on the trait

- **Full display title** ("Show · S01E02 · Title") — composing this
  for an episode requires a JOIN to the parent show. Callers use
  the existing `events::display::episode_display_title` helper.
  Keeping it off the trait avoids forcing every method to be async
  + take a pool.
- **Score profile + scoring** — Phase A.4's
  `AcquisitionPolicy::evaluate` will be the unified scoring entry
  point; the trait stays focused on identity + state.
- **Grab semantics** (season-pack handling, etc) — Phase B
  migration will decide whether grab is a trait method or a free
  function that takes `impl ReleaseTarget`.

## Generic, not `dyn`

The polymorphic call sites (search loops, grab paths, watch-now
pickup) take a single typed target — the caller already knows
whether it's holding a `Movie` or an `Episode`. Generics avoid
the boxing + vtable overhead `dyn` would impose, and let us use
native AFIT (no `async_trait` macro). If a use case appears that
genuinely needs a heterogeneous `Vec<Box<dyn ReleaseTarget>>`,
revisit then.

The cost of this choice: the trait is not object-safe today
(`impl Future` return types). To make it object-safe, switch to
`Pin<Box<dyn Future>>` or pull in `async_trait`. We'll cross that
bridge if we ever need it.

## Adding a new impl

If we ever ship a third release target (e.g. an artist for a music
library), the steps are:

1. Add a variant to `ReleaseTargetKind`.
2. `impl ReleaseTarget for Artist`.
3. Add fixture-based tests for every method, using the same DB-row
   pattern as `release/target.rs`'s test module.
4. Audit any site that calls `target.kind()` inside a `match` —
   the compiler will flag those as non-exhaustive automatically.

The same disciplines that apply to a state-machine enum apply to
the trait: classification methods over scattered `matches!()`
chains; pinned-set tests for the kind enum; never bypass the trait
and reach for the underlying row's column directly from a
polymorphic call site.

## Cross-references

- [`state-machines.md`](./state-machines.md) — `DownloadPhase`
  drives the "active states" filter inside
  `current_active_download`.
- [`operations.md`](./operations.md) — `stamp_searched` /
  `clear_search_stamp` take a transaction because the stamp must
  commit atomically with the rest of the search operation; this
  is the operation pattern at work.
- [`consistency-model.md`](./consistency-model.md) — the trait
  enforces re-read-at-decision-time: every method goes back to the
  DB rather than trusting a previous snapshot.
