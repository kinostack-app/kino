/**
 * Lists landing page (subsystem 17).
 *
 * Shows every followed list — MDBList, TMDB, Trakt custom, and the
 * auto-managed Trakt watchlist. Each entry renders a poster preview
 * strip so the page reads as "look what's in here" rather than just
 * metadata. The user can:
 *
 *   - Toggle between a poster-rich grid and a compact list view
 *   - Filter by source (all / mdblist / tmdb / trakt)
 *   - Sort by title, recently polled, or item count
 *   - Pin / unfollow / refresh / open source from the card itself
 *
 * The Trakt watchlist always sorts first and is lock-marked — it's
 * auto-managed from the Trakt connection and can't be unfollowed
 * manually.
 */

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import {
  ExternalLink,
  LayoutGrid,
  List as ListIcon,
  Lock,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  Trash2,
} from 'lucide-react';
import { useMemo, useState } from 'react';
import {
  deleteList,
  getHomePreferences,
  listLists,
  refreshList,
  updateHomePreferences,
} from '@/api/generated/sdk.gen';
import type { HomePreferences, ListView } from '@/api/generated/types.gen';
import { AddListModal } from '@/components/AddListModal';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { tmdbImage } from '@/lib/api';
import { cn } from '@/lib/utils';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

// ── View preferences persisted in localStorage so the user's choice
// survives reloads. Tiny enough to keep inline rather than in
// preferences subsystem.
type ViewMode = 'grid' | 'list';
type SortMode = 'title' | 'polled' | 'items';
type SourceFilter = 'all' | 'mdblist' | 'tmdb_list' | 'trakt';

const LS_VIEW = 'kino.lists.view';
const LS_SORT = 'kino.lists.sort';

function readLS<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  if (typeof window === 'undefined') return fallback;
  const v = window.localStorage.getItem(key);
  return (allowed as readonly string[]).includes(v ?? '') ? (v as T) : fallback;
}

