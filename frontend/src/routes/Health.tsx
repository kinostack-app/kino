/**
 * /health — live-state dashboard that composes every subsystem panel.
 *
 * Pure observability: every card has a "Manage" link out to the
 * relevant settings page for actions. The dashboard itself never
 * mutates state. Backed by a single `/api/v1/health` snapshot; the
 * backend fans out to per-panel collectors in parallel with a 500ms
 * budget each. Refetches on a 15-second interval so state drifts
 * feel current without hammering the server.
 */

import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import {
  AlertTriangle,
  Database,
  Download,
  Film,
  HelpCircle,
  ListChecks,
  Radio,
  Search,
  Shield,
  Zap,
} from 'lucide-react';
import type { ReactNode } from 'react';
import { getHealth } from '@/api/generated/sdk.gen';
import type { HealthResponse, HealthStatus } from '@/api/generated/types.gen';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { cn } from '@/lib/utils';

function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(0)} MB`;
  if (bytes < 1024 ** 4) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  return `${(bytes / 1024 ** 4).toFixed(2)} TB`;
}

function formatDuration(secs: number | null | undefined): string {
  if (secs == null) return '—';
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.round(secs / 60)}m ago`;
  if (secs < 86_400) return `${Math.round(secs / 3600)}h ago`;
  return `${Math.round(secs / 86_400)}d ago`;
}

/** Styling tokens keyed off the per-panel status. Operational is
 *  deliberately near-silent — the dot is the only tint — so a healthy
 *  dashboard reads calm instead of looking like a sea of green.
 *  Degraded/critical use amber/red to earn attention; only the dashboard
 *  summary line reaches for them. */
const STATUS_STYLES: Record<HealthStatus, { dot: string; text: string; ring: string }> = {
  operational: {
    dot: 'bg-emerald-500',
    text: 'text-[var(--text-muted)]',
    ring: 'ring-white/10',
  },
  degraded: {
    dot: 'bg-amber-500',
    text: 'text-amber-400',
    ring: 'ring-amber-500/25',
  },
  critical: {
    dot: 'bg-red-500',
    text: 'text-red-400',
    ring: 'ring-red-500/30',
  },
  unknown: {
    dot: 'bg-white/20',
    text: 'text-[var(--text-muted)]',
    ring: 'ring-white/10',
  },
};

/** Summary banner that mirrors the `overall` status. Operational is
 *  rendered as a neutral card with a small green dot — we don't want
 *  the page screaming "everything is fine" in green every time you
 *  look at it. Degraded/critical get the coloured ring so they stand
 *  out on a page that's otherwise calm. */
function OverallBanner({ overall }: { overall: HealthStatus }) {
  const message = {
    operational: 'All systems operational',
    degraded: 'Minor issues detected',
    critical: 'Issues detected',
    unknown: 'Gathering status…',
  }[overall];
  const s = STATUS_STYLES[overall];

  if (overall === 'operational' || overall === 'unknown') {
    return (
      <div className="flex items-center gap-3 px-4 py-3 rounded-xl bg-[var(--bg-card)] ring-1 ring-white/10">
        <span className={cn('inline-block w-2 h-2 rounded-full', s.dot)} />
        <span className="text-sm text-[var(--text-secondary)]">{message}</span>
      </div>
    );
  }

  return (
    <div
      className={cn(
        'flex items-center gap-3 px-4 py-3 rounded-xl bg-[var(--bg-card)] ring-1',
        s.ring
      )}
    >
      <AlertTriangle size={18} className={s.text} />
      <span className={cn('text-sm font-medium', s.text)}>{message}</span>
    </div>
  );
}

interface HealthCardProps {
  title: string;
  icon: ReactNode;
  status: HealthStatus;
  summary: string;
  manageTo?: string;
  children?: ReactNode;
}

