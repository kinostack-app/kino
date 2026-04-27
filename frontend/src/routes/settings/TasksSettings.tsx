/**
 * Scheduled tasks admin — lists kino's background tasks with their
 * interval, last/next run and a manual trigger.
 *
 * Backend: GET /api/v1/tasks, POST /api/v1/tasks/{name}/run.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { AlertTriangle, CheckCircle2, Eye, EyeOff, Play, RefreshCw } from 'lucide-react';
import { useMemo, useState } from 'react';
import {
  listTasksOptions,
  listTasksQueryKey,
  runTaskMutation,
} from '@/api/generated/@tanstack/react-query.gen';
import type { TaskInfo } from '@/api/generated/types.gen';
import { kinoToast } from '@/components/kino-toast';

interface TaskMeta {
  label: string;
  description: string;
  /**
   * Where the interval actually comes from. `config` rows point the
   * user at the Automation settings page so they don't try to
   * change it here. `internal` rows are hidden behind the "Show
   * internal" toggle — typically sub-minute polls that would
   * clutter the list.
   */
  source?: 'config' | 'internal';
}

const TASK_LABELS: Record<string, TaskMeta> = {
  wanted_search: {
    label: 'Wanted search',
    description: 'Search indexers for wanted content + upgrade candidates',
    source: 'config',
  },
  stale_download_check: {
    label: 'Download monitor',
    description: 'Poll librqbit for active torrent progress',
    source: 'internal',
  },
  cleanup: {
    label: 'Cleanup',
    description: 'Remove watched content past the configured delay',
  },
  metadata_refresh: {
    label: 'Metadata refresh',
    description: 'Re-fetch TMDB data + detect new episodes for monitored shows',
    source: 'config',
  },
  indexer_health: {
    label: 'Indexer health',
    description: 'Ping each indexer, auto-disable on failure with backoff',
  },
  webhook_retry: {
    label: 'Webhook retry',
    description: 'Re-enable webhooks whose failure backoff has expired',
  },
  vpn_health: {
    label: 'VPN health',
    description: 'Check WireGuard handshake age, reconnect if stale',
  },
  transcode_cleanup: {
    label: 'Transcode cleanup',
    description: 'Kill idle transcode sessions + remove orphan temp dirs',
  },
  trickplay_generation: {
    label: 'Trickplay generation',
    description: 'Generate hover-preview sprite sheets for new media',
  },
  log_retention: {
    label: 'Log retention',
    description: 'Cap the log_entry table — deletes oldest rows past the limit',
  },
};

function formatInterval(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)}h`;
  return `${Math.round(seconds / 86400)}d`;
}

function formatDurationMs(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms / 60_000)}m`;
}

