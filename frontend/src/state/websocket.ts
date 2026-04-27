/**
 * WebSocket client — connects to the backend event stream and
 * updates TanStack Query caches in real-time.
 *
 * Events flow: server → WebSocket → cache update → components re-render.
 * No polling needed — the UI is instantly reactive.
 */

import type { QueryClient } from '@tanstack/react-query';
import type { AppEvent as GeneratedAppEvent } from '@/api/generated/types.gen';
import { kinoToast } from '@/components/kino-toast';
import { fireIfSubscribedTraced } from '@/lib/browser-notifications';
import { useAuthStore } from '@/state/auth';
import { useCastStore } from '@/state/cast-store';
import { useConnectionStore } from '@/state/connection';
import { dispatchEventToQueries } from '@/state/invalidation';
import { useTrickplayStreamStore } from '@/state/trickplay-stream-store';
import {
  type ActiveDownload,
  CONFIG_KEY,
  DOWNLOADS_KEY,
  INDEXERS_KEY,
  LIBRARY_MOVIES_KEY,
  LIBRARY_SHOWS_KEY,
  type LibraryShow,
  QUALITY_PROFILES_KEY,
  STATUS_KEY,
  WEBHOOKS_KEY,
} from './library-cache';

/**
 * Inbound event from the backend WebSocket. The generated
 * `AppEvent` discriminated union covers every variant including
 * `Lagged` (emitted by the WS handler when its broadcast receiver
 * fell behind and the client should resync). No hand-crafted
 * types — the generated contract is exhaustive.
 */
type AppEvent = GeneratedAppEvent;

let ws: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
// True when the socket closed after ever being open. Used to
// distinguish "first connect" from "reconnect after a gap" — we
// invalidate everything on the latter so missed events can't leave
// the UI permanently stale.
let hadConnection = false;
// Per-socket auth latch. We only start processing events after the
// server confirms the post-upgrade auth handshake; a socket that
// opens but times out on auth will never flip this to true and the
// close handler will schedule a reconnect.
let authenticated = false;

/**
 * Tear down any existing socket before we open a new one. A socket in
 * `CONNECTING` state would slip past a naive `readyState === OPEN`
 * guard, so we always close + null out handlers regardless of state.
 * Also protects against HMR re-entering this module with `ws` still
 * pointing at a previously-bound socket — without this, every reload
 * adds another parallel subscriber and every event fires twice, then
 * three times, then …
 */
function teardownExistingSocket() {
  if (!ws) return;
  ws.onopen = null;
  ws.onclose = null;
  ws.onerror = null;
  ws.onmessage = null;
  try {
    ws.close();
  } catch {
    // Already closed or in a terminal state — nothing to do.
  }
  ws = null;
}

/**
 * Connect to the WebSocket and wire events to query cache updates.
 * Idempotent: repeated calls replace any in-flight socket instead of
 * stacking, so React StrictMode double-invokes and HMR reloads don't
 * leave duplicate subscribers behind.
 */
export async function connectWebSocket(queryClient: QueryClient) {
  // Wait for backend to be reachable before opening WebSocket
  try {
    const res = await fetch('/api/v1/status');
    if (!res.ok) throw new Error('not ready');
  } catch {
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => connectWebSocket(queryClient), 5000);
    return;
  }

  doConnect(queryClient);
}

