import { useInfiniteQuery } from '@tanstack/react-query';
import {
  CheckCircle2,
  Clock,
  Download,
  Eye,
  FilePlus,
  Layers,
  Sparkles,
  Trash2,
  TriangleAlert,
  Tv,
} from 'lucide-react';
import type { ElementType } from 'react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { listHistory } from '@/api/generated/sdk.gen';
import type { AppEvent, History } from '@/api/generated/types.gen';
import { cn } from '@/lib/utils';

const PAGE_SIZE = 50;

interface EventMeta {
  icon: ElementType;
  color: string;
  verb: string;
}

const EVENT_CONFIG: Record<string, EventMeta> = {
  movie_added: { icon: FilePlus, color: 'text-blue-400', verb: 'Added' },
  show_added: { icon: FilePlus, color: 'text-blue-400', verb: 'Added' },
  release_grabbed: { icon: Download, color: 'text-purple-400', verb: 'Grabbed' },
  grabbed: { icon: Download, color: 'text-purple-400', verb: 'Grabbed' },
  download_started: { icon: Download, color: 'text-sky-300', verb: 'Starting' },
  download_complete: { icon: Download, color: 'text-sky-400', verb: 'Downloaded' },
  download_failed: { icon: TriangleAlert, color: 'text-red-400', verb: 'Failed' },
  failed: { icon: TriangleAlert, color: 'text-red-400', verb: 'Failed' },
  imported: { icon: CheckCircle2, color: 'text-emerald-400', verb: 'Imported' },
  upgraded: { icon: Sparkles, color: 'text-emerald-300', verb: 'Upgraded' },
  watched: { icon: Eye, color: 'text-amber-400', verb: 'Watched' },
  new_episode: { icon: Tv, color: 'text-blue-300', verb: 'New Episode' },
  movie_deleted: { icon: Trash2, color: 'text-red-400', verb: 'Removed' },
  show_deleted: { icon: Trash2, color: 'text-red-400', verb: 'Removed' },
  content_removed: { icon: Trash2, color: 'text-red-400', verb: 'Removed' },
  health_warning: { icon: TriangleAlert, color: 'text-amber-400', verb: 'Health' },
  search_started: { icon: Layers, color: 'text-[var(--text-muted)]', verb: 'Searching' },
};

/**
 * Pill options for the faceted filter row. Each pill maps to one
 * or more backend `event_type` values — Discord-style "Grabs" pill
 * covers both `release_grabbed` (the canonical name) and `grabbed`
 * (the abbreviated alias we use in some emit sites). The backend
 * accepts a csv `event_types` param and does an IN() query.
 */
interface FilterPill {
  key: string;
  label: string;
  /** Event types this pill activates — csv-sent to the backend. */
  types: string[];
}

const FILTER_PILLS: FilterPill[] = [
  { key: 'grabbed', label: 'Grabs', types: ['release_grabbed', 'grabbed'] },
  { key: 'downloaded', label: 'Downloads', types: ['download_complete'] },
  { key: 'failed', label: 'Failures', types: ['download_failed', 'failed'] },
  { key: 'imported', label: 'Imports', types: ['imported'] },
  { key: 'upgraded', label: 'Upgrades', types: ['upgraded'] },
  { key: 'watched', label: 'Watched', types: ['watched'] },
  { key: 'new_episode', label: 'New episodes', types: ['new_episode'] },
  { key: 'added', label: 'Added', types: ['movie_added', 'show_added'] },
];

function formatBytes(bytes: number | null | undefined): string | null {
  if (bytes == null || bytes <= 0) return null;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

function formatDurationMs(ms: number | null | undefined): string | null {
  if (ms == null || ms < 0) return null;
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return s === 0 ? `${m}m` : `${m}m ${s}s`;
  }
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return m === 0 ? `${h}h` : `${h}h ${m}m`;
}

/**
 * Absolute timestamp for the row's `title` tooltip.
 */
function formatExact(dateStr: string): string {
  const date = new Date(dateStr);
  return date.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatLeftTimestamp(dateStr: string, bucket: string): string {
  const date = new Date(dateStr);
  const hm = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (bucket === 'Today' || bucket === 'Yesterday') return hm;
  if (bucket === 'Earlier this week') {
    const wd = date.toLocaleDateString(undefined, { weekday: 'short' });
    return `${wd} ${hm}`;
  }
  const md = date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  return `${md} ${hm}`;
}

function bucketLabel(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const startOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const diffDays = Math.floor((startOfDay - date.getTime()) / 86_400_000);
  if (diffDays <= 0) return 'Today';
  if (diffDays === 1) return 'Yesterday';
  if (diffDays < 7) return 'Earlier this week';
  if (diffDays < 30) return 'Earlier this month';
  return date.toLocaleDateString(undefined, { year: 'numeric', month: 'long' });
}

/**
 * The `data` column is the serialized AppEvent — one big JSON blob.
 * We parse it once per row and pull the fields each event type
 * cares about. Unknown/malformed blobs return an empty object so
 * the row still renders its base line.
 */
/**
 * Parse the history row's `data` column back into a typed `AppEvent`.
 *
 * The backend writes `serde_json::to_string(&event)` to this column
 * (see `events::listeners::log_history`), which emits the full
 * discriminated union including the `"event"` tag. Consumers narrow
 * on `parsed.event` to pull variant-specific fields without `as`
 * casts, and a backend variant rename fails the build here.
 */
function parseData(raw: string | null | undefined): AppEvent | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    // Minimal shape guard — if the blob doesn't have an `event` tag,
    // we can't narrow it. Dropping to null keeps the rest of the
    // row rendering off the `event_type` column.
    if (
      parsed &&
      typeof parsed === 'object' &&
      typeof (parsed as { event?: unknown }).event === 'string'
    ) {
      return parsed as AppEvent;
    }
    return null;
  } catch {
    return null;
  }
}

