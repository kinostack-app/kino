import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useSearch } from '@tanstack/react-router';
import {
  AlertCircle,
  Bug,
  ChevronDown,
  ChevronRight,
  Download,
  Filter,
  Info,
  Radio,
  Search,
  TriangleAlert,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

/**
 * /settings/logs — queryable view over the backend `log_entry` table.
 *
 * Filters (level / subsystem / source / search / since) feed directly
 * into the backend `/api/v1/logs` query; pagination is cursor-based on
 * `id DESC` (pass the oldest row's id as `before` to load the next page).
 *
 * Filter state is mirrored to the URL search params so a debugging
 * session is shareable and survives refresh.
 */

interface LogRow {
  id: number;
  ts_us: number;
  level: number;
  target: string;
  subsystem: string | null;
  trace_id: string | null;
  span_id: string | null;
  message: string;
  fields_json: string | null;
  source: string;
}

const LEVELS = [
  { value: 0, label: 'Errors only', short: 'ERR' },
  { value: 1, label: 'Warnings +', short: 'WARN' },
  { value: 2, label: 'Info +', short: 'INFO' },
  { value: 3, label: 'Debug +', short: 'DEBUG' },
];

const LEVEL_STYLES: Record<number, { color: string; icon: typeof Info; label: string }> = {
  0: { color: 'text-red-400 bg-red-500/10 ring-red-500/20', icon: AlertCircle, label: 'ERROR' },
  1: {
    color: 'text-amber-400 bg-amber-500/10 ring-amber-500/20',
    icon: TriangleAlert,
    label: 'WARN',
  },
  2: { color: 'text-sky-400 bg-sky-500/10 ring-sky-500/20', icon: Info, label: 'INFO' },
  3: { color: 'text-[var(--text-muted)] bg-white/5 ring-white/5', icon: Bug, label: 'DEBUG' },
  4: { color: 'text-[var(--text-muted)] bg-white/5 ring-white/5', icon: Bug, label: 'TRACE' },
};

// "5m / 1h / 24h / All" → minutes. 0 means unlimited.
const TIME_RANGES: Array<{ label: string; mins: number }> = [
  { label: '5m', mins: 5 },
  { label: '1h', mins: 60 },
  { label: '24h', mins: 60 * 24 },
  { label: 'All', mins: 0 },
];

function formatTsLocal(us: number): string {
  const d = new Date(us / 1000);
  const hh = d.getHours().toString().padStart(2, '0');
  const mm = d.getMinutes().toString().padStart(2, '0');
  const ss = d.getSeconds().toString().padStart(2, '0');
  const ms = Math.floor((us / 1000) % 1000)
    .toString()
    .padStart(3, '0');
  return `${hh}:${mm}:${ss}.${ms}`;
}

function formatTsUtc(us: number): string {
  const d = new Date(us / 1000);
  const hh = d.getUTCHours().toString().padStart(2, '0');
  const mm = d.getUTCMinutes().toString().padStart(2, '0');
  const ss = d.getUTCSeconds().toString().padStart(2, '0');
  const ms = Math.floor((us / 1000) % 1000)
    .toString()
    .padStart(3, '0');
  return `${hh}:${mm}:${ss}.${ms}`;
}

function relativeDate(us: number): string {
  const now = Date.now();
  const diffMs = now - us / 1000;
  if (diffMs < 60_000) return 'just now';
  if (diffMs < 3_600_000) return `${Math.floor(diffMs / 60_000)}m ago`;
  if (diffMs < 86_400_000) return `${Math.floor(diffMs / 3_600_000)}h ago`;
  return new Date(us / 1000).toLocaleDateString();
}

/** Try to pretty-print a JSON fields blob; fall back to the raw string. */
function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

// One row in the stream (no `id` — server only assigns ids after the DB
// write, and the live broadcast fires before that). We synthesise a
// temporary negative id so React's `key={}` still works.
interface StreamRow {
  ts_us: number;
  level: number;
  target: string;
  subsystem: string | null;
  trace_id: string | null;
  span_id: string | null;
  message: string;
  fields_json: string | null;
  source: string;
}

// Shape of the URL search params that survive refresh + navigation.
interface LogSearch {
  level?: number;
  subsystem?: string;
  source?: 'all' | 'backend' | 'frontend';
  q?: string;
  trace?: string;
  since?: number; // minutes back from now; 0 = unlimited
  tz?: 'local' | 'utc';
}

export function LogsSettings() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const search = useSearch({ strict: false }) as LogSearch;

  const level = search.level ?? 2;
  const subsystem = search.subsystem ?? '';
  const source = search.source ?? 'all';
  const traceFilter = search.trace ?? '';
  const sinceMins = search.since ?? 0;
  const tz = search.tz ?? 'local';

  // Search input is local state (debounced) but mirrors the URL `q` param.
  const [q, setQ] = useState<string>(search.q ?? '');
  const [debouncedQ, setDebouncedQ] = useState<string>(search.q ?? '');

  const [live, setLive] = useState<boolean>(true);
  const [liveRows, setLiveRows] = useState<Array<LogRow & { __local: true }>>([]);
  const [lagged, setLagged] = useState<number>(0);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [autoScroll, setAutoScroll] = useState<boolean>(true);
  const containerRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);

  // Single updater that merges new params into the URL. `undefined`
  // removes a key so the URL stays tidy when a filter is cleared.
  const updateSearch = useCallback(
    (patch: Partial<LogSearch>) => {
      navigate({
        to: '/settings/logs',
        search: (prev) => {
          const next = { ...(prev as LogSearch), ...patch };
          for (const [k, v] of Object.entries(next)) {
            if (v === '' || v === undefined || v === null) {
              delete (next as Record<string, unknown>)[k];
            }
          }
          return next;
        },
      });
    },
    [navigate]
  );

  // Keep URL `q` in sync after debounce — avoids a history entry per keystroke.
  useEffect(() => {
    const t = setTimeout(() => {
      setDebouncedQ(q);
      updateSearch({ q: q.length >= 2 ? q : undefined });
    }, 250);
    return () => clearTimeout(t);
  }, [q, updateSearch]);

  // Live-tail WS subscription. Only open when the toggle is on; close
  // on toggle-off or unmount. Reconnects on unexpected close with a 2 s
  // backoff — keeps the UI working through a backend restart.
  useEffect(() => {
    if (!live) return;

    let ws: WebSocket | null = null;
    let retry: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;
    let localIdSeed = -1;

    const connect = async () => {
      if (cancelled) return;
      // Firefox aborts WS connections opened before the page is fully
      // loaded ("was interrupted while the page was loading"). Gate the
      // first open on a status fetch — same pattern the main WS uses —
      // which also lets us back off cleanly when the backend is
      // restarting.
      try {
        const res = await fetch('/api/v1/status');
        if (!res.ok) throw new Error(`status ${res.status}`);
      } catch {
        if (!cancelled) retry = setTimeout(connect, 2000);
        return;
      }
      if (cancelled) return;

      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      // Cookie auto-attaches on the WS upgrade in cookie mode.
      const url = `${protocol}//${window.location.host}/api/v1/logs/stream`;
      ws = new WebSocket(url);

      // Whenever the WS (re)connects — including after a backend restart —
      // invalidate the REST backlog so anything written during the gap
      // gets refetched. Without this, the page stays blank after `just
      // reset` because live-tail only shows events that arrive *after*
      // the socket opens.
      ws.onopen = () => {
        queryClient.invalidateQueries({ queryKey: ['kino', 'logs'] });
      };

      ws.onmessage = (e) => {
        try {
          const data = JSON.parse(e.data as string);
          if (typeof data?.lagged === 'number') {
            setLagged((n) => n + (data.lagged as number));
            return;
          }
          const row = data as StreamRow;
          const id = localIdSeed;
          localIdSeed -= 1;
          setLiveRows((prev) => [{ ...row, id, __local: true as const }, ...prev].slice(0, 500));
        } catch {
          // Malformed frame — ignore, keep the socket.
        }
      };

      ws.onclose = () => {
        if (!cancelled) retry = setTimeout(connect, 2000);
      };
      ws.onerror = () => {
        ws?.close();
      };
    };

    connect();

    return () => {
      cancelled = true;
      if (retry) clearTimeout(retry);
      ws?.close();
    };
  }, [live, queryClient]);

  // Clear buffered live rows when filters change — the polled list
  // will refetch with the new filter, and mixing old live rows would
  // show pre-filter content.
  // biome-ignore lint/correctness/useExhaustiveDependencies: deps are intentional change triggers
  useEffect(() => {
    setLiveRows([]);
    setLagged(0);
  }, [level, subsystem, source, debouncedQ, traceFilter, sinceMins]);

  // `since_us` is derived from the UI's minutes value; when "All" is
  // selected (0 mins), we simply omit the param.
  const sinceUs = useMemo(() => {
    if (!sinceMins) return null;
    return (Date.now() - sinceMins * 60_000) * 1000;
  }, [sinceMins]);

  const queryKey = useMemo(
    () => [
      'kino',
      'logs',
      { level, subsystem, source, q: debouncedQ, trace: traceFilter, since: sinceMins },
    ],
    [level, subsystem, source, debouncedQ, traceFilter, sinceMins]
  );

  const { data, isFetching, fetchNextPage, hasNextPage, refetch } = useInfiniteQuery({
    queryKey,
    queryFn: async ({ pageParam }) => {
      const params = new URLSearchParams();
      params.set('level', String(level));
      params.set('limit', '200');
      if (subsystem) params.set('subsystem', subsystem);
      if (source !== 'all') params.set('source', source);
      if (debouncedQ.length >= 2) params.set('q', debouncedQ);
      if (traceFilter) params.set('trace_id', traceFilter);
      if (sinceUs !== null) params.set('since_us', String(sinceUs));
      if (pageParam) params.set('before', String(pageParam));
      const res = await fetch(`/api/v1/logs?${params.toString()}`, {
        credentials: 'include',
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return (await res.json()) as LogRow[];
    },
    initialPageParam: 0 as number,
    getNextPageParam: (lastPage) => {
      if (lastPage.length === 0) return undefined;
      return lastPage[lastPage.length - 1].id;
    },
    // When live-tail is on, the WS pushes updates — no need to poll.
    // When it's off, fall back to a gentle 5s refresh so the page
    // doesn't feel frozen if the user leaves it open.
    refetchInterval: live ? false : 5_000,
    refetchIntervalInBackground: false,
  });

  // Merge live-tail rows with the paginated backlog. Live rows are at
  // the top (newest), client-side filtered so stale buffered rows after
  // a filter change don't linger. Dedup by ts_us + message + target —
  // the same row can appear as a live event AND as a polled row after
  // the next refresh window.
  const rows = useMemo(() => {
    const polled = data?.pages.flat() ?? [];
    if (!live) return polled;
    const needle = debouncedQ.toLowerCase();
    const floorUs = sinceUs;
    const filteredLive = liveRows.filter((r) => {
      if (r.level > level) return false;
      if (subsystem && r.subsystem !== subsystem) return false;
      if (source !== 'all' && r.source !== source) return false;
      if (needle.length >= 2 && !r.message.toLowerCase().includes(needle)) return false;
      if (traceFilter && r.trace_id !== traceFilter) return false;
      if (floorUs !== null && r.ts_us < floorUs) return false;
      return true;
    });
    // Drop any polled row whose fingerprint matches a live row we've
    // already shown, to avoid double-display around the polling boundary.
    const liveKeys = new Set(filteredLive.map((r) => `${r.ts_us}|${r.target}|${r.message}`));
    const dedupedPolled = polled.filter(
      (r) => !liveKeys.has(`${r.ts_us}|${r.target}|${r.message}`)
    );
    return [...filteredLive, ...dedupedPolled];
  }, [data, liveRows, live, level, subsystem, source, debouncedQ, traceFilter, sinceUs]);

  // Auto-scroll to top (newest) when new live events arrive, unless the
  // user has scrolled down — classic tail -f behavior. Re-arms as soon
  // as the user scrolls back to the top. `liveRows` is a change-trigger
  // only; not read in the body.
  // biome-ignore lint/correctness/useExhaustiveDependencies: liveRows is a change trigger
  useEffect(() => {
    if (!live || !autoScroll) return;
    const el = containerRef.current;
    if (!el) return;
    el.scrollTop = 0;
  }, [liveRows, live, autoScroll]);

  const onTableScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    setAutoScroll(e.currentTarget.scrollTop <= 8);
  }, []);

  // Keyboard shortcuts: `/` focuses search, `Esc` clears search + trace.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const inInput =
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);
      if (e.key === '/' && !inInput) {
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      } else if (e.key === 'Escape' && (inInput || q || traceFilter)) {
        setQ('');
        updateSearch({ q: undefined, trace: undefined });
        searchInputRef.current?.blur();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [q, traceFilter, updateSearch]);

  // Export matching entries as NDJSON. Can't use a plain <a download>
  // because the endpoint needs the API key in the Authorization header
  // — fetch → blob → synthetic anchor click covers that. Filename comes
  // from the server's Content-Disposition.
  const handleExport = async () => {
    const params = new URLSearchParams();
    params.set('level', String(level));
    if (subsystem) params.set('subsystem', subsystem);
    if (source !== 'all') params.set('source', source);
    if (debouncedQ.length >= 2) params.set('q', debouncedQ);
    if (traceFilter) params.set('trace_id', traceFilter);
    if (sinceUs !== null) params.set('since_us', String(sinceUs));

    const res = await fetch(`/api/v1/logs/export?${params.toString()}`, {
      credentials: 'include',
    });
    if (!res.ok) return;

    // Respect the server's suggested filename; fall back to a timestamp.
    const dispo = res.headers.get('content-disposition') ?? '';
    const filenameMatch = /filename="([^"]+)"/.exec(dispo);
    const filename =
      filenameMatch?.[1] ?? `kino-logs-${new Date().toISOString().replace(/[:.]/g, '-')}.ndjson`;

    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  };

  const clearFilters = () => {
    setQ('');
    updateSearch({
      level: undefined,
      subsystem: undefined,
      source: undefined,
      q: undefined,
      trace: undefined,
      since: undefined,
    });
  };

  const hasActiveFilters =
    level !== 2 ||
    subsystem !== '' ||
    source !== 'all' ||
    q !== '' ||
    traceFilter !== '' ||
    sinceMins !== 0;

  // Collect subsystems seen on the current page for the filter dropdown.
  const subsystems = useMemo(() => {
    const s = new Set<string>();
    for (const r of rows) if (r.subsystem) s.add(r.subsystem);
    return [...s].sort();
  }, [rows]);

  const toggleExpanded = (id: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const formatTs = tz === 'utc' ? formatTsUtc : formatTsLocal;

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h1 className="text-xl font-bold">Logs</h1>
          <p className="text-sm text-[var(--text-muted)]">
            Backend + frontend events.{' '}
            {live
              ? autoScroll
                ? 'Live — new rows appear instantly.'
                : 'Live (scroll up to resume autoscroll).'
              : 'Paused — auto-refresh every 5\u00a0s.'}{' '}
            <span className="opacity-60">
              Press <kbd className="px-1 rounded bg-white/5">/</kbd> to search.
            </span>
          </p>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-xs text-[var(--text-muted)] tabular-nums">
            {rows.length.toLocaleString()} rows
          </span>
          {lagged > 0 && (
            <span
              className="text-xs text-amber-400"
              title="Stream fell behind — reload to see the full backlog"
            >
              ⚠ {lagged} skipped
            </span>
          )}
          <button
            type="button"
            onClick={() => setLive((v) => !v)}
            className={cn(
              'flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition',
              live
                ? 'bg-[var(--accent)]/15 text-[var(--accent)] ring-1 ring-[var(--accent)]/30'
                : 'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white'
            )}
            title={live ? 'Streaming via WebSocket' : 'Click to stream live'}
          >
            <Radio size={14} className={live ? 'animate-pulse' : ''} />
            {live ? 'Live' : 'Paused'}
          </button>
          <button
            type="button"
            onClick={handleExport}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] hover:text-white transition"
            title="Download matching entries as NDJSON"
          >
            <Download size={14} />
            Export
          </button>
          <button
            type="button"
            onClick={() => refetch()}
            className="px-3 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] hover:text-white transition"
          >
            Refresh
          </button>
        </div>
      </div>

      {/* Filters */}
      <div className="flex flex-wrap items-center gap-2 mb-3">
        {/* Level pill group */}
        <div className="flex items-center gap-0.5 bg-white/5 rounded-lg p-0.5">
          {LEVELS.map((l) => (
            <button
              key={l.value}
              type="button"
              onClick={() => updateSearch({ level: l.value === 2 ? undefined : l.value })}
              className={cn(
                'px-2.5 py-1 rounded-md text-xs font-medium transition',
                level === l.value
                  ? 'bg-white/10 text-white'
                  : 'text-[var(--text-muted)] hover:text-white'
              )}
            >
              {l.short}
            </button>
          ))}
        </div>

        {/* Source pill group */}
        <div className="flex items-center gap-0.5 bg-white/5 rounded-lg p-0.5">
          {(['all', 'backend', 'frontend'] as const).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => updateSearch({ source: s === 'all' ? undefined : s })}
              className={cn(
                'px-2.5 py-1 rounded-md text-xs font-medium transition capitalize',
                source === s
                  ? 'bg-white/10 text-white'
                  : 'text-[var(--text-muted)] hover:text-white'
              )}
            >
              {s}
            </button>
          ))}
        </div>

        {/* Time-range pill group */}
        <div className="flex items-center gap-0.5 bg-white/5 rounded-lg p-0.5">
          {TIME_RANGES.map((r) => (
            <button
              key={r.label}
              type="button"
              onClick={() => updateSearch({ since: r.mins === 0 ? undefined : r.mins })}
              className={cn(
                'px-2.5 py-1 rounded-md text-xs font-medium transition',
                sinceMins === r.mins
                  ? 'bg-white/10 text-white'
                  : 'text-[var(--text-muted)] hover:text-white'
              )}
            >
              {r.label}
            </button>
          ))}
        </div>

        {/* Subsystem filter */}
        <div className="relative">
          <select
            value={subsystem}
            onChange={(e) => updateSearch({ subsystem: e.target.value || undefined })}
            className="appearance-none h-8 pl-3 pr-8 rounded-lg bg-white/5 text-xs text-[var(--text-secondary)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
          >
            <option value="">All subsystems</option>
            {subsystems.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
          <ChevronDown
            size={12}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-[var(--text-muted)] pointer-events-none"
          />
        </div>

        {/* Timezone toggle */}
        <button
          type="button"
          onClick={() => updateSearch({ tz: tz === 'local' ? 'utc' : undefined })}
          className="h-8 px-2.5 rounded-lg bg-white/5 hover:bg-white/10 text-xs text-[var(--text-muted)] hover:text-white transition font-mono"
          title="Toggle timestamp between local and UTC"
        >
          {tz === 'utc' ? 'UTC' : 'local'}
        </button>

        {/* Search */}
        <div className="relative flex-1 min-w-[200px]">
          <Search
            size={14}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
          />
          <input
            ref={searchInputRef}
            type="text"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search messages… ( / to focus )"
            className="w-full h-8 pl-8 pr-3 rounded-lg bg-white/5 text-xs text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
          />
        </div>

        {traceFilter && (
          <button
            type="button"
            onClick={() => updateSearch({ trace: undefined })}
            className="flex items-center gap-1.5 h-8 px-2.5 rounded-lg bg-[var(--accent)]/15 text-[var(--accent)] text-xs font-medium"
          >
            <Filter size={12} />
            trace {traceFilter}
            <span className="opacity-60">×</span>
          </button>
        )}

        {hasActiveFilters && (
          <button
            type="button"
            onClick={clearFilters}
            className="flex items-center gap-1 h-8 px-2.5 rounded-lg bg-white/5 hover:bg-white/10 text-xs text-[var(--text-muted)] hover:text-white transition"
            title="Reset all filters"
          >
            <X size={12} />
            Clear
          </button>
        )}
      </div>

      {/* Table */}
      <div
        ref={containerRef}
        onScroll={onTableScroll}
        className="border border-white/5 rounded-lg bg-[var(--bg-card)]/40 overflow-auto max-h-[calc(100vh-16rem)] font-mono text-xs"
      >
        {rows.length === 0 && !isFetching ? (
          <p className="p-8 text-center text-[var(--text-muted)]">No log entries match.</p>
        ) : (
          <div className="divide-y divide-white/5">
            {rows.map((r) => {
              const style = LEVEL_STYLES[r.level] ?? LEVEL_STYLES[2];
              const Icon = style.icon;
              const isExpanded = expanded.has(r.id);
              const hasDetails = Boolean(r.fields_json);
              return (
                <div key={r.id}>
                  {/* biome-ignore lint/a11y/noStaticElementInteractions: role+tabindex applied only when interactive */}
                  <div
                    role={hasDetails ? 'button' : undefined}
                    tabIndex={hasDetails ? 0 : undefined}
                    onClick={hasDetails ? () => toggleExpanded(r.id) : undefined}
                    onKeyDown={
                      hasDetails
                        ? (e) => {
                            if (e.key === 'Enter' || e.key === ' ') {
                              e.preventDefault();
                              toggleExpanded(r.id);
                            }
                          }
                        : undefined
                    }
                    className={cn(
                      'w-full flex items-start gap-3 px-3 py-2 transition',
                      hasDetails ? 'hover:bg-white/[0.04] cursor-pointer' : 'hover:bg-white/[0.02]'
                    )}
                  >
                    <ChevronRight
                      size={12}
                      className={cn(
                        'flex-shrink-0 mt-1 text-[var(--text-muted)] transition-transform',
                        !hasDetails && 'opacity-0',
                        isExpanded && 'rotate-90'
                      )}
                    />
                    <span
                      className="text-[var(--text-muted)] whitespace-nowrap tabular-nums w-20 pt-0.5"
                      title={new Date(r.ts_us / 1000).toISOString()}
                    >
                      {formatTs(r.ts_us)}
                    </span>
                    <span
                      className={cn(
                        'inline-flex items-center gap-1 h-5 px-1.5 rounded text-[10px] font-semibold uppercase tracking-wide ring-1 flex-shrink-0 mt-0.5',
                        style.color
                      )}
                    >
                      <Icon size={10} />
                      {style.label}
                    </span>
                    {r.subsystem && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          updateSearch({ subsystem: r.subsystem || undefined });
                        }}
                        className="text-[var(--text-muted)] hover:text-white w-20 truncate text-left pt-0.5"
                        title={`Filter to ${r.subsystem}`}
                      >
                        {r.subsystem}
                      </button>
                    )}
                    <div className="flex-1 min-w-0 break-words text-white">
                      {r.message}
                      {r.fields_json && !isExpanded && (
                        <span className="text-[var(--text-muted)] ml-2">{r.fields_json}</span>
                      )}
                    </div>
                    {r.trace_id && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          updateSearch({ trace: r.trace_id || undefined });
                        }}
                        className="text-[var(--text-muted)] hover:text-white whitespace-nowrap pt-0.5 hidden md:inline"
                        title={`Filter to trace ${r.trace_id}`}
                      >
                        {r.trace_id}
                      </button>
                    )}
                    <span
                      className="text-[var(--text-muted)] whitespace-nowrap pt-0.5 w-16 text-right"
                      title={new Date(r.ts_us / 1000).toISOString()}
                    >
                      {relativeDate(r.ts_us)}
                    </span>
                  </div>
                  {isExpanded && r.fields_json && (
                    <div className="px-3 pb-3 pl-[4.25rem] bg-white/[0.02]">
                      <pre className="whitespace-pre-wrap break-all text-[var(--text-secondary)] text-[11px] leading-relaxed">
                        {prettyJson(r.fields_json)}
                      </pre>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {hasNextPage && (
        <div className="mt-3 flex justify-center">
          <button
            type="button"
            onClick={() => fetchNextPage()}
            disabled={isFetching}
            className="px-4 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 disabled:opacity-50 text-sm text-[var(--text-secondary)] hover:text-white transition"
          >
            {isFetching ? 'Loading…' : 'Load older'}
          </button>
        </div>
      )}
    </div>
  );
}