function formatRelative(iso: string | null | undefined): string {
  if (!iso) return 'never';
  const t = new Date(iso).getTime();
  const delta = Date.now() - t;
  if (delta < 0) {
    const abs = Math.abs(delta);
    if (abs < 60_000) return `in ${Math.round(abs / 1000)}s`;
    if (abs < 3_600_000) return `in ${Math.round(abs / 60_000)}m`;
    return `in ${Math.round(abs / 3_600_000)}h`;
  }
  if (delta < 60_000) return `${Math.round(delta / 1000)}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

export function TasksSettings() {
  const qc = useQueryClient();
  const { data, isLoading, refetch } = useQuery({
    ...listTasksOptions(),
    refetchInterval: 5_000,
  });
  const tasks = (data ?? []) as TaskInfo[];

  // Tracks the names of tasks currently being triggered from this UI.
  // Uses a Set so multiple runs in flight (e.g. from "Run all")
  // each show the "running" state on their row, and the earlier
  // cross-row flicker can't come back. The mutation's own
  // `isPending` flag is intentionally unused here — it's a "some
  // mutation is in flight" signal, too coarse for per-row gating.
  const [triggeringNames, setTriggeringNames] = useState<Set<string>>(new Set());
  const [showInternal, setShowInternal] = useState(false);

  const runMutation = useMutation({
    ...runTaskMutation(),
    onMutate: (vars) => {
      setTriggeringNames((prev) => {
        const next = new Set(prev);
        next.add(vars.path.name);
        return next;
      });
    },
    onError: (err) => kinoToast.error(`Task failed: ${err.message ?? err}`),
    onSettled: (_res, _err, vars) => {
      setTriggeringNames((prev) => {
        const next = new Set(prev);
        next.delete(vars.path.name);
        return next;
      });
      qc.invalidateQueries({ queryKey: listTasksQueryKey() });
    },
  });

  const visibleTasks = useMemo(() => {
    const meta = (name: string): TaskMeta | undefined => TASK_LABELS[name];
    return tasks.filter((t) => showInternal || meta(t.name)?.source !== 'internal');
  }, [tasks, showInternal]);

  const hiddenCount = tasks.length - visibleTasks.length;

  const triggerOne = (name: string) => {
    runMutation.mutate({ path: { name } });
    kinoToast.success(`Triggered ${TASK_LABELS[name]?.label ?? name}`);
  };

  const runAll = () => {
    const toRun = visibleTasks.filter((t) => !t.running && !triggeringNames.has(t.name));
    if (toRun.length === 0) return;
    for (const t of toRun) {
      runMutation.mutate({ path: { name: t.name } });
    }
    kinoToast.success(`Triggered ${toRun.length} task${toRun.length === 1 ? '' : 's'}`);
  };

  const runAllPending = visibleTasks.some((t) => !t.running && !triggeringNames.has(t.name));

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between">
        <div>
          <h2 className="text-xl font-semibold">Scheduled Tasks</h2>
          <p className="text-sm text-[var(--text-muted)] mt-1">
            Background jobs that keep the library in sync. Run one now to refresh without waiting
            for its next scheduled tick.
          </p>
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <button
            type="button"
            onClick={() => refetch()}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-white/[0.04] hover:bg-white/10 text-sm transition"
          >
            <RefreshCw size={14} />
            Refresh
          </button>
          <button
            type="button"
            onClick={runAll}
            disabled={!runAllPending}
            title="Trigger every visible task at once"
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] disabled:opacity-50 disabled:cursor-not-allowed text-white text-sm font-medium transition"
          >
            <Play size={14} />
            Run all
          </button>
        </div>
      </div>

      {isLoading && (
        <div className="flex items-center justify-center py-12">
          <div className="w-5 h-5 border-2 border-white/20 border-t-white rounded-full animate-spin" />
        </div>
      )}

      <div className="space-y-2">
        {visibleTasks.map((t) => {
          const meta = TASK_LABELS[t.name] ?? { label: t.name, description: '' };
          const isTriggering = triggeringNames.has(t.name);
          const rowRunning = t.running || isTriggering;
          return (
            <div key={t.name} className="p-4 rounded-lg bg-white/[0.04] border border-white/5">
              <div className="flex items-start justify-between gap-3">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium">{meta.label}</span>
                    <code className="text-[10px] text-[var(--text-muted)] px-1.5 py-0.5 rounded bg-white/5">
                      {t.name}
                    </code>
                    {t.running && (
                      <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-blue-500/20 text-blue-300">
                        running
                      </span>
                    )}
                    {!t.running && t.last_error && (
                      <span
                        className="inline-flex items-center gap-1 text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-red-500/15 text-red-300"
                        title={t.last_error}
                      >
                        <AlertTriangle size={10} />
                        failed
                      </span>
                    )}
                    {!t.running && !t.last_error && t.last_duration_ms != null && (
                      <CheckCircle2 size={12} className="text-green-400/70" />
                    )}
                  </div>
                  <p className="text-xs text-[var(--text-muted)] mt-1">{meta.description}</p>
                  <div className="flex items-center gap-4 text-xs text-[var(--text-muted)] mt-2 flex-wrap">
                    <span>
                      Every <span className="text-white">{formatInterval(t.interval_seconds)}</span>
                      {meta.source === 'config' && (
                        <span className="text-[var(--text-muted)] italic">
                          {' '}
                          (set in Automation)
                        </span>
                      )}
                    </span>
                    <span>
                      Last: <span className="text-white">{formatRelative(t.last_run_at)}</span>
                    </span>
                    {t.last_duration_ms != null && (
                      <span>
                        Took:{' '}
                        <span className="text-white">{formatDurationMs(t.last_duration_ms)}</span>
                      </span>
                    )}
                    {t.next_run_at && (
                      <span>
                        Next: <span className="text-white">{formatRelative(t.next_run_at)}</span>
                      </span>
                    )}
                  </div>
                  {t.last_error && (
                    <p
                      className="mt-2 text-[11px] font-mono text-red-300 break-all"
                      title={t.last_error}
                    >
                      {t.last_error}
                    </p>
                  )}
                </div>
                <button
                  type="button"
                  disabled={rowRunning}
                  onClick={() => triggerOne(t.name)}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-medium transition flex-shrink-0"
                >
                  <Play size={12} />
                  Run now
                </button>
              </div>
            </div>
          );
        })}
      </div>

      {(hiddenCount > 0 || showInternal) && (
        <button
          type="button"
          onClick={() => setShowInternal((s) => !s)}
          className="inline-flex items-center gap-1.5 text-xs text-[var(--text-muted)] hover:text-white transition"
        >
          {showInternal ? <EyeOff size={12} /> : <Eye size={12} />}
          {showInternal
            ? 'Hide internal tasks'
            : `Show ${hiddenCount} internal task${hiddenCount === 1 ? '' : 's'}`}
        </button>
      )}
    </div>
  );
}