function useHistoryQuery(selectedPills: Set<string>) {
  // Expand the selected pill keys into every concrete event_type
  // they cover, then send a csv to the backend. Empty set = no
  // filter (all events).
  const eventTypes = FILTER_PILLS.filter((p) => selectedPills.has(p.key))
    .flatMap((p) => p.types)
    .join(',');
  return useInfiniteQuery({
    queryKey: ['kino', 'history', eventTypes],
    queryFn: async ({ pageParam }) => {
      const { data } = await listHistory({
        query: {
          limit: PAGE_SIZE,
          event_types: eventTypes || undefined,
          cursor: pageParam ?? undefined,
        },
      });
      // `/history` is paginated per the 09-api contract: opaque
      // base64 cursor + PaginatedResponse envelope. We return the
      // page itself so `getNextPageParam` can read
      // `next_cursor` directly.
      return (
        data ?? {
          results: [] as History[],
          next_cursor: null,
          has_more: false,
        }
      );
    },
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
    initialPageParam: undefined as string | undefined,
    // No polling — meta drives invalidation on every event the
    // backend persists as a history row.
    meta: {
      invalidatedBy: [
        'movie_added',
        'show_added',
        'imported',
        'upgraded',
        'content_removed',
        'release_grabbed',
        'download_started',
        'download_complete',
        'download_failed',
        'download_cancelled',
        'watched',
      ],
    },
  });
}

