import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { showSeasonEpisodesByTmdb } from '@/api/generated/sdk.gen';
import type { EpisodeView } from '@/api/generated/types.gen';
import type { InvalidationRule } from '@/state/invalidation';

/** Episode list state flips on library events (the left-join fields
 *  change) + download and watched transitions. */
const SHOW_EPISODES_INVALIDATED_BY: InvalidationRule[] = [
  'movie_added',
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

/**
 * Episodes for a given show+season. Always returns an
 * `EpisodeView[]` — TMDB metadata left-joined with library state
 * (status, monitored, watched_at, active download info, imported
 * media) when the show is in the library. Library fields are null
 * for not-yet-followed shows.
 *
 * Single canonical endpoint means `EpisodeCard` can render the
 * correct state without the parent branching on follow status.
 */
export function useSeasonEpisodes(showTmdbId: number, seasonNumber: number, totalSeasons: number) {
  const queryClient = useQueryClient();

  const query = useQuery({
    queryKey: ['show-episodes', showTmdbId, seasonNumber],
    queryFn: async (): Promise<EpisodeView[]> => {
      const { data } = await showSeasonEpisodesByTmdb({
        path: { tmdb_id: showTmdbId, season_number: seasonNumber },
      });
      return data ?? [];
    },
    // Moderate stale time — TMDB data rarely changes. Library-state
    // invalidation is driven by WS events (see websocket.ts) rather
    // than wall-clock polls.
    staleTime: 60_000,
    gcTime: 10 * 60_000,
    meta: { invalidatedBy: SHOW_EPISODES_INVALIDATED_BY },
  });

  // Prefetch adjacent seasons for instant switching. Include
  // Season 0 in range since specials are now browsable.
  useEffect(() => {
    const prefetch = (sn: number) => {
      if (sn < 0 || sn > totalSeasons) return;
      queryClient.prefetchQuery({
        queryKey: ['show-episodes', showTmdbId, sn],
        queryFn: async (): Promise<EpisodeView[]> => {
          const { data } = await showSeasonEpisodesByTmdb({
            path: { tmdb_id: showTmdbId, season_number: sn },
          });
          return data ?? [];
        },
        staleTime: 60_000,
      });
    };
    const timer = setTimeout(() => {
      prefetch(seasonNumber - 1);
      prefetch(seasonNumber + 1);
    }, 400);
    return () => clearTimeout(timer);
  }, [showTmdbId, seasonNumber, totalSeasons, queryClient]);

  return query;
}
