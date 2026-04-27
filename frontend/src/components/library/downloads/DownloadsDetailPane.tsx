import { useQuery } from '@tanstack/react-query';
import { AlertTriangle, FileText, Info, Loader2, Users } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { downloadFiles, downloadPeers, downloadPieces } from '@/api/generated/sdk.gen';
import { cn } from '@/lib/utils';
import { on } from '@/state/invalidation';
import type { ActiveDownload } from '@/state/library-cache';
import { formatBytes, formatRelativeTime, formatSpeed, stateDisplay } from './formatters';

type TabId = 'overview' | 'files' | 'peers';

/**
 * Bottom drawer below the downloads table. Shows details for the
 * currently-focused torrent (last-clicked row). Checkbox selection is
 * independent and drives bulk actions only — keeping the two concepts
 * separate is a standard table-with-detail-pane pattern.
 */
export function DownloadsDetailPane({
  download,
  onClose,
}: {
  download: ActiveDownload | null;
  onClose?: () => void;
}) {
  const [tab, setTab] = useState<TabId>('overview');

  if (!download) {
    return (
      <div className="h-full border-t border-white/5 flex items-center justify-center text-sm text-[var(--text-muted)] bg-[var(--bg-secondary)]/40">
        Click a row to view details
      </div>
    );
  }

  return (
    <div className="h-full border-t border-white/5 flex flex-col bg-[var(--bg-secondary)]/40">
      <div className="flex items-center justify-between px-3 border-b border-white/5">
        <div className="flex items-center gap-1">
          <TabBtn active={tab === 'overview'} onClick={() => setTab('overview')}>
            <Info size={12} /> Overview
          </TabBtn>
          <TabBtn active={tab === 'files'} onClick={() => setTab('files')}>
            <FileText size={12} /> Files
          </TabBtn>
          <TabBtn active={tab === 'peers'} onClick={() => setTab('peers')}>
            <Users size={12} /> Peers
          </TabBtn>
        </div>
        {onClose && (
          <button
            type="button"
            onClick={onClose}
            className="text-[11px] text-[var(--text-muted)] hover:text-white"
          >
            Hide
          </button>
        )}
      </div>
      <div className="flex-1 overflow-auto p-4 text-sm">
        {tab === 'overview' && <OverviewTab download={download} />}
        {tab === 'files' && <FilesTab download={download} />}
        {tab === 'peers' && <PeersTab download={download} />}
      </div>
    </div>
  );
}

function TabBtn({
  children,
  active,
  onClick,
}: {
  children: React.ReactNode;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'px-3 py-2 text-xs font-medium inline-flex items-center gap-1.5 border-b-2 transition',
        active
          ? 'border-[var(--accent)] text-white'
          : 'border-transparent text-[var(--text-muted)] hover:text-white'
      )}
    >
      {children}
    </button>
  );
}

