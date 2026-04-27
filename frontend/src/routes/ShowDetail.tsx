import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useParams, useSearch } from '@tanstack/react-router';
import { Calendar, Check, ChevronDown, ExternalLink, Star, Tv } from 'lucide-react';
import type { ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import {
  showDetailsOptions,
  showWatchStateOptions,
} from '@/api/generated/@tanstack/react-query.gen';
import {
  acquireEpisode,
  acquireEpisodeByTmdb,
  discardEpisode,
  monitoredSeasons as fetchMonitoredSeasons,
  markEpisodeWatched,
  pauseDownload,
  redownloadEpisode,
  resumeDownload,
  unmarkEpisodeWatched,
  updateShowMonitor,
} from '@/api/generated/sdk.gen';
import type { MonitorNewItems, SeasonAcquireState } from '@/api/generated/types.gen';
import { ConfirmDialog } from '@/components/ConfirmDialog';
import { DetailLayout } from '@/components/DetailLayout';
import { EpisodeCard, EpisodeCardSkeleton } from '@/components/EpisodeCard';
import { FollowShowDialog } from '@/components/FollowShowDialog';
import { PosterCard } from '@/components/PosterCard';
import { RateWidget } from '@/components/RateWidget';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { useSeasonEpisodes } from '@/hooks/useSeasonEpisodes';
import { useWatchNow } from '@/hooks/useWatchNow';
import { tmdbImage } from '@/lib/api';
import { cn } from '@/lib/utils';
import type { InvalidationRule } from '@/state/invalidation';
import { useShowByTmdbId } from '@/state/library-cache';
import { queryMatchesId } from '@/state/query-utils';
import { type ShowState, useContentState } from '@/state/use-content-state';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

/** Build the PosterCard sublabel for the show-detail poster. Mirrors
 *  the home show card's priority order so the two surfaces read the
 *  same: Next Up if mid-series, "All caught up" for followed-and-
 *  fully-watched, "Start watching" for cold + aired, nothing
 *  otherwise. Keeps the string that drives Play honest about what
 *  the poster's Play button will actually do. */
function detailSublabel(
  state: ShowState,
  nextUp: { season: number; episode: number; title?: string | null } | null,
  airedCount: number
): string | undefined {
  if (nextUp) {
    const sxe = `S${String(nextUp.season).padStart(2, '0')}E${String(nextUp.episode).padStart(2, '0')}`;
    return nextUp.title ? `${sxe} · ${nextUp.title}` : `${sxe} · Next up`;
  }
  if (state.phase === 'none') return 'S01E01 · Start watching';
  // Followed + nothing queued: either caught up (aired > 0) or the
  // show has no aired episodes yet (upcoming / future premiere).
  if (airedCount > 0) return 'All caught up';
  return undefined;
}

/** Next Up / per-season counts + aired rollups — affected by the
 *  full library lifecycle on this show, plus download-state and
 *  watched transitions that shift "what to play next." */
const SHOW_WATCH_STATE_INVALIDATED_BY: InvalidationRule[] = [
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
  // Live position updates while the user is watching — keeps the
  // top poster's resume bar current without a manual refresh.
  'playback_progress',
  'unwatched',
  'show_monitor_changed',
  'new_episode',
  'trakt_synced',
];

interface TmdbSeason {
  season_number: number;
  name: string | null;
  episode_count: number | null;
  poster_path: string | null;
  air_date: string | null;
}

export function ShowDetail() {
  const { tmdbId } = useParams({ from: '/show/$tmdbId' });
  const id = Number(tmdbId);
  const navigate = useNavigate();
  // URL-backed season state so deep-linking, back button, and
  // bookmarks reflect which season the user was viewing.
  const { s, follow } = useSearch({ strict: false }) as {
    s?: string | number;
    follow?: '1';
  };
  const activeSeason = (() => {
    const n = typeof s === 'string' ? Number.parseInt(s, 10) : s;
    return typeof n === 'number' && Number.isFinite(n) ? n : null;
  })();
  const seasonsAnchorRef = useRef<HTMLDivElement>(null);
  const setActiveSeason = (sn: number) => {
    navigate({
      to: '/show/$tmdbId',
      params: { tmdbId: String(id) },
      search: { s: String(sn) } as never,
      replace: true,
      // Keep the sticky season picker visible — default navigation
      // resets scroll to the top of the document, which is jarring
      // on a long show detail page.
      resetScroll: false,
    });
    // A shorter season shrinks the page, and the browser clamps the
    // scroll position to the new max-height — visually that's a
    // "jump up". Scrolling the seasons anchor into view is a
    // deliberate re-anchor that's consistent regardless of season
    // length. `scroll-mt` accounts for the page's sticky header +
    // sticky picker so the anchor lands just under them.
    requestAnimationFrame(() => {
      seasonsAnchorRef.current?.scrollIntoView({
        behavior: 'smooth',
        block: 'start',
      });
    });
  };

  const {
    data,
    isLoading,
    isError: showIsError,
    error: showError,
  } = useQuery(showDetailsOptions({ path: { tmdb_id: id } }));
  const { data: watchStateData } = useQuery({
    ...showWatchStateOptions({ path: { tmdb_id: id } }),
    meta: { invalidatedBy: SHOW_WATCH_STATE_INVALIDATED_BY },
  });
  const state = useContentState(id, 'show');
  const libraryShow = useShowByTmdbId(id);
  const { watchNow } = useWatchNow();

  // PATCH /shows/{id}/monitor for the re-open / Manage downloads
  // path. create_show would 409 on an existing show and wouldn't
  // re-apply per-season monitoring; this endpoint is the idempotent
  // surface for updating preferences.
  const monitorMutation = useMutationWithToast({
    verb: 'update monitoring',
    mutationFn: async (args: {
      id: number;
      body: {
        monitor_new_items?: MonitorNewItems;
        seasons_to_monitor?: number[];
        monitor_specials?: boolean;
      };
    }) => {
      await updateShowMonitor({ path: { id: args.id }, body: args.body });
    },
    // Backend emits `ShowMonitorChanged` → dispatcher refreshes
    // library/shows + showWatchState + show-episodes + monitored-
    // seasons via meta. No onSuccess needed.
  });
  const [showSeasonDialog, setShowSeasonDialog] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);

  // Auto-open from `?follow=1` (Home cards' + button redirects here
  // so the user sees full show context before committing to any
  // monitoring scope). Strip the param after opening via `replace` so
  // a refresh or back/forward doesn't re-trigger; `s` is preserved.
  // Ref-guard makes the body one-shot regardless of dep churn.
  const followOnMountRef = useRef(follow === '1');
  useEffect(() => {
    if (!followOnMountRef.current) return;
    followOnMountRef.current = false;
    setShowSeasonDialog(true);
    navigate({
      to: '/show/$tmdbId',
      params: { tmdbId: String(id) },
      search: (s != null ? { s: String(s) } : {}) as never,
      replace: true,
    });
  }, [id, navigate, s]);

  // Fetch the seasons currently being acquired so the Manage dialog
  // can seed its "Specific seasons" checkboxes with real state. Only
  // issued once the dialog opens on an already-followed show — no
  // point paying the cost on every Show page view.
  const monitoredSeasonsQuery = useQuery<SeasonAcquireState[]>({
    queryKey: ['kino', 'shows', libraryShow?.id ?? null, 'monitored-seasons'],
    queryFn: async () => {
      if (libraryShow?.id == null) return [];
      const r = await fetchMonitoredSeasons({ path: { id: libraryShow.id } });
      return r.data ?? [];
    },
    enabled: showSeasonDialog && libraryShow?.id != null,
    meta: {
      invalidatedBy: [
        'show_monitor_changed',
        'search_started',
        'release_grabbed',
        'imported',
        'content_removed',
      ],
    },
  });

  const show = data as Record<string, unknown> | undefined;

  useDocumentTitle(typeof show?.name === 'string' ? show.name : null);

  if (isLoading) {
    return (
      <div className="min-h-screen">
        <div className="h-[55vh] skeleton" />
        <div className="px-4 md:px-12 -mt-32 relative z-10 space-y-4 max-w-4xl">
          <div className="h-10 w-80 skeleton rounded" />
          <div className="h-5 w-48 skeleton rounded" />
          <div className="h-12 w-48 skeleton rounded-lg" />
        </div>
      </div>
    );
  }

  if (!show) {
    // Distinguish true 404s from fetch failures. On backend outage
    // the old message lied ("not found" when the show very much
    // exists).
    const msg = showIsError
      ? showError instanceof Error
        ? showError.message
        : 'Request failed'
      : null;
    return (
      <div className="flex items-center justify-center min-h-[50vh] text-[var(--text-muted)] text-center px-6">
        {msg ? (
          <div>
            <p className="mb-1 text-white">Couldn't load show</p>
            <p className="text-xs">{msg}</p>
          </div>
        ) : (
          'Show not found'
        )}
      </div>
    );
  }

  const title = String(show.name ?? '');
  const firstAir = show.first_air_date as string | undefined;
  const year = firstAir?.slice(0, 4);
  const showStatus = show.status as string | undefined;
  const numSeasons = show.number_of_seasons as number | undefined;
  const numEpisodes = show.number_of_episodes as number | undefined;
  const rating = show.vote_average as number | undefined;
  const genres = show.genres as Array<{ id: number; name: string }> | undefined;
  const networks = show.networks as Array<{ name: string }> | undefined;
  const networkName = networks?.[0]?.name;
  const videos = show.videos as
    | { results?: Array<{ key: string; site: string; type: string }> }
    | undefined;
  const trailer = videos?.results?.find((v) => v.site === 'YouTube' && v.type === 'Trailer');
  // All seasons, including Season 0 ("Specials") — users need to be
  // able to browse and play specials even though they're excluded
  // from smart-Play auto-targeting. The dialog's "specific seasons"
  // list inherits this; bulk-downloading specials is the user's call.
  const seasons = (show.seasons as TmdbSeason[]) ?? [];
  const bulkDownloadSeasons = seasons.filter((s) => s.season_number > 0);

  // Default active season: Next Up's season if mid-series, else the
  // first aired regular season (skip S0 so the initial view matches
  // user expectation even for shows whose only aired content is
  // specials — Season 0 stays reachable via the picker). Kept in a
  // separate pass so the stateful view doesn't re-derive from
  // `watchStateData` every render.
  const defaultSeason =
    watchStateData?.next_up?.season ??
    bulkDownloadSeasons.find((s) => !s.air_date || new Date(s.air_date) <= new Date())
      ?.season_number ??
    bulkDownloadSeasons[0]?.season_number ??
    seasons[0]?.season_number ??
    1;
  const resolvedSeason = activeSeason ?? defaultSeason;

  const statusBadge = showStatus ? (
    <span
      className={cn(
        'px-2 py-0.5 rounded text-xs font-medium',
        showStatus === 'Returning Series'
          ? 'bg-green-500/10 text-green-400 ring-1 ring-green-500/20'
          : 'bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10'
      )}
    >
      {showStatus}
    </span>
  ) : null;

  const meta: Array<{ icon?: ReactNode; label: string; href?: string }> = [
    ...(year
      ? [{ icon: <Calendar size={13} className="text-[var(--text-muted)]" />, label: year }]
      : []),
    ...(rating
      ? [
          {
            icon: <Star size={13} className="text-yellow-500 fill-yellow-500" />,
            label: rating.toFixed(1),
          },
        ]
      : []),
    ...(numSeasons
      ? [
          {
            icon: <Tv size={13} className="text-[var(--text-muted)]" />,
            label: `${numSeasons} season${numSeasons !== 1 ? 's' : ''}`,
          },
        ]
      : []),
    ...(numEpisodes ? [{ label: `${numEpisodes} eps` }] : []),
    ...(networkName ? [{ label: networkName }] : []),
    ...(genres?.map((g) => ({ label: g.name })) ?? []),
    ...(trailer
      ? [
          {
            icon: <ExternalLink size={13} />,
            label: 'Trailer',
            href: `https://www.youtube.com/watch?v=${trailer.key}`,
          },
        ]
      : []),
  ];

  return (
    <>
      <DetailLayout
        title={title}
        tagline={show.tagline as string | undefined}
        overview={show.overview as string | undefined}
        backdropUrl={tmdbImage(show.backdrop_path as string | undefined, 'w1280')}
        meta={meta}
        badges={statusBadge}
        state={state}
        onAddOverride={() => setShowSeasonDialog(true)}
        addLabel="Follow Show"
        onManageDownloads={() => setShowSeasonDialog(true)}
        // Full PosterCard instead of the custom overlay — matches
        // movie detail + the home show card pixel-for-pixel. Carries
        // Play (plays next-up), +/× (follow dialog / remove), pause/
        // resume (show-level endpoint), download sweep + progress
        // badge. Sublabel is the next-up episode's context — same
        // string home cards use, just rendered a little larger here.
        poster={
          <PosterCard
            title={title}
            sublabel={detailSublabel(
              state,
              watchStateData?.next_up ?? null,
              watchStateData?.aired_count ?? 0
            )}
            posterUrl={tmdbImage(show.poster_path as string | undefined, 'w500')}
            phase={state.phase}
            progressBadge={
              watchStateData && watchStateData.aired_count > 0
                ? `${watchStateData.available_count}/${watchStateData.aired_count}`
                : undefined
            }
            // Resume bar on the poster when the next-up episode is
            // mid-play. Mirrors the Home continue-watching card so
            // the two surfaces agree on "where you are in this show."
            watchProgress={
              watchStateData?.next_up?.progress_percent != null
                ? Math.round(watchStateData.next_up.progress_percent * 100)
                : undefined
            }
            downloadProgress={state.downloadProgress}
            downloadSpeed={state.downloadSpeed}
            activeDownloadCount={state.activeDownloadCount}
            onPlay={() => {
              const next = watchStateData?.next_up;
              if (next) {
                watchNow({
                  kind: 'episode',
                  episodeId: next.episode_id,
                  title: `${title} · S${String(next.season).padStart(2, '0')}E${String(next.episode).padStart(2, '0')}${next.title ? ` · ${next.title}` : ''}`,
                });
                return;
              }
              // Nothing to play yet — cold / caught-up / no aired
              // episodes. Smart-play auto-follows if needed and
              // picks the pilot.
              watchNow({ kind: 'show_smart_play', showTmdbId: id, title });
            }}
            onAdd={state.canAdd ? () => setShowSeasonDialog(true) : undefined}
            onRemove={state.canRemove ? () => setConfirmRemove(true) : undefined}
            onPause={state.canPause ? state.pause : undefined}
            onResume={state.canResume ? state.resume : undefined}
            isAdding={state.isAdding}
          />
        }
        belowActions={
          libraryShow ? (
            <RateWidget kind="show" id={libraryShow.id} value={libraryShow.user_rating} />
          ) : undefined
        }
      >
        {/* Seasons section */}
        {seasons.length > 0 && (
          <div ref={seasonsAnchorRef} className="mt-8 max-w-6xl scroll-mt-16">
            <SeasonPicker
              seasons={seasons}
              // Only feed real stats when the show is still in the
              // library — on remove we want chips to revert to the
              // cold "N eps" state immediately rather than show a
              // stale "4/10 downloaded" badge that disagrees with
              // reality (we just deleted the files).
              stats={!state.canAdd ? (watchStateData?.season_stats ?? []) : []}
              activeSeason={resolvedSeason}
              onChange={setActiveSeason}
            />

            {/* Season content */}
            {(() => {
              const season = seasons.find((s) => s.season_number === resolvedSeason);
              return season ? (
                <SeasonContent
                  key={resolvedSeason}
                  showTmdbId={id}
                  showTitle={title}
                  season={season}
                  totalSeasons={seasons.length}
                />
              ) : null;
            })()}
          </div>
        )}
      </DetailLayout>

      {/* Conditionally mount so `useState` initializers in the
          dialog re-run on every open with the current props. The
          dialog used to be always-rendered (just with open=false),
          which meant the "first follow vs. manage existing"
          defaults (isAlreadyFollowed, currentMonitorNewItems)
          were captured on page-mount and never refreshed — open
          the dialog AFTER an adhoc Get and you'd see first-follow
          defaults even though the show was now in the library. */}
      {showSeasonDialog && (
        <FollowShowDialog
          open={showSeasonDialog}
          showTitle={title}
          seasons={bulkDownloadSeasons}
          isAlreadyFollowed={!state.canAdd}
          currentMonitorNewItems={
            libraryShow?.monitor_new_items === 'future' || libraryShow?.monitor_new_items === 'none'
              ? libraryShow.monitor_new_items
              : undefined
          }
          seasonStates={monitoredSeasonsQuery.data}
          hasSpecials={seasons.some((s) => s.season_number === 0 && (s.episode_count ?? 0) > 0)}
          currentMonitorSpecials={libraryShow?.monitor_specials}
          isLoading={state.isAdding || monitorMutation.isPending}
          onConfirm={(options) => {
            // First-time Follow hits create_show via state.add; re-open
            // (Manage downloads) hits the PATCH monitor endpoint
            // because create_show 409s on an existing show and
            // wouldn't re-apply per-season flags anyway.
            if (state.canAdd) {
              state.add(options);
            } else if (state.libraryId != null) {
              monitorMutation.mutate({
                id: state.libraryId,
                body: {
                  monitor_new_items: options.monitorNewItems,
                  seasons_to_monitor: options.seasonsToMonitor,
                  monitor_specials: options.monitorSpecials,
                },
              });
            }
            setShowSeasonDialog(false);
          }}
          onCancel={() => setShowSeasonDialog(false)}
        />
      )}
      <ConfirmDialog
        open={confirmRemove}
        title="Remove from library?"
        description={`This will remove "${title}" and cancel any active downloads.`}
        confirmLabel="Remove"
        onConfirm={() => {
          state.remove();
          setConfirmRemove(false);
        }}
        onCancel={() => setConfirmRemove(false)}
      />
    </>
  );
}

