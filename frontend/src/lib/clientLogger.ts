/**
 * Frontend → backend log shipping.
 *
 * Captures and batches into `POST /api/v1/client-logs`:
 *   • `window.onerror`            — uncaught exceptions
 *   • `unhandledrejection`        — rejected promises nobody awaited
 *   • `console.error / warn`      — code paths that call these directly
 *   • fetch interceptor           — non-2xx responses to our own API
 *   • `clientLog.{info,warn,debug,error}` — explicit breadcrumbs
 *
 * Design:
 *   • In-memory queue, flushed every 5 s or at 20 entries (first to trip).
 *   • `sendBeacon` on `visibilitychange → hidden` so a tab-close still
 *     ships the outstanding queue.
 *   • Dedup by `level+message+stackFirstLine` over a 60 s window —
 *     a crash loop sends one entry with `count: N` instead of N.
 *   • Hard cap at 100 queued entries (drop-oldest) so a runaway bug
 *     can't DoS the backend.
 *
 * Not sent: anything during the first 500 ms after load (avoid racing
 * the import pipeline's HMR/dev-mode noise). This is cheap insurance;
 * remove if it ever hides a real startup bug.
 */

import { useConnectionStore } from '@/state/connection';

type Level = 'error' | 'warn' | 'info' | 'debug';

interface QueueEntry {
  level: Level;
  message: string;
  stack?: string;
  url?: string;
  ts_ms: number;
  count: number;
  /** dedup key — computed once when the entry is first seen */
  key: string;
}

const FLUSH_EVERY_MS = 5_000;
const FLUSH_AT_SIZE = 20;
const MAX_QUEUE = 100;
const DEDUP_WINDOW_MS = 60_000;
const IGNORE_FIRST_MS = 500;
/** Ring buffer cap for the diagnostic snapshot. Bigger than the
 *  in-flight queue because we want a decent scrollback even if
 *  shipping is currently failing. */
const RING_CAP = 200;

const queue: QueueEntry[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;
const startupAt = Date.now();
let enabled = false;

/** Local ring buffer — mirror of the queue that survives ship
 *  failures. This is what `lib/diagnostics.ts` dumps when the user
 *  clicks "Copy diagnostics." Crucial: the in-flight `queue` drains
 *  on flush (even failed flushes clear it), so without this mirror
 *  there'd be nothing to paste when the backend is the thing that's
 *  broken. */
const ringBuffer: QueueEntry[] = [];

function dedupKey(level: Level, message: string, stack?: string): string {
  const stackHead = stack?.split('\n').slice(0, 1).join('') ?? '';
  return `${level}|${message.slice(0, 160)}|${stackHead.slice(0, 200)}`;
}

function enqueue(
  partial: Omit<QueueEntry, 'count' | 'key' | 'ts_ms'> & { ts_ms?: number; explicit?: boolean }
) {
  if (!enabled) return;
  // Automatic captures (console interception, error events, fetch) get
  // muted for the first 500 ms to skip HMR/dev-mode noise. Explicit
  // clientLog.* calls bypass the guard so the "initialized" breadcrumb
  // and any startup-path breadcrumbs actually land.
  if (!partial.explicit && Date.now() - startupAt < IGNORE_FIRST_MS) return;

  const ts_ms = partial.ts_ms ?? Date.now();
  const key = dedupKey(partial.level, partial.message, partial.stack);

  // Collapse into an existing recent entry if present.
  const existing = queue.find((e) => e.key === key && ts_ms - e.ts_ms < DEDUP_WINDOW_MS);
  if (existing) {
    existing.count += 1;
    existing.ts_ms = ts_ms;
    return;
  }

  // Strip the `explicit` flag — it's request-local, not a queue field.
  const { explicit: _explicit, ...rest } = partial;
  const entry: QueueEntry = { ...rest, ts_ms, count: 1, key };
  queue.push(entry);
  // Mirror to the ring buffer so diagnostics survive a failed flush.
  ringBuffer.push(entry);
  while (ringBuffer.length > RING_CAP) ringBuffer.shift();

  // Drop-oldest cap.
  while (queue.length > MAX_QUEUE) queue.shift();

  if (queue.length >= FLUSH_AT_SIZE) {
    flush();
  } else if (!flushTimer) {
    flushTimer = setTimeout(flush, FLUSH_EVERY_MS);
  }
}

function flush() {
  if (flushTimer) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  if (queue.length === 0) return;

  // Take a snapshot; clear before awaiting so concurrent adds don't
  // duplicate-send.
  const payload = {
    entries: queue.map((e) => ({
      level: e.level,
      message: e.message,
      stack: e.stack,
      url: e.url,
      ts_ms: e.ts_ms,
      count: e.count,
    })),
  };
  queue.length = 0;

  const body = JSON.stringify(payload);

  // sendBeacon is the only method guaranteed to ship on tab-close.
  // It can't set headers but it carries credentials (cookies) when
  // the request is same-origin — which is the cookie-mode case.
  // Cross-origin (bearer-mode) deploys lose this last-gasp send;
  // the per-tick keepalive fetch covers everything except the
  // final tab-close beat there.
  if (document.visibilityState === 'hidden' && 'sendBeacon' in navigator) {
    const blob = new Blob([body], { type: 'application/json' });
    navigator.sendBeacon('/api/v1/client-logs', blob);
    return;
  }

  void fetch('/api/v1/client-logs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'include',
    body,
    keepalive: true,
  }).catch(() => {
    // Never feed back into the error handler. Silently drop on network
    // failure — backend will see nothing, but that's strictly better
    // than recursing forever.
  });
}