function HealthCard({ title, icon, status, summary, manageTo, children }: HealthCardProps) {
  const s = STATUS_STYLES[status];
  return (
    <div className="rounded-xl bg-[var(--bg-card)] ring-1 ring-white/10 p-4 flex flex-col gap-3">
      <div className="flex items-start gap-3">
        <div className="mt-0.5 text-[var(--text-secondary)]">{icon}</div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-semibold text-white truncate">{title}</h3>
            <span
              className={cn('inline-block w-1.5 h-1.5 rounded-full', s.dot)}
              title={`Status: ${status}`}
            />
          </div>
          <p className={cn('text-xs mt-0.5 truncate', s.text)}>{summary}</p>
        </div>
        {manageTo && (
          <Link
            to={manageTo}
            className="text-[11px] text-[var(--text-muted)] hover:text-white transition-colors shrink-0"
          >
            Manage →
          </Link>
        )}
      </div>
      {children && <div className="text-xs text-[var(--text-secondary)] space-y-1">{children}</div>}
    </div>
  );
}

function StoragePanel({ panel }: { panel: HealthResponse['panels']['storage'] }) {
  // Group folders by `device_id` so a single drive isn't repeated three
  // times when kino's data, library, and downloads paths all live on
  // the same disk. Paths without a device_id (e.g. Windows) fall into
  // their own per-path "drive" — the page still renders, just without
  // the grouping benefit.
  const groups = (() => {
    type Group = {
      key: string;
      free_bytes: number | null | undefined;
      total_bytes: number | null | undefined;
      free_pct: number | null | undefined;
      paths: typeof panel.paths;
    };
    const map = new Map<string, Group>();
    for (const p of panel.paths) {
      const key = p.device_id != null ? `dev:${p.device_id}` : `path:${p.path}`;
      const existing = map.get(key);
      if (existing) {
        existing.paths.push(p);
      } else {
        map.set(key, {
          key,
          free_bytes: p.free_bytes,
          total_bytes: p.total_bytes,
          free_pct: p.free_pct,
          paths: [p],
        });
      }
    }
    return Array.from(map.values());
  })();

  return (
    <HealthCard
      title="Storage"
      icon={<Database size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/settings/library"
    >
      {groups.map((g) => {
        const pct = g.free_pct ?? 0;
        // The filled part represents space *used*. Neutral white while
        // there's headroom — only escalate into amber/red at the
        // thresholds the backend flags.
        const color = pct < 2 ? 'bg-red-500' : pct < 10 ? 'bg-amber-500' : 'bg-[var(--text-muted)]';
        return (
          <div key={g.key} className="space-y-1.5">
            <div className="flex items-baseline justify-between gap-2">
              <span className="text-[var(--text-muted)] text-[10px] uppercase tracking-wide">
                Drive {g.paths[0].path.split('/')[1] || '/'}
              </span>
              <span className="tabular-nums text-[var(--text-muted)] shrink-0">
                {formatBytes(g.free_bytes)} free of {formatBytes(g.total_bytes)}
              </span>
            </div>
            <div className="h-1 rounded-full bg-[var(--bg-elevated)] overflow-hidden">
              <div
                className={cn('h-full transition-all', color)}
                style={{ width: `${100 - pct}%` }}
              />
            </div>
            <div className="pl-2 space-y-0.5">
              {g.paths.map((p) => (
                <div key={p.path} className="flex items-baseline justify-between gap-2 text-[11px]">
                  <span className="text-white truncate">{p.label}</span>
                  <span className="tabular-nums text-[var(--text-muted)] shrink-0">
                    {p.used_bytes != null ? `${formatBytes(p.used_bytes)} used` : '—'}
                  </span>
                </div>
              ))}
            </div>
          </div>
        );
      })}
    </HealthCard>
  );
}

function VpnPanel({ panel }: { panel: NonNullable<HealthResponse['panels']['vpn']> }) {
  return (
    <HealthCard
      title="VPN"
      icon={<Shield size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/settings/vpn"
    >
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Interface</span>
        <span className="text-white tabular-nums">{panel.interface ?? '—'}</span>
      </div>
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Forwarded port</span>
        <span className="text-white tabular-nums">{panel.forwarded_port ?? '—'}</span>
      </div>
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Last handshake</span>
        <span className="text-white tabular-nums">
          {formatDuration(panel.last_handshake_ago_secs)}
        </span>
      </div>
    </HealthCard>
  );
}

function IndexersPanel({ panel }: { panel: HealthResponse['panels']['indexers'] }) {
  return (
    <HealthCard
      title="Indexers"
      icon={<Search size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/settings/indexers"
    >
      <div className="flex gap-3 text-[11px]">
        <span>
          <span className="text-white tabular-nums">{panel.healthy}</span>
          <span className="text-[var(--text-muted)] ml-1">healthy</span>
        </span>
        {panel.failing > 0 && (
          <span>
            <span className="text-amber-400 tabular-nums">{panel.failing}</span>
            <span className="text-[var(--text-muted)] ml-1">failing</span>
          </span>
        )}
        {panel.disabled > 0 && (
          <span>
            <span className="text-[var(--text-secondary)] tabular-nums">{panel.disabled}</span>
            <span className="text-[var(--text-muted)] ml-1">disabled</span>
          </span>
        )}
      </div>
      {/* Show the top three problematic indexers by name so an at-a-
          glance scan surfaces which ones actually need attention. */}
      {panel.items
        .filter((i) => i.state !== 'healthy')
        .slice(0, 3)
        .map((i) => (
          <div key={i.id} className="flex justify-between">
            <span className="text-[var(--text-secondary)] truncate">{i.name}</span>
            <span className={STATUS_STYLES[i.state === 'disabled' ? 'unknown' : 'degraded'].text}>
              {i.state}
            </span>
          </div>
        ))}
    </HealthCard>
  );
}

function DownloadsPanel({ panel }: { panel: HealthResponse['panels']['downloads'] }) {
  return (
    <HealthCard
      title="Downloads"
      icon={<Download size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/library/downloading"
    >
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Active</span>
        <span className="text-white tabular-nums">{panel.active}</span>
      </div>
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Importing</span>
        <span className="text-white tabular-nums">{panel.importing}</span>
      </div>
      {panel.stuck_importing > 0 && (
        <div className="flex justify-between">
          <span className="text-red-400">Stuck importing</span>
          <span className="text-red-400 tabular-nums">{panel.stuck_importing}</span>
        </div>
      )}
      {panel.failed_last_24h > 0 && (
        <div className="flex justify-between">
          <span className="text-[var(--text-muted)]">Failed (24h)</span>
          <span className="text-amber-400 tabular-nums">{panel.failed_last_24h}</span>
        </div>
      )}
    </HealthCard>
  );
}

function TranscoderPanel({ panel }: { panel: HealthResponse['panels']['transcoder'] }) {
  return (
    <HealthCard
      title="Transcoder"
      icon={<Film size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/settings/playback"
    >
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Sessions</span>
        <span className="text-white tabular-nums">
          {panel.active_sessions} / {panel.session_cap}
        </span>
      </div>
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">FFmpeg</span>
        <span className={panel.ffmpeg_available ? 'text-white' : 'text-red-400'}>
          {panel.ffmpeg_available ? 'available' : 'missing'}
        </span>
      </div>
      <div className="flex justify-between">
        <span className="text-[var(--text-muted)]">Hardware</span>
        <span className="text-white">{panel.hw_acceleration}</span>
      </div>
      {panel.hw_available_but_off && (
        <p className="text-[11px] text-amber-400 pt-1">Hardware acceleration available but off</p>
      )}
    </HealthCard>
  );
}

function SchedulerPanel({ panel }: { panel: HealthResponse['panels']['scheduler'] }) {
  return (
    <HealthCard
      title="Scheduler"
      icon={<ListChecks size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/settings/tasks"
    >
      {panel.failing_tasks.length === 0 ? (
        <p className="text-[var(--text-muted)] italic">All tasks running cleanly</p>
      ) : (
        panel.failing_tasks.slice(0, 3).map((t) => (
          <div key={t.name} className="space-y-0.5">
            <div className="flex justify-between">
              <span className="text-white">{t.name}</span>
              <span className="text-red-400 text-[11px]">failed</span>
            </div>
            <p className="text-[11px] text-[var(--text-muted)] truncate">{t.last_error}</p>
          </div>
        ))
      )}
    </HealthCard>
  );
}

function MetadataPanel({ panel }: { panel: HealthResponse['panels']['metadata'] }) {
  return (
    <HealthCard
      title="Metadata"
      icon={<Radio size={16} />}
      status={panel.status}
      summary={panel.summary}
      manageTo="/settings/metadata"
    />
  );
}

export function Health() {
  useDocumentTitle('Health');

  const { data, isLoading, error } = useQuery<HealthResponse | null>({
    queryKey: ['kino', 'health'],
    queryFn: async () => {
      const res = await getHealth();
      return (res.data as HealthResponse | undefined) ?? null;
    },
    // Event-driven refresh via `meta.invalidatedBy` — every state-
    // changing backend path already emits one of these variants, so
    // the page stays live without polling in the common case.
    meta: {
      invalidatedBy: [
        'indexer_changed',
        'config_changed',
        'download_started',
        'download_complete',
        'download_failed',
        'download_cancelled',
        'download_paused',
        'download_resumed',
        'imported',
        'upgraded',
        'content_removed',
        'health_warning',
        'health_recovered',
        'ip_leak_detected',
      ],
    },
    // Safety net: if a backend emits nothing (e.g. a subsystem
    // panics between state-change and event send), refresh every
    // 30s so the page doesn't drift permanently stale. When the
    // full WS tagging is complete this can drop to a longer
    // interval — the goal is "visibly live", not "polled every tick".
    refetchInterval: 30_000,
    refetchIntervalInBackground: false,
  });

  if (error) {
    return (
      <div className="px-4 md:px-6 py-6 max-w-6xl mx-auto space-y-4">
        <h1 className="text-2xl font-bold text-white">Health</h1>
        <div className="rounded-xl bg-red-500/10 ring-1 ring-red-500/20 px-4 py-3 text-sm text-red-300">
          Couldn&apos;t load health state.
        </div>
      </div>
    );
  }

  if (isLoading || !data) {
    return (
      <div className="px-4 md:px-6 py-6 max-w-6xl mx-auto space-y-4">
        <h1 className="text-2xl font-bold text-white">Health</h1>
        <div className="rounded-xl bg-[var(--bg-card)] ring-1 ring-white/10 px-4 py-8 text-sm text-[var(--text-muted)] flex items-center gap-3">
          <div className="w-4 h-4 rounded-full border-2 border-white/10 border-t-white/60 animate-spin" />
          Gathering status…
        </div>
      </div>
    );
  }

  const p = data.panels;

  return (
    <div className="px-4 md:px-6 py-6 max-w-6xl mx-auto space-y-4">
      <div className="flex items-baseline justify-between">
        <div>
          <h1 className="text-2xl font-bold text-white">Health</h1>
          <p className="text-sm text-[var(--text-muted)] mt-0.5">
            Live state across every kino subsystem.
          </p>
        </div>
        <div className="text-[11px] text-[var(--text-muted)] flex items-center gap-1.5">
          <HelpCircle size={12} />
          <span>Refreshes every 15s</span>
        </div>
      </div>

      <OverallBanner overall={data.overall} />

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
        <StoragePanel panel={p.storage} />
        {p.vpn && <VpnPanel panel={p.vpn} />}
        <IndexersPanel panel={p.indexers} />
        <DownloadsPanel panel={p.downloads} />
        <TranscoderPanel panel={p.transcoder} />
        <SchedulerPanel panel={p.scheduler} />
        <MetadataPanel panel={p.metadata} />
      </div>

      <p className="text-[11px] text-[var(--text-muted)] text-right">
        Checked {new Date(data.checked_at).toLocaleTimeString()} ·
        <Zap size={10} className="inline mx-1" />
        Instantaneous
      </p>
    </div>
  );
}