function doConnect(queryClient: QueryClient) {
  teardownExistingSocket();
  authenticated = false;

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const url = `${protocol}//${window.location.host}/api/v1/ws`;

  const socket = new WebSocket(url);
  ws = socket;

  socket.onopen = () => {
    // In cookie mode the upgrade request carries the `kino-session`
    // cookie automatically and the backend pre-auths from it — the
    // server's first frame is `{"type":"auth_ok"}` and we don't
    // need to send anything. We still send an empty auth frame as
    // a no-op so bearer-mode deploys (cookieless cross-origin) can
    // optionally inject a token into this same path later without
    // a protocol change.
    const bearer = useAuthStore.getState().bearerToken;
    const frame = bearer ? { type: 'auth', api_key: bearer } : { type: 'auth' };
    try {
      socket.send(JSON.stringify(frame));
    } catch {
      try {
        socket.close();
      } catch {
        // ignore
      }
    }
  };

  socket.onmessage = (msg) => {
    // Guard against messages from a stale socket that's been
    // replaced but whose handlers hadn't been nulled out in time.
    if (ws !== socket) return;
    try {
      const frame = JSON.parse(msg.data as string) as AppEvent | { type: string };
      // Control frames (auth_ok, pong) are tagged with `type`;
      // events are tagged with `event`. The two namespaces don't
      // overlap, so a single JSON.parse handles both.
      if ('type' in frame && typeof frame.type === 'string') {
        handleControlFrame(frame.type, queryClient);
        return;
      }
      if (!authenticated) {
        // Server somehow sent an event before accepting us —
        // shouldn't happen, but drop defensively rather than let
        // it churn through caches.
        return;
      }
      handleEvent(queryClient, frame as AppEvent);
    } catch {
      // Ignore malformed messages
    }
  };

  socket.onclose = () => {
    // Only the current socket schedules a reconnect. A stale socket
    // closing (e.g. after teardownExistingSocket) would otherwise
    // fire a parallel reconnect and re-introduce the duplicate.
    if (ws !== socket) return;
    console.log('[ws] disconnected, reconnecting in 5s');
    ws = null;
    // Surface the outage to the UI — the reconnecting banner lights
    // up while we wait, escalates to the offline shell if we stay
    // dark past 30 s.
    useConnectionStore.getState().noteFailure();
    reconnectTimer = setTimeout(() => doConnect(queryClient), 5000);
  };

  socket.onerror = () => {
    if (ws !== socket) return;
    socket.close();
  };
}

/**
 * Handle a server control frame (auth_ok, pong). These never
 * invalidate caches or fire notifications — they're pure session
 * plumbing.
 */
function handleControlFrame(type: string, queryClient: QueryClient) {
  if (type === 'auth_ok') {
    authenticated = true;
    console.log('[ws] authenticated');
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    // A healthy (auth'd) socket is a strong "backend is alive"
    // signal — snap connection state out of reconnecting/offline
    // even if no HTTP request has landed yet.
    useConnectionStore.getState().noteSuccess();
    // Recovery: after a reconnect, we can't know which events we
    // missed in the gap. Invalidate every cache that depends on
    // event-driven state so the next render refetches fresh data.
    if (hadConnection) {
      console.info('[ws] reconnected — invalidating caches to recover missed events');
      invalidateEverything(queryClient);
    }
    hadConnection = true;
    return;
  }
  if (type === 'pong') {
    // Server reply to an app-level ping. Future use: liveness
    // probes that don't require a full event round-trip. Currently
    // unused; accepting it silently.
    return;
  }
  console.debug('[ws] unknown control frame type:', type);
}

export function disconnectWebSocket() {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  ws?.close();
  ws = null;
}

/**
 * Tick-rate `setQueryData` patch for `download_progress`. Runs up to
 * 1×/sec per active download — going through `invalidateQueries`
 * would fire one HTTP GET per tick per tab, which is why this path
 * bypasses the meta dispatcher entirely.
 */
function patchDownloadProgress(
  qc: QueryClient,
  event: Extract<GeneratedAppEvent, { event: 'download_progress' }>
) {
  const dlId = event.download_id;
  const patch = {
    downloaded: event.downloaded,
    uploaded: event.uploaded,
    download_speed: event.speed,
    upload_speed: event.upload_speed,
    seeders: event.seeders ?? null,
    leechers: event.leechers ?? null,
    eta: event.eta ?? null,
  };
  qc.setQueryData<ActiveDownload[]>([...DOWNLOADS_KEY], (old) => {
    if (!old) return old;
    return old.map((d) => {
      if (d.id !== dlId) return d;
      // Don't let stale progress events overwrite an optimistic pause/resume
      if (d.state === 'paused' || d.state === 'failed') return d;
      return { ...d, ...patch };
    });
  });
  // Show cards carry a per-show `active_download` projection that
  // drives the poster overlay + sweep. Patch it in-place on every
  // tick so the progress bar animates smoothly — same reason we
  // don't invalidate the downloads cache wholesale.
  qc.setQueryData<LibraryShow[]>([...LIBRARY_SHOWS_KEY], (old) => {
    if (!old) return old;
    return old.map((s) => {
      if (!s.active_download || s.active_download.download_id !== dlId) return s;
      return {
        ...s,
        active_download: {
          ...s.active_download,
          downloaded: event.downloaded,
          download_speed: event.speed,
        },
      };
    });
  });
}

