/**
 * Browser notifications via the Web Notifications API.
 *
 * We listen on the existing app-event WebSocket (`state/websocket.ts`)
 * and fire a `new Notification(...)` for events the user has opted
 * in to. Prefs live in localStorage — no backend state needed.
 *
 * Rule of thumb: fire only when the tab is *not* in the foreground.
 * When the tab is visible the user is already getting an in-app toast
 * via sonner, and doubling up creates noise. When the tab is hidden
 * (minimised, different window, different tab) the system
 * notification is the only way we can get their attention.
 */

export type BrowserNotifEventKey = 'import' | 'upgrade' | 'failure' | 'health';

export interface BrowserNotifPrefs {
  enabled: boolean;
  events: Record<BrowserNotifEventKey, boolean>;
}

const STORAGE_KEY = 'kino.browserNotifs';

/**
 * Default browser-notification prefs. Only events the user would want
 * to *come back to the tab for* get a default-on. Grab / download /
 * watched are deliberately not user-facing events — they're pipeline
 * milestones rolled into the `imported` hero toast.
 */
export const DEFAULT_PREFS: BrowserNotifPrefs = {
  enabled: false,
  events: {
    import: true,
    upgrade: true,
    failure: true,
    health: true,
  },
};

export function isSupported(): boolean {
  return typeof window !== 'undefined' && 'Notification' in window;
}

/** Mirror of the same-named helper in `state/websocket.ts`. Kept
 *  duplicated here so this module has zero cross-module deps and
 *  can be tested in isolation. */
function isOnPlaybackRoute(): boolean {
  if (typeof window === 'undefined') return false;
  const p = window.location.pathname;
  return p.startsWith('/play/') || p.startsWith('/watch/');
}

export function getPermission(): NotificationPermission {
  return isSupported() ? Notification.permission : 'denied';
}

export function readPrefs(): BrowserNotifPrefs {
  if (typeof window === 'undefined') return DEFAULT_PREFS;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_PREFS;
    const parsed = JSON.parse(raw) as Partial<BrowserNotifPrefs>;
    return {
      enabled: Boolean(parsed.enabled),
      events: { ...DEFAULT_PREFS.events, ...(parsed.events ?? {}) },
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function writePrefs(prefs: BrowserNotifPrefs): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Quota or private-mode — silent. The prefs just won't persist.
  }
}

/**
 * Map an AppEvent's `event` field to the pref key it falls under.
 * Returning null means "no browser notification for this event type".
 * Most events are routed to null — the notification ruleset is
 * deliberately narrow (see `SILENT_NOTIF_EVENTS` in
 * `src/state/websocket.ts`).
 */
function eventKey(eventType: string): BrowserNotifEventKey | null {
  switch (eventType) {
    case 'imported':
      return 'import';
    case 'upgraded':
      return 'upgrade';
    case 'download_failed':
      return 'failure';
    case 'health_warning':
      return 'health';
    default:
      return null;
  }
}

interface AppEventLike {
  event: string;
  title?: unknown;
  quality?: unknown;
  message?: unknown;
  error?: unknown;
}

/**
 * Build a notification title + body for an AppEvent. Kept separate
 * from the fire path so testing is straightforward.
 */
function composeNotification(event: AppEventLike): { title: string; body: string } {
  const title = (event.title as string) || '';
  const quality = (event.quality as string) || '';
  const message = (event.message as string) || '';
  const error = (event.error as string) || '';

  switch (event.event) {
    case 'release_grabbed':
      return { title: 'Grabbed', body: quality ? `${title} — ${quality}` : title };
    case 'download_started':
      return { title: 'Downloading', body: title };
    case 'download_complete':
      return { title: 'Download complete', body: title };
    case 'imported':
      return { title: 'Ready to play', body: quality ? `${title} — ${quality}` : title };
    case 'upgraded':
      return { title: 'Upgraded', body: quality ? `${title} — ${quality}` : title };
    case 'download_failed':
      return { title: 'Download failed', body: `${title}${error ? ` — ${error}` : ''}` };
    case 'watched':
      return { title: 'Marked watched', body: title };
    case 'health_warning':
      return { title: 'Health warning', body: message || title };
    default:
      return { title: event.event, body: title };
  }
}

/**
 * Fire a browser notification for an AppEvent if the user has
 * subscribed to it and the tab is currently backgrounded. Returns a
 * structured decision so the caller can emit one unified trace line
 * per event across all notification surfaces (toast + browser).
 *
 * No-op (but still returns a decision) in every skip case: unsupported
 * browser, permission not granted, tab in foreground, on the player
 * route, prefs disabled, etc.
 */
export function fireIfSubscribedTraced(event: AppEventLike): {
  outcome: 'fired' | 'skipped';
  reason?: string;
} {
  if (!isSupported()) return { outcome: 'skipped', reason: 'unsupported' };
  if (getPermission() !== 'granted') return { outcome: 'skipped', reason: 'permission' };
  // Tab in foreground → toast is already showing; skip the system notif.
  if (document.visibilityState === 'visible') return { outcome: 'skipped', reason: 'visible' };
  // Video is playing (possibly PiP / fullscreen / backgrounded) → we
  // don't want a system notification interrupting what they're
  // watching. The user can catch up on the in-app UI when they're done.
  if (isOnPlaybackRoute()) return { outcome: 'skipped', reason: 'on-playback-route' };

  const key = eventKey(event.event);
  if (!key) return { outcome: 'skipped', reason: 'no-event-key' };

  const prefs = readPrefs();
  if (!prefs.enabled) return { outcome: 'skipped', reason: 'prefs-disabled' };
  if (!prefs.events[key]) return { outcome: 'skipped', reason: `pref-off:${key}` };

  const { title, body } = composeNotification(event);
  try {
    const n = new Notification(title, { body, tag: key, icon: '/favicon.ico' });
    // Let the user jump back to the tab by clicking the notification.
    n.onclick = () => {
      window.focus();
      n.close();
    };
    return { outcome: 'fired' };
  } catch (e) {
    // Some browsers throw on construction when not in a secure context.
    return { outcome: 'skipped', reason: `throw:${e instanceof Error ? e.message : 'unknown'}` };
  }
}

/**
 * Request permission from the user. Returns the resulting permission
 * state. Already-granted / already-denied return immediately without
 * a prompt.
 */
export async function requestPermission(): Promise<NotificationPermission> {
  if (!isSupported()) return 'denied';
  if (Notification.permission !== 'default') return Notification.permission;
  try {
    return await Notification.requestPermission();
  } catch {
    return 'denied';
  }
}

/**
 * Fire a one-shot "kino test" notification so the user can see what
 * the system toast looks like without waiting for a real event.
 */
export function fireTestNotification(): void {
  if (!isSupported() || getPermission() !== 'granted') return;
  try {
    new Notification('kino — test notification', {
      body: 'If you can see this, browser notifications are working.',
      icon: '/favicon.ico',
    });
  } catch {
    // ignore
  }
}
