/**
 * Global library cache — single source of truth for all content in the user's library.
 *
 * Two queries: movies and shows. Both cached globally with long stale time,
 * updated optimistically on mutations, and background-refreshed periodically.
 *
 * Types come straight from the generated OpenAPI schema so backend
 * field renames / enum tightenings fail the frontend build loudly
 * instead of drifting into silent stale-field bugs. The aliases are
 * kept as `LibraryMovie` / `LibraryShow` / `ActiveDownload` so
 * consumers don't have to care which flavour they're reading —
 * they match the UI's view of library content.
 */

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { listDownloads, listMovies, listShows } from '@/api/generated/sdk.gen';
import type { DownloadWithContent, Movie, ShowListItem } from '@/api/generated/types.gen';
import type { InvalidationRule } from '@/state/invalidation';

export type LibraryMovie = Movie;
export type LibraryShow = ShowListItem;
export type ActiveDownload = DownloadWithContent;

const CACHE_CONFIG = {
  staleTime: 30_000, // Background refetch every 30s
  gcTime: Number.POSITIVE_INFINITY, // Never garbage collect
  refetchOnWindowFocus: true,
};

// ── Query keys (stable references) ──
//
// One place to edit when we want to invalidate a whole domain from the
// WS event handler. Each key is a prefix — use `queryClient.invalidate
// Queries({ queryKey: KEY })` and TanStack matches any query starting
// with this array.

export const LIBRARY_MOVIES_KEY = ['kino', 'library', 'movies'] as const;
export const LIBRARY_SHOWS_KEY = ['kino', 'library', 'shows'] as const;
export const DOWNLOADS_KEY = ['kino', 'downloads'] as const;
export const STATUS_KEY = ['kino', 'status'] as const;
export const INDEXERS_KEY = ['kino', 'indexers'] as const;
export const CONFIG_KEY = ['kino', 'config'] as const;
export const QUALITY_PROFILES_KEY = ['kino', 'quality-profiles'] as const;
export const WEBHOOKS_KEY = ['kino', 'webhooks'] as const;

// ── Invalidation rule presets ──
//
// Co-located with the hooks that subscribe to them so "what events
// refresh this?" is readable at the source. The WS handler's
// dispatcher walks `meta.invalidatedBy` — these arrays feed into it.

export const LIBRARY_MOVIES_INVALIDATED_BY: InvalidationRule[] = [
  'movie_added',
  'imported',
  'upgraded',
  'content_removed',
  'search_started',
  'release_grabbed',
  'download_started',
  'download_complete',
  'download_failed',
  'download_cancelled',
  'download_paused',
  'download_resumed',
  'watched',
  'unwatched',
  'rated',
  'trakt_synced',
];

export const LIBRARY_SHOWS_INVALIDATED_BY: InvalidationRule[] = [
  'show_added',
  'imported',
  'upgraded',
  'content_removed',
  'search_started',
  'release_grabbed',
  'download_started',
  'download_complete',
  'download_failed',
  'download_cancelled',
  'download_paused',
  'download_resumed',
  'watched',
  'unwatched',
  'rated',
  'show_monitor_changed',
  'new_episode',
  'trakt_synced',
];

export const DOWNLOADS_INVALIDATED_BY: InvalidationRule[] = [
  'imported',
  'upgraded',
  'content_removed',
  'search_started',
  'release_grabbed',
  'download_started',
  'download_complete',
  'download_failed',
  'download_cancelled',
  'download_paused',
  'download_resumed',
];

// ── Hooks to read the caches ──

export function useLibraryMovies() {
  return useQuery({
    queryKey: [...LIBRARY_MOVIES_KEY],
    queryFn: async () => {
      const { data } = await listMovies();
      return data?.results ?? [];
    },
    ...CACHE_CONFIG,
    meta: { invalidatedBy: LIBRARY_MOVIES_INVALIDATED_BY },
  });
}

export function useLibraryShows() {
  return useQuery({
    queryKey: [...LIBRARY_SHOWS_KEY],
    queryFn: async () => {
      const { data } = await listShows();
      return data?.results ?? [];
    },
    ...CACHE_CONFIG,
    meta: { invalidatedBy: LIBRARY_SHOWS_INVALIDATED_BY },
  });
}

export function useDownloads() {
  return useQuery({
    queryKey: [...DOWNLOADS_KEY],
    queryFn: async () => {
      // `/downloads` is paginated per the 09-api spec — `.results`
      // carries the current page. Library + Downloading tab
      // consumers pull a single page's worth (default 25) which
      // covers every realistic active-download count; the UI never
      // needs to surface thousands of queued torrents in one view.
      // When that assumption ever breaks we'd swap this for an
      // infinite query and render a scroller.
      const { data } = await listDownloads();
      return data?.results ?? [];
    },
    ...CACHE_CONFIG,
    staleTime: 3_000, // Match backend poll interval
    meta: { invalidatedBy: DOWNLOADS_INVALIDATED_BY },
  });
}

// ── Lookup helpers (used by useContentState) ──