/**
 * Full cache reset — used by the reconnect and lagged paths. Invalidates
 * every query that declares `meta.invalidatedBy` plus the hand-rolled
 * library caches. We can't replay missed events, so this is the
 * convergence path; cheap since TanStack only refetches queries with
 * active observers.
 */
function invalidateEverything(qc: QueryClient) {
  // Hand-rolled keys not reached via meta (pure settings CRUD caches
  // that rely on the `invalidatedBy` path but double-cover here so a
  // lagged reconnect leaves nothing stale).
  qc.invalidateQueries({ queryKey: [...LIBRARY_MOVIES_KEY] });
  qc.invalidateQueries({ queryKey: [...LIBRARY_SHOWS_KEY] });
  qc.invalidateQueries({ queryKey: [...DOWNLOADS_KEY] });
  qc.invalidateQueries({ queryKey: [...INDEXERS_KEY] });
  qc.invalidateQueries({ queryKey: [...STATUS_KEY] });
  qc.invalidateQueries({ queryKey: [...CONFIG_KEY] });
  qc.invalidateQueries({ queryKey: [...QUALITY_PROFILES_KEY] });
  qc.invalidateQueries({ queryKey: [...WEBHOOKS_KEY] });
  // Any query tagged with `invalidatedBy` gets refetched regardless
  // of which event type it listed — on reconnect we assume we might
  // have missed any of them.
  qc.invalidateQueries({
    predicate: (q) => Array.isArray(q.meta?.invalidatedBy) && q.meta.invalidatedBy.length > 0,
  });
}

/**
 * Route an event to caches + the notification surfaces.
 *
 * Cache invalidation is driven by `meta.invalidatedBy` on each query
 * (see `state/invalidation.ts`) — adding a new feature means tagging
 * its query, not editing a central switch. The switch below only
 * covers the two bespoke paths that can't be expressed as
 * "invalidate this key":
 *
 *   - `download_progress` — tick-rate `setQueryData` patch (1 HTTP
 *     call per tick per download would be a cost catastrophe).
 *   - `trickplay_stream_updated` — bumps a Zustand store, not a
 *     TanStack cache.
 *
 * Reconnect / lagged recovery is handled separately above.
 */
function handleEvent(qc: QueryClient, event: AppEvent) {
  // Backend broadcast channel lag: if the broker tells us events
  // were dropped, we have no way to replay them. Fall back to a full
  // cache refresh so the UI converges even under pressure.
  if (event.event === 'lagged') {
    console.info('[ws] server reported lag — invalidating caches');
    invalidateEverything(qc);
    return;
  }

  // Bespoke fast paths that aren't a "just invalidate a key" shape.
  if (event.event === 'download_progress') {
    patchDownloadProgress(qc, event);
    // Don't fall through to meta dispatch — download_progress is
    // tick-rate and no query should refetch on it.
    return;
  }
  if (event.event === 'trickplay_stream_updated') {
    const dlId = event.download_id as number | undefined;
    if (dlId != null) useTrickplayStreamStore.getState().bump(dlId);
    return;
  }

  // Cast sender (subsystem 32). High-frequency MEDIA_STATUS frames
  // and the terminal session_ended frame both go straight into the
  // cast Zustand store without touching TanStack — the cast UI
  // reads from the store, not from query caches.
  if (event.event === 'cast_status') {
    useCastStore
      .getState()
      ._applyStatus(event.session_id, event.position_ms ?? null, event.status_json);
    return;
  }
  if (event.event === 'cast_session_ended') {
    useCastStore.getState()._applyEnded(event.session_id);
    return;
  }

  // Everything else is meta-driven: walk the cache, match on
  // `meta.invalidatedBy`, invalidate. Dev mode warns when an event
  // matches zero queries — that's almost always a missing tag.
  dispatchEventToQueries(qc, event);

  // ── Notification surfaces ──
  // Pure plumbing events never produce user-facing notifications, so
  // skip the decision pipeline entirely — no toast switch, no browser
  // pref check, no trace line. Keeps the [notif] log scannable and
  // avoids a pointless switch fall-through for every progress tick.
  if (SILENT_NOTIF_EVENTS.has(event.event)) return;

  // Each surface (toast, browser) returns its own decision so we can
  // emit a single trace line per event showing who fired and who
  // skipped (and why). Scan the console filtered by "[notif]" to
  // audit a session of clicks.
  const decisions: NotifDecision[] = [];
  decisions.push(decideToast(event));
  const browserOutcome = fireIfSubscribedTraced(event);
  decisions.push({ surface: 'browser', ...browserOutcome });
  const traceTitle = 'title' in event && typeof event.title === 'string' ? event.title : '';
  logNotif(event.event, traceTitle, decisions);
}

