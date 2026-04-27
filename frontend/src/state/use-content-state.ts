/**
 * useContentState — discriminated union returning either `MovieState`
 * or `ShowState` based on the content kind.
 *
 * Motivation (from the architecture audit): movies and shows have
 * genuinely different state machines. Movies have the acquisition
 * lifecycle (wanted → searching → downloading → available → watched)
 * with per-content pause/resume/retry; shows don't — at the show
 * level they're "in the library or not," and acquisition lives on
 * the episode rows surfaced by `show_season_episodes_by_tmdb`.
 *
 * Before this split, `ContentState` was one flat shape with
 * `type: 'movie' | 'show'` and the compiler couldn't stop a show
 * consumer from reading `canPause`. The old `computePhase` even
 * short-circuited shows to `'available'` to paper over this.
 *
 * Now: call `useContentState(id, 'movie')` to get `MovieState`,
 * `useContentState(id, 'show')` to get `ShowState`. Consumers that
 * accept either narrow on `state.type` and the compiler enforces
 * the right accesses.
 */

import { useQueryClient } from '@tanstack/react-query';
import {
  createMovie,
  createShow,
  deleteMovie,
  deleteShow,
  pauseDownload,
  pauseShowDownloads,
  resumeDownload,
  resumeShowDownloads,
} from '@/api/generated/sdk.gen';
import type {
  ContentStatus,
  CreateShow,
  DownloadState,
  MonitorNewItems,
} from '@/api/generated/types.gen';
import { kinoToast } from '@/components/kino-toast';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';
import {
  DOWNLOADS_KEY,
  LIBRARY_MOVIES_KEY,
  LIBRARY_SHOWS_KEY,
  type LibraryMovie,
  type LibraryShow,
  useDownloadForMovie,
  useLibraryCacheUpdater,
  useMovieByTmdbId,
  useShowByTmdbId,
} from './library-cache';
import { queryMatchesId } from './query-utils';

// ── Phase taxonomy ────────────────────────────────────────────

/** Acquisition lifecycle phases for a movie. */
export type MoviePhase =
  | 'none' // Not in library
  | 'searching' // In library, wanted, searching indexers
  | 'queued' // Download queued / getting metadata
  | 'downloading' // Actively downloading
  | 'stalled' // Has peers but no speed
  | 'paused' // User paused
  | 'failed' // Download failed
  | 'importing' // Post-download, creating media
  | 'available' // File ready, never fully watched
  | 'watched'; // Completed playback

/** Show-level phase mirrors movie phases for the *active episode* the
 * backend surfaces via `ShowListItem.active_download`. When no episode
 * is in flight, we fall back to the binary two-state.
 *
 * Per-episode state on the ShowDetail page still lives on `EpisodeView`
 * from `show_season_episodes_by_tmdb` — this phase is card-surface
 * only: "what's happening for this show right now?"
 */
export type ShowPhase = MoviePhase;

/** Legacy alias for surfaces (e.g. `PosterCard`) that accept any
 * phase string. Prefer the narrow types above when you control the
 * call site. */
export type ContentPhase = MoviePhase;

/** Options accepted by the add mutation — used by the Follow Show
 * dialog's per-season picker + the monitor-future-episodes policy.
 * Movies ignore every field. */
export interface AddContentOptions {
  /** What to do with new episodes TMDB reveals over time.
   *  - `'future'` (default): auto-grab new episodes as they air.
   *  - `'none'`: track only; user grabs manually via the episode
   *    card's + button.
   */
  monitorNewItems?: MonitorNewItems;
  /** Season numbers to monitor (download). Omit to monitor all.
   *  Empty array means "track the show, download nothing". */
  seasonsToMonitor?: number[];
  /** Opt into Season 0 ("Specials"). Off by default — many shows
   *  drop weekly specials the user doesn't want clogging Next Up
   *  and the calendar. The backend normalises Season 0 episodes
   *  based on this flag. */
  monitorSpecials?: boolean;
}

// ── Base + variants ────────────────────────────────────────────

