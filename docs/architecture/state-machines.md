# State machines

Kino models several long-lived entities as explicit state machines.
Each machine has a single canonical enum + methods that classify
its values for different consumers. No subsystem is allowed to
invent its own categorisation.

This doc is the canonical reference. If code disagrees with this
doc, the doc wins — fix the code.

## The macro state (user-visible)

What the user sees in the UI per content entity:

```
unfollowed → wanted → searching → acquiring → imported → playing → watched → cleaned-up
                                                  ↓ ↓ ↓
                                          (or seeding indefinitely / failed / discarded)
```

Nine states. Derived (not stored) by joining `movie`/`episode` +
`download` + `media` + watched flag. The derivation lives in
`content::derived_state`; every read-side surface that classifies a
content row goes through it. Code that needs the macro state never
re-implements the derivation.

## Sub-machine 1: Download lifecycle

`enum DownloadPhase` on `download.state`. Twelve values; methods
classify them for each consumer.

```
Searching → Queued → Grabbing → Downloading → Stalled
                                       ↓
                                   Completed → Importing → Imported → Seeding → CleanedUp
                                       ↓
                                    Failed | Cancelled | Paused
```

| Method | Purpose | Returns true for |
|---|---|---|
| `is_runtime_monitored()` | Should the per-tick monitor poll? | Searching, Queued, Grabbing, Downloading, Stalled, Completed, Importing, Imported, Seeding |
| `needs_startup_reconcile()` | Should startup re-anchor against librqbit? | Grabbing, Downloading, Stalled, Paused, Completed, Importing, Imported, Seeding |
| `is_streamable()` | Can a `<video>` source be served from this row? | Downloading, Stalled, Completed, Importing, Imported, Seeding |
| `needs_seed_limit_check()` | Subject to ratio/time enforcement? | Imported, Seeding |
| `is_terminal()` | No further transitions; safe to delete | CleanedUp, Failed, Cancelled |

**Codex bugs closed by enforcing these:** #10 (completed stranded),
#22 (finished as seeding), #29 (terminal masquerades as source),
#37 (imported skips seed-limit), #42 (paused not reconciled).

## Sub-machine 2: Watch-now phase

`enum WatchNowPhase` on the same `download` row, separate column.
Replaces today's ad-hoc `phase_two_downloads: Mutex<HashSet>` +
`watch_now_lock`.

```
PhaseOne (initial release pick) → PhaseTwo (background alternate-release loop) → Settled
                                       ↓
                                   Cancelled (user explicitly cancelled)
```

Why on the row, not in memory: cancellation must persist across a
crash. The current HashMap-based tracking is racy because the
in-memory state can disagree with the DB (codex #7, #8).

**Codex bugs closed:** #7 (cancel race), #8 (retry suppression
timing).

## Sub-machine 3: Transcode session

`enum TranscodeSessionState` on the in-memory session struct.
**In-memory only** — the session dies with the process; no DB column.

```
Active ──producer ahead──▶ Suspended ──SIGCONT──▶ Active
  │
  ├──HW failure + more rungs──▶ Respawning ──spawn ok──▶ Active
  │
  ├──HW failure + chain exhausted──▶ Failed
  │
  ├──clean exit──▶ Exited
  │
  └──user stop / watchdog kill──▶ Cancelled
```

`has_session()` / `session_master_playlist()` must check the live
state, not just "row in HashMap exists". Segment requests serve
only when `is_running()` (Active or Suspended). Terminal states
evict immediately and the temp dir is cleared.

**Naming note:** the variants chosen during implementation map 1:1
onto the existing `is_suspended: bool` + `child.try_wait()`
inference; the previous sketch (`Spawning / Live / Idle / Reaped /
Dead`) invented a `Spawning` step the code doesn't have (the
session struct doesn't exist before spawn) and conflated `Reaped`
with `Dead`.

**Codex bug closed:** #43 (dead transcode kept alive by HLS recovery).

## Sub-machine 4: Trakt scrobble

`enum ScrobbleState` per queued scrobble — the lifecycle of the
`trakt_scrobble_queue` row, not Trakt's `start/pause/stop` verbs
(those are payload).

```
Pending ─emit attempt─▶ InFlight ─2xx─▶ Sent
   ▲                        │
   │                        ├─error─▶ Failed (re-queued)
   │                        │
   └─────────retry──────────┘

Pending / Failed ─stale (>5min start, >24h any)─▶ Dropped
Pending ─Trakt disconnected / scrobble disabled─▶ Skipped
```

`Pending`, `InFlight`, `Failed` are non-terminal. `Sent`, `Dropped`,
`Skipped` are terminal. Predicates: `is_drain_eligible()` (Pending
or Failed), `succeeded()` (only Sent), `is_user_visible_problem()`
(only Dropped — `Skipped` is "user turned Trakt off", not a problem).

**Naming note:** the previous sketch (`NotStarted / Started /
Paused / Stopped`) modelled Trakt's verbs, not the queue row's
state. Verbs are the payload of an emission; state is whether the
emission is queued / in-flight / done.

## Rules for adding states / transitions

1. **Never use the raw string in code.** Always go through the enum.
2. **Add the state to every `match` site.** The enum is non-exhaustive
   in spirit but exhaustive at the compiler level — add a state =
   compiler tells you every site to update.
3. **Add a method, not a `matches!()` chain.** If three sites need
   the same predicate (`is_active_for_X`), add it as a method on
   the enum once.
4. **Document the transition.** Update this doc with the new state
   and any allowed transitions in/out.
5. **Reconcile externally if state crosses a system boundary.** Any
   transition that depends on librqbit / disk / TMDB needs a
   reconciliation path. See `architecture/consistency-model.md`.

## Anti-patterns this prevents

- **Scattered match sites disagreeing.** The bug class behind half
  the codex findings.
- **Side-channel state in HashMaps.** `phase_two_downloads` is the
  current example; replaced by an explicit column.
- **String-comparing state in SQL.** `state IN ('queued', 'grabbing')`
  is fine in queries (text values match the enum); building filter
  strings dynamically from a method on the enum is preferred for
  any non-trivial set.