/** Season content with poster + episode list */
function SeasonContent({
  showTmdbId,
  showTitle,
  season,
  totalSeasons,
}: {
  showTmdbId: number;
  showTitle: string;
  season: TmdbSeason;
  totalSeasons: number;
}) {
  const { data: episodes, isLoading } = useSeasonEpisodes(
    showTmdbId,
    season.season_number,
    totalSeasons
  );
  const { watchNow } = useWatchNow();
  const queryClient = useQueryClient();

  // Mark-watched toggle + re-download — targeted mutations that
  // invalidate the season list + watch-state so Next Up re-rolls
  // after the action. WS events would eventually refetch too, but
  // invalidating now makes the UI respond instantly. Also refetch
  // LIBRARY_SHOWS_KEY so the show card on Home reflects the new
  // `wanted_episode_count` / `active_download` rollup — otherwise
  // navigating back from detail lands on a stale library cache
  // and the card doesn't show "searching" or "downloading" until
  // the next WS event (which can lag a few seconds).
  const invalidate = () => {
    queryClient.invalidateQueries({
      queryKey: ['show-episodes', showTmdbId, season.season_number],
    });
    queryClient.invalidateQueries({
      predicate: (q) => queryMatchesId(q, 'showWatchState'),
    });
    queryClient.refetchQueries({ queryKey: ['kino', 'library', 'shows'] });
  };
  const watchedMutation = useMutationWithToast({
    verb: 'update watched status',
    mutationFn: async (args: { episodeId: number; watched: boolean }) => {
      if (args.watched) {
        await markEpisodeWatched({ path: { id: args.episodeId } });
      } else {
        await unmarkEpisodeWatched({ path: { id: args.episodeId } });
      }
    },
    onSuccess: invalidate,
  });
  const redownloadMutation = useMutationWithToast({
    verb: 'redownload episode',
    mutationFn: async (episodeId: number) => {
      await redownloadEpisode({ path: { id: episodeId } });
    },
    onSuccess: invalidate,
  });
  // "Get" — flip acquire=1 for a previously-ignored episode. Separate
  // from redownload because redownload carries destructive cleanup
  // (unlink media, prune orphans) that's nonsensical when the episode
  // has never been imported.
  const acquireMutation = useMutationWithToast({
    verb: 'get episode',
    mutationFn: async (episodeId: number) => {
      await acquireEpisode({ path: { id: episodeId } });
    },
    onSuccess: invalidate,
  });
  // Cold-start variant: show isn't in the library yet (no episode_id),
  // but user still wants to grab this specific episode. Backend
  // auto-follows the show with "future only" monitoring, creates the
  // episode row, and fires a search. Same UX as movie card's "+" on
  // a non-library movie but for episodes.
  const acquireByTmdbMutation = useMutationWithToast({
    verb: 'get episode',
    mutationFn: async (args: {
      showTmdbId: number;
      seasonNumber: number;
      episodeNumber: number;
    }) => {
      await acquireEpisodeByTmdb({
        body: {
          show_tmdb_id: args.showTmdbId,
          season_number: args.seasonNumber,
          episode_number: args.episodeNumber,
        },
      });
    },
    onSuccess: async () => {
      // `refetchQueries` (not just invalidate) so the library cache
      // is populated BEFORE the user can click Manage. Without this,
      // opening Manage in the ~300ms window between click and
      // refetch lands the FollowShowDialog in first-follow mode
      // (isAlreadyFollowed=false) and shows the wrong defaults.
      // show-episodes likewise refetched synchronously so the new
      // episode row lands immediately.
      await Promise.all([
        queryClient.refetchQueries({ queryKey: ['kino', 'library', 'shows'] }),
        queryClient.refetchQueries({
          predicate: (q) =>
            queryMatchesId(q, 'show-episodes') || queryMatchesId(q, 'showWatchState'),
        }),
      ]);
    },
  });
  // "Remove" — symmetric inverse of Get. Cancels in-flight downloads,
  // unlinks any imported media (file left on disk), sets acquire=0.
  // Invalidates downloads so the card's download-state-driven phase
  // snaps across tabs the same way cancel does from the Downloads
  // page.
  const discardMutation = useMutationWithToast({
    verb: 'remove episode',
    mutationFn: async (episodeId: number) => {
      await discardEpisode({ path: { id: episodeId } });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['kino', 'downloads'] });
      invalidate();
    },
  });
  // Pause / Resume pass through to the download by download_id —
  // same endpoints the DownloadingTab uses. Invalidates downloads so
  // the card's phase flips within one WS hop, matching the pattern
  // established for movie cards.
  const pauseMutation = useMutationWithToast({
    verb: 'pause download',
    mutationFn: async (downloadId: number) => {
      await pauseDownload({ path: { id: downloadId } });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['kino', 'downloads'] });
      invalidate();
    },
  });
  const resumeMutation = useMutationWithToast({
    verb: 'resume download',
    mutationFn: async (downloadId: number) => {
      await resumeDownload({ path: { id: downloadId } });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['kino', 'downloads'] });
      invalidate();
    },
  });

  // Progressive rendering — show 20 initially, load more on scroll
  const BATCH_SIZE = 20;
  const [visibleCount, setVisibleCount] = useState(BATCH_SIZE);
  const loadMoreRef = useRef<HTMLDivElement>(null);

  const totalEpisodes = episodes?.length ?? 0;
  const hasMore = visibleCount < totalEpisodes;
  const visibleEpisodes = episodes?.slice(0, visibleCount);

  // Intersection observer for loading more
  useEffect(() => {
    if (!hasMore) return;
    const el = loadMoreRef.current;
    if (!el) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting) {
          setVisibleCount((prev) => Math.min(prev + BATCH_SIZE, totalEpisodes));
        }
      },
      { threshold: 0.1 }
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasMore, totalEpisodes]);

  return (
    <div className="flex flex-col md:flex-row gap-6 mt-4">
      {/* Season poster — sticky on desktop */}
      {season.poster_path && (
        <div className="flex-shrink-0 hidden md:block">
          <div className="sticky top-32 w-36">
            <img
              src={tmdbImage(season.poster_path) ?? ''}
              alt={season.name ?? `Season ${season.season_number}`}
              className="w-full rounded-lg ring-1 ring-white/10"
            />
            <p className="mt-2 text-xs text-[var(--text-muted)]">
              {season.episode_count} episode{season.episode_count !== 1 ? 's' : ''}
              {season.air_date && ` · ${season.air_date.slice(0, 4)}`}
            </p>
          </div>
        </div>
      )}

      {/* Episode list */}
      <div className="flex-1 min-w-0">
        {isLoading ? (
          <div className="space-y-1">
            {Array.from({ length: Math.min(season.episode_count ?? 6, 8) }, (_, i) => (
              <EpisodeCardSkeleton key={String(i)} />
            ))}
          </div>
        ) : (
          <div className="space-y-1">
            {visibleEpisodes?.map((ep) => (
              <EpisodeCard
                key={ep.episode_number}
                episode={ep}
                onPlay={() =>
                  watchNow({
                    kind: 'episode_by_tmdb',
                    showTmdbId,
                    season: season.season_number,
                    episode: ep.episode_number,
                    title: `${showTitle} · S${String(season.season_number).padStart(2, '0')}E${String(ep.episode_number).padStart(2, '0')}${ep.name ? ` · ${ep.name}` : ''}`,
                  })
                }
                onToggleWatched={
                  ep.episode_id != null
                    ? (watched) =>
                        watchedMutation.mutate({ episodeId: ep.episode_id as number, watched })
                    : undefined
                }
                onRedownload={
                  ep.episode_id != null
                    ? () => redownloadMutation.mutate(ep.episode_id as number)
                    : undefined
                }
                onGet={() => {
                  if (ep.episode_id != null) {
                    acquireMutation.mutate(ep.episode_id);
                  } else {
                    // Cold-start: auto-follow show + create episode row
                    // + fire search. User doesn't need to navigate
                    // through the Follow dialog for a single episode.
                    acquireByTmdbMutation.mutate({
                      showTmdbId,
                      seasonNumber: season.season_number,
                      episodeNumber: ep.episode_number,
                    });
                  }
                }}
                onRemove={
                  ep.episode_id != null
                    ? () => discardMutation.mutate(ep.episode_id as number)
                    : undefined
                }
                onPause={
                  ep.download_id != null
                    ? () => pauseMutation.mutate(ep.download_id as number)
                    : undefined
                }
                onResume={
                  ep.download_id != null
                    ? () => resumeMutation.mutate(ep.download_id as number)
                    : undefined
                }
              />
            ))}

            {/* Load more trigger */}
            {hasMore && (
              <div ref={loadMoreRef} className="py-2">
                <EpisodeCardSkeleton />
              </div>
            )}

            {(!episodes || episodes.length === 0) && (
              <p className="text-sm text-[var(--text-muted)] py-8 text-center">
                No episode data available
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * SeasonPicker — inline-first strip of season chips with at-a-glance
 * per-season progress.
 *
 * Every season renders as its own chip in a horizontally-scrollable
 * strip. No dropdown: clicking a chip selects the season directly,
 * so the common case (pick a season on a short-to-medium show) is
 * one click not two. Long-running shows scroll horizontally; the
 * currently-active chip auto-scrolls into view so navigating via
 * keyboard or URL deep-link keeps it visible.
 *
 * Each chip carries two meta pieces:
 *   - "watched / aired" progress (mini bar across the bottom)
 *   - downloaded count + waiting/downloading indicator
 *
 * `stats` is optional — chips degrade gracefully to just the season
 * label + episode count when the show isn't followed and we have no
 * per-season rollup yet.
 */
function SeasonPicker({
  seasons,
  stats,
  activeSeason,
  onChange,
}: {
  seasons: TmdbSeason[];
  stats: SeasonStatLike[];
  activeSeason: number;
  onChange: (sn: number) => void;
}) {
  const byNum = new Map<number, SeasonStatLike>();
  for (const s of stats) byNum.set(s.season_number, s);

  // Auto-scroll the active chip into view when the selection
  // changes — otherwise picking a season via URL deep-link can
  // leave the chip off-screen on long-running shows.
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current?.querySelector<HTMLElement>(`[data-season="${activeSeason}"]`);
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'center' });
  }, [activeSeason]);

  const idx = seasons.findIndex((s) => s.season_number === activeSeason);
  const prev = idx > 0 ? seasons[idx - 1] : null;
  const next = idx >= 0 && idx < seasons.length - 1 ? seasons[idx + 1] : null;

  return (
    <div className="sticky top-14 z-20 bg-[var(--bg-primary)] -mx-4 px-4 md:-mx-12 md:px-12">
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          onClick={() => prev && onChange(prev.season_number)}
          disabled={!prev}
          aria-label="Previous season"
          className="flex-shrink-0 w-9 h-9 grid place-items-center rounded-lg hover:bg-white/10 disabled:opacity-30 disabled:hover:bg-transparent transition"
        >
          <ChevronDown size={16} className="rotate-90" />
        </button>

        {/* Scrollable chip strip. Vertical padding on the scroll
            container gives ring/shadow effects room to breathe —
            `overflow-x: auto` implicitly clips y, so the y-padding
            is what prevents the active chip's ring from being cut
            off at the top / bottom edges. */}
        <div
          ref={scrollRef}
          className="flex-1 flex gap-2 overflow-x-auto scrollbar-hide py-3 -mx-1 px-1"
        >
          {seasons.map((s) => (
            <SeasonChip
              key={s.season_number}
              season={s}
              stat={byNum.get(s.season_number)}
              active={s.season_number === activeSeason}
              onClick={() => onChange(s.season_number)}
            />
          ))}
        </div>

        <button
          type="button"
          onClick={() => next && onChange(next.season_number)}
          disabled={!next}
          aria-label="Next season"
          className="flex-shrink-0 w-9 h-9 grid place-items-center rounded-lg hover:bg-white/10 disabled:opacity-30 disabled:hover:bg-transparent transition"
        >
          <ChevronDown size={16} className="-rotate-90" />
        </button>
      </div>
    </div>
  );
}

interface SeasonStatLike {
  season_number: number;
  total: number;
  aired: number;
  available: number;
  watched: number;
  downloading: number;
}

function SeasonChip({
  season,
  stat,
  active,
  onClick,
}: {
  season: TmdbSeason;
  stat: SeasonStatLike | undefined;
  active: boolean;
  onClick: () => void;
}) {
  const label =
    season.season_number === 0
      ? 'Specials'
      : season.name && !/^season\s+\d+$/i.test(season.name)
        ? season.name
        : `Season ${season.season_number}`;
  const short =
    season.season_number === 0
      ? 'Specials'
      : /^season\s+\d+$/i.test(season.name ?? '')
        ? `S${season.season_number}`
        : label;

  const total = stat?.total ?? season.episode_count ?? null;
  const available = stat?.available ?? 0;
  const watched = stat?.watched ?? 0;
  const downloading = stat?.downloading ?? 0;
  const allWatched = total != null && total > 0 && watched === total;
  const poster = tmdbImage(season.poster_path, 'w154');

  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-pressed={active}
      data-season={season.season_number}
      className={cn(
        'group/chip relative flex-shrink-0 flex flex-col items-stretch w-[108px] text-left transition focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] rounded-lg'
      )}
    >
      {/* Poster — the primary visual. Active ring sits OUTSIDE the
          clipped inner so overlays can render freely on top. */}
      <span
        className={cn(
          'relative block aspect-poster rounded-md overflow-hidden ring-1 transition',
          active
            ? 'ring-[var(--accent)] ring-2 shadow-[0_0_0_2px_rgba(0,0,0,0.5)]'
            : 'ring-white/5 group-hover/chip:ring-white/20'
        )}
      >
        {poster ? (
          <img src={poster} alt="" className="w-full h-full object-cover" loading="lazy" />
        ) : (
          <span className="absolute inset-0 grid place-items-center bg-white/5 text-[var(--text-muted)]">
            <Tv size={18} />
          </span>
        )}

        {/* Subtle dim on inactive chips so the active one pops */}
        {!active && (
          <span className="absolute inset-0 bg-black/20 group-hover/chip:bg-black/0 transition-colors" />
        )}

        {/* Top-right status badge. Only one wins at a time, ranked by
            importance: watched → downloading → none. */}
        {allWatched ? (
          <span
            title="All watched"
            className="absolute top-1.5 right-1.5 w-5 h-5 rounded-full bg-green-500/90 text-white grid place-items-center shadow"
          >
            <Check size={11} strokeWidth={3} />
          </span>
        ) : downloading > 0 ? (
          <span
            title={`${downloading} downloading`}
            className="absolute top-1.5 right-1.5 flex items-center gap-1 px-1.5 h-5 rounded-full bg-blue-500/90 text-white text-[10px] font-semibold shadow"
          >
            <span className="w-1.5 h-1.5 rounded-full bg-white animate-pulse" />
            {downloading}
          </span>
        ) : null}

        {/* Bottom count pill — "4 / 10" with a slash separator so the
            meaning is unambiguous (downloaded of total, not watched-
            through-progress). Cold shows fall back to TMDB's episode
            count in the same pill shape so there's no layout shift
            when stats arrive. */}
        {total != null && (
          <span className="absolute bottom-1.5 left-1.5 right-1.5 flex items-center justify-center h-5 px-1.5 rounded bg-black/75 backdrop-blur-sm ring-1 ring-white/10 text-[11px] font-semibold text-white tabular-nums">
            {stat ? (
              <>
                {available}
                <span className="text-white/40 mx-0.5">/</span>
                {total}
              </>
            ) : (
              <>
                {total} ep{total !== 1 ? 's' : ''}
              </>
            )}
          </span>
        )}
      </span>

      {/* Label below poster — accent-tinted when active. */}
      <span
        className={cn(
          'mt-2 text-xs font-medium text-center truncate transition',
          active ? 'text-white' : 'text-[var(--text-muted)] group-hover/chip:text-white'
        )}
      >
        {short}
      </span>
    </button>
  );
}