interface BaseState {
  tmdbId: number;
  /** Internal DB id (used for API calls). Undefined for not-yet-
   *  in-library content or for optimistic placeholders. */
  libraryId?: number;
  posterPath?: string;
  canAdd: boolean;
  canRemove: boolean;
  /** When true, removing this item should prompt for confirmation
   *  (active work, on-disk file, etc.). */
  needsConfirmToRemove: boolean;
  /** Add to library. Accepts AddContentOptions; movies ignore them,
   *  shows use them for the per-season picker. */
  add: (options?: AddContentOptions) => void;
  remove: () => void;
  isAdding: boolean;
  isRemoving: boolean;
}

export interface MovieState extends BaseState {
  type: 'movie';
  phase: MoviePhase;
  /** Formatted resolution string, e.g. `1080p`. */
  quality?: string;
  /** 0-100, continue-watching bar. */
  watchProgress?: number;
  /** 0-100, download progress bar. */
  downloadProgress?: number;
  /** Bytes/s, 0 while paused. */
  downloadSpeed?: number;
  canPlay: boolean;
  canPause: boolean;
  canResume: boolean;
  canRetry: boolean;
  /** Pause the active download. No-op when `canPause` is false. */
  pause: () => void;
  /** Resume a paused download. No-op when `canResume` is false. */
  resume: () => void;
}

export interface ShowState extends BaseState {
  type: 'show';
  phase: ShowPhase;
  /** The episode a show-level Play will target. Populated whenever
   *  the backend has a concrete "next up" (unwatched + aired). Null
   *  for fully-watched shows or shows with no aired episodes yet —
   *  Play still works (falls through to a replay) but the card won't
   *  advertise a specific episode. */
  nextEpisode?: { season: number; episode: number; available: boolean };
  /** The single most-relevant episode currently downloading, if any.
   *  Drives the card's sweep overlay + progress pill. */
  activeEpisode?: { season: number; episode: number };
  /** 0-100, progress for the active downloading episode. Mirrors
   *  `MovieState.downloadProgress`. */
  downloadProgress?: number;
  /** Bytes/s for the active downloading episode. */
  downloadSpeed?: number;
  /** Total active-state downloads for the show (leader + siblings).
   *  Drives the "×N" chip on the card when multiple episodes are
   *  moving at once — users see at a glance that pause-all will
   *  affect more than the visible leader. */
  activeDownloadCount?: number;
  /** Show-level canPause: true when any episode download is
   *  currently downloading or stalled. Clicking the paired `pause`
   *  hits the show-level endpoint which pauses every active
   *  torrent for this show in one call. */
  canPause: boolean;
  /** Mirror — true when any episode download is paused. */
  canResume: boolean;
  /** Pause every active download for this show. No-op when
   *  `canPause` is false. */
  pause: () => void;
  /** Resume every paused download for this show. No-op when
   *  `canResume` is false. */
  resume: () => void;
}

export type ContentState = MovieState | ShowState;

// ── Hook — overloaded so narrow variant flows to the call site ─