/**
 * Event types that never produce a user-facing notification. They
 * still drive cache invalidation above — this is purely about
 * filtering the toast / browser-notif / trace pipeline.
 */
const SILENT_NOTIF_EVENTS = new Set<string>([
  // Tick-rate internals
  'download_progress',
  'playback_progress',
  'trickplay_stream_updated',
  'stream_probe_ready',
  // Cast sender — store-only state updates; the cast UI surfaces
  // the changes (mini-bar / overlay / popover), no toast pipeline.
  'cast_status',
  'cast_session_ended',
  // Backend plumbing with no user-facing moment
  'search_started',
  'release_grabbed', // rolled into download_started / imported narratives
  'lagged',
  // Settings writes — cache invalidation is the whole job
  'indexer_changed',
  'config_changed',
  'quality_profile_changed',
  'webhook_changed',
  // Trakt background sync — History page is the surface if users care
  'trakt_synced',
  // Metadata drip — followed-show list grows silently
  'new_episode',
  // User-initiated, already visible in UI
  'movie_added',
  'show_added',
  'content_removed',
  'watched',
  // Download milestones rolled into imported
  'download_started',
  'download_complete',
  // User-initiated cancel — explicit intent, not worth notifying.
  'download_cancelled',
]);

// Track recent toasts to suppress duplicates and collapse rapid sequences
const recentToasts = new Map<string, number>();

/**
 * Skip all passive (event-driven) toasts while the user is watching
 * video. Mutation-triggered toasts still fire because those are user-
 * initiated (they pressed save, they want feedback). This is only
 * for WS notifications that can interrupt playback.
 */
function isOnPlaybackRoute(): boolean {
  if (typeof window === 'undefined') return false;
  const p = window.location.pathname;
  return p.startsWith('/play/') || p.startsWith('/watch/');
}

/**
 * One-line decision log for every inbound AppEvent so the user can
 * see — in the browser console — which surfaces fired and which
 * decided to skip. Keep the output terse; the point is to be able
 * to skim a session of clicks and spot unexpected fires.
 */
export interface NotifDecision {
  surface: 'toast' | 'browser';
  outcome: 'fired' | 'batched' | 'deduped' | 'skipped' | 'unhandled';
  reason?: string;
}
function logNotif(eventType: string, title: string, decisions: NotifDecision[]) {
  const path = typeof window === 'undefined' ? '?' : window.location.pathname;
  const vis = typeof document === 'undefined' ? '?' : document.visibilityState;
  const parts = decisions.map((d) =>
    d.reason ? `${d.surface}:${d.outcome}(${d.reason})` : `${d.surface}:${d.outcome}`
  );
  console.debug(`[notif] ${eventType} "${title}" path=${path} vis=${vis} → ${parts.join(', ')}`);
}

/** Did this call actually push a toast (vs. get swallowed by the
 *  dedupe window)? `dedupeToast` returns true when it fires. */
function dedupeToastTraced(id: string, fn: () => void, cooldownMs = 3000): boolean {
  const now = Date.now();
  const last = recentToasts.get(id);
  if (last && now - last < cooldownMs) return false;
  recentToasts.set(id, now);
  fn();
  if (recentToasts.size > 50) {
    for (const [k, v] of recentToasts) {
      if (now - v > 10_000) recentToasts.delete(k);
    }
  }
  return true;
}

