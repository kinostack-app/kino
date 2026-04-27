import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { ConfirmDialog } from '@/components/ConfirmDialog';
import { PosterCard } from '@/components/PosterCard';
import { useWatchNow } from '@/hooks/useWatchNow';
import { tmdbImage } from '@/lib/api';
import { useShowByTmdbId } from '@/state/library-cache';
import { useContentState } from '@/state/use-content-state';

interface TmdbShowCardProps {
  id: number;
  name: string;
  firstAirDate?: string | null;
  posterPath?: string | null;
  /** Pre-computed year (from library cache) */
  year?: number;
  /** Optional BlurHash for the poster (library items only). */
  blurhash?: string | null;
}

export function TmdbShowCard({
  id,
  name,
  firstAirDate,
  posterPath,
  year,
  blurhash,
}: TmdbShowCardProps) {
  const navigate = useNavigate();
  const state = useContentState(id, 'show');
  const show = useShowByTmdbId(id);
  const { watchNow } = useWatchNow();

  // Badge showing "1 of 8" when the show is in the library and has
  // at least one aired episode. Tells the whole-show story at a
  // glance — the old binary tick only said "something's here",
  // not "how much of it you've acquired."
  const progressBadge = (() => {
    if (!show) return undefined;
    const avail = show.available_episode_count ?? 0;
    const aired = show.aired_episode_count ?? 0;
    if (aired <= 0) return undefined;
    return `${avail}/${aired}`;
  })();
  const displayYear =
    year ?? (firstAirDate ? Number.parseInt(firstAirDate.slice(0, 4), 10) : undefined);

  // Compose the card's sublabel — what's about to happen for this
  // show, at a glance. Priority order matches what the backend's
  // `watch_now_show_smart` resolver will actually do on Play:
  //   1. Active download → show that episode (overlay pill already
  //      says "Downloading"; sublabel just adds episode context).
  //   2. In-library next-up → "S02E05 · Next up" (resolver tier 1+2).
  //   3. Not in library + aired → "S01E01 · Start watching"
  //      (resolver tier 2 for a fresh library row).
  //   4. Anything else (caught-up, unaired, unknown) → no sublabel
  //      so we don't over-promise what Play will pick.
  const episodeTag = (() => {
    const ep = state.activeEpisode ?? state.nextEpisode;
    if (!ep) return undefined;
    const s = String(ep.season).padStart(2, '0');
    const e = String(ep.episode).padStart(2, '0');
    return `S${s}E${e}`;
  })();
  const hasAired = (() => {
    if (!firstAirDate) return false;
    const t = new Date(firstAirDate).getTime();
    return Number.isFinite(t) && t <= Date.now();
  })();
  const sublabel = (() => {
    if (state.activeEpisode && episodeTag) return episodeTag;
    if (state.nextEpisode && episodeTag) return `${episodeTag} · Next up`;
    if (state.phase === 'none' && hasAired) return 'S01E01 · Start watching';
    return undefined;
  })();
  const [confirmRemove, setConfirmRemove] = useState(false);

  // Show-level Play = "start watching this show" — the backend
  // resolves to the right episode (first unwatched aired, pilot
  // fallback) and auto-follows if needed. No dialog, no per-episode
  // picker. Users who want finer control use the Follow button or
  // click through to the detail page.
  const handlePlay = () => {
    watchNow({ kind: 'show_smart_play', showTmdbId: id, title: name });
  };

  // "+" (Follow) intentionally routes to the detail page with the
  // dialog auto-opening, rather than committing inline. A show card
  // has poster + title + year — not enough context to pick a sane
  // monitoring scope for a show the user doesn't know. The detail
  // page gives overview, seasons, air dates, which the Follow
  // dialog needs to be a real decision surface. Movies still
  // commit inline because one-file vs ten-seasons.
  const handleAdd = () => {
    navigate({
      to: '/show/$tmdbId',
      params: { tmdbId: String(id) },
      search: { follow: '1' } as never,
    });
  };

  return (
    <>
      <PosterCard
        title={name}
        year={displayYear}
        sublabel={sublabel}
        posterUrl={tmdbImage(posterPath)}
        blurhash={blurhash}
        phase={state.phase}
        progressBadge={progressBadge}
        downloadProgress={state.downloadProgress}
        downloadSpeed={state.downloadSpeed}
        activeDownloadCount={state.activeDownloadCount}
        onClick={() => navigate({ to: '/show/$tmdbId', params: { tmdbId: String(id) } })}
        onPlay={handlePlay}
        onAdd={state.canAdd ? handleAdd : undefined}
        onRemove={state.canRemove ? () => setConfirmRemove(true) : undefined}
        onPause={state.canPause ? state.pause : undefined}
        onResume={state.canResume ? state.resume : undefined}
        isAdding={state.isAdding}
      />
      <ConfirmDialog
        open={confirmRemove}
        title="Remove from library?"
        description={`This will remove "${name}" and cancel any active downloads.`}
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
