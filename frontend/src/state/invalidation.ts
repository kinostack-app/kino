/**
 * Event-driven cache invalidation.
 *
 * Each query declares which backend events invalidate it via
 * `meta.invalidatedBy`. The WS handler walks the cache, matches
 * event → queries via the predicate below, and invalidates in one
 * pass. No central switch of 85 `invalidateQueries` calls to forget
 * a key in.
 *
 * Adding a new event:
 *   1. Backend: add `AppEvent` variant, emit it.
 *   2. `EVENT_REGISTRY` below: add the new event name (fails `tsc`
 *      otherwise once regen lands). This is the exhaustiveness gate.
 *   3. Tag the queries that depend on it — `meta: { invalidatedBy:
 *      ['your_event'] }` on each `useQuery` / hook.
 *
 * Adding a new query:
 *   1. Declare `meta.invalidatedBy` with the events that affect it.
 *      Skip the meta entirely for queries that don't have event-
 *      driven freshness (pure client state, third-party APIs).
 *
 * Runtime safety: in dev mode the dispatcher warns when an event
 * produces zero matches — that's almost always a missing or typo'd
 * `invalidatedBy` on a query that should have reacted.
 */

import type { QueryClient } from '@tanstack/react-query';
import type { AppEvent } from '@/api/generated/types.gen';

export type EventName = AppEvent['event'];

/**
 * One rule — either "invalidate on this event" or "invalidate only
 * when `only` returns true." The latter scopes per-entity so queries
 * keyed by e.g. `download_id` don't refetch for unrelated downloads.
 */
export type InvalidationRule =
  | EventName
  | { event: EventName; only?: (event: AppEvent) => boolean };

/**
 * Type-safe constructor for scoped rules. `E` is inferred as the
 * literal event name, so `only`'s first argument is narrowed to that
 * specific `AppEvent` variant — no casts at call sites.
 */
export function on<E extends EventName>(
  event: E,
  only?: (event: Extract<AppEvent, { event: E }>) => boolean
): InvalidationRule {
  if (!only) return event;
  return {
    event,
    only: only as (event: AppEvent) => boolean,
  };
}

/**
 * Augment TanStack Query's meta so `q.meta.invalidatedBy` is typed
 * globally — every `useQuery` / `queryOptions` call gets IntelliSense
 * + typo-catching on rule names without an opt-in.
 */
declare module '@tanstack/react-query' {
  interface Register {
    queryMeta: {
      invalidatedBy?: InvalidationRule[];
    };
    mutationMeta: {
      invalidatedBy?: InvalidationRule[];
    };
  }
}

/**
 * Exhaustive registry of every backend event name. Forces a `tsc`
 * error when a new `AppEvent` variant lands but this file hasn't
 * been updated — catches the "new event has no subscribers and no
 * one noticed" class of bug at compile time.
 *
 * A `true` value means "should match at least one query in
 * production." A `false` marks events we deliberately don't route
 * through the generic dispatcher (bespoke fast-paths: tick-rate
 * progress, store bumps).
 */
