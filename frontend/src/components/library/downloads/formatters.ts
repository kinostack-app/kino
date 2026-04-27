/**
 * Formatting helpers shared across the Downloads tab — table rows,
 * detail pane, peer list. Extracted to keep the components thin.
 */

import type { DownloadState } from '@/api/generated/types.gen';

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || bytes < 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatSpeed(bps: number | null | undefined): string {
  if (!bps || bps <= 0) return '—';
  if (bps < 1024) return `${bps} B/s`;
  if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${(bps / (1024 * 1024)).toFixed(1)} MB/s`;
}

export function formatEta(seconds: number | null | undefined): string {
  if (!seconds || seconds <= 0) return '—';
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

export function formatRelativeTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const ms = Date.now() - Date.parse(iso);
  if (!Number.isFinite(ms) || ms < 0) return '—';
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export const ACTIVE_STATES: ReadonlySet<DownloadState> = new Set<DownloadState>([
  'queued',
  'grabbing',
  'downloading',
  'stalled',
  'importing',
]);

export const TERMINAL_STATES: ReadonlySet<DownloadState> = new Set<DownloadState>([
  'failed',
  'imported',
]);

export interface StateDisplay {
  label: string;
  /** Tailwind bg/text/ring classes for the state chip. */
  chipClass: string;
  /** Progress-bar color. */
  barClass: string;
}

export function stateDisplay(state: string): StateDisplay {
  switch (state) {
    case 'queued':
      return {
        label: 'Queued',
        chipClass: 'bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10',
        barClass: 'bg-white/10',
      };
    case 'grabbing':
      return {
        label: 'Metadata',
        chipClass: 'bg-sky-500/15 text-sky-300 ring-1 ring-sky-500/30',
        barClass: 'bg-sky-500',
      };
    case 'downloading':
      return {
        label: 'Downloading',
        chipClass: 'bg-sky-500/15 text-sky-300 ring-1 ring-sky-500/30',
        barClass: 'bg-sky-500',
      };
    case 'stalled':
      return {
        label: 'Stalled',
        chipClass: 'bg-amber-500/15 text-amber-300 ring-1 ring-amber-500/30',
        barClass: 'bg-amber-500',
      };
    case 'paused':
      return {
        label: 'Paused',
        chipClass: 'bg-amber-500/15 text-amber-300 ring-1 ring-amber-500/30',
        barClass: 'bg-amber-500',
      };
    case 'seeding':
      return {
        label: 'Seeding',
        chipClass: 'bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/30',
        barClass: 'bg-emerald-500',
      };
    case 'completed':
      return {
        label: 'Completed',
        chipClass: 'bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/30',
        barClass: 'bg-emerald-500',
      };
    case 'importing':
      return {
        label: 'Importing',
        chipClass: 'bg-purple-500/15 text-purple-300 ring-1 ring-purple-500/30',
        barClass: 'bg-purple-500',
      };
    case 'imported':
      return {
        label: 'Imported',
        chipClass: 'bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/30',
        barClass: 'bg-emerald-500',
      };
    case 'failed':
      return {
        label: 'Failed',
        chipClass: 'bg-rose-500/15 text-rose-300 ring-1 ring-rose-500/30',
        barClass: 'bg-rose-500',
      };
    default:
      return {
        label: state,
        chipClass: 'bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10',
        barClass: 'bg-white/20',
      };
  }
}