function OverviewTab({ download }: { download: ActiveDownload }) {
  const state = stateDisplay(download.state);
  const progress =
    download.size && download.size > 0 ? (download.downloaded / download.size) * 100 : 0;

  return (
    <div className="space-y-3 max-w-3xl">
      <div>
        <p className="text-xs text-[var(--text-muted)] mb-1">Title</p>
        <p className="text-white font-medium break-words">{download.title}</p>
      </div>
      {download.error_message && (
        <div className="flex items-start gap-2 p-2.5 rounded-md bg-rose-500/10 ring-1 ring-rose-500/30">
          <AlertTriangle size={14} className="text-rose-300 mt-0.5 flex-shrink-0" />
          <p className="text-xs text-rose-200">{download.error_message}</p>
        </div>
      )}
      <PiecesBar download={download} />
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-6 gap-x-6 gap-y-3">
        <Field label="State">
          <span
            className={cn(
              'inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium',
              state.chipClass
            )}
          >
            {state.label}
          </span>
        </Field>
        <Field label="Progress">{progress.toFixed(1)}%</Field>
        <Field label="Size">{formatBytes(download.size)}</Field>
        <Field label="Downloaded">{formatBytes(download.downloaded)}</Field>
        <Field label="Uploaded">{formatBytes(download.uploaded)}</Field>
        <Field label="↓ Speed">{formatSpeed(download.download_speed)}</Field>
        <Field label="↑ Speed">{formatSpeed(download.upload_speed)}</Field>
        <Field label="Peers" allowWrap>
          <span className="whitespace-nowrap">{download.seeders ?? 0} connected</span>
          {download.leechers != null && download.leechers > 0 && (
            <>
              {' '}
              <span className="whitespace-nowrap text-[var(--text-muted)]">
                · {download.leechers} known
              </span>
            </>
          )}
        </Field>
        <Field label="Added">{formatRelativeTime(download.added_at)}</Field>
        <Field label="Completed">{formatRelativeTime(download.completed_at)}</Field>
        <Field label="Info hash" allowWrap>
          <code className="text-[11px] text-[var(--text-secondary)] font-mono break-all">
            {download.torrent_hash ?? '—'}
          </code>
        </Field>
      </div>
    </div>
  );
}

function Field({
  label,
  children,
  allowWrap,
  span,
}: {
  label: string;
  children: React.ReactNode;
  /** Opt-in to content that may overflow a single line (e.g. multi-part
   *  stats like "N connected · M known"). Default `truncate` keeps the
   *  grid row heights stable for simple scalar values. */
  allowWrap?: boolean;
  /** Extra Tailwind classes for wider fields (e.g. info-hash spanning
   *  multiple columns so the full SHA-1 is visible). */
  span?: string;
}) {
  return (
    <div className={cn('min-w-0', span)}>
      <p className="text-[10px] uppercase tracking-wide text-[var(--text-muted)] mb-1">{label}</p>
      <div className={cn('text-sm text-white', !allowWrap && 'truncate')}>{children}</div>
    </div>
  );
}

