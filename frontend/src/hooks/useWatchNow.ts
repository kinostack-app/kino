import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { watchNow } from '@/api/generated/sdk.gen';
import { kinoToast } from '@/components/kino-toast';
import { DOWNLOADS_KEY, LIBRARY_MOVIES_KEY, LIBRARY_SHOWS_KEY } from '@/state/library-cache';

/**
 * Kick off a Watch-now session for a movie (by TMDB id), an episode
 * (by library id), or a show (smart-play).
 *
 * Flow: the backend's two-phase orchestrator returns a `download_id`
 * immediately — a placeholder `searching` row — then runs the search
 * + grab + start in a background task. We navigate straight to the
 * player on that id, which drives its loading stepper off the same
 * download's state transitions (searching → queued → grabbing →
 * downloading). Nothing to poll on the caller side; nothing in-flight
 * to hide behind.
 *
 * Pre-navigation failures (no indexers configured, unknown episode,
 * auto-follow hiccup) surface as toasts — the player never loads for
 * those cases because the backend couldn't even create a placeholder.
 * Post-navigation failures (search timed out, no releases) land on
 * the download row as `state='failed'` + `error_message` and the
 * player renders them inline from there.
 */
export type WatchNowInput =
  | { kind: 'movie'; tmdbId: number; title: string }
  | { kind: 'episode'; episodeId: number; title: string }
  | {
      kind: 'episode_by_tmdb';
      showTmdbId: number;
      season: number;
      episode: number;
      title: string;
    }
  | { kind: 'show_smart_play'; showTmdbId: number; title: string };

export function useWatchNow() {
  const navigate = useNavigate();
  const qc = useQueryClient();

  const mutation = useMutation({
    mutationFn: async (input: WatchNowInput) => {
      const body =
        input.kind === 'movie'
          ? { kind: 'movie' as const, tmdb_id: input.tmdbId }
          : input.kind === 'episode'
            ? { kind: 'episode' as const, episode_id: input.episodeId }
            : input.kind === 'episode_by_tmdb'
              ? {
                  kind: 'episode_by_tmdb' as const,
                  show_tmdb_id: input.showTmdbId,
                  season: input.season,
                  episode: input.episode,
                }
              : {
                  kind: 'show_smart_play' as const,
                  show_tmdb_id: input.showTmdbId,
                };
      // throwOnError: true so any 4xx surfaces via `onError` with the
      // parsed `{error:{code,message}}` body intact — otherwise the SDK
      // returns `data: undefined` and our `reply.kind` dereference
      // would crash silently.
      const { data } = await watchNow({ body, throwOnError: true });
      return { reply: data };
    },
    onSuccess: ({ reply }) => {
      // Kick a library refetch off in parallel with navigation. The
      // WS `download_started` event invalidates these caches, but
      // because this tab is about to unmount Home and mount /play,
      // there are no observers — invalidate marks stale but doesn't
      // fire a refetch. When the user navigates back from the
      // player, TanStack's refetch-on-mount kicks in, but only after
      // the stale data has already painted, producing a visible flash
      // of "available with 0/X badge" instead of the downloading
      // overlay. `refetchQueries` forces the refetch now, in parallel
      // with navigation, so the cache is fresh by the time Home
      // remounts. Fire-and-forget — we don't block navigation on it.
      void qc.refetchQueries({ queryKey: [...LIBRARY_SHOWS_KEY] });
      void qc.refetchQueries({ queryKey: [...LIBRARY_MOVIES_KEY] });
      void qc.refetchQueries({ queryKey: [...DOWNLOADS_KEY] });
      navigate({
        to: '/play/$kind/$entityId',
        params: { kind: reply.kind, entityId: String(reply.entity_id) },
      });
    },
    onError: (err) => {
      const { message, action } = describeWatchNowError(err);
      kinoToast.error(message, {
        action: action
          ? {
              label: action.label,
              onClick: () => {
                void navigate({ to: action.href });
              },
            }
          : undefined,
      });
    },
  });

  return { watchNow: mutation.mutate, isPending: mutation.isPending };
}

/** Extract a human message and (where relevant) a "fix it" action
 * from a thrown watch-now error. The backend's `{error:{code,message}}`
 * body is what reaches us here when `throwOnError: true`. */
function describeWatchNowError(err: unknown): {
  message: string;
  action?: { label: string; href: string };
} {
  const fallback = 'Couldn\u2019t start watch-now. Try again in a moment.';
  if (err && typeof err === 'object' && 'error' in err) {
    const inner = (err as { error?: { message?: string; code?: string } }).error;
    const message = inner?.message ?? fallback;
    if (message.startsWith('No indexers configured')) {
      return {
        message,
        action: { label: 'Go to Indexers', href: '/settings/indexers' },
      };
    }
    return { message };
  }
  if (err instanceof Error && err.message) {
    return { message: err.message };
  }
  return { message: fallback };
}