export function Lists() {
  useDocumentTitle('Lists');
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);
  const [view, setView] = useState<ViewMode>(() =>
    readLS(LS_VIEW, ['grid', 'list'] as const, 'grid')
  );
  const [sort, setSort] = useState<SortMode>(() =>
    readLS(LS_SORT, ['title', 'polled', 'items'] as const, 'polled')
  );
  const [filter, setFilter] = useState<SourceFilter>('all');

  const setViewPersist = (v: ViewMode) => {
    setView(v);
    window.localStorage.setItem(LS_VIEW, v);
  };
  const setSortPersist = (s: SortMode) => {
    setSort(s);
    window.localStorage.setItem(LS_SORT, s);
  };

  const { data: lists = [], isLoading } = useQuery<ListView[]>({
    queryKey: ['kino', 'lists'],
    queryFn: async () => {
      const r = await listLists();
      return (r.data as ListView[] | undefined) ?? [];
    },
    meta: {
      invalidatedBy: ['list_bulk_growth', 'list_unreachable', 'list_auto_added', 'list_deleted'],
    },
  });

  // Pin state is expressed via HomePreferences.section_order — a list
  // is "pinned" iff `list:<id>` appears in that array.
  const { data: prefs } = useQuery<HomePreferences | null>({
    queryKey: ['kino', 'preferences', 'home'],
    queryFn: async () => {
      const r = await getHomePreferences();
      return (r.data as HomePreferences | undefined) ?? null;
    },
  });
  const pinnedIds = new Set<number>(
    (prefs?.section_order ?? [])
      .filter((s) => s.startsWith('list:'))
      .map((s) => Number(s.slice(5)))
      .filter((n) => Number.isFinite(n))
  );

  // Refresh emits `ListBulkGrowth` (when items actually landed) and
  // `ListUnreachable` on poll failure; delete emits `ListDeleted`.
  // The meta dispatcher routes each to the list queries. Only
  // home-preferences needs explicit invalidation — it's not tagged
  // (pure UI-local cache).
  const refreshMutation = useMutationWithToast({
    verb: 'refresh list',
    mutationFn: async (id: number) => {
      await refreshList({ path: { id } });
    },
  });

  const deleteMutation = useMutationWithToast({
    verb: 'delete list',
    mutationFn: async (id: number) => {
      await deleteList({ path: { id } });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'preferences', 'home'] });
    },
  });

  const pinMutation = useMutationWithToast({
    verb: 'pin list',
    mutationFn: async (vars: { id: number; pin: boolean }) => {
      const marker = `list:${vars.id}`;
      const current = prefs?.section_order ?? [];
      const next = vars.pin
        ? current.includes(marker)
          ? current
          : [...current, marker]
        : current.filter((s) => s !== marker);
      await updateHomePreferences({ body: { section_order: next } });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['kino', 'preferences', 'home'] }),
  });

  // Apply filter + sort. The Trakt watchlist (`is_system`) always
  // floats to the top regardless of sort so it's findable even in a
  // long list sorted by item count.
  const visible = useMemo(() => {
    const filtered = lists.filter((l) => {
      if (filter === 'all') return true;
      if (filter === 'trakt')
        return l.source_type === 'trakt_list' || l.source_type === 'trakt_watchlist';
      return l.source_type === filter;
    });
    const sorted = [...filtered].sort((a, b) => {
      if (a.is_system !== b.is_system) return a.is_system ? -1 : 1;
      switch (sort) {
        case 'title':
          return a.title.localeCompare(b.title);
        case 'items':
          return b.item_count - a.item_count;
        default: {
          const at = a.last_polled_at ?? '';
          const bt = b.last_polled_at ?? '';
          return bt.localeCompare(at);
        }
      }
    });
    return sorted;
  }, [lists, filter, sort]);

  return (
    <div className="min-h-screen px-4 md:px-12 py-8">
      <header className="flex items-start justify-between mb-6 max-w-6xl gap-4 flex-wrap">
        <div className="flex-1 min-w-0">
          <h1 className="text-2xl md:text-3xl font-bold tracking-tight">Lists</h1>
          <p className="mt-1 text-sm text-[var(--text-muted)] max-w-xl">
            Curated collections of movies and shows. Your Trakt watchlist, community lists, or
            anything you can paste a URL for — all in one place.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setAddOpen(true)}
          className="inline-flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold shadow"
        >
          <Plus size={15} />
          Add list
        </button>
      </header>

      {/* Toolbar — filter chips + sort select + view toggle. Collapses
          to a single line on wide viewports; wraps gracefully on mobile. */}
      {lists.length > 0 && (
        <div className="max-w-6xl mb-5 flex items-center justify-between gap-3 flex-wrap">
          <div className="flex items-center gap-1">
            <FilterChip
              label="All"
              active={filter === 'all'}
              onClick={() => setFilter('all')}
              count={lists.length}
            />
            <FilterChip
              label="Trakt"
              active={filter === 'trakt'}
              onClick={() => setFilter('trakt')}
              count={lists.filter((l) => l.source_type.startsWith('trakt_')).length}
            />
            <FilterChip
              label="MDBList"
              active={filter === 'mdblist'}
              onClick={() => setFilter('mdblist')}
              count={lists.filter((l) => l.source_type === 'mdblist').length}
            />
            <FilterChip
              label="TMDB"
              active={filter === 'tmdb_list'}
              onClick={() => setFilter('tmdb_list')}
              count={lists.filter((l) => l.source_type === 'tmdb_list').length}
            />
          </div>
          <div className="flex items-center gap-2">
            <select
              value={sort}
              onChange={(e) => setSortPersist(e.target.value as SortMode)}
              aria-label="Sort lists"
              className="h-8 px-2 text-xs rounded-md bg-[var(--bg-card)] ring-1 ring-white/10 text-[var(--text-secondary)] hover:text-white focus:outline-none focus:ring-[var(--accent)]/40"
            >
              <option value="polled">Recently polled</option>
              <option value="title">Alphabetical</option>
              <option value="items">Most items</option>
            </select>
            <div className="inline-flex rounded-md bg-[var(--bg-card)] ring-1 ring-white/10 p-0.5">
              <ViewToggle
                active={view === 'grid'}
                onClick={() => setViewPersist('grid')}
                icon={<LayoutGrid size={13} />}
                label="Grid"
              />
              <ViewToggle
                active={view === 'list'}
                onClick={() => setViewPersist('list')}
                icon={<ListIcon size={13} />}
                label="List"
              />
            </div>
          </div>
        </div>
      )}

      {isLoading ? (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 max-w-6xl">
          {Array.from({ length: 3 }, (_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: skeleton
            <div key={i} className="h-72 rounded-xl skeleton" />
          ))}
        </div>
      ) : lists.length === 0 ? (
        <EmptyState onAdd={() => setAddOpen(true)} />
      ) : view === 'grid' ? (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 max-w-6xl">
          {visible.map((l) => (
            <GridCard
              key={l.id}
              list={l}
              pinned={pinnedIds.has(l.id)}
              onRefresh={() => refreshMutation.mutate(l.id)}
              onDelete={() => {
                if (confirm(`Unfollow "${l.title}"? This won't affect your library.`)) {
                  deleteMutation.mutate(l.id);
                }
              }}
              onTogglePin={() => pinMutation.mutate({ id: l.id, pin: !pinnedIds.has(l.id) })}
              refreshing={refreshMutation.isPending && refreshMutation.variables === l.id}
            />
          ))}
        </div>
      ) : (
        <div className="max-w-6xl divide-y divide-white/5 rounded-xl bg-[var(--bg-card)] ring-1 ring-white/5 overflow-hidden">
          {visible.map((l) => (
            <ListRow
              key={l.id}
              list={l}
              pinned={pinnedIds.has(l.id)}
              onRefresh={() => refreshMutation.mutate(l.id)}
              onDelete={() => {
                if (confirm(`Unfollow "${l.title}"? This won't affect your library.`)) {
                  deleteMutation.mutate(l.id);
                }
              }}
              onTogglePin={() => pinMutation.mutate({ id: l.id, pin: !pinnedIds.has(l.id) })}
              refreshing={refreshMutation.isPending && refreshMutation.variables === l.id}
            />
          ))}
        </div>
      )}

      <AddListModal
        open={addOpen}
        onClose={() => setAddOpen(false)}
        onCreated={() => qc.invalidateQueries({ queryKey: ['kino', 'lists'] })}
      />
    </div>
  );
}

