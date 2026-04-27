/**
 * Backend-reachability state, surfaced to the UI so we can show a
 * coordinated "we're offline" signal instead of N confused per-query
 * error states (broken posters, empty pages, stuck spinners).
 *
 * Three visual tiers:
 *   - `healthy`       — green path. Nothing rendered.
 *   - `reconnecting`  — amber top-banner. Transient: backend mid-restart,
 *                       network hiccup. Banner clears on first success.
 *   - `offline`       — full-screen shell. Persistent: been failing for
 *                       > `OFFLINE_THRESHOLD_MS`, or WebSocket closed
 *                       without ever opening. Manual Retry button.
 *
 * Writers:
 *   - Fetch interceptor in `lib/clientLogger.ts` calls `noteSuccess`
 *     on 2xx responses to our own API, `noteFailure` on network-error
 *     or 5xx.
 *   - `state/websocket.ts` calls `noteFailure('ws')` when the socket
 *     closes without first opening, and `noteSuccess` on connect.
 *
 * Zustand (not TanStack-Query) because this is cross-cutting UI state,
 * not server state. Keeping it out of the query cache means the
 * "offline shell" renders even when no query has been issued yet.
 */

import { create } from 'zustand';

export type ConnectionPhase = 'healthy' | 'reconnecting' | 'offline';

/** How long we stay in `reconnecting` before escalating to `offline`.
 *  A backend restart is typically 2–5 s; 30 s is well past "this is
 *  just a blip." */
const OFFLINE_THRESHOLD_MS = 30_000;

interface ConnectionState {
  phase: ConnectionPhase;
  /** Wall-clock of the most recent successful request. */
  lastSuccessAt: number;
  /** Wall-clock of the first failure in the current streak. `null`
   *  when healthy. */
  failStreakStartedAt: number | null;
  /** Wall-clock of the most recent failure. `null` when healthy. */
  lastFailAt: number | null;

  /** Record a 2xx response / successful WS open. Snaps us back to
   *  healthy and resets the streak. */
  noteSuccess: () => void;
  /** Record a failure. Escalates to `offline` once the streak exceeds
   *  the threshold. */
  noteFailure: () => void;
  /** User clicked "Retry" in the offline shell. Resets the streak
   *  clock so they get another `reconnecting` window before the
   *  shell reappears. */
  manualRetry: () => void;
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  phase: 'healthy',
  lastSuccessAt: Date.now(),
  failStreakStartedAt: null,
  lastFailAt: null,

  noteSuccess: () => {
    // No-op when already healthy — saves a render on every request.
    if (get().phase === 'healthy' && get().failStreakStartedAt === null) {
      set({ lastSuccessAt: Date.now() });
      return;
    }
    set({
      phase: 'healthy',
      lastSuccessAt: Date.now(),
      failStreakStartedAt: null,
      lastFailAt: null,
    });
  },

  noteFailure: () => {
    const now = Date.now();
    const { failStreakStartedAt } = get();
    const start = failStreakStartedAt ?? now;
    const streakMs = now - start;
    const nextPhase: ConnectionPhase =
      streakMs >= OFFLINE_THRESHOLD_MS ? 'offline' : 'reconnecting';
    set({
      phase: nextPhase,
      failStreakStartedAt: start,
      lastFailAt: now,
    });
  },

  manualRetry: () => {
    // Reset the streak so the shell doesn't flash back up
    // immediately. If the retry succeeds, `noteSuccess` will clear
    // everything; if it fails again, we start the 30 s grace over.
    set({ phase: 'reconnecting', failStreakStartedAt: Date.now() });
  },
}));

/** Narrow selector — components only re-render when the phase
 *  actually changes, not on every success/failure note. */
export const useConnectionPhase = () => useConnectionStore((s) => s.phase);
