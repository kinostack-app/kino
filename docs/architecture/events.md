# Events

`AppEvent` is kino's typed cross-subsystem messaging. Every
state-changing operation emits one; the WebSocket layer broadcasts
them to connected SPAs; listeners use them to trigger downstream
work.

## The contract

`AppEvent` is a Rust enum with serde tags. Each variant carries
exactly the fields a consumer needs to scope on. The variant name
is the wire string the frontend matches.

The OpenAPI spec exposes the full enum so the frontend gets a
typed discriminated union via codegen.

## Emission rules

From `.claude/rules/backend.md` (the canonical source):

- **After the write, not before.** Emit on the line *after* the
  successful `tx.commit()`. A frontend refetch triggered by the
  event otherwise races the DB.
- **Every state-changing endpoint emits a matching variant.** Skipping
  an emit leaves other tabs / future Cast wrappers stale until
  page reload.
- **Toggles emit symmetric variants** (Watched / Unwatched), not
  the same variant with a flag.
- **Probe endpoints don't emit** (testIndexer, testWebhook, etc).
- **Pure UI-preference writes can skip.** Home layout, sort orders
  — local-only caches; commented in the handler.

## Frontend invalidation

The SPA's TanStack Query layer wires events to query-cache
invalidation via `meta.invalidatedBy`:

```ts
useQuery({
  ...showWatchStateOptions({ path: { tmdb_id: id } }),
  meta: { invalidatedBy: ['imported', 'upgraded', 'watched', 'show_monitor_changed'] },
});
```

The WebSocket handler dispatches inbound events against the
`meta.invalidatedBy` tags in one pass. No central switch to edit
per feature.

For scoped (per-entity) invalidation, use the `on()` helper:

```ts
meta: {
  invalidatedBy: [
    on('download_metadata_ready', (e) => e.download_id === download.id),
  ],
}
```

## Adding a new variant

1. Add to the `AppEvent` enum in `events/mod.rs` with `#[derive(ToSchema)]`.
2. Update the `event_type_matches_serde_tag` test in
   `events/mod.rs` so the serde tag and `event_type()` string stay
   in lockstep.
3. Run `just codegen` to regenerate the frontend SDK.
4. Update the `EVENT_REGISTRY` in `frontend/src/state/invalidation.ts`
   (the record is exhaustive — tsc will fail otherwise).
5. Add the variant name to any `meta.invalidatedBy` lists that
   need it.

## Why events instead of polling

Events are kino's primary live-update mechanism. Polling is a
last resort, used only when:

- librqbit data has no backend event source (peer lists, piece
  bitmaps).
- Intentionally-infrequent cache warm-ups (1h TMDB trending refresh).
- Diagnostic panels where events would be overkill (transcode
  session list, task registry).

Every `refetchInterval` in the frontend must be paired with a
comment explaining why WS isn't sufficient.

## Anti-patterns this prevents

- **State-changing mutation with no event.** Other tabs can drift
  for hours until manual refresh.
- **`refetchInterval` instead of meta tags.** Network spam, slow
  invalidation, drained batteries.
- **String-typed events on the frontend.** Generated TS types make
  every consumer typed-narrow.
- **Sender-decides-receiver-effects.** Listeners declare what they
  react to; sender just emits the fact.

## Cross-references

- [`operations.md`](./operations.md) — events emit AFTER the
  operation's transaction commits.
- [`consistency-model.md`](./consistency-model.md) — events are
  notifications of state change; they're NOT the state. Consumers
  re-read state at decision time.
