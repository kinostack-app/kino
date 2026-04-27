/**
 * Diagnostic bundle — what the user copies into a GitHub issue when
 * something goes wrong.
 *
 * No telemetry. Nothing leaves the browser until the user explicitly
 * clicks a button. The output is a Markdown block suitable for
 * pasting into an issue body; keep it human-readable (reviewers will
 * see it unindented).
 *
 * Kept narrow on purpose: version + UA + route + last N log entries.
 * The backend's `/settings/logs` page is the source of truth for
 * deeper diagnostics; this is the "I hit a wall right now, here's the
 * scene of the crime" snapshot.
 */

import { recentClientLogs } from '@/lib/clientLogger';

export interface DiagnosticsBundle {
  ts: string;
  route: string;
  user_agent: string;
  viewport: string;
  kino_version: string;
  ws_status: string;
  recent_logs: string;
}

/** Collect the current diagnostic snapshot. Safe to call at any
 *  time — reads from memory-resident state only, no network. */
export function collectDiagnostics(wsStatus: string): DiagnosticsBundle {
  return {
    ts: new Date().toISOString(),
    route:
      typeof window === 'undefined' ? '?' : `${window.location.pathname}${window.location.search}`,
    user_agent: typeof navigator === 'undefined' ? '?' : navigator.userAgent,
    viewport: typeof window === 'undefined' ? '?' : `${window.innerWidth}×${window.innerHeight}`,
    // Vite injects `__APP_VERSION__` via `define`; falls back to
    // `dev` when the build didn't set it (local HMR).
    kino_version:
      typeof __APP_VERSION__ === 'string' && __APP_VERSION__.length > 0 ? __APP_VERSION__ : 'dev',
    ws_status: wsStatus,
    recent_logs: formatLogs(recentClientLogs(50)),
  };
}

/** Render a diagnostics bundle as a Markdown code block. The shape
 *  matches what GitHub's issue templates expect when a user pastes
 *  into the "System info" field. */
export function formatDiagnosticsMarkdown(d: DiagnosticsBundle): string {
  return (
    '**Diagnostics**\n\n' +
    '```\n' +
    `timestamp:   ${d.ts}\n` +
    `kino:        ${d.kino_version}\n` +
    `route:       ${d.route}\n` +
    `viewport:    ${d.viewport}\n` +
    `ws:          ${d.ws_status}\n` +
    `user_agent:  ${d.user_agent}\n` +
    '```\n\n' +
    '**Recent frontend logs**\n\n' +
    '```\n' +
    d.recent_logs +
    '\n```\n'
  );
}

/** Copy diagnostics to the clipboard. Returns `true` on success.
 *  Silent on failure — the caller shows a toast on either outcome. */
export async function copyDiagnostics(wsStatus: string): Promise<boolean> {
  const md = formatDiagnosticsMarkdown(collectDiagnostics(wsStatus));
  try {
    await navigator.clipboard.writeText(md);
    return true;
  } catch {
    return false;
  }
}

/** Build a GitHub issue-report URL. The body is *not* pre-filled
 *  with the full diagnostics — URL length caps at ~8 KB and
 *  truncation is worse than a two-step flow. Instead we pre-fill
 *  the title + a short template body that asks the user to paste
 *  the diagnostics they just copied. */
export function buildIssueUrl(title: string, route: string): string {
  const body =
    `**What happened**\n\n\n\n` +
    `**What were you doing?**\n\n\n\n` +
    `**Diagnostics**\n\n` +
    `_Paste the diagnostics block you copied with the "Copy diagnostics" button._\n\n` +
    `_Route at the time: \`${route}\`_\n`;
  const params = new URLSearchParams({
    title,
    body,
    labels: 'bug',
  });
  return `https://github.com/kinostack-app/kino/issues/new?${params.toString()}`;
}

function formatLogs(entries: { level: string; message: string; ts_ms: number }[]): string {
  if (entries.length === 0) return '(no recent entries)';
  return entries
    .map((e) => {
      const t = new Date(e.ts_ms).toISOString().slice(11, 23);
      return `${t}  ${e.level.toUpperCase().padEnd(5)}  ${e.message}`;
    })
    .join('\n');
}

// `__APP_VERSION__` is injected via Vite's `define` at build time.
// Declare the global so tsc doesn't complain about `typeof`.
declare const __APP_VERSION__: string | undefined;
