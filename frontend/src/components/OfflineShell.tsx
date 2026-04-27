/**
 * Full-screen fallback when the backend is persistently unreachable.
 *
 * Replaces the infinite spinner on first load (status query failing)
 * and replaces the app shell when a previously-healthy session has
 * been failing for > `OFFLINE_THRESHOLD_MS`. Gives the user concrete
 * signals instead of a void:
 *   - what's happening ("Can't reach kino")
 *   - what's being tried (auto-retry, elapsed time)
 *   - what they can do (manual retry, copy diagnostics)
 *   - where to go for help (GitHub issue link)
 *
 * Full-takeover screen for "backend down" — beats a per-query error
 * on every surface.
 */

import { AlertTriangle, Copy, ExternalLink, RefreshCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { kinoToast } from '@/components/kino-toast';
import { buildIssueUrl, copyDiagnostics } from '@/lib/diagnostics';
import { useConnectionStore } from '@/state/connection';

export function OfflineShell() {
  const failStreakStartedAt = useConnectionStore((s) => s.failStreakStartedAt);
  const manualRetry = useConnectionStore((s) => s.manualRetry);
  const [, setTick] = useState(0);

  // Update the "been offline for X" counter once per second so the
  // duration reads live without re-renders from every state change.
  useEffect(() => {
    const id = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(id);
  }, []);

  const elapsedSec = failStreakStartedAt
    ? Math.max(0, Math.floor((Date.now() - failStreakStartedAt) / 1000))
    : 0;
  const elapsed = formatElapsed(elapsedSec);

  const onRetry = () => {
    // Bumping the streak clock snaps us back to `reconnecting` for a
    // fresh 30-s grace window. If the next request succeeds, the
    // fetch interceptor flips us healthy and the shell unmounts.
    manualRetry();
    // Kick a status probe by reloading the page — simpler than
    // threading the query client in here and covers the case where
    // the user's tab has been sitting idle for minutes and our
    // background queries have all paused.
    window.location.reload();
  };

  const onCopyDiagnostics = async () => {
    const ok = await copyDiagnostics('offline');
    if (ok) kinoToast.success('Diagnostics copied to clipboard');
    else kinoToast.error("Couldn't copy — clipboard unavailable");
  };

  const issueUrl = buildIssueUrl(
    `Can't reach backend`,
    typeof window === 'undefined' ? '?' : window.location.pathname
  );

  return (
    <div className="min-h-screen bg-[var(--bg-primary)] flex items-center justify-center px-6">
      <div className="w-full max-w-md">
        <div className="flex items-center gap-3 mb-4">
          <div className="w-10 h-10 rounded-full bg-amber-500/15 text-amber-400 grid place-items-center">
            <AlertTriangle size={20} />
          </div>
          <div>
            <h1 className="text-xl font-semibold text-white leading-tight">Can't reach kino</h1>
            <p className="text-sm text-[var(--text-muted)]">Backend unreachable for {elapsed}</p>
          </div>
        </div>
        <p className="text-sm text-[var(--text-secondary)] leading-relaxed mb-6">
          The server stopped responding. This is usually a container restart (wait a few seconds) or
          a misconfiguration on the host. The page will retry automatically; you can also reload
          manually.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={onRetry}
            className="flex items-center gap-2 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white font-medium text-sm transition"
          >
            <RefreshCw size={14} />
            Reload
          </button>
          <button
            type="button"
            onClick={onCopyDiagnostics}
            className="flex items-center gap-2 px-4 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white font-medium text-sm transition"
          >
            <Copy size={14} />
            Copy diagnostics
          </button>
          <a
            href={issueUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-2 px-4 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white font-medium text-sm transition"
          >
            <ExternalLink size={14} />
            Report issue
          </a>
        </div>
        <p className="mt-6 text-xs text-[var(--text-muted)]">
          Paste the copied diagnostics into your issue — it carries the browser, app version, and
          recent frontend logs. Nothing is sent anywhere unless you click the GitHub link.
        </p>
      </div>
    </div>
  );
}

function formatElapsed(sec: number): string {
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  if (m < 60) return s > 0 ? `${m}m ${s}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  return rem > 0 ? `${h}h ${rem}m` : `${h}h`;
}
