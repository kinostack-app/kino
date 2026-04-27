# kino

Single-binary self-hosted media automation and streaming server. Discovers, acquires, organises, transcodes, streams, and casts your media library from one Rust application.

## For other LLM coding agents

This file is the canonical orientation. Non-Claude agents (Codex, Cursor, Aider, Continue, etc.) should also read [`AGENTS.md`](./AGENTS.md) — same content, sibling for cross-vendor convention.

Path-scoped guidance lives in [`.claude/rules/`](./.claude/rules/) — Claude loads these automatically based on the files you're touching:
- `commands.md` — the canonical justfile / npm-script reference
- `backend.md` — Rust backend conventions (clippy lints to pre-empt, OpenAPI contract, AppEvent emission, …)
- `frontend.md` — TypeScript / React conventions (TanStack Query meta-tags, generated SDK rules, …)
- `testing.md` — test-suite conventions
- `web.md` — Astro / Starlight conventions for the kinostack.app site

## Tech Stack

- **Backend:** Rust, Axum, sqlx (SQLite), utoipa (OpenAPI), librqbit (BitTorrent)
- **Frontend:** React 19, TypeScript, Vite, TanStack Query, shadcn/ui, Tailwind CSS v4
- **Quality:** clippy + rustfmt (backend), biome (frontend)
- **Video:** FFmpeg (transcode + probe), hls.js (frontend), Vidstack (player)

## Project Structure

```
/kino/
├── backend/                  # Rust workspace
│   ├── Cargo.toml           # Workspace root
│   ├── justfile             # Task runner (use this, not raw cargo)
│   ├── migrations/          # sqlx SQL migrations
│   └── crates/
│       └── kino/            # Main binary
│           └── src/
├── frontend/                 # Vite/React SPA — kino's in-app UI
│   ├── package.json
│   ├── src/
│   │   ├── api/generated/   # Auto-generated from OpenAPI (never edit)
│   │   ├── components/ui/   # shadcn/ui primitives
│   │   ├── features/        # Feature modules
│   │   ├── hooks/
│   │   ├── lib/
│   │   └── routes/
│   └── biome.json
├── web/                      # Public-facing sites (two Astro projects)
│   ├── site/                # kinostack.app — marketing (Astro + Tailwind)
│   ├── docs/                # docs.kinostack.app — Starlight docs
│   └── shared/              # Master brand assets, shared CSS tokens
├── docs/                     # Specification (start at docs/README.md)
│   ├── architecture/        # Cross-cutting patterns (events, invariants, …)
│   ├── data-model/          # SQL schema + derived state
│   ├── decisions/           # ADRs
│   ├── subsystems/          # Per-domain reference (shipped behaviour)
│   ├── roadmap/             # Planned, not-yet-shipped subsystems
│   ├── runbooks/            # Operator how-tos
│   └── archive/             # Superseded specs / pre-refactor history
├── .claude/                  # Claude Code config
│   ├── settings.json        # Hooks, permissions
│   └── rules/               # Path-scoped rules
└── .devcontainer/            # Dev environment
```

## Environment

We run inside a **devcontainer** with 4 containers sharing a network namespace:

| Container | Role | Port |
|-----------|------|------|
| `kino-dev` | Claude Code shell — run quality gates, tests, docker commands | — |
| `kino-backend` | Rust server, auto-rebuilds via watchexec on .rs changes | 8080 |
| `kino-frontend` | Vite dev server for the in-app React SPA, instant HMR | 5173 |
| `kino-web` | Two Astro dev servers (marketing + docs) | 4321 (kinostack.app) + 4322 (docs.kinostack.app) |

All containers share `network_mode: "service:dev"` so `localhost` reaches everything. The dev container has the Docker socket mounted, so `docker` CLI commands work.

**Do NOT start services manually** — just edit code and they rebuild/restart.

### Key Commands (from `backend/`)

