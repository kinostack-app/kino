import * as Dialog from '@radix-ui/react-dialog';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Download, Loader2, X } from 'lucide-react';
import { useMemo } from 'react';
import { episodeReleases, grabRelease, movieReleases } from '@/api/generated/sdk.gen';
import type { ReleaseWithStatus } from '@/api/generated/types.gen';
import { cn } from '@/lib/utils';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

interface Scope {
  kind: 'episode' | 'movie';
  id: number;
  /** Shown in the dialog header — e.g. "Severance · S01E01" or a movie title. */
  subtitle?: string;
}

interface Props {
  open: boolean;
  onClose: () => void;
  scope: Scope | null;
}

/**
 * Release-history / manual-picker drawer.
 *
 * Surfaces every release we've seen for an episode or movie with the
 * outcome of our own grab attempts (downloaded / failed / blocklisted)
 * and a Grab button per row so the user can override kino's automatic
 * pick when the best-scored release is stuck, broken, or the wrong
 * encode. Pairs with the auto-retry listener: a failed grab will have
 * already been tombstoned, so the same release doesn't show up as
 * grabbable again.
 */
export function ReleasesDialog({ open, onClose, scope }: Props) {
  // Prefixed with 'kino' so the central WS handler's library-event
  // invalidation (which targets `queryKey[0] === 'kino'`) catches
  // release state changes without a per-query wire-up.
  const queryKey = useMemo(
    () => ['kino', 'releases', scope?.kind, scope?.id ?? null],
    [scope?.kind, scope?.id]
  );

  const query = useQuery({
    queryKey,
    queryFn: async () => {
      if (!scope) return [];
      if (scope.kind === 'episode') {
        const r = await episodeReleases({ path: { id: scope.id } });
        return (r.data as ReleaseWithStatus[] | undefined) ?? [];
      }
      const r = await movieReleases({ path: { id: scope.id } });
      return (r.data as ReleaseWithStatus[] | undefined) ?? [];
    },
    enabled: open && scope != null,
    // No polling — release rows flip to `grabbed` / `downloading`
    // via release_grabbed + download-lifecycle events. Meta drives
    // invalidation via the central dispatcher.
    meta: {
      invalidatedBy: [
        'release_grabbed',
        'search_started',
        'download_started',
        'download_complete',
        'download_failed',
        'download_cancelled',
        'imported',
        'upgraded',
      ],
    },
  });

  const qc = useQueryClient();
  const grab = useMutationWithToast({
    verb: 'grab release',
    mutationFn: async (id: number) => {
      await grabRelease({ path: { id } });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey });
      // Also nudge the downloads pane so a fresh grab appears there.
      qc.invalidateQueries({ queryKey: ['listDownloads'] });
    },
  });

  const releases = query.data ?? [];

  return (
    <Dialog.Root open={open} onOpenChange={(next) => (next ? null : onClose())}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/70 backdrop-blur-sm data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2 w-[min(58rem,calc(100vw-2rem))] max-h-[min(calc(100vh-4rem),44rem)] flex flex-col bg-[var(--bg-secondary)] rounded-xl ring-1 ring-white/10 shadow-2xl overflow-hidden data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95">
          <div className="flex items-center justify-between px-5 py-4 border-b border-white/5">
            <div className="min-w-0">
              <Dialog.Title className="text-lg font-semibold">Releases</Dialog.Title>
              {scope?.subtitle && (
                <Dialog.Description className="text-sm text-[var(--text-muted)] mt-0.5 truncate">
                  {scope.subtitle}
                </Dialog.Description>
              )}
            </div>
            <Dialog.Close asChild>
              <button
                type="button"
                className="p-1.5 rounded-lg hover:bg-white/10 text-[var(--text-muted)] hover:text-white transition"
                aria-label="Close"
              >
                <X size={18} />
              </button>
            </Dialog.Close>
          </div>

          <div className="flex-1 overflow-y-auto">
            {query.isLoading && (
              <div className="p-8 flex items-center justify-center text-[var(--text-muted)]">
                <Loader2 size={18} className="animate-spin mr-2" /> Loading releases…
              </div>
            )}
            {!query.isLoading && releases.length === 0 && (
              <div className="p-8 text-center text-[var(--text-muted)] text-sm">
                No releases indexed yet. Search hasn't run, or no indexer returned results for this
                title.
              </div>
            )}
            {releases.length > 0 && (
              <table className="w-full text-sm">
                <thead className="sticky top-0 bg-[var(--bg-secondary)] z-10">
                  <tr className="text-[10px] uppercase tracking-wide text-[var(--text-muted)] border-b border-white/5">
                    <th className="text-left px-4 py-2 font-medium">Release</th>
                    <th className="text-left px-2 py-2 font-medium">Quality</th>
                    <th className="text-right px-2 py-2 font-medium">Size</th>
                    <th className="text-left px-2 py-2 font-medium">Indexer</th>
                    <th className="text-right px-2 py-2 font-medium">Seeds</th>
                    <th className="text-right px-2 py-2 font-medium">Age</th>
                    <th className="text-right px-2 py-2 font-medium">Score</th>
                    <th className="text-left px-2 py-2 font-medium">Status</th>
                    <th className="text-right px-4 py-2 font-medium" />
                  </tr>
                </thead>
                <tbody>
                  {releases.map((r) => (
                    <ReleaseRow
                      key={r.id}
                      release={r}
                      onGrab={() => grab.mutate(r.id)}
                      grabbing={grab.isPending && grab.variables === r.id}
                    />
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="px-5 py-3 border-t border-white/5 text-[11px] text-[var(--text-muted)]">
            {releases.length > 0 && (
              <>
                Showing {releases.length} release{releases.length !== 1 ? 's' : ''} · sorted by
                score
              </>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ReleaseRow({
  release,
  onGrab,
  grabbing,
}: {
  release: ReleaseWithStatus;
  onGrab: () => void;
  grabbing: boolean;
}) {
  const qualityLabel = [
    release.source,
    release.resolution != null ? `${release.resolution}p` : null,
    release.video_codec,
    release.is_remux ? 'Remux' : null,
    release.is_proper ? 'PROPER' : null,
    release.is_repack ? 'REPACK' : null,
  ]
    .filter(Boolean)
    .join(' · ');

  const ageDays = (() => {
    if (!release.publish_date) return null;
    const ms = Date.now() - Date.parse(release.publish_date);
    if (!Number.isFinite(ms) || ms < 0) return null;
    const days = Math.round(ms / (24 * 60 * 60 * 1000));
    return days;
  })();

  const statusChip = statusFor(release);

  // A release is "grabbable" when we don't already have an in-flight or
  // imported download for it, and it isn't blocklisted. We still show
  // grabbed/failed states so the user can see the attempt history —
  // the Grab button just becomes inert for those.
  const grabbable =
    !release.is_blocklisted &&
    (release.download_state == null ||
      release.download_state === 'failed' ||
      release.download_state === 'completed');

  return (
    <tr
      className={cn(
        'border-b border-white/5 hover:bg-white/[0.02] transition',
        release.is_blocklisted && 'opacity-50'
      )}
    >
      <td className="px-4 py-2.5 max-w-[24rem] truncate" title={release.title}>
        {release.title}
      </td>
      <td className="px-2 py-2.5 text-[var(--text-secondary)] text-xs">{qualityLabel || '—'}</td>
      <td className="px-2 py-2.5 text-right tabular-nums text-[var(--text-secondary)]">
        {formatSize(release.size)}
      </td>
      <td className="px-2 py-2.5 text-[var(--text-secondary)]">{release.indexer_name ?? '—'}</td>
      <td className="px-2 py-2.5 text-right tabular-nums text-[var(--text-secondary)]">
        {release.seeders ?? '—'}
      </td>
      <td className="px-2 py-2.5 text-right tabular-nums text-[var(--text-secondary)]">
        {ageDays != null ? `${ageDays}d` : '—'}
      </td>
      <td className="px-2 py-2.5 text-right tabular-nums text-[var(--text-secondary)]">
        {release.quality_score ?? '—'}
      </td>
      <td className="px-2 py-2.5">
        <span
          className={cn(
            'inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium',
            statusChip.bg
          )}
          title={release.download_error ?? undefined}
        >
          {statusChip.label}
        </span>
      </td>
      <td className="px-4 py-2.5 text-right">
        <button
          type="button"
          disabled={!grabbable || grabbing}
          onClick={onGrab}
          className={cn(
            'inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition',
            grabbable && !grabbing
              ? 'bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white'
              : 'bg-white/5 text-[var(--text-muted)] cursor-not-allowed'
          )}
        >
          {grabbing ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
          {release.download_state === 'failed' ? 'Retry' : 'Grab'}
        </button>
      </td>
    </tr>
  );
}

function statusFor(r: ReleaseWithStatus): { label: string; bg: string } {
  if (r.is_blocklisted) {
    return { label: 'Blocked', bg: 'bg-rose-500/15 text-rose-300 ring-1 ring-rose-500/30' };
  }
  switch (r.download_state) {
    case 'imported':
      return {
        label: 'In library',
        bg: 'bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/30',
      };
    case 'completed':
    case 'seeding':
      return {
        label: r.download_state,
        bg: 'bg-emerald-500/10 text-emerald-300 ring-1 ring-emerald-500/20',
      };
    case 'downloading':
    case 'grabbing':
    case 'queued':
    case 'importing':
      return { label: r.download_state, bg: 'bg-sky-500/15 text-sky-300 ring-1 ring-sky-500/30' };
    case 'paused':
    case 'stalled':
      return {
        label: r.download_state,
        bg: 'bg-amber-500/15 text-amber-300 ring-1 ring-amber-500/30',
      };
    case 'failed':
      return { label: 'Failed', bg: 'bg-rose-500/15 text-rose-300 ring-1 ring-rose-500/30' };
    default:
      return { label: 'Available', bg: 'bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10' };
  }
}

function formatSize(bytes: number | null | undefined): string {
  if (bytes == null || bytes <= 0) return '—';
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  const mb = bytes / 1024 ** 2;
  return `${mb.toFixed(0)} MB`;
}
