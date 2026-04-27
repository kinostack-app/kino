/**
 * Per-tab incognito flag — when on, playback progress reports include
 * `incognito: true` so the backend skips the Trakt scrobble path
 * (local resume point + watched_at still update).
 *
 * sessionStorage rather than localStorage because the spec calls for
 * "session-stored" — opening a fresh tab should NOT inherit the
 * previous tab's incognito state. That mirrors how Chrome's own
 * incognito mode works.
 *
 * Implemented with `useSyncExternalStore` so React components stay in
 * sync without dragging in Zustand for one boolean.
 */

import { useSyncExternalStore } from 'react';

const KEY = 'kino.trakt.incognito';

function read(): boolean {
  try {
    return sessionStorage.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

function write(v: boolean) {
  try {
    if (v) sessionStorage.setItem(KEY, '1');
    else sessionStorage.removeItem(KEY);
  } catch {
    // sessionStorage denied — degrade silently. Default-off is the
    // safer fallback (we'd rather scrobble than not, given the user
    // can't reach the toggle to turn off scrobbling either way).
  }
}

// Hand-rolled subscribe: useSyncExternalStore wants a `subscribe(cb)`
// that fires whenever the value changes. Storage doesn't fire events
// in the same tab (only across tabs), so we maintain a local
// listener set and invoke them on `setIncognito`.
const listeners = new Set<() => void>();
function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

export function useIncognito(): boolean {
  return useSyncExternalStore(subscribe, read, () => false);
}

export function setIncognito(v: boolean) {
  write(v);
  for (const cb of listeners) cb();
}

/** Read once without subscribing — for places that need the current
 *  value at request time (e.g. building a progress-report body)
 *  without re-rendering on every change. */
export function getIncognito(): boolean {
  return read();
}