export function useMovieByTmdbId(tmdbId: number): LibraryMovie | undefined {
  const { data } = useLibraryMovies();
  return data?.find((m) => m.tmdb_id === tmdbId);
}

export function useShowByTmdbId(tmdbId: number): LibraryShow | undefined {
  const { data } = useLibraryShows();
  return data?.find((s) => s.tmdb_id === tmdbId);
}

export function useDownloadForMovie(movieId: number | undefined): ActiveDownload | undefined {
  const { data } = useDownloads();
  if (!movieId) return undefined;
  const active = new Set<ActiveDownload['state']>([
    'queued',
    'grabbing',
    'downloading',
    'paused',
    'stalled',
  ]);
  return data?.find((d) => d.content_movie_id === movieId && active.has(d.state));
}

// ── Cache mutation helpers ──

export function useLibraryCacheUpdater() {
  const queryClient = useQueryClient();

  return {
    /** Optimistically add a movie to the cache. The placeholder is
     *  overwritten by `replaceMovie` as soon as the server response
     *  lands, so fields we can't know yet (added_at, last_played_at,
     *  …) get zero-ish defaults that satisfy the generated `Movie`
     *  shape without lying via `as`. */
    addMovie(tmdbId: number, partial: Partial<LibraryMovie>) {
      const placeholder: LibraryMovie = {
        id: -tmdbId,
        tmdb_id: tmdbId,
        title: '',
        status: 'wanted',
        monitored: true,
        quality_profile_id: 1,
        playback_position_ticks: 0,
        play_count: 0,
        added_at: '',
        ...partial,
      };
      queryClient.setQueryData<LibraryMovie[]>([...LIBRARY_MOVIES_KEY], (old) => {
        if (!old) return [placeholder];
        if (old.some((m) => m.tmdb_id === tmdbId)) return old;
        return [placeholder, ...old];
      });
    },

    /** Replace an optimistic movie entry with real server data */
    replaceMovie(tmdbId: number, movie: LibraryMovie) {
      queryClient.setQueryData<LibraryMovie[]>([...LIBRARY_MOVIES_KEY], (old) => {
        if (!old) return [movie];
        return old.map((m) => (m.tmdb_id === tmdbId ? movie : m));
      });
    },

    /** Remove a movie from the cache */
    removeMovie(tmdbId: number) {
      queryClient.setQueryData<LibraryMovie[]>([...LIBRARY_MOVIES_KEY], (old) => {
        if (!old) return [];
        return old.filter((m) => m.tmdb_id !== tmdbId);
      });
    },

    /** Optimistically add a show. Same no-`as` placeholder discipline
     *  as `addMovie` — populate every required field of the generated
     *  `ShowListItem` shape up-front so the compiler keeps us honest
     *  when the backend adds a new required rollup. */
    addShow(tmdbId: number, partial: Partial<LibraryShow>) {
      const placeholder: LibraryShow = {
        id: -tmdbId,
        tmdb_id: tmdbId,
        title: '',
        monitored: true,
        monitor_new_items: 'future',
        follow_intent: 'explicit',
        quality_profile_id: 1,
        added_at: '',
        aired_episode_count: 0,
        available_episode_count: 0,
        episode_count: 0,
        upcoming_episode_count: 0,
        watched_episode_count: 0,
        wanted_episode_count: 0,
        ...partial,
      };
      queryClient.setQueryData<LibraryShow[]>([...LIBRARY_SHOWS_KEY], (old) => {
        if (!old) return [placeholder];
        if (old.some((s) => s.tmdb_id === tmdbId)) return old;
        return [placeholder, ...old];
      });
    },

    /** Replace an optimistic show entry */
    replaceShow(tmdbId: number, show: LibraryShow) {
      queryClient.setQueryData<LibraryShow[]>([...LIBRARY_SHOWS_KEY], (old) => {
        if (!old) return [show];
        return old.map((s) => (s.tmdb_id === tmdbId ? show : s));
      });
    },

    /** Remove a show */
    removeShow(tmdbId: number) {
      queryClient.setQueryData<LibraryShow[]>([...LIBRARY_SHOWS_KEY], (old) => {
        if (!old) return [];
        return old.filter((s) => s.tmdb_id !== tmdbId);
      });
    },

    /** Optimistically patch a download in the cache */
    patchDownload(downloadId: number, patch: Partial<ActiveDownload>) {
      queryClient.setQueryData<ActiveDownload[]>([...DOWNLOADS_KEY], (old) => {
        if (!old) return old;
        return old.map((d) => (d.id === downloadId ? { ...d, ...patch } : d));
      });
    },

    /** Rollback — refetch from server */
    refetchAll() {
      queryClient.invalidateQueries({ queryKey: [...LIBRARY_MOVIES_KEY] });
      queryClient.invalidateQueries({ queryKey: [...LIBRARY_SHOWS_KEY] });
      queryClient.invalidateQueries({ queryKey: [...DOWNLOADS_KEY] });
    },
  };
}
