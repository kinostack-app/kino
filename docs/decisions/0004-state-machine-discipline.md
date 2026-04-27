# ADR-0004: Explicit state machines for long-lived entities

**Status:** accepted
**Date:** 2026-04-25

## Context

The 2026-04-25 codex review identified ~12 bugs that all share the
same shape: **scattered code paths disagree on what a state means**.
Examples:

- `download.state = 'imported'` is "terminal" to the runtime
  monitor, "needs source-file removal" to startup, "needs seed-limit
  enforcement" to the cleanup sweep — three different opinions.
- `download.state = 'cleaned_up'` is "terminal" to most code but
  still serves as a playback source via `lookup_active_download`.
- Watch-now phase tracking lives in a side-channel
  `Mutex<HashMap<i64, ()>>` with no persisted column; cancellation
  races the in-memory state.

Every site does its own `match download.state.as_str()` and
forgets one of the 12 values.

## Decision

**Every long-lived entity gets an explicit `enum` for its state, plus
classification methods that serve specific consumers.** Code calls
the methods; never re-implements the classification.

Four state machines are explicit:

1. `DownloadPhase` (12 values, 38 sites)
2. `WatchNowPhase` (replaces ad-hoc HashMap tracking)
3. `TranscodeSessionState` (live / dead / idle / reaped)
4. `ScrobbleState` (already mostly modelled)

Methods like `is_runtime_monitored()`, `is_streamable()`,
`needs_seed_limit_check()`, `is_terminal()` provide the
classification. Adding a new state = update one file; the
compiler's exhaustiveness check forces every site to react.

The macro state machine (the user's view of a content entity:
unfollowed → wanted → searching → ... → watched) is **derived**,
not stored, and lives in a single function (`derived_state.rs`).
No subsystem invents its own categorisation.

## Alternatives considered

- **`String` columns + ad-hoc `match`.** What we have today. The
  bug source.
- **Typestate pattern (compile-time state in the type).** Would
  catch even more at compile time, but requires every operation to
  consume + return the typed value. Massive call-site churn for
  marginal benefit. Skipped.
- **A state-machine library / DSL.** Adds a dependency for what's
  fundamentally `enum + impl`. Overkill.

## Consequences

- **Win:** the bug shape "two sites disagree on what a state means"
  becomes impossible by construction. New code calls the same method
  every other site does.
- **Win:** adding a state is a single-file edit + compiler-driven
  audit of all match sites. No more "I forgot to update the cleanup
  sweep."
- **Win:** the doc in `architecture/state-machines.md` becomes the
  authoritative reference. Code that disagrees is a bug.
- **Cost:** initial migration touches every site that currently
  `match`es state strings. Done once, in the architectural refactor
  (see `REFACTOR.md` Phase 1).
- **Cost:** wire format must remain stable across the migration.
  `as_str()` returns exactly the strings the existing SQL filters
  expect. No data migration needed.

## Supersedes

None — codifies a decision the codebase had been evolving away from.