/** Wrap the global `fetch` so:
 *  - non-2xx responses to our own API land in the client log stream
 *  - connection-health state stays in sync with reality (drives the
 *    reconnecting banner + offline shell — see `state/connection.ts`)
 *
 *  The `/api/v1/client-logs` endpoint is excluded from both: we
 *  never log about the logging pipe (recursion), and its success
 *  isn't a meaningful signal about backend liveness since we send
 *  even when other things are broken. */
function interceptFetch() {
  const original = window.fetch;
  window.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const isOwnApi = url.startsWith('/api/') && !url.startsWith('/api/v1/client-logs');
    try {
      const res = await original(input, init);
      if (isOwnApi) {
        if (res.ok) {
          useConnectionStore.getState().noteSuccess();
        } else {
          enqueue({
            level: res.status >= 500 ? 'error' : 'warn',
            message: `HTTP ${res.status} ${res.statusText}`,
            url,
          });
          // Only 5xx / 502/503/504 signal "backend isn't serving."
          // 4xx is usually the client asking for something invalid
          // (bad params, missing entity) — treat as a successful
          // round-trip for connectivity purposes.
          if (res.status >= 500) {
            useConnectionStore.getState().noteFailure();
          } else {
            useConnectionStore.getState().noteSuccess();
          }
        }
      }
      return res;
    } catch (err) {
      if (isOwnApi) {
        enqueue({
          level: 'error',
          message: `fetch failed: ${err instanceof Error ? err.message : String(err)}`,
          stack: err instanceof Error ? err.stack : undefined,
          url,
        });
        useConnectionStore.getState().noteFailure();
      }
      throw err;
    }
  };
}

/** Wrap `console.warn` / `console.error` so intentional log calls ship too. */
function interceptConsole() {
  const origWarn = console.warn.bind(console);
  const origError = console.error.bind(console);
  // Keep the native output (devtools still works) and mirror to the pipeline.
  console.warn = (...args: unknown[]) => {
    origWarn(...args);
    enqueue({ level: 'warn', message: stringifyArgs(args) });
  };
  console.error = (...args: unknown[]) => {
    origError(...args);
    const maybeErr = args.find((a): a is Error => a instanceof Error);
    enqueue({
      level: 'error',
      message: stringifyArgs(args),
      stack: maybeErr?.stack,
    });
  };
}

function stringifyArgs(args: unknown[]): string {
  return args
    .map((a) => {
      if (typeof a === 'string') return a;
      if (a instanceof Error) return a.message;
      try {
        return JSON.stringify(a);
      } catch {
        return String(a);
      }
    })
    .join(' ')
    .slice(0, 2000);
}

/**
 * Explicit breadcrumbs from app code. Mirrors the backend `tracing` levels
 * so developers can think in one vocabulary. Safe to call before
 * `initClientLogger` — entries are just dropped.
 */
export const clientLog = {
  debug: (message: string, extra?: Record<string, unknown>) =>
    enqueue({
      level: 'debug',
      message: extra ? `${message} ${stringifyArgs([extra])}` : message,
      explicit: true,
    }),
  info: (message: string, extra?: Record<string, unknown>) =>
    enqueue({
      level: 'info',
      message: extra ? `${message} ${stringifyArgs([extra])}` : message,
      explicit: true,
    }),
  warn: (message: string, extra?: Record<string, unknown>) =>
    enqueue({
      level: 'warn',
      message: extra ? `${message} ${stringifyArgs([extra])}` : message,
      explicit: true,
    }),
  error: (message: string, err?: unknown) =>
    enqueue({
      level: 'error',
      message,
      stack: err instanceof Error ? err.stack : undefined,
      explicit: true,
    }),
};

/** Call once from App.tsx. Safe to call twice (idempotent). */
export function initClientLogger() {
  if (enabled) return;
  enabled = true;

  window.addEventListener('error', (e) => {
    enqueue({
      level: 'error',
      message: e.message || 'Uncaught error',
      stack: e.error instanceof Error ? e.error.stack : undefined,
      url: e.filename,
    });
  });

  window.addEventListener('unhandledrejection', (e) => {
    const reason = e.reason;
    enqueue({
      level: 'error',
      message:
        reason instanceof Error
          ? `Unhandled rejection: ${reason.message}`
          : `Unhandled rejection: ${String(reason)}`,
      stack: reason instanceof Error ? reason.stack : undefined,
    });
  });

  // Ship outstanding entries when the tab is hidden / closed.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden' && queue.length > 0) {
      flush();
    }
  });

  interceptConsole();
  interceptFetch();

  clientLog.info('client logger initialized');
}

/** Return the most recent in-memory log entries for diagnostic
 *  copy-paste. Snapshots the ring buffer (drops the dedup key
 *  which is an internal detail). Used by `lib/diagnostics.ts` to
 *  build the "Copy diagnostics" clipboard payload — critical that
 *  this reads from the ring buffer, not the flush queue, because
 *  the flush queue empties even on failed ships. */
export function recentClientLogs(
  limit = 50
): Array<{ level: Level; message: string; ts_ms: number; url?: string }> {
  const tail = ringBuffer.slice(-limit);
  return tail.map((e) => ({
    level: e.level,
    message: e.message,
    ts_ms: e.ts_ms,
    url: e.url,
  }));
}
