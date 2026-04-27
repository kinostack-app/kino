/**
 * Hook to navigate to the unified player for a movie or episode.
 *
 * Callers know the entity they're playing, so the navigation is
 * direct to `/play/$kind/$entityId` with no pre-flight lookup.
 */

import { useNavigate } from '@tanstack/react-router';
import { useCallback } from 'react';

export function usePlayMedia() {
  const navigate = useNavigate();

  const playMovie = useCallback(
    (movieId: number) => {
      void navigate({
        to: '/play/$kind/$entityId',
        params: { kind: 'movie', entityId: String(movieId) },
      });
    },
    [navigate]
  );

  const playEpisode = useCallback(
    (episodeId: number) => {
      void navigate({
        to: '/play/$kind/$entityId',
        params: { kind: 'episode', entityId: String(episodeId) },
      });
    },
    [navigate]
  );

  return { playMovie, playEpisode, isLoading: false };
}
