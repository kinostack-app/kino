---
paths:
  - "frontend/**"
---
# Frontend Rules

## Components

- shadcn/ui primitives from `@/components/ui/`
- Feature-based organization in `src/features/`
- Shared components in `src/components/`

## Data Fetching

- TanStack Query for all server state
- hey-api generated hooks from `@/api/generated/`
- Zustand for client-only state (player, sidebar, cast session)

### Freshness: meta.invalidatedBy, WS-driven, polling as last resort

**Default: meta-tag-driven invalidation.** The backend emits typed `AppEvent` variants (see `backend/.../events/mod.rs`). Each frontend `useQuery` that depends on event-driven freshness declares which events refresh it via `meta.invalidatedBy`. The WS handler (`src/state/websocket.ts`) dispatches inbound events against those tags in one pass — no central switch to edit per feature.

```ts
useQuery({
  ...showWatchStateOptions({ path: { tmdb_id: id } }),
  meta: { invalidatedBy: ['imported', 'upgraded', 'watched', 'show_monitor_changed'] },
});
```

For scoped (per-entity) invalidation, use `on(event, filter)` from `@/state/invalidation`:

```ts
meta: {
  invalidatedBy: [
    on('download_metadata_ready', (e) => e.download_id === download.id),
  ],
};
```

**Adding a new live surface:**
1. Declare the query with `meta.invalidatedBy` listing every `AppEvent` name that should refresh it.
2. If the backend lacks a matching event, add one — a state-changing mutation that has no event is a cross-tab sync bug. Emit after the DB write commits.
3. Extend the `EVENT_REGISTRY` in `src/state/invalidation.ts` with the new variant name (the record is exhaustive — tsc fails otherwise).

**Adding a new mutation:**
1. Backend handler emits a matching `AppEvent` after its DB write.
2. Frontend mutation's `onSuccess` stays minimal — no `invalidateQueries` unless the target query isn't meta-tagged (e.g., pure UI-state caches like `['kino', 'preferences', 'home']`).
3. If the caller needs fresh data *synchronously* after the mutation resolves (e.g., to open a dialog that reads the cache), use `refetchQueries` — meta only marks stale.

**Rules of thumb:**
- **State-changing mutation → emit an event.** If it writes to the DB and other tabs / the future Chromecast wrapper care, it needs a variant.
- **Pure UI-preference mutations** (home layout, sort orders) can skip emission — those caches aren't meta-tagged either.
- **Probe / test endpoints** (`testIndexer`, `testWebhook`) don't write state → no event needed.
- **Dev-mode warning** logs when a dispatched event matches zero meta-tagged queries — if you see `[invalidation] "X" matched 0 queries`, a query is missing `invalidatedBy: ['X']`.

**Never reach for `refetchInterval` without checking meta tags first.** If an event can carry the change, tag the query. If no event exists, add one. Polling is the last resort.

**Acceptable polling — narrow exceptions:**
- Live librqbit data with no backend event source (peer lists, piece bitmaps). Annotate with "would graduate to WS if we add per-download subscriptions."
- Intentionally-infrequent cache warm-ups (e.g. 1h TMDB trending refresh). Comment why.
- Diagnostic panels where events would be overkill (transcode session list, task registry). Comment why.

Every `refetchInterval` must be paired with a comment explaining why WS isn't sufficient. Reviewers reject polls that can be event-driven.

## API Client

- Run `npm run codegen` after ANY backend route/handler/response type changes
- Generated types in `src/api/generated/` — never edit manually
- Zod schemas generated alongside TypeScript types

### The contract is generated — don't shadow it

Every shape the backend sends (HTTP responses, WS events, History blobs, enum fields) must be imported from `@/api/generated/types.gen` — never re-declared as a local `interface` or string union. This is the full rule, short version:

- **No hand-rolled DTO mirrors.** `LibraryMovie`, `Webhook`, `PlaybackInfo` etc. are `type X = GeneratedX` aliases at most.
- **No hand-rolled string unions** shadowing backend enums. `'queued' | 'grabbing' | ...` → import `DownloadState`. `'all' | 'future' | 'none'` → import `MonitorNewItems`. If the generated field is `string` instead of the typed union, fix the backend (`#[schema(value_type = TheEnum)]`) — don't mirror it on the frontend.
- **`as never` / `as SomeBackendType` on mutation bodies or query results is a regression.** Build the body with the generated request type (`CreateShow`, `UpdateWebhook`, `ConfigUpdate`). The only acceptable `as` escapes are unrelated language / library limitations — CSS custom-property names, TanStack Router parametric-route search params.
- **WebSocket handlers narrow on `AppEvent.event`**, not `event.title as string`. Switch per variant; no `[key: string]: unknown` access.
- **History blob rows parse to `AppEvent | null`**, not `Record<string, unknown>`.

When a backend change lands: `npm run codegen` → `npm run typecheck` → fix every site the compiler points at. If you're reaching for `as` to silence a type error, the right fix is almost always on the backend schema, not the frontend call site.

## Styling

- Tailwind CSS v4 — utility classes
- Dark theme by default (neutral-950 background)
- Use `cn()` from `@/lib/utils` for conditional classes

## Video Player

- Vidstack for playback (headless provider + custom UI)
- hls.js for HLS transcode playback
- Report progress to server every 10 seconds during playback

## Common biome lints to pre-empt

`npm run lint` treats errors as fatal in CI; warnings allowed. Most a11y warnings cluster around hand-rolled overlays + clickable wrappers — established patterns:

- **Suppression comment positioning** — `// biome-ignore lint/<rule>: reason` MUST be on the line directly preceding the offending JSX element. Multi-line comment blocks where only the first line carries the directive are silently ignored. Collapse to a single line.
- **`a11y/noStaticElementInteractions`** — `<div onClick=...>` without `role` + `tabIndex` + `onKeyDown` fires it. For modal backdrops, `role="presentation"` plus the comment "backdrop click is a visual dismiss; keyboard dismissal is handled by useModalA11y" is the established escape hatch. For clickable cards (PosterCard, Calendar event chip), use `role="button"` + `tabIndex={0}` + `onKeyDown` for Enter/Space.
- **`a11y/useSemanticElements`** — wants real `<button>` / `<output>` / `<fieldset>` instead of `role="button"` etc. Suppress with biome-ignore when the semantic element would inherit form defaults that fight the layout (absolute-positioned overlay children, inline rating layouts).
- **Stale `// biome-ignore`** suppressions become `suppressions/unused` warnings when biome renames a rule. Common rename: `useKeyWithClickEvents` → `noStaticElementInteractions`. When you see `suppressions/unused`, rename the suppression rather than delete it.
- **Inner-modal `onClick={(e) => e.stopPropagation()}`** — usually redundant. The outer dialog's onClick should already use `if (e.target === e.currentTarget) onClose()` to gate close-on-backdrop. Drop the inner stopPropagation div entirely.
- **`a11y/noLabelWithoutControl`** on generic `Field`-style wrapper components — biome can't prove the children include a control. Suppress with a comment naming the call sites that pass real inputs/selects.
