---
paths:
  - "backend/**"
---
# Rust Backend Rules

## Always Use Scripts

```bash
cd backend
just fix-ci        # Auto-fix then verify
just test          # Run tests
```

## Diagnostics

- `just fix-ci` (cargo clippy) is the **authoritative** compiler check
- Do NOT reference "rust-analyzer diagnostics" — they are not authoritative
- Never re-verify after a clean clippy run — clippy IS rustc + lints

## Common clippy lints to pre-empt

Our clippy profile treats `-D warnings` as errors. Writing code that trips one of these then fixing in a second pass wastes a whole build. Check for these *while writing*:

- **`doc_markdown`** — any bare identifier in a `///` or `//!` comment that looks like code (CamelCase types, `snake_case` fns, file paths, tokens with underscores) must be in backticks. Examples caught previously: `TorrentSession`, `AppState`, `do_import`, `TorznabClient`, `SQLite`, `BitTorrent`, `{base_url}`. If in doubt, wrap it.
- **`items_after_statements`** — `use`, `struct`, `fn` declarations inside a function body go at the top of the body (or become free items at module scope). Don't write `let x = ...; use sha2::Sha256; let y = ...`.
- **`too_many_lines`** — any function over 100 lines needs `#[allow(clippy::too_many_lines)]`. Don't refactor to split just to dodge it; the `#[allow]` is the accepted escape hatch.
- **`cast_precision_loss` / `cast_possible_truncation` / `cast_sign_loss`** — `u64 as f64` and `i64 as f64` trigger these. Use `#![allow(...)]` at the top of a module that does synthetic-maths in fakes/tests; in production code, do the cast with a saturating `u32`-sized intermediate.
- **`manual_div_ceil`** — `(n + 7) / 8` fails; use `n.div_ceil(8)`.
- **`redundant_feature_names`** — Cargo features suffixed with `_support`, `_test`, `_util` trigger this. Pick a distinct name (we used `harness`).
- **`missing_debug_implementations`** — any public struct needs `#[derive(Debug)]` or a manual `impl Debug`. For generic structs, add `#[derive(Debug)]` unconditionally and accept that inner types must be `Debug` too.
- **`type_complexity`** — any type alias for a tuple of 5+ `Option<String>`s / similar. Define a named `type Foo = (...)` and use that.
- **`url_bare_url`** (rustdoc) — tokens with a colon-slash shape like `file:line` or `src/foo.rs:42` in doc comments get mistaken for bare URLs. Wrap them in backticks.
- **`unused_async`** — function is `async` but its body has no `await`. Comes up under `cfg(test)` no-op stubs. Either drop `async` (if every caller can tolerate sync) or add `#[allow(clippy::unused_async)]` next to the cfg-gated stub.
- **`implicit_clone`** — `t.field().to_string()` on a value already typed `String` calls `to_string` via `Deref<Target = str>`. Use `.clone()` instead. Same for `to_owned()` on a `&String`.
- **`ref_option`** — `&Option<T>` arg should be `Option<&T>`. Pass `state.field.as_ref()` at the call site.
- **`unnecessary_wraps`** — function always returns `Some(...)` / `Ok(...)`. Common with `cfg`-branched functions where each per-target branch unconditionally wraps. Add `#[allow(clippy::unnecessary_wraps)]` with a one-line "wrap is load-bearing across the cfg branches" note.
- **`collapsible_if`** — `if let Ok(x) = a { if let Some(y) = b { ... } }` flagged by clippy in Rust 1.84+; use `if let Ok(x) = a && let Some(y) = b { ... }` (the let-chains stabilisation we already use elsewhere).
- **Stale `// biome-ignore`** suppressions (frontend-side, but same shape) — biome rule renames break old comments. Run `npm run lint` and rename `useKeyWithClickEvents` → `noStaticElementInteractions` / `useSemanticElements` etc. when the lint output points at a `suppressions/unused`.
- **Cargo-deny additions** — when adding a new direct dep, check it against `backend/deny.toml`. New unique licenses (LGPL, MPL, CDLA, Unicode-3.0) need either an `[licenses.allow]` entry or a per-crate exception. New transitive duplicates fire `multiple_crate_versions` (we have it `allow`-set globally; warn-only fine).

If you're about to hit one, add the `#[allow(clippy::...)]` inline (or at module scope) with a one-line comment explaining why — don't rewrite the code to dodge the lint for no structural reason.

