# kino architecture overview

Single-binary self-hosted media automation + streaming server.
One process owns acquisition, import, playback, watch-now, HLS
transcode, trickplay, intro skipping, Cast handoff, events, and
the SPA backend.

## Tech stack

| Layer | Choice | Why |
|---|---|---|
| Backend | Rust + Axum + sqlx (SQLite) | Single binary, no runtime deps, predictable |
| HTTP API | utoipa-driven OpenAPI | Frontend types are codegen'd, no string contracts |
| Frontend | React 19 + Vite + TanStack Query + shadcn/ui + Tailwind v4 | Standard SPA stack |
| Quality gates | clippy + rustfmt + biome | Strict; CI exits non-zero on any |
| Video pipeline | FFmpeg (transcode + probe) + hls.js + Vidstack | Battle-tested |
| BitTorrent | librqbit | Real torrent client, in-process |
| VPN | embedded WireGuard via boringtun | Optional; in-process userspace tunnel — no sidecar |

Full library inventory + per-platform packaging in [`tech-stack.md`](./tech-stack.md).

## Process shape

```
┌──────────────────────────────────────────┐
│ kino (single binary)                     │
│                                          │
│  HTTP API ←──── SPA (React, served via Vite or static)
│     │                                    │
│  Auth middleware                         │
│     │                                    │
│  Domain modules ─ Operations             │
│     │                                    │
│  AppEvent broadcast ─→ WS clients + listeners
│     │                                    │
│  AppState ─ scheduler, transcode, librqbit, TMDB, ...
│                                          │
│  SQLite (single .db file in data dir)    │
└──────────────────────────────────────────┘
```

## Deployment shapes

- **Same-origin (default).** kino binary serves the SPA on a single
  port. Browser hits `localhost:8080` directly. Cookie auth.
- **Cross-origin (advanced).** SPA hosted separately (CF Pages,
  static host); points at remote kino backend via
  `VITE_KINO_API_BASE`. Bearer + signed-URL auth.

Both shapes are first-class. See [`auth.md`](./auth.md).

## Architectural principles

The following docs detail kino's chosen patterns. They're not
optional — code review uses them to evaluate PRs.

- [`state-machines.md`](./state-machines.md) — every long-lived
  entity has an explicit state machine with classification methods.
- [`operations.md`](./operations.md) — state-changing functions
  follow the validate → atomic execute → verify → emit shape.
- [`invariants.md`](./invariants.md) — predicates that must always
  hold; checked in tests, periodically in prod.
- [`consistency-model.md`](./consistency-model.md) — eventual vs
  strong consistency contracts per system boundary.
- [`crate-layout.md`](./crate-layout.md) — domain-module
  organisation; what lives where.
- [`auth.md`](./auth.md) — session/cookie/bearer/signed-URL model.
- [`events.md`](./events.md) — `AppEvent` + invalidation pattern.
- [`logging.md`](./logging.md) — log-level discipline + the silent-error rule.

## What kino is NOT

- Not multi-user. Single-user by design.
- Not horizontally scalable. SQLite + single process; vertical only.
- Not a multi-process media-server stack. One binary; one process;
  one config; one log.
- Not designed for tinkerers. The "it works" audience comes first;
  power-user knobs come second.