function decideToast(event: AppEvent): NotifDecision {
  // Rule 1: never interrupt the player with passive toasts. The user
  // still gets fresh state on every in-app surface via WS cache
  // invalidation — they're just not pulled out of the video.
  if (isOnPlaybackRoute()) {
    return { surface: 'toast', outcome: 'skipped', reason: 'on-playback-route' };
  }

  // Narrow by `event.event` — the generated discriminated union
  // gives us exhaustive field typing inside each case, so no more
  // `as number | undefined` casts or stringly-typed field reads.
  switch (event.event) {
    case 'imported': {
      const fired = dedupeToastTraced(`imp-${event.media_id ?? event.title}`, () =>
        kinoToast.hero({
          title: event.title,
          quality: event.quality ?? null,
          movieId: event.movie_id ?? undefined,
          episodeId: event.episode_id ?? undefined,
          showId: event.show_id ?? undefined,
        })
      );
      return { surface: 'toast', outcome: fired ? 'fired' : 'deduped' };
    }

    case 'upgraded': {
      const newQ = event.new_quality ?? '';
      const fired = dedupeToastTraced(`upg-${event.title}`, () =>
        kinoToast.success('Upgraded', {
          id: `upg-${event.title}`,
          description: newQ ? `${event.title} → ${newQ}` : event.title,
        })
      );
      return { surface: 'toast', outcome: fired ? 'fired' : 'deduped' };
    }

    case 'download_failed': {
      const fired = dedupeToastTraced(`dlf-${event.download_id}`, () =>
        kinoToast.failure({
          title: event.title,
          error: event.error,
          downloadId: event.download_id,
          // DownloadFailed doesn't carry movie_id / episode_id /
          // show_id — the failure card's "Pick alternate" link
          // falls back to null. Surfacing the link would need the
          // backend event to JOIN through download_content; noted
          // but not blocking.
          movieId: undefined,
          episodeId: undefined,
          showId: undefined,
        })
      );
      return { surface: 'toast', outcome: fired ? 'fired' : 'deduped' };
    }

    case 'list_unreachable': {
      const fired = dedupeToastTraced(`lu-${event.list_id}`, () =>
        kinoToast.warning('Can\u2019t reach list', {
          id: `lu-${event.list_id}`,
          description: `${event.title}${event.reason ? ` — ${event.reason}` : ''}`,
          action: {
            label: 'Fix',
            onClick: () => {
              window.location.assign('/settings/integrations');
            },
          },
        })
      );
      return { surface: 'toast', outcome: fired ? 'fired' : 'deduped' };
    }

    case 'health_warning':
    case 'health_recovered':
      // HealthBanner + HealthDot own the in-app surface; a toast
      // would triplicate. Browser-notif still fires via the separate
      // pipeline when tab is backgrounded — that's where it pays off.
      return {
        surface: 'toast',
        outcome: 'skipped',
        reason: 'covered-by-health-banner',
      };

    case 'ip_leak_detected': {
      // Critical safety event — the killswitch already paused every
      // active download, but the user needs to know *now*. Toast
      // overrides the playback-route guard because a leak takes
      // precedence over not interrupting the player.
      const fired = dedupeToastTraced(
        'ip-leak',
        () =>
          kinoToast.failure({
            title: 'VPN leak detected — downloads paused',
            error: `Your IP appeared as ${event.observed_ip || 'unknown'}, expected ${event.expected_ip || 'unknown'}. Check your VPN.`,
            downloadId: undefined,
            movieId: undefined,
            episodeId: undefined,
            showId: undefined,
          }),
        // Long cooldown so a wedged endpoint doesn't fire every 5min.
        60 * 60 * 1000
      );
      return { surface: 'toast', outcome: fired ? 'fired' : 'deduped' };
    }

    case 'list_bulk_growth': {
      const listId = event.list_id;
      const fired = dedupeToastTraced(`lbg-${listId}`, () =>
        kinoToast.info(`${event.added} new in ${event.title}`, {
          id: `lbg-${listId}`,
          description: 'Open Lists to browse them',
          action: {
            label: 'Open',
            onClick: () => {
              window.location.assign(`/lists/${listId}`);
            },
          },
        })
      );
      return { surface: 'toast', outcome: fired ? 'fired' : 'deduped' };
    }

    default:
      return { surface: 'toast', outcome: 'unhandled' };
  }
}
