# Crate layout

The `kino` crate is organised by **domain**, not by **layer**.
Each domain owns its types, queries, operations, invariants, and
HTTP handlers in one tree. This keeps related code together and
makes symmetric operations (movie ↔ episode) hard to drift.

## Why domain modules instead of layer modules

The codebase grew with a layer-cake organisation:

```
api/      — HTTP handlers
services/ — business logic ("services/" is the dumping ground)
models/   — DB structs
events/   — event types + listeners
```

Result: `services/` became a 7000-line catch-all of search, import,
metadata, monitoring, intro skipping, logos, opensubtitles, etc —
none of which belong together. Operations spanned 3-4 files in
different folders; symmetry between movie/episode paths was hard
to maintain because they were nowhere near each other.

Domain modules co-locate everything for one concept:

```
acquisition/
├── policy.rs           — AcquisitionPolicy gate
├── release.rs          — Release model + scoring + /releases handlers
├── blocklist.rs        — Blocklist model + /blocklist handlers
├── grab.rs             — Grab operation
├── release_target.rs   — ReleaseTarget trait (shared movie/episode)
├── search/
│   ├── mod.rs          — search helpers
│   ├── movie.rs
│   ├── episode.rs
│   └── wanted_sweep.rs
└── mod.rs              — public surface, AcquisitionPolicy + RejectReason
```

Add a new policy rule? It's `acquisition/policy.rs`. Add a new
search type? Sibling file under `acquisition/search/`. Symmetric
movie/episode functions sit two lines apart.

## The full tree

```
crates/kino/src/
├── main.rs                 — wiring + Axum router assembly
├── state.rs                — AppState
├── auth.rs                 — auth middleware
├── error.rs                — AppError
├── init.rs                 — first-boot defaults
├── startup.rs              — server bootstrap
├── db.rs                   — pool helpers
├── time.rs                 — Timestamp newtype
├── clock.rs                — Clock trait + SystemClock + MockClock
├── pagination.rs           — PaginatedResponse<T>
├── images.rs               — image-proxy helpers
├── tests.rs                — in-module integration harness root
│
├── content/                — Movie + Show + Episode + Media
│   ├── mod.rs
│   ├── derived_state.rs    — phase derivation
│   ├── movie/              — model + handlers
│   ├── show/               — model + handlers + episode + episode_handlers + series
│   └── media/              — model + handlers
│
├── acquisition/            — search + grab + scoring + blocklist
│   ├── mod.rs              — AcquisitionPolicy + RejectReason
│   ├── policy.rs
│   ├── release.rs          — Release model, scoring, /releases handlers
│   ├── blocklist.rs        — Blocklist model + handlers
│   ├── grab.rs
│   ├── release_target.rs   — shared movie/episode trait
│   └── search/
│       ├── mod.rs
│       ├── movie.rs
│       ├── episode.rs
│       └── wanted_sweep.rs
│
├── download/               — torrent lifecycle + VPN
│   ├── mod.rs
│   ├── model.rs
│   ├── phase.rs            — DownloadPhase enum
│   ├── manager.rs
│   ├── monitor.rs
│   ├── session.rs          — TorrentSession trait
│   ├── torrent_client.rs   — librqbit impl
│   ├── handlers.rs
│   └── vpn/                — boringtun + port-forward
│
├── import/                 — completed download → library file
│   ├── mod.rs
│   ├── trigger.rs          — entry point + naming/materialise helpers
│   ├── single.rs           — one-file release path
│   ├── pack.rs             — season-pack path
│   ├── archive.rs / ffprobe.rs / naming.rs / pipeline.rs / transfer.rs
│   └── tests.rs
│
├── playback/               — direct + HLS + transcode + trickplay
│   ├── mod.rs              — PlayKind + re-exports
│   ├── decision.rs         — playback plan algorithm
│   ├── source.rs           — byte-source resolution
│   ├── handlers.rs         — prepare / direct / progress endpoints
│   ├── hls/
│   │   ├── master.rs
│   │   ├── variant.rs
│   │   └── segment.rs
│   ├── transcode.rs / transcode_state.rs / transcode_reason.rs
│   ├── trickplay.rs / trickplay_gen.rs / trickplay_stream.rs
│   ├── stream.rs / stream_model.rs / stream_probe.rs
│   ├── subtitle.rs
│   ├── intro_skipper.rs
│   ├── cast.rs / cast_token.rs
│   ├── progress.rs
│   ├── profile.rs / hwa_error.rs
│   ├── hw_probe.rs / hw_probe_cache.rs
│   ├── file_pick.rs / downmix.rs / chapter_model.rs / ffmpeg_bundle.rs
│   ├── probe_handlers.rs / watch_state.rs
│
├── watch_now/              — multi-phase grab-and-watch orchestration
│   ├── mod.rs
│   ├── phase.rs            — WatchNowPhase enum
│   └── handlers.rs
│
├── metadata/               — TMDB-driven enrichment
│   ├── mod.rs
│   ├── refresh.rs
│   ├── logos.rs
│   ├── image_handlers.rs / tmdb_handlers.rs / test_handlers.rs
│
├── tmdb/                   — TMDB API client (used by metadata + integrations)
│   ├── client.rs
│   └── types.rs
│
├── library/                — read-side queries spanning domains
│   ├── mod.rs              — search + calendar + stats
│   └── handlers.rs
│
├── home/                   — Home page composition
│   ├── mod.rs
│   ├── preferences.rs
│   └── handlers.rs
│
├── integrations/           — external services
│   ├── mod.rs
│   ├── opensubtitles.rs
│   ├── trakt/              — auth, client, sync, scrobble, reconcile, handlers
│   └── lists/              — MDBList / TMDB / Trakt list import
│
├── indexers/               — Cardigann engine (kino's built-in)
├── torznab/                — Torznab parser/client
├── parser/                 — release-name parsing
│
├── auth_session/           — sessions + signed URLs
│   ├── mod.rs
│   ├── model.rs
│   └── handlers.rs
│
├── cleanup/                — orphan media + stale download sweeper
│   ├── mod.rs
│   ├── executor.rs
│   └── tracker.rs
│
├── notification/           — webhooks + history + WebSocket fan-out
│   ├── mod.rs
│   ├── history.rs
│   ├── webhook.rs / webhook_retry.rs / ws_handlers.rs
│   └── websocket.rs
│
├── observability/          — structured logging + retention
├── scheduler/              — background tick (handlers + model)
├── events/                 — AppEvent + listeners
├── settings/               — config + quality_profile
│
├── invariants/             — cross-domain invariant checks
├── reconcile/              — periodic reconciliation framework
├── conventions/            — SQL conventions + helpers
├── models/                 — cross-domain enums only (enums.rs)
│
├── api/                    — top-level system endpoints (not domain-specific)
│   ├── fs.rs               — filesystem browse for path pickers
│   ├── health.rs           — /api/v1/health
│   └── status.rs           — /api/v1/status (setup-required, warnings)
│
├── flow_tests/             — end-to-end HTTP flow tests
└── test_support/           — test fakes (TMDB, Trakt, Torznab, librqbit)
```