export function useContentState(tmdbId: number, type: 'movie'): MovieState;
export function useContentState(tmdbId: number, type: 'show'): ShowState;
export function useContentState(tmdbId: number, type: 'movie' | 'show'): ContentState;
export function useContentState(tmdbId: number, type: 'movie' | 'show'): ContentState {
  // Rules of hooks: always call both. Unused branch returns undefined.
  const movieResult = useMovieByTmdbId(tmdbId);
  const showResult = useShowByTmdbId(tmdbId);
  const cache = useLibraryCacheUpdater();
  const queryClient = useQueryClient();

  const movie = type === 'movie' ? movieResult : undefined;
  const show = type === 'show' ? showResult : undefined;
  const inLibrary = type === 'movie' ? movie != null : show != null;
  const rawLibraryId = type === 'movie' ? movie?.id : show?.id;
  const libraryId = rawLibraryId && rawLibraryId > 0 ? rawLibraryId : undefined;

  // Active download (movie-scoped — show-level downloads aren't a
  // concept; per-episode state lives elsewhere).
  const activeDownload = useDownloadForMovie(type === 'movie' ? libraryId : undefined);
  // Generated types spell optional as `T | null | undefined`; our
  // downstream props accept `T | undefined` only, so coalesce here.
  const posterPath = movie?.poster_path ?? show?.poster_path ?? undefined;

  // ── Mutations ──
  // Return a discriminated union from `mutationFn` so `onSuccess`
  // can narrow on `kind` and pass the right shape to the right
  // cache updater without any `as` casts.
  type AddResult =
    | { kind: 'movie'; data: LibraryMovie | undefined }
    | { kind: 'show'; data: LibraryShow };
  const addMutation = useMutationWithToast({
    verb: type === 'movie' ? 'add movie' : 'follow show',
    mutationFn: async (options?: AddContentOptions): Promise<AddResult> => {
      if (type === 'movie') {
        const { data } = await createMovie({ body: { tmdb_id: tmdbId } });
        return { kind: 'movie', data };
      }
      const body: CreateShow = {
        tmdb_id: tmdbId,
        monitor_new_items: options?.monitorNewItems,
        seasons_to_monitor: options?.seasonsToMonitor,
        monitor_specials: options?.monitorSpecials,
      };
      const { data } = await createShow({ body });
      // `createShow` returns `Show` (no rollup counts). `LibraryShow`
      // is `ShowListItem = Show & {rollups...}`; a freshly-followed
      // show has no episodes on the library yet, so the rollups are
      // genuinely zero. Narrow here (not at the cache boundary) so
      // the cache updater keeps its strict `ShowListItem` signature.
      if (!data) throw new Error('createShow returned no data');
      const show: LibraryShow = {
        ...data,
        aired_episode_count: 0,
        available_episode_count: 0,
        episode_count: 0,
        upcoming_episode_count: 0,
        watched_episode_count: 0,
        wanted_episode_count: 0,
      };
      return { kind: 'show', data: show };
    },
    onMutate: () => {
      if (type === 'movie') cache.addMovie(tmdbId, {});
      else cache.addShow(tmdbId, {});
    },
    onSuccess: (result, options) => {
      if (result.kind === 'movie') {
        if (result.data) cache.replaceMovie(tmdbId, result.data);
        // No toast: the Add button flips to Added instantly, that's
        // feedback enough for a single-movie mutation.
      } else {
        const show = result.data;
        cache.replaceShow(tmdbId, show);
        // One summary toast on follow — confirms the trigger and
        // tells the user what's being monitored, so they don't have
        // to guess whether the scheduler is about to grab 1 or 80
        // episodes in the background. Downstream per-episode events
        // (grab/download/complete) are silent by ruleset.
        const seasons = options?.seasonsToMonitor?.length;
        const scope =
          seasons === 0
            ? 'future episodes only'
            : seasons != null && seasons > 0
              ? `${seasons} season${seasons === 1 ? '' : 's'} monitored`
              : 'all seasons monitored';
        kinoToast.success(`Following ${show.title || 'show'}`, {
          id: `follow-${show.id}`,
          description: scope,
        });
      }
    },
    onError: () => {
      if (type === 'movie') cache.removeMovie(tmdbId);
      else cache.removeShow(tmdbId);
    },
  });

  const removeMutation = useMutationWithToast({
    verb: type === 'movie' ? 'remove movie' : 'remove show',
    mutationFn: async () => {
      if (!libraryId) return;
      if (type === 'movie') await deleteMovie({ path: { id: libraryId } });
      else await deleteShow({ path: { id: libraryId } });
    },
    onMutate: () => {
      if (type === 'movie') cache.removeMovie(tmdbId);
      else cache.removeShow(tmdbId);
      // Invalidate the show-scoped queries immediately. Without this
      // the ShowDetail page's Up Next / SeasonPicker keep rendering
      // with stale data until the WS `content_removed` event round-
      // trips — which feels like a "lag" when the library card is
      // already gone.
      if (type === 'show') {
        queryClient.invalidateQueries({
          predicate: (q) =>
            queryMatchesId(q, 'showWatchState') || queryMatchesId(q, 'show-episodes'),
        });
      }
    },
    onSuccess: () => {
      // Optimistic removeShow/removeMovie handled the cache write,
      // but the WS `content_removed` event races the delete API
      // response and can re-invalidate LIBRARY_SHOWS_KEY while the
      // backend is still processing — the subsequent refetch then
      // briefly re-includes the row. Force-refetch explicitly on
      // mutation success so the final state is reconciled from the
      // server AFTER the delete has committed. Same pattern the
      // watch-now flow uses for the mirror case (auto-follow).
      queryClient.refetchQueries({
        queryKey: type === 'movie' ? [...LIBRARY_MOVIES_KEY] : [...LIBRARY_SHOWS_KEY],
      });
      queryClient.refetchQueries({ queryKey: [...DOWNLOADS_KEY] });
    },
    onError: () => cache.refetchAll(),
  });

  const pauseMutation = useMutationWithToast({
    verb: 'pause download',
    mutationFn: async () => {
      if (activeDownload?.id) await pauseDownload({ path: { id: activeDownload.id } });
    },
    onMutate: () => {
      if (activeDownload?.id) {
        cache.patchDownload(activeDownload.id, {
          state: 'paused',
          download_speed: 0,
          upload_speed: 0,
        });
      }
    },
    onSuccess: () => cache.refetchAll(),
  });

  const resumeMutation = useMutationWithToast({
    verb: 'resume download',
    mutationFn: async () => {
      if (activeDownload?.id) await resumeDownload({ path: { id: activeDownload.id } });
    },
    onMutate: () => {
      if (activeDownload?.id) {
        cache.patchDownload(activeDownload.id, { state: 'downloading' });
      }
    },
    onSuccess: () => cache.refetchAll(),
  });

  // Show-level pause/resume — targets the backend's `pause-downloads`
  // endpoint which iterates every active torrent linked to this
  // show's episodes. Matches user mental model ("pause the show")
  // rather than pausing just the single `active_download` the
  // projection surfaces. No-op for movies.
  const pauseShowMutation = useMutationWithToast({
    verb: 'pause show downloads',
    mutationFn: async () => {
      if (type === 'show' && show?.id) await pauseShowDownloads({ path: { id: show.id } });
    },
    onSuccess: () => cache.refetchAll(),
  });
  const resumeShowMutation = useMutationWithToast({
    verb: 'resume show downloads',
    mutationFn: async () => {
      if (type === 'show' && show?.id) await resumeShowDownloads({ path: { id: show.id } });
    },
    onSuccess: () => cache.refetchAll(),
  });

  const base: BaseState = {
    tmdbId,
    libraryId,
    posterPath,
    canAdd: !inLibrary,
    canRemove: inLibrary,
    needsConfirmToRemove: false, // overwritten per-variant below
    add: (options) => addMutation.mutate(options),
    remove: () => removeMutation.mutate(),
    isAdding: addMutation.isPending,
    isRemoving: removeMutation.isPending,
  };

  if (type === 'show') {
    // Show card phase is driven by the active download (if any) — so
    // a show whose S02E05 is downloading renders the same
    // sweep+progress pill a movie gets mid-grab. No active download →
    // plain binary `available` / `none`.
    //
    // Can't reuse `computeMoviePhase` here — its fallback for
    // "in-library but no download state" is `'searching'`, which
    // would light up the dots pill on every caught-up show. Narrow
    // path, in priority order:
    //   • not in library → 'none'
    //   • active download → map its state (same mapping as movies)
    //   • any aired-but-not-yet-downloaded episode (wanted_episode_count > 0)
    //     → 'searching' (covers the ~seconds between "clicked Get"
    //     and "download row appears" where active_download is still
    //     null but the scheduler is actively searching)
    //   • otherwise → 'available'
    const active = show?.active_download;
    const showPhase: ShowPhase = !inLibrary
      ? 'none'
      : active?.state
        ? computeMoviePhase(true, undefined, active.state)
        : (show?.wanted_episode_count ?? 0) > 0
          ? 'searching'
          : 'available';
    const nextEp = show?.next_episode
      ? {
          season: show.next_episode.season_number,
          episode: show.next_episode.episode_number,
          available: show.next_episode.available,
        }
      : undefined;
    const activeEp = active
      ? { season: active.season_number, episode: active.episode_number }
      : undefined;
    const dlProgress =
      active?.total_size && active.total_size > 0
        ? Math.round((active.downloaded / active.total_size) * 100)
        : undefined;
    // Pause/resume eligibility mirrors the projection's state: if
    // the earliest active torrent is moving, pause is live; if it's
    // paused, resume is live. Using the projection (instead of
    // checking every downloaded across all of the show's episodes)
    // keeps the hook O(1) and matches the phase the card's already
    // displaying — users see "downloading → pause" consistently.
    return {
      ...base,
      type: 'show',
      phase: showPhase,
      nextEpisode: nextEp,
      activeEpisode: activeEp,
      downloadProgress: dlProgress,
      downloadSpeed: showPhase === 'paused' ? 0 : active?.download_speed,
      activeDownloadCount: active?.active_count ?? undefined,
      canPause: showPhase === 'downloading' || showPhase === 'stalled',
      canResume: showPhase === 'paused',
      pause: () => pauseShowMutation.mutate(),
      resume: () => resumeShowMutation.mutate(),
      needsConfirmToRemove: inLibrary,
    };
  }

  // Movie branch
  const phase = computeMoviePhase(inLibrary, movie?.status, activeDownload?.state);
  // Resolution lives on the `media` row (the imported file), not on
  // `movie` — the old hand-rolled DTO optimistically read `movie.resolution`
  // which the backend never populates. Leaving quality blank here
  // until we surface a proper rollup from the Movie list endpoint.
  const quality: string | undefined = undefined;
  let watchProgress: number | undefined;
  // Movie.runtime is in minutes; convert to Jellyfin ticks (100ns) to
  // match the stored playback_position_ticks.
  const runtimeTicks = movie?.runtime ? movie.runtime * 60 * 10_000_000 : 0;
  if (movie?.playback_position_ticks && runtimeTicks > 0) {
    watchProgress = Math.round((movie.playback_position_ticks / runtimeTicks) * 100);
  }

  return {
    ...base,
    type: 'movie',
    phase,
    quality,
    watchProgress,
    downloadProgress: activeDownload?.size
      ? Math.round((activeDownload.downloaded / activeDownload.size) * 100)
      : undefined,
    downloadSpeed: phase === 'paused' ? 0 : activeDownload?.download_speed,
    canPlay: phase === 'available' || phase === 'watched',
    canPause: phase === 'downloading' || phase === 'stalled',
    canResume: phase === 'paused',
    canRetry: phase === 'failed',
    needsConfirmToRemove: phase === 'downloading' || phase === 'stalled' || phase === 'available',
    pause: () => pauseMutation.mutate(),
    resume: () => resumeMutation.mutate(),
  };
}

function computeMoviePhase(
  inLibrary: boolean,
  status: ContentStatus | string | undefined,
  downloadState: DownloadState | string | undefined
): MoviePhase {
  if (!inLibrary) return 'none';
  if (downloadState) {
    switch (downloadState) {
      case 'queued':
      case 'grabbing':
        return 'queued';
      case 'downloading':
        return 'downloading';
      case 'stalled':
        return 'stalled';
      case 'paused':
        return 'paused';
      case 'failed':
        return 'failed';
      case 'importing':
        return 'importing';
    }
  }
  switch (status) {
    case 'wanted':
      return 'searching';
    case 'downloading':
      return 'downloading';
    case 'available':
      return 'available';
    case 'watched':
      return 'watched';
    default:
      return 'searching';
  }
}