const EVENT_REGISTRY: Record<EventName, boolean> = {
  movie_added: true,
  show_added: true,
  search_started: true,
  release_grabbed: true,
  download_started: true,
  download_progress: false, // setQueryData fast path in websocket.ts
  download_complete: true,
  download_failed: true,
  download_cancelled: true,
  download_paused: true,
  download_resumed: true,
  download_metadata_ready: true,
  imported: true,
  upgraded: true,
  watched: true,
  // Fires every ~10 s during playback. Continue-watching /
  // ShowDetail next-up queries tag for it — regular invalidate is
  // fine at that cadence (a single row join per refetch).
  playback_progress: true,
  // Fires once per streaming download when ffprobe on the partial
  // file completes. `/prepare` tags for it so the info chip
  // populates + the decision-engine plan switches from the
  // "assume transcode, no reasons" default to the real plan
  // (tonemap on HDR, right audio codec, etc.).
  stream_probe_ready: true,
  trickplay_stream_updated: false, // Zustand store bump, not a cache
  new_episode: true,
  health_warning: true,
  health_recovered: true,
  content_removed: true,
  indexer_changed: true,
  config_changed: true,
  quality_profile_changed: true,
  webhook_changed: true,
  trakt_connected: true,
  trakt_disconnected: true,
  trakt_synced: true,
  list_bulk_growth: true,
  list_unreachable: true,
  list_auto_added: true,
  list_deleted: true,
  show_monitor_changed: true,
  rated: true,
  unwatched: true,
  // `lagged` is handled on a bespoke fast path in websocket.ts
  // (full-cache invalidation), not via meta.invalidatedBy tags —
  // individual queries don't opt into it, it's a global reset.
  lagged: false,
  // ffmpeg bundle download: progress is modal-local state
  // (polled from the tracker endpoint) so no query uses it.
  // Terminal events flip the status banner's ffmpeg warnings
  // and the settings-page probe card — both have meta tags
  // for these.
  ffmpeg_download_progress: false,
  ffmpeg_download_completed: true,
  ffmpeg_download_failed: true,
  // VPN killswitch (subsystem 33 phase B). Mismatch between observed
  // and VPN-expected egress IP — the leak self-test paused every
  // active download. The /health VPN panel renders the new
  // protected/observed/expected fields, so health-tagged queries
  // pick this up automatically. Kept `true` here to flag it as a
  // dispatcher-routed event (no zero-match warnings expected).
  ip_leak_detected: true,
  // Cast sender (subsystem 32). Per-tick MEDIA_STATUS frames go
  // straight into a Zustand store — no TanStack-cache invalidation
  // needed (would refetch downloads/library/etc on every position
  // tick at 4 Hz, which is exactly the wrong default). The
  // session_ended event flips the cast mini-bar; it doesn't
  // invalidate downstream queries either.
  cast_status: false,
  cast_session_ended: false,
  // Backup & restore (subsystem 19). All three flip the Settings →
  // Backup page's list query, which carries the matching tags.
  backup_created: true,
  backup_deleted: true,
  backup_restored: true,
};

/**
 * Invalidate every cached query whose `meta.invalidatedBy` matches
 * the inbound event. Returns the match count so the caller can log
 * or assert.
 *
 * Events flagged `EVENT_REGISTRY[event] === false` are expected to
 * bypass this dispatcher (they have a bespoke handler elsewhere) and
 * won't trigger the "zero matches" warning.
 */
export function dispatchEventToQueries(qc: QueryClient, event: AppEvent): number {
  let matched = 0;
  qc.invalidateQueries({
    predicate: (q) => {
      const rules = q.meta?.invalidatedBy;
      if (!rules || rules.length === 0) return false;
      const isMatch = rules.some((rule) => {
        if (typeof rule === 'string') return rule === event.event;
        if (rule.event !== event.event) return false;
        return rule.only ? rule.only(event) : true;
      });
      if (isMatch) matched++;
      return isMatch;
    },
  });
  if (
    import.meta.env.DEV &&
    matched === 0 &&
    EVENT_REGISTRY[event.event] !== false &&
    !HIGH_FREQ_EVENTS.has(event.event)
  ) {
    console.warn(
      `[invalidation] "${event.event}" matched 0 queries — check ` +
        `meta.invalidatedBy on queries that should depend on this event.`
    );
  }
  return matched;
}

/**
 * Events that fire at tick cadence and are expected to land on a
 * page where the tagged queries aren't mounted — no point warning
 * every 10 s that the player route doesn't have Home's
 * continue-watching cache open. They still dispatch normally;
 * this set just silences the "missing meta tag" heuristic for
 * them.
 */
const HIGH_FREQ_EVENTS = new Set<string>(['playback_progress']);
