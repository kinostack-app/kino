/**
 * React render-error boundary.
 *
 * Catches thrown errors during render / lifecycle so a bug in one
 * component doesn't blank the whole page. Two variants:
 *
 * - `AppErrorBoundary`: root-level. Falls back to a full-screen
 *   "Something went wrong" shell with Reload / Copy diagnostics /
 *   Report issue actions. Reserved for truly unhandled crashes —
 *   most errors should be caught inline by TanStack Query's
 *   `isError` on the query they originated from.
 *
 * - `RouteErrorBoundary`: per-route. Renders a smaller inline
 *   fallback so the TopNav keeps working and the user can navigate
 *   out. Still logs the error + offers diagnostics.
 *
 * `clientLogger` already captures `window.error` / `unhandledrejection`,
 * which covers async errors outside React's tree. This fills the gap
 * for render-time crashes that React swallows by default.
 */

import { AlertTriangle, Copy, ExternalLink, Home, RefreshCw } from 'lucide-react';
import { Component, type ErrorInfo, type ReactNode } from 'react';
import { kinoToast } from '@/components/kino-toast';
import { buildIssueUrl, copyDiagnostics } from '@/lib/diagnostics';

interface Props {
  children: ReactNode;
  /** Override the default "full screen" fallback with a compact
   *  inline one — used by per-route boundaries so the nav stays
   *  reachable. */
  compact?: boolean;
}

interface State {
  error: Error | null;
}

export class AppErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // `clientLogger`'s window.error handler doesn't see React render
    // errors — React catches them first. Log explicitly so the
    // in-app log viewer still reflects them.
    console.error('[error-boundary]', error, info.componentStack);
  }

  reset = () => this.setState({ error: null });

  render() {
    if (!this.state.error) return this.props.children;
    return this.props.compact ? (
      <InlineFallback error={this.state.error} onReset={this.reset} />
    ) : (
      <FullFallback error={this.state.error} onReset={this.reset} />
    );
  }
}

function FullFallback({ error, onReset }: { error: Error; onReset: () => void }) {
  return (
    <div className="min-h-screen bg-[var(--bg-primary)] flex items-center justify-center px-6">
      <div className="w-full max-w-md">
        <div className="flex items-center gap-3 mb-4">
          <div className="w-10 h-10 rounded-full bg-red-500/15 text-red-400 grid place-items-center">
            <AlertTriangle size={20} />
          </div>
          <div>
            <h1 className="text-xl font-semibold text-white leading-tight">Something went wrong</h1>
            <p className="text-sm text-[var(--text-muted)]">An error broke the page.</p>
          </div>
        </div>
        <div className="rounded-lg bg-red-950/30 ring-1 ring-red-500/20 px-3 py-2 mb-4">
          <p className="text-xs text-red-200 font-mono leading-relaxed break-words">
            {error.message || String(error)}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="flex items-center gap-2 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white font-medium text-sm transition"
          >
            <RefreshCw size={14} />
            Reload
          </button>
          <button
            type="button"
            onClick={onReset}
            className="flex items-center gap-2 px-4 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white font-medium text-sm transition"
          >
            Try again
          </button>
          <ActionButtons error={error} />
        </div>
      </div>
    </div>
  );
}

function InlineFallback({ error, onReset }: { error: Error; onReset: () => void }) {
  return (
    <div className="mx-auto max-w-2xl my-10 px-4">
      <div className="rounded-xl bg-[var(--bg-secondary)] ring-1 ring-red-500/20 p-5">
        <div className="flex items-center gap-3 mb-3">
          <div className="w-8 h-8 rounded-full bg-red-500/15 text-red-400 grid place-items-center flex-shrink-0">
            <AlertTriangle size={16} />
          </div>
          <div>
            <p className="text-sm font-semibold text-white">This page crashed</p>
            <p className="text-xs text-[var(--text-muted)] break-words">
              {error.message || String(error)}
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={onReset}
            className="flex items-center gap-2 px-3 py-1.5 rounded-md bg-white/5 hover:bg-white/10 text-xs font-medium text-white transition"
          >
            <RefreshCw size={12} />
            Try again
          </button>
          <a
            href="/"
            className="flex items-center gap-2 px-3 py-1.5 rounded-md bg-white/5 hover:bg-white/10 text-xs font-medium text-[var(--text-secondary)] hover:text-white transition"
          >
            <Home size={12} />
            Home
          </a>
          <ActionButtons error={error} compact />
        </div>
      </div>
    </div>
  );
}

function ActionButtons({ error, compact = false }: { error: Error; compact?: boolean }) {
  const cls = compact
    ? 'flex items-center gap-2 px-3 py-1.5 rounded-md bg-white/5 hover:bg-white/10 text-xs font-medium text-[var(--text-secondary)] hover:text-white transition'
    : 'flex items-center gap-2 px-4 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white font-medium text-sm transition';
  const iconSize = compact ? 12 : 14;
  const route = typeof window === 'undefined' ? '?' : window.location.pathname;
  return (
    <>
      <button
        type="button"
        onClick={async () => {
          const ok = await copyDiagnostics('render-error');
          if (ok) kinoToast.success('Diagnostics copied to clipboard');
          else kinoToast.error("Couldn't copy — clipboard unavailable");
        }}
        className={cls}
      >
        <Copy size={iconSize} />
        Copy diagnostics
      </button>
      <a
        href={buildIssueUrl(`Render error: ${error.message.slice(0, 80)}`, route)}
        target="_blank"
        rel="noopener noreferrer"
        className={cls}
      >
        <ExternalLink size={iconSize} />
        Report issue
      </a>
    </>
  );
}