export function HistoryTab() {
  const [selectedPills, setSelectedPills] = useState<Set<string>>(new Set());
  const loadMoreRef = useRef<HTMLDivElement>(null);

  const q = useHistoryQuery(selectedPills);

  const togglePill = (key: string) => {
    setSelectedPills((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const hasActiveFilter = selectedPills.size > 0;

  const events = useMemo(() => q.data?.pages.flatMap((p) => p.results) ?? [], [q.data]);

  const groups = useMemo(() => {
    const out: { label: string; events: History[] }[] = [];
    for (const e of events) {
      const label = bucketLabel(e.date);
      const last = out[out.length - 1];
      if (last && last.label === label) {
        last.events.push(e);
      } else {
        out.push({ label, events: [e] });
      }
    }
    return out;
  }, [events]);

  useEffect(() => {
    const el = loadMoreRef.current;
    if (!el) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && q.hasNextPage && !q.isFetchingNextPage) {
          q.fetchNextPage();
        }
      },
      { threshold: 0.1 }
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [q]);

  return (
    <div className="space-y-4">
      {/* Faceted filter — click All to clear; click any pill to
          toggle. Multiple pills combine (e.g. Grabs + Failures).
          Sends csv to the backend so the server does the IN() query
          rather than us re-filtering paginated pages in the client. */}
      <div className="flex items-center gap-1.5 flex-wrap">
        <button
          type="button"
          onClick={() => setSelectedPills(new Set())}
          className={cn(
            'px-2.5 py-1 rounded-full text-xs font-medium transition-colors',
            !hasActiveFilter
              ? 'bg-white/15 text-white'
              : 'bg-white/5 text-[var(--text-muted)] hover:text-white hover:bg-white/10'
          )}
        >
          All
        </button>
        {FILTER_PILLS.map((pill) => {
          const active = selectedPills.has(pill.key);
          return (
            <button
              key={pill.key}
              type="button"
              onClick={() => togglePill(pill.key)}
              className={cn(
                'px-2.5 py-1 rounded-full text-xs font-medium transition-colors',
                active
                  ? 'bg-[var(--accent)] text-white'
                  : 'bg-white/5 text-[var(--text-secondary)] hover:text-white hover:bg-white/10'
              )}
            >
              {pill.label}
            </button>
          );
        })}
      </div>

      {q.isLoading && (
        <div className="flex items-center justify-center min-h-[40vh]">
          <div className="w-6 h-6 border-2 border-white/20 border-t-white rounded-full animate-spin" />
        </div>
      )}

      {!q.isLoading && events.length === 0 && (
        <div className="flex flex-col items-center justify-center min-h-[40vh] text-center gap-4">
          <div className="w-16 h-16 rounded-full bg-white/5 grid place-items-center">
            <Clock size={28} className="text-[var(--text-muted)]" />
          </div>
          <div>
            <p className="text-lg font-medium">
              {hasActiveFilter ? 'No matching events' : 'No activity yet'}
            </p>
            <p className="text-sm text-[var(--text-muted)] mt-1">
              {hasActiveFilter
                ? 'Try different event types, or click All to clear.'
                : 'Events will appear here as you add and watch content.'}
            </p>
          </div>
        </div>
      )}

      {groups.length > 0 && (
        <div className="divide-y divide-white/[0.04]">
          {groups.map((g) => (
            <section key={g.label}>
              <h3 className="text-[10px] font-semibold uppercase tracking-wider text-[var(--text-muted)] pt-3 pb-1 px-2">
                {g.label}
              </h3>
              <div>
                {g.events.map((event) => (
                  <EventRow key={event.id} event={event} bucket={g.label} />
                ))}
              </div>
            </section>
          ))}
        </div>
      )}

      {events.length > 0 && (
        <div ref={loadMoreRef} className="py-6 flex justify-center">
          {q.isFetchingNextPage && (
            <div className="w-5 h-5 border-2 border-white/20 border-t-white rounded-full animate-spin" />
          )}
          {!q.hasNextPage && !q.isFetchingNextPage && (
            <p className="text-xs text-[var(--text-muted)]">End of history</p>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Build the event-specific second line from the JSON blob. Returns
 * an array of string fragments which the row joins with a separator
 * — kept as an array so fragments can be independently conditional
 * without stringly concatenating empty pieces.
 */
function buildSubtitle(data: AppEvent | null): string[] {
  if (!data) return [];
  const out: string[] = [];
  switch (data.event) {
    case 'release_grabbed': {
      const size = formatBytes(data.size);
      if (data.indexer) out.push(`via ${data.indexer}`);
      if (size) out.push(size);
      break;
    }
    case 'download_complete': {
      const size = formatBytes(data.size);
      const dur = formatDurationMs(data.duration_ms);
      if (size && dur) out.push(`${size} in ${dur}`);
      else if (size) out.push(size);
      else if (dur) out.push(`in ${dur}`);
      break;
    }
    case 'download_failed': {
      if (data.error) out.push(data.error);
      break;
    }
    case 'upgraded': {
      if (data.old_quality && data.new_quality) {
        out.push(`${data.old_quality} → ${data.new_quality}`);
      } else if (data.new_quality) {
        out.push(data.new_quality);
      }
      break;
    }
    case 'new_episode': {
      const sxe = `S${String(data.season).padStart(2, '0')}E${String(data.episode).padStart(2, '0')}`;
      const epTitle = typeof data.episode_title === 'string' ? data.episode_title : null;
      out.push(epTitle ? `${sxe} · ${epTitle}` : sxe);
      break;
    }
    default:
      break;
  }
  return out;
}

function EventRow({ event, bucket }: { event: History; bucket: string }) {
  const config: EventMeta = EVENT_CONFIG[event.event_type] ?? {
    icon: Clock,
    color: 'text-[var(--text-muted)]',
    verb: event.event_type,
  };
  const Icon = config.icon;
  const title = event.source_title ?? `(${event.movie_id ?? event.episode_id ?? '?'})`;
  const data = useMemo(() => parseData(event.data), [event.data]);
  const subtitle = useMemo(() => buildSubtitle(data), [data]);

  // For failure rows we want the error visible but also want to
  // give the row a red tint so it's scannable; other event types
  // stay neutral-background.
  const isFailure = event.event_type === 'download_failed' || event.event_type === 'failed';

  return (
    <div
      className={cn(
        'grid grid-cols-[12ch_14px_12ch_1fr_auto] items-start gap-x-3 px-2 py-1.5 rounded transition',
        isFailure ? 'hover:bg-red-500/[0.06]' : 'hover:bg-white/[0.04]'
      )}
      title={formatExact(event.date)}
    >
      <span className="text-[11px] text-[var(--text-muted)] tabular-nums whitespace-nowrap pt-[2px]">
        {formatLeftTimestamp(event.date, bucket)}
      </span>
      <Icon size={13} className={cn('flex-shrink-0 mt-[3px]', config.color)} />
      <span
        className={cn(
          'text-[10px] font-semibold uppercase tracking-wider whitespace-nowrap pt-[2px]',
          config.color
        )}
      >
        {config.verb}
      </span>
      <div className="min-w-0">
        <p className="truncate text-sm text-white">{title}</p>
        {subtitle.length > 0 && (
          <p
            className={cn(
              'truncate text-[11px] mt-0.5',
              isFailure ? 'text-red-300/90 font-mono' : 'text-[var(--text-muted)]'
            )}
          >
            {subtitle.join(' · ')}
          </p>
        )}
      </div>
      {event.quality ? (
        <span className="hidden sm:inline px-1.5 py-0.5 rounded bg-white/5 ring-1 ring-white/10 text-[10px] font-mono text-[var(--text-secondary)] flex-shrink-0 mt-[2px]">
          {event.quality}
        </span>
      ) : (
        <span className="hidden sm:inline" aria-hidden="true" />
      )}
    </div>
  );
}
