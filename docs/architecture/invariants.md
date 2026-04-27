# Invariants

Invariants are predicates over kino's state that must always hold.
Tests catch the bugs you anticipate; **invariants catch the ones
you don't**. Each one is a small async function:

```rust
pub async fn check(pool: &SqlitePool) -> sqlx::Result<Vec<Violation>>;
```

It returns zero violations when the predicate holds, one per
offending row when it doesn't. Each violation carries enough context
(entity id, what was wrong) for a human or an auto-repair routine to
act on.

## Surface

The built-in suite is a closed set enumerated by `StandardInvariant`:

```rust
pub enum StandardInvariant {
    ImportedHasMedia,
    ActiveDownloadHasTorrent,
    BlocklistHashesNormalized,
    ShowHasSeasons,
    MediaHasOwner,
    StuckPartialFollow,
}

pub async fn check_all(pool: &SqlitePool) -> sqlx::Result<InvariantReport>;
```

`check_all` runs every variant in declaration order and aggregates
into `InvariantReport { passed, violations }`. `log_violations`
emits one `tracing::warn!` per violation with structured fields.

The trait uses native AFIT, so it isn't object-safe — the suite
dispatches via the enum's `match` arms. Adding a new invariant is
three changes the compiler enforces: a submodule, an enum variant,
and the three match arms (`name`, `description`, `check`).

## Where they run

1. **Tests.** Flow tests assert `check_all` passes after the
   scenario completes; new code that violates an invariant fails CI.
2. **Continuous reconciliation** (next phase). The scheduler ticks
   `check_all` on a fixed cadence and routes violations to the
   health surface. Whitelisted invariants get auto-repair; the
   rest surface for admin attention.
3. **On demand.** A `kino check-invariants` CLI subcommand for
   operators (planned).

## The starter set

### `imported_has_media`
Every `download` row with `state = 'imported'` has at least one
linked media row (movie via `media.movie_id`, episode via
`media_episode`). A violation means the import path committed the
state transition without persisting the media row — playback would
later 404.

### `active_download_has_torrent`
Every `download` row whose state is in
`DownloadPhase::needs_startup_reconcile` has a non-empty
`torrent_hash`. A violation means the row claims an active or
recoverable download but holds no handle to the torrent client.

### `blocklist_hashes_normalized`
Every `blocklist.torrent_info_hash` value is lowercase. The match
path compares case-insensitively, but a row written with mixed case
still trips equality-only consumers. Auto-repair candidate.

### `show_has_seasons`
Every `show` row has at least one `series` (season) row. A show with
no seasons is the result of a follow that crashed between the show
INSERT and the series fan-out — orphaned from the user's perspective.

### `media_has_owner`
Every `media` row links to either a movie (via `media.movie_id`) or
to at least one episode (via `media_episode`). Pure-DB check. The
filesystem-side orphan scan (file on disk with no DB row) lives in
`cleanup::orphan_file_scan`.

### `stuck_partial_follow`
Every `show` row with `partial = 1` was added within the last
~10 minutes. A row stuck partial longer than that is the result of
a follow whose season-fanout crashed mid-flight; it's invisible to
all user-facing reads (which filter `partial = 0`) but still
present, so the operator needs to know to retry or delete it.

## How invariants relate to other patterns

- **State machines** declare what transitions are valid. Invariants
  declare what facts must hold *between* transitions.
- **Operations** must leave invariants holding when they commit.
  An operation that breaks an invariant is a bug.
- **CleanupTracker** retries operations that left an invariant
  broken (e.g. removed-from-DB-but-not-from-librqbit).
- **Reconciliation** is invariants-as-actions: when an invariant
  fails, the reconciler tries to repair it.

## Adding an invariant

When fixing a "the database said X but reality was Y" bug:

1. Write the invariant first; ensure it fails on the bug.
2. Fix the bug.
3. Verify the invariant now passes.
4. Add the submodule + enum variant + match arms.

The bug class can't recur silently; the invariant catches any
future regression.

## Cost

Each invariant: ~10 lines of code, runs in ~10 ms on a healthy DB.
Even with twenty invariants, the periodic sweep takes ~200 ms.
