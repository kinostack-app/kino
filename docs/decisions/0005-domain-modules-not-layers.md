# ADR-0005: Domain modules, not layer modules

**Status:** accepted
**Date:** 2026-04-25

## Context

The crate grew with a layer-cake organisation: `api/` for HTTP
handlers, `services/` for business logic, `models/` for DB structs,
`events/` for the event bus. By 2026-04-25:

- `services/` contains 14 unrelated files (search, import, metadata,
  monitor, intro skipper, logos, opensubtitles, indexer health, hw
  probe, log retention, trickplay gen + stream + probe). 7000+ lines
  of "stuff that's not a handler and not a model". Dumping ground.
- `api/play.rs` is 3700 lines: prepare, direct, HLS master, HLS
  variant, HLS segment, subtitle, trickplay, progress. Six features.
- `api/shows.rs` is 2200 lines: CRUD, monitor controls, episode
  acquire, season management, redownload.
- An "operation" like Grab spans `api/releases.rs` (handler) +
  `services/search.rs` (logic) + `events/mod.rs` (event) +
  `models/download.rs` (struct) — four files in three different
  trees.

The codex review identifies "symmetry asymmetry" (movie path drifts
from episode path, single-file path drifts from season-pack path)
as a recurring bug source. With the current layout, symmetric
operations live in different modules and routinely drift.

## Decision

**Reorganise around domain modules.** Each domain owns its types,
queries, operations, invariants, and HTTP handlers in one tree:

```
domain/
├── mod.rs          — public types, traits
├── <feature>.rs    — operations, query helpers
├── handlers.rs     — thin Axum handlers calling operations
└── invariants.rs   — domain-local invariants
```

Domains are derived from the actual concepts in the system:
`content/`, `acquisition/`, `download/`, `import/`, `playback/`,
`watch_now/`, `metadata/`, `library/`, `home/`, `auth_session/`,
`settings/`, plus existing leaf modules that already have good
cohesion (`indexers/`, `torznab/`, `parser/`, `notification/`,
`observability/`, `scheduler/`, `events/`, `integrations/`).

Full layout in [`../architecture/crate-layout.md`](../architecture/crate-layout.md).

## Alternatives considered

- **Keep the layer-cake.** Status quo. Bugs that cluster at module
  boundaries keep recurring.
- **Multi-crate workspace.** Codex earlier suggested extracting
  `kino-playback-core`. Adds cross-crate visibility games for no
  win at our scale; skipped.
- **DDD-style aggregate roots with behaviour-bearing entities.**
  Too much abstraction. Domain modules + free functions/operations
  give us the same colocation without the framework.
- **Hexagonal / ports + adapters.** Same.

## Consequences

- **Win:** symmetric movie/episode logic colocated under
  `content/`. Drift becomes a compile error or a one-line diff
  away.
- **Win:** operations live next to their model. New contributor
  finds everything for "Grab a release" in `acquisition/grab.rs`,
  not five files in three trees.
- **Win:** `services/` ceases to exist. No more "where does this
  go" debates.
- **Win:** big files split. `api/play.rs` becomes
  `playback/source.rs` + `playback/direct.rs` + `playback/hls/*.rs`.
  Each ~300-500 lines.
- **Win:** new domains (when we add music or podcasts) get their
  own tree without arguing about where things live.
- **Cost:** one-time massive `git mv` diff. Touches every import
  in every file.
- **Cost:** in-flight branches need rebase. None right now.
- **Mitigation:** the restructure landed as pure module moves
  (no logic changes), independent of behaviour-changing commits,
  so it bisects cleanly.

## Supersedes

None.