### sqlx + db tests
- **`sqlx::Column` + `sqlx::Row`** trait imports — calling `.name()` on a column ref or `.try_get()`/`.columns()` on a row needs these traits in scope. `use sqlx::{Column as _, Row as _}` lets you call the methods without polluting the namespace.
- **`max_connections=1` in tests** — `db::create_test_pool()` uses a single-connection pool. Code that `await`s on multiple `state.db` handles in nested scopes can deadlock. Drop intermediate `Vec`s before recursive DB calls.
- **Time-bombed test dates** — never hardcode "yesterday" or "an_hour_ago" as a string literal. Compute via `chrono::Utc::now() - Duration::hours(N)` so the relative window holds regardless of when the test runs.
- **Disk-space-sensitive flow tests** — `acquisition::grab::ensure_free_space_for_grab` is `cfg(test)`-bypassed because CI tempdirs can be tight. If you add new disk-space gates in production code, mirror the bypass.

## OpenAPI contract

Anything the frontend will read — HTTP response bodies, `AppEvent` variants, enums the UI branches on — goes through `utoipa`. The full rule sits in the root `CLAUDE.md`; the backend-side checklist:

- `#[derive(ToSchema)]` on every public DTO + every sub-enum.
- Rust fields typed `String` that are semantically a typed enum (e.g. `download.state`, `movie.status`, `show.monitor_new_items`) get `#[schema(value_type = TheEnum)]` so the generated TS narrows.
- Register in `main.rs` under `components(schemas(...))` if not reached via a `body = ...` on a path macro. New `AppEvent` variants, new domain enums — both belong here.
- New `AppEvent` variant → add an arm in the `event_type_matches_serde_tag` unit test in `events/mod.rs`. The serde tag and `AppEvent::event_type()` string must stay in lockstep or it fails CI.
- List-endpoint responses declare `body = PaginatedResponse<T>` — not an empty `200` — or the generated SDK returns `unknown` and the frontend regresses to `as` casts.

## AppEvent emission

Every state-changing handler emits a matching `AppEvent` — the frontend's `meta.invalidatedBy` cache-invalidation story depends on it. Skipping an emit leaves other tabs / future clients stale until page reload.

- **After the write, not before.** Emit on the line *after* the successful `sqlx::query(...).execute()` / `tx.commit()` — never before. A frontend refetch triggered by the event otherwise races the DB.
- **Every mutation endpoint.** Create / update / delete paths each emit. Toggles (watched / unwatched, rate / unrate) emit the symmetric variant, not the same one with a flag.
- **Probe endpoints don't emit.** `testIndexer`, `testWebhook`, `testPath`, `testTmdb` etc. don't change state → no event.
- **Pure UI-preference writes can skip.** Home layout / sort orders are per-user session state; the matching cache is local-only and not meta-tagged on the frontend. Comment the handler to make the choice explicit.
- **Events carry every id the frontend would need to scope on.** `Imported` carries `movie_id` / `episode_id` / `show_id` so the receiver can refresh parent surfaces without a JOIN.

## Dev Server

The backend runs automatically in the `kino-backend` container via `watchexec`. Just edit code — it rebuilds and restarts.

```bash
just logs          # View backend logs
just logs-tail 20  # Last 20 lines
just restart       # Force restart container
just status        # Check all containers + health
just reset         # Delete DB, restart fresh (setup wizard appears)
```

## Architecture

Single binary (`kino`) with modular internal structure:
- `src/main.rs` — entry point, CLI (`kino` server, `kino reset`), server startup
- Modules organized by subsystem (metadata, search, download, import, playback, etc.)
- axum for HTTP, sqlx for database, utoipa for OpenAPI
- librqbit for BitTorrent (real torrent client, not simulated)

## Key Subsystems

- **AppState** (`state.rs`) — holds db pool, event_tx, tmdb client, torrent client, scheduler
- **Scheduler** (`scheduler/mod.rs`) — 3s tick, runs download monitor, wanted search, cleanup
- **Download monitor** (`services/download_monitor.rs`) — polls librqbit, emits progress events
- **Import trigger** (`services/import_trigger.rs`) — creates Media on download complete
- **Events** (`events/`) — typed AppEvent enum, broadcast to WS/history/webhooks
- **Status** (`api/status.rs`) — health checks, setup_required flag, warnings

## Patterns

- All errors via `Result<T, AppError>` — no unwrap in production code
- Use `tracing::info!` / `tracing::error!` for logging
- SQL queries use runtime `query_as` with `FromRow` derive (not compile-time macros)
- snake_case for JSON fields (serde default)
- Zombie state prevention: always mark downloads as `failed` on errors, never silently return Ok

## Database

- SQLite with sqlx. WAL mode.
- Migrations in `backend/migrations/`
- `just reset` deletes the DB entirely — `ensure_defaults` recreates from env vars on next boot