// ── Shared pieces ──────────────────────────────────────────────────

interface CardHandlers {
  list: ListView;
  pinned: boolean;
  onRefresh: () => void;
  onDelete: () => void;
  onTogglePin: () => void;
  refreshing: boolean;
}

/** Stacked-poster preview strip — always the same height whether
 *  populated or empty, so cards don't jump as polls land. Shingles
 *  up to four posters; if the list has more, the last slot gets a
 *  "+N" overlay so the strip reads as a window onto something
 *  bigger rather than "only four items".
 *
 *  Empty state: same card footprint, muted gradient placeholders
 *  matching the shingle layout. No text — the meta row below already
 *  says "Pending first poll" via the relative-time formatter. */
function PosterStrip({
  posters,
  totalItems,
  className,
  size = 'md',
}: {
  posters: string[];
  totalItems: number;
  className?: string;
  size?: 'sm' | 'md';
}) {
  const dims =
    size === 'sm'
      ? { w: 'w-8', h: 'h-12', gap: '-space-x-2.5', ring: 'ring-[3px]' }
      : { w: 'w-[56px]', h: 'h-[84px]', gap: '-space-x-3', ring: 'ring-[3px]' };
  const slots = 4;
  const extra = Math.max(0, totalItems - slots);
  // Fill empty slots with placeholder posters so the footprint is
  // identical when the list is pending a first poll.
  const padded = [...posters.slice(0, slots)];
  while (padded.length < Math.min(slots, Math.max(totalItems, 1))) padded.push('');
  while (padded.length < slots && padded.length < 1) padded.push('');
  // Guarantee at least 3 visible shingles for visual mass even when
  // the list genuinely has 1-2 items — the extras are muted gradient
  // placeholders so the user isn't misled into thinking there's more.
  const visible = padded.length > 0 ? padded : Array.from({ length: 3 }, () => '');

  return (
    <div className={cn('flex', dims.gap, className)}>
      {visible.map((p, idx) => {
        const url = p ? tmdbImage(p, 'w185') : null;
        const isLast = idx === visible.length - 1;
        const showPlus = isLast && extra > 0;
        return (
          <div
            // biome-ignore lint/suspicious/noArrayIndexKey: stable source-order slice
            key={idx}
            className={cn(
              'relative shrink-0 rounded-md overflow-hidden bg-[var(--bg-secondary)]',
              dims.w,
              dims.h,
              dims.ring,
              'ring-[var(--bg-card)]'
            )}
            style={{ zIndex: 10 - idx }}
          >
            {url ? (
              <img src={url} alt="" loading="lazy" className="w-full h-full object-cover" />
            ) : (
              <div className="w-full h-full bg-gradient-to-br from-white/[0.04] to-white/[0.02]" />
            )}
            {showPlus && (
              <div className="absolute inset-0 flex items-center justify-center bg-black/55 backdrop-blur-[1px] text-white text-[11px] font-semibold tabular-nums">
                +{extra}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function SourceBadge({ sourceType }: { sourceType: string }) {
  if (sourceType === 'trakt_list' || sourceType === 'trakt_watchlist') {
    return (
      <span
        title={sourceType === 'trakt_watchlist' ? 'Trakt watchlist' : 'Trakt list'}
        className="inline-flex items-center"
      >
        <img src="/trakt-mark.svg" alt="Trakt" className="h-5 w-5 opacity-80" />
      </span>
    );
  }
  const label =
    sourceType === 'mdblist' ? 'MDBList' : sourceType === 'tmdb_list' ? 'TMDB' : sourceType;
  return (
    <span
      title={label}
      className="inline-flex items-center px-2 py-0.5 rounded-md text-[10px] font-semibold uppercase tracking-wider bg-white/5 text-[var(--text-muted)] ring-1 ring-white/5"
    >
      {label}
    </span>
  );
}

function FilterChip({
  label,
  active,
  onClick,
  count,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  count?: number;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        'inline-flex items-center gap-1.5 h-8 px-3 rounded-md text-xs font-medium transition',
        active
          ? 'bg-white/10 text-white'
          : 'text-[var(--text-muted)] hover:text-white hover:bg-white/5'
      )}
    >
      {label}
      {count != null && count > 0 && (
        <span className="tabular-nums text-[10px] text-[var(--text-muted)]">{count}</span>
      )}
    </button>
  );
}

function ViewToggle({
  active,
  onClick,
  icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      aria-label={`${label} view`}
      title={`${label} view`}
      className={cn(
        'inline-flex items-center justify-center h-7 w-8 rounded text-xs transition',
        active ? 'bg-white/10 text-white' : 'text-[var(--text-muted)] hover:text-white'
      )}
    >
      {icon}
    </button>
  );
}

function formatRelative(iso: string | null | undefined): string {
  if (!iso) return 'never polled';
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return 'never polled';
  const diff = Math.max(0, Date.now() - then);
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(iso).toLocaleDateString();
}

function ItemTypeLabel({ t }: { t: string }) {
  if (t === 'movies') return <span>Movies</span>;
  if (t === 'shows') return <span>Shows</span>;
  if (t === 'mixed') return <span>Mixed</span>;
  return <span>{t}</span>;
}

// ── Grid card ───────────────────────────────────────────────────────

function GridCard({ list, pinned, onRefresh, onDelete, onTogglePin, refreshing }: CardHandlers) {
  const isSystem = list.is_system;

  return (
    <div
      className={cn(
        'relative group flex flex-col rounded-xl overflow-hidden bg-[var(--bg-card)] border border-white/5 hover:border-white/10 transition',
        isSystem && 'ring-1 ring-red-500/20'
      )}
    >
      {/* Poster strip — same footprint whether populated or empty so
          cards don't jump sizes across the grid. Click-through link. */}
      <Link
        to="/lists/$id"
        params={{ id: String(list.id) }}
        className="block px-4 pt-4 pb-3 bg-gradient-to-b from-white/[0.03] to-transparent"
      >
        <div className="flex items-center justify-center h-[84px]">
          <PosterStrip posters={list.preview_posters} totalItems={list.item_count} />
        </div>
      </Link>

      {/* Body */}
      <div className="flex flex-col flex-1 px-4 pb-3">
        {/* Title line: source mark · title · delete (hover) / lock (system) */}
        <div className="flex items-center gap-2 min-w-0">
          <SourceBadge sourceType={list.source_type} />
          <Link
            to="/lists/$id"
            params={{ id: String(list.id) }}
            className="text-sm font-semibold text-white line-clamp-1 flex-1 min-w-0 hover:text-[var(--text-secondary)] transition"
          >
            {list.title}
          </Link>
          {isSystem ? (
            <span
              title="System list — disconnect Trakt to remove"
              className="text-[var(--text-muted)] shrink-0"
            >
              <Lock size={14} />
            </span>
          ) : (
            <button
              type="button"
              onClick={onDelete}
              title="Unfollow"
              aria-label="Unfollow list"
              className="p-1 rounded text-[var(--text-muted)] hover:text-red-400 hover:bg-white/5 opacity-0 group-hover:opacity-100 transition shrink-0"
            >
              <Trash2 size={15} />
            </button>
          )}
        </div>

        {list.description && (
          <p className="mt-1 text-xs text-[var(--text-secondary)] line-clamp-2">
            {list.description}
          </p>
        )}

        <div className="mt-2.5 pt-2.5 border-t border-white/5 flex items-center justify-between text-xs text-[var(--text-muted)] gap-2">
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="tabular-nums">{list.item_count}</span>
            <ItemTypeLabel t={list.item_type} />
            <span className="text-white/20">•</span>
            <span className="truncate">{formatRelative(list.last_polled_at)}</span>
          </div>
          <CardActions
            pinned={pinned}
            sourceUrl={list.source_url}
            refreshing={refreshing}
            onTogglePin={onTogglePin}
            onRefresh={onRefresh}
          />
        </div>

        {list.last_poll_status?.startsWith('error:') && (
          <p className="mt-2 text-[10px] text-red-400">
            {list.consecutive_poll_failures >= 3
              ? 'Unreachable — check settings'
              : list.last_poll_status}
          </p>
        )}
      </div>
    </div>
  );
}

// ── List row ────────────────────────────────────────────────────────

function ListRow({ list, pinned, onRefresh, onDelete, onTogglePin, refreshing }: CardHandlers) {
  const isSystem = list.is_system;
  return (
    <div
      className={cn(
        'group relative flex items-center gap-4 px-4 py-3 hover:bg-white/[0.02] transition',
        isSystem && 'bg-red-500/[0.02]'
      )}
    >
      <div className="flex-shrink-0 w-40 hidden sm:block">
        <PosterStrip posters={list.preview_posters} totalItems={list.item_count} size="sm" />
      </div>

      <Link to="/lists/$id" params={{ id: String(list.id) }} className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <SourceBadge sourceType={list.source_type} />
          <h3 className="text-sm font-semibold text-white line-clamp-1">{list.title}</h3>
          {isSystem && <Lock size={11} className="text-[var(--text-muted)] shrink-0" />}
        </div>
        {list.description && (
          <p className="mt-1 text-xs text-[var(--text-secondary)] line-clamp-2 max-w-2xl">
            {list.description}
          </p>
        )}
        <div className="mt-1.5 flex items-center gap-1.5 text-xs text-[var(--text-muted)]">
          <span className="tabular-nums">{list.item_count}</span>
          <ItemTypeLabel t={list.item_type} />
          <span className="text-white/20">•</span>
          <span>{formatRelative(list.last_polled_at)}</span>
          {list.last_poll_status?.startsWith('error:') && (
            <>
              <span className="text-white/20">•</span>
              <span className="text-red-400">
                {list.consecutive_poll_failures >= 3
                  ? 'Unreachable'
                  : list.last_poll_status.replace(/^error:\s*/, '')}
              </span>
            </>
          )}
        </div>
      </Link>

      <div className="flex items-center gap-0.5 shrink-0">
        <CardActions
          pinned={pinned}
          sourceUrl={list.source_url}
          refreshing={refreshing}
          onTogglePin={onTogglePin}
          onRefresh={onRefresh}
        />
        {!isSystem && (
          <button
            type="button"
            onClick={onDelete}
            title="Unfollow"
            aria-label="Unfollow list"
            className="p-1.5 rounded text-[var(--text-muted)] hover:text-red-400 hover:bg-white/5 opacity-0 group-hover:opacity-100 transition"
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>
    </div>
  );
}

function CardActions({
  pinned,
  sourceUrl,
  refreshing,
  onTogglePin,
  onRefresh,
}: {
  pinned: boolean;
  sourceUrl: string;
  refreshing: boolean;
  onTogglePin: () => void;
  onRefresh: () => void;
}) {
  return (
    <div className="flex items-center gap-0.5">
      <button
        type="button"
        onClick={onTogglePin}
        title={pinned ? 'Unpin from Home' : 'Pin to Home'}
        aria-label={pinned ? 'Unpin from Home' : 'Pin to Home'}
        aria-pressed={pinned}
        className={cn(
          'p-2 rounded transition',
          pinned ? 'text-amber-400 hover:bg-amber-500/10' : 'hover:bg-white/5 hover:text-white'
        )}
      >
        {pinned ? <Pin size={15} className="fill-current" /> : <PinOff size={15} />}
      </button>
      {sourceUrl && (
        <a
          href={sourceUrl}
          target="_blank"
          rel="noopener noreferrer"
          title="Open source"
          className="p-2 rounded hover:bg-white/5 hover:text-white"
        >
          <ExternalLink size={15} />
        </a>
      )}
      <button
        type="button"
        onClick={onRefresh}
        disabled={refreshing}
        title="Refresh now"
        aria-label="Refresh list"
        className="p-2 rounded hover:bg-white/5 hover:text-white disabled:opacity-50"
      >
        <RefreshCw size={15} className={refreshing ? 'motion-safe:animate-spin' : undefined} />
      </button>
    </div>
  );
}

function EmptyState({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center text-center py-24 max-w-md mx-auto">
      <div className="w-14 h-14 rounded-full bg-white/5 grid place-items-center mb-4">
        <Plus size={24} className="text-[var(--text-muted)]" />
      </div>
      <h2 className="text-lg font-semibold">No lists yet</h2>
      <p className="mt-2 text-sm text-[var(--text-secondary)]">
        Collections of movies and shows — the IMDb Top 250, a friend's recommended-films list, your
        own Trakt watchlist. Paste any URL to follow.
      </p>
      <button
        type="button"
        onClick={onAdd}
        className="mt-5 inline-flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold"
      >
        <Plus size={15} />
        Add your first list
      </button>
    </div>
  );
}
