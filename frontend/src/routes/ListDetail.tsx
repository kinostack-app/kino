/**
 * List detail (subsystem 17).
 *
 * Grid of poster cards, one per list item. Each card shows its
 * acquisition state (not_in_library / monitoring / acquired /
 * watched / ignored) via a small badge so the user can see at a
 * glance what's in their library vs waiting.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useParams } from '@tanstack/react-router';
import { ArrowLeft, ExternalLink, RefreshCw } from 'lucide-react';
import { useMemo } from 'react';
import { getList, listItems, refreshList } from '@/api/generated/sdk.gen';
import type { ListItemView, List as ListRow } from '@/api/generated/types.gen';
import { TmdbMovieCard } from '@/components/TmdbMovieCard';
import { TmdbShowCard } from '@/components/TmdbShowCard';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';

export function ListDetail() {
  const { id } = useParams({ from: '/lists/$id' });
  const listId = Number(id);
  const qc = useQueryClient();

  const { data: list } = useQuery<ListRow | null>({
    queryKey: ['kino', 'lists', listId],
    queryFn: async () => {
      const r = await getList({ path: { id: listId } });
      return (r.data as ListRow | undefined) ?? null;
    },
    meta: {
      invalidatedBy: ['list_bulk_growth', 'list_unreachable', 'list_auto_added', 'list_deleted'],
    },
  });

  const { data: items = [], isLoading } = useQuery<ListItemView[]>({
    queryKey: ['kino', 'lists', listId, 'items'],
    queryFn: async () => {
      const r = await listItems({ path: { id: listId } });
      return (r.data as ListItemView[] | undefined) ?? [];
    },
    meta: {
      invalidatedBy: ['list_bulk_growth', 'list_unreachable', 'list_auto_added', 'list_deleted'],
    },
  });

  useDocumentTitle(list?.title ?? null);

  const refreshMutation = useMutation({
    mutationFn: async () => {
      await refreshList({ path: { id: listId } });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'lists', listId] });
      qc.invalidateQueries({ queryKey: ['kino', 'lists', listId, 'items'] });
    },
  });

  const sorted = useMemo(
    () =>
      [...items].sort((a, b) => {
        const ap = a.position ?? Number.MAX_SAFE_INTEGER;
        const bp = b.position ?? Number.MAX_SAFE_INTEGER;
        if (ap !== bp) return ap - bp;
        return a.title.localeCompare(b.title);
      }),
    [items]
  );

  if (!list) {
    return (
      <div className="px-4 md:px-12 py-8">
        <Link
          to="/lists"
          className="inline-flex items-center gap-1 text-sm text-[var(--text-muted)] hover:text-white"
        >
          <ArrowLeft size={14} /> Lists
        </Link>
      </div>
    );
  }

  return (
    <div className="min-h-screen px-4 md:px-12 py-8">
      <div className="max-w-6xl">
        <Link
          to="/lists"
          className="inline-flex items-center gap-1 text-xs uppercase tracking-wider text-[var(--text-muted)] hover:text-white transition"
        >
          <ArrowLeft size={14} /> Lists
        </Link>

        <header className="mt-3 flex items-start justify-between gap-6">
          <div className="flex-1 min-w-0">
            <h1 className="text-2xl md:text-3xl font-bold tracking-tight">{list.title}</h1>
            {list.description && (
              <p className="mt-2 text-sm text-[var(--text-secondary)] max-w-2xl">
                {list.description}
              </p>
            )}
            <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[var(--text-muted)]">
              <span className="tabular-nums">{list.item_count} items</span>
              <span>•</span>
              <a
                href={list.source_url}
                target="_blank"
                rel="noopener noreferrer"
                title={sourceTypeLabel(list.source_type)}
                className="inline-flex items-center gap-1 hover:text-white transition"
              >
                {list.source_type.startsWith('trakt_') ? (
                  <img src="/trakt-mark.svg" alt="Trakt" className="h-4 w-4 opacity-80" />
                ) : (
                  <span>{sourceTypeLabel(list.source_type)}</span>
                )}
                <ExternalLink size={11} />
              </a>
              {list.last_polled_at && (
                <>
                  <span>•</span>
                  <span>Updated {new Date(list.last_polled_at).toLocaleString()}</span>
                </>
              )}
            </div>
          </div>
          <button
            type="button"
            onClick={() => refreshMutation.mutate()}
            disabled={refreshMutation.isPending}
            className="inline-flex items-center gap-2 px-3 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-sm font-medium disabled:opacity-50"
          >
            <RefreshCw
              size={14}
              className={refreshMutation.isPending ? 'motion-safe:animate-spin' : undefined}
            />
            Refresh
          </button>
        </header>

        <div className="mt-8">
          {isLoading ? (
            <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 gap-4">
              {Array.from({ length: 12 }, (_, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: skeleton
                <div key={i} className="aspect-[2/3] rounded-lg skeleton" />
              ))}
            </div>
          ) : sorted.length === 0 ? (
            <p className="text-sm text-[var(--text-muted)] py-8 text-center">
              No items. The next poll will populate this list.
            </p>
          ) : (
            <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 gap-4">
              {sorted.map((it) =>
                it.item_type === 'show' ? (
                  <TmdbShowCard
                    key={it.id}
                    id={it.tmdb_id}
                    name={it.title}
                    posterPath={it.poster_path ?? undefined}
                  />
                ) : (
                  <TmdbMovieCard
                    key={it.id}
                    id={it.tmdb_id}
                    title={it.title}
                    posterPath={it.poster_path ?? undefined}
                  />
                )
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function sourceTypeLabel(st: string): string {
  switch (st) {
    case 'mdblist':
      return 'MDBList';
    case 'tmdb_list':
      return 'TMDB';
    case 'trakt_list':
      return 'Trakt';
    case 'trakt_watchlist':
      return 'Trakt watchlist';
    default:
      return st;
  }
}