```bash
# Dev operations
just logs              # Backend logs (follow)
just logs-tail 50      # Last 50 lines
just logs-frontend     # Frontend logs
just restart           # Restart backend
just restart-frontend  # Restart frontend (picks up vite.config changes)
just restart-all       # Restart everything
just status            # Container status + health check
just reset             # Delete DB + librqbit session, restart fresh

# Quality gates
just fix-ci            # Auto-fix then verify (preferred)
just test              # Run tests

# Frontend (from frontend/)
npm run lint:fix       # Auto-fix
npm run typecheck      # Type check
npm run codegen        # Regenerate API client from OpenAPI spec
```

### Environment Variables

Set in `.env` (loaded by docker-compose). Key vars:

- `KINO_TMDB_API_KEY` — TMDB Read Access Token
- `KINO_MEDIA_PATH` / `KINO_DOWNLOAD_PATH` — file storage paths
- `KINO_PROWLARR_URL` / `KINO_PROWLARR_API_KEY` — auto-configure Prowlarr indexer

The backend generates a random `api_key` on first boot (no env var
needed). The dev SPA reaches authed endpoints via the AutoLocalhost
session cookie that `GET /api/v1/bootstrap` auto-issues for
same-host browsers.

### Reset / Clean Slate

```bash
cd backend && just reset
```

Deletes the SQLite database and librqbit session. Backend auto-restarts with fresh DB, re-reads env vars for config. Frontend shows the setup wizard.

## Critical Rules

### Always Use Scripts

```bash
# Backend — NEVER use raw cargo
cd backend
just fix-ci          # Auto-fix then verify (preferred)
just test            # Run tests
just codegen         # Regenerate OpenAPI + frontend SDK after backend schema change

# Frontend — NEVER use raw vitest/biome
cd frontend
npm run lint:fix     # Auto-fix
npm run typecheck    # Type check
npm run test         # Tests
# API codegen: `cd ../backend && just codegen`. Do NOT run
# `npm run codegen` by itself — it reads `backend/openapi.json`,
# which is only regenerated by the backend side of `just codegen`.
```

### The OpenAPI spec is the backend/frontend contract

Every piece of data the frontend receives from the backend — HTTP responses, WebSocket events, History blob columns — must be declared in OpenAPI and consumed by the frontend as a generated type. No hand-written TypeScript interfaces that mirror backend structs; no string unions that mirror Rust enums.

When adding a new type or enum:

1. **Rust side**: `#[derive(ToSchema)]` on the type and any sub-types. For Rust fields typed as `String` that should be a typed enum in TS (e.g. a DB-stored enum value), add `#[schema(value_type = TheEnum)]`.
2. **Register it** in `main.rs` under `components(schemas(...))` if it isn't reached via a `body = ...` on a `#[utoipa::path]`. Event / domain enums that aren't HTTP bodies go here explicitly.
3. **Codegen**: `cd backend && just codegen`. This exports `openapi.json` from the in-binary spec, regenerates `frontend/src/api/generated/*`, and typechecks the frontend in one pass. **Never run `npm run codegen` alone** — it only reads the JSON, it can't regenerate it.
4. **Frontend side**: import from `@/api/generated/types.gen` — never re-declare the shape.

`AppEvent` (the WebSocket + history-column union) follows the same rule: adding a new variant requires `#[derive(ToSchema)]` coverage + a corresponding arm in the `event_type_matches_serde_tag` test in `events/mod.rs` so the serde tag / `event_type()` consistency stays enforced.

The only acceptable `as` escapes in frontend code are language / library limitations unrelated to the backend contract (CSS custom-property names, TanStack Router search params). Any `as never` / `as SomeBackendType` on a mutation body or query response is a regression — fix the underlying schema instead.

### Spec Documents

The `docs/` directory contains the full system specification. Read relevant docs before implementing a subsystem. The spec is the source of truth for design decisions.

## Development Flow

1. **Read spec** — Check `docs/subsystems/` for the relevant subsystem
2. **Implement** — Follow existing patterns in `backend/crates/kino/src/`
3. **Quality** — `just fix-ci` (backend) / `npm run lint:fix` (frontend)
4. **Test** — `just test` / `npm run test`
5. **Codegen** — `npm run codegen` if API changed
6. **Check logs** — `just logs` if something broke