## Conventions per domain module

Each domain module follows the same shape where applicable:

- **`mod.rs`** — public interface, top-level types, `## Public API`
  doc comment that names every cross-domain re-export.
- **`<feature>.rs`** — one feature per file. Where a concept has
  both a DB row and HTTP handlers, the file carries both rather
  than splitting into `<x>_model.rs` + `<x>_handlers.rs` pairs.
- **`handlers.rs`** — thin Axum extractor → operation call →
  response shape. Never contains business logic. Used when a
  domain has many endpoints that don't naturally pair with one
  feature file.

## Visibility

- `pub` — only on items used by other domains. Conservative.
- `pub(crate)` — items used by `main.rs` / cross-domain glue.
- `pub(super)` — most internal collaboration between sibling
  modules within a domain.
- Private — default.

This is a binary crate, so `pub` and `pub(crate)` are
semantically equivalent; we use `pub` only on the items called
out in each domain `mod.rs`'s `## Public API` section as a
self-imposed contract surface.

## Anti-patterns this prevents

- **`services/` as a dumping ground.** No more "where does this
  go" arguments — it goes in the relevant domain.
- **Operations spanning 4 files.** Each operation lives next to
  its model and handler.
- **Symmetric movie/episode logic drifting.** They're siblings
  under `content/`, with `acquisition/release_target.rs` holding
  the shared trait.
- **`<x>_model.rs` + `<x>_handlers.rs` pairs.** Layer-cake creep.
  One concept = one file (`blocklist.rs`, `release.rs`, etc).