function FilesTab({ download }: { download: ActiveDownload }) {
  // No polling — the dispatcher invalidates only this download's
  // entry when `download_metadata_ready` (librqbit info-dict) or any
  // state-transition event for THIS download fires. Scoped match on
  // `download_id` stops us refetching every download's file list on
  // every other download's events.
  const query = useQuery({
    queryKey: ['downloadFiles', download.id],
    queryFn: async () => {
      const { data } = await downloadFiles({ path: { id: download.id } });
      return data ?? { files: [], metadata_pending: true };
    },
    meta: {
      invalidatedBy: [
        on('download_metadata_ready', (e) => e.download_id === download.id),
        on('download_started', (e) => e.download_id === download.id),
        on('download_complete', (e) => e.download_id === download.id),
        on('download_failed', (e) => e.download_id === download.id),
        on('download_cancelled', (e) => e.download_id === download.id),
      ],
    },
  });

  if (query.isLoading) {
    return (
      <div className="flex items-center gap-2 text-[var(--text-muted)]">
        <Loader2 size={14} className="animate-spin" /> Loading files…
      </div>
    );
  }
  if (query.data?.metadata_pending) {
    return (
      <p className="text-[var(--text-muted)] text-xs">
        Torrent metadata still resolving — file list will appear once the info-dict arrives.
      </p>
    );
  }
  const files = query.data?.files ?? [];
  if (files.length === 0) {
    return <p className="text-[var(--text-muted)] text-xs">No files.</p>;
  }
  return (
    <table className="w-full text-xs">
      <thead>
        <tr className="border-b border-white/5 text-[10px] uppercase tracking-wide text-[var(--text-muted)]">
          <th className="text-left px-2 py-1.5">File</th>
          <th className="text-right px-2 py-1.5 w-24">Size</th>
          <th className="text-center px-2 py-1.5 w-20">Selected</th>
        </tr>
      </thead>
      <tbody>
        {files.map((f) => (
          <tr key={f.index} className="border-b border-white/5">
            <td className="px-2 py-1.5 truncate max-w-[32rem]" title={f.path}>
              {f.path}
            </td>
            <td className="px-2 py-1.5 text-right tabular-nums text-[var(--text-secondary)]">
              {formatBytes(f.size)}
            </td>
            <td className="px-2 py-1.5 text-center">
              <span
                className={cn(
                  'inline-block w-2 h-2 rounded-full',
                  f.selected ? 'bg-emerald-400' : 'bg-white/20'
                )}
              />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function PeersTab({ download }: { download: ActiveDownload }) {
  const query = useQuery({
    queryKey: ['downloadPeers', download.id],
    queryFn: async () => {
      const { data } = await downloadPeers({ path: { id: download.id } });
      return data ?? { peers: [], not_live: true };
    },
    refetchInterval: 2_000,
  });

  if (query.isLoading) {
    return (
      <div className="flex items-center gap-2 text-[var(--text-muted)]">
        <Loader2 size={14} className="animate-spin" /> Loading peers…
      </div>
    );
  }
  if (query.data?.not_live) {
    return (
      <p className="text-[var(--text-muted)] text-xs">
        Torrent isn't live — no peer connections to show. (Paused, initializing, or terminal state.)
      </p>
    );
  }
  const peers = [...(query.data?.peers ?? [])].sort(
    (a, b) => (b.fetched_bytes ?? 0) - (a.fetched_bytes ?? 0)
  );
  if (peers.length === 0) {
    return <p className="text-[var(--text-muted)] text-xs">No peer connections yet.</p>;
  }
  return (
    <table className="w-full text-xs">
      <thead>
        <tr className="border-b border-white/5 text-[10px] uppercase tracking-wide text-[var(--text-muted)]">
          <th className="text-left px-2 py-1.5">Peer</th>
          <th className="text-left px-2 py-1.5">State</th>
          <th className="text-left px-2 py-1.5">Conn</th>
          <th className="text-right px-2 py-1.5">↓ Received</th>
          <th className="text-right px-2 py-1.5">↑ Sent</th>
          <th className="text-right px-2 py-1.5">Errors</th>
        </tr>
      </thead>
      <tbody>
        {peers.map((p) => (
          <tr key={p.addr} className="border-b border-white/5">
            <td className="px-2 py-1.5 font-mono text-[11px]">{p.addr}</td>
            <td className="px-2 py-1.5">
              <span
                className={cn(
                  'inline-flex px-1.5 py-0.5 rounded text-[10px]',
                  p.state === 'live'
                    ? 'bg-emerald-500/15 text-emerald-300'
                    : p.state === 'connecting' || p.state === 'queued'
                      ? 'bg-sky-500/15 text-sky-300'
                      : 'bg-white/5 text-[var(--text-muted)]'
                )}
              >
                {p.state}
              </span>
            </td>
            <td className="px-2 py-1.5 text-[var(--text-secondary)]">{p.conn_kind ?? '—'}</td>
            <td className="px-2 py-1.5 text-right tabular-nums">{formatBytes(p.fetched_bytes)}</td>
            <td className="px-2 py-1.5 text-right tabular-nums">{formatBytes(p.uploaded_bytes)}</td>
            <td className="px-2 py-1.5 text-right tabular-nums text-[var(--text-muted)]">
              {p.errors > 0 ? p.errors : '—'}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function PiecesBar({ download }: { download: ActiveDownload }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  // Pieces poll faster while downloading, slow while paused/seeding,
  // mirroring librqbit's webui — keeps the bar responsive during
  // active downloads without hammering the API on idle torrents.
  const isLive =
    download.state === 'downloading' ||
    download.state === 'stalled' ||
    download.state === 'grabbing';
  const interval = isLive ? 2_000 : 30_000;

  const query = useQuery({
    queryKey: ['downloadPieces', download.id],
    queryFn: async () => {
      const { data } = await downloadPieces({ path: { id: download.id } });
      return data ?? { bitmap_b64: '', total_pieces: 0, not_available: true };
    },
    refetchInterval: interval,
  });

  // Decode base64 bitmap once per tick; keep as Uint8Array so the
  // canvas effect can do bitwise reads in MSB0 order without re-
  // parsing on every resize.
  const bitmap = (() => {
    const b64 = query.data?.bitmap_b64;
    if (!b64) return null;
    try {
      const bin = atob(b64);
      const out = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
      return out;
    } catch {
      return null;
    }
  })();

  const totalPieces = query.data?.total_pieces ?? 0;

  // Render bitmap → run-length-coalesced fill rects on a thin canvas
  // sized to the container width. Re-render on every bitmap change.
  // ResizeObserver keeps the bar pixel-aligned when the drawer width
  // changes (e.g. window resize / sidebar toggle).
  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container || !bitmap || totalPieces === 0) return;

    const draw = () => {
      const ctx = canvas.getContext('2d');
      if (!ctx) return;
      const width = Math.max(container.clientWidth, 100);
      const height = 14;
      canvas.width = width;
      canvas.height = height;

      // Background = "missing" colour. Foreground = "have".
      ctx.fillStyle = 'rgba(255,255,255,0.06)';
      ctx.fillRect(0, 0, width, height);

      const hasPiece = (i: number): boolean => {
        const byte = i >> 3;
        const bit = 7 - (i & 7);
        return byte < bitmap.length && ((bitmap[byte] >> bit) & 1) === 1;
      };

      ctx.fillStyle = 'rgb(56,189,248)'; // sky-400
      const pieceWidth = width / totalPieces;
      let runStart = -1;
      for (let i = 0; i < totalPieces; i++) {
        if (hasPiece(i)) {
          if (runStart === -1) runStart = i;
        } else if (runStart !== -1) {
          ctx.fillRect(runStart * pieceWidth, 0, (i - runStart) * pieceWidth, height);
          runStart = -1;
        }
      }
      if (runStart !== -1) {
        ctx.fillRect(runStart * pieceWidth, 0, (totalPieces - runStart) * pieceWidth, height);
      }
    };

    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(container);
    return () => ro.disconnect();
  }, [bitmap, totalPieces]);

  if (query.isLoading) {
    return (
      <div className="flex items-center gap-2 text-[var(--text-muted)]">
        <Loader2 size={14} className="animate-spin" /> Loading pieces…
      </div>
    );
  }
  if (query.data?.not_available || totalPieces === 0) {
    return (
      <p className="text-[var(--text-muted)] text-xs">
        Piece bitmap not available — torrent metadata still resolving, or download terminal.
      </p>
    );
  }

  const havePieces = bitmap ? Array.from(bitmap).reduce((acc, b) => acc + popcount(b), 0) : 0;
  const cappedHavePieces = Math.min(havePieces, totalPieces);
  const pct = totalPieces > 0 ? (cappedHavePieces / totalPieces) * 100 : 0;

  return (
    <div className="space-y-2 max-w-3xl">
      <div ref={containerRef} className="w-full">
        <canvas
          ref={canvasRef}
          className="w-full rounded"
          style={{ height: '14px', imageRendering: 'pixelated' }}
        />
      </div>
      <p className="text-[11px] text-[var(--text-muted)] tabular-nums">
        {cappedHavePieces.toLocaleString()} / {totalPieces.toLocaleString()} pieces ·{' '}
        {pct.toFixed(1)}%
      </p>
    </div>
  );
}

function popcount(b: number): number {
  let n = b;
  n = n - ((n >> 1) & 0x55);
  n = (n & 0x33) + ((n >> 2) & 0x33);
  return (n + (n >> 4)) & 0x0f;
}
