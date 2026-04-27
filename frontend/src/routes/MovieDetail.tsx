import { useQuery } from '@tanstack/react-query';
import { useParams } from '@tanstack/react-router';
import { Calendar, Clock, ExternalLink, Star } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { movieDetailsOptions } from '@/api/generated/@tanstack/react-query.gen';
import { ConfirmDialog } from '@/components/ConfirmDialog';
import { DetailLayout } from '@/components/DetailLayout';
import { PosterCard } from '@/components/PosterCard';
import { RateWidget } from '@/components/RateWidget';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { usePlayMedia } from '@/hooks/usePlayMedia';
import { useWatchNow } from '@/hooks/useWatchNow';
import { tmdbImage } from '@/lib/api';
import { useDownloadForMovie, useMovieByTmdbId } from '@/state/library-cache';
import { useContentState } from '@/state/use-content-state';

export function MovieDetail() {
  const { tmdbId } = useParams({ from: '/movie/$tmdbId' });
  const id = Number(tmdbId);
  const { data, isLoading, isError, error } = useQuery(
    movieDetailsOptions({ path: { tmdb_id: id } })
  );
  const state = useContentState(id, 'movie');
  const { playMovie } = usePlayMedia();
  const { watchNow } = useWatchNow();
  const activeDownload = useDownloadForMovie(state.libraryId);
  const [confirmRemove, setConfirmRemove] = useState(false);

  const movie = data as Record<string, unknown> | undefined;

  useDocumentTitle(typeof movie?.title === 'string' ? movie.title : null);

  if (isLoading) {
    return (
      <div className="-mt-14 min-h-screen">
        <div className="h-[60vh] skeleton" />
        <div className="px-4 md:px-12 -mt-32 relative z-10 space-y-4 max-w-4xl">
          <div className="h-10 w-80 skeleton rounded" />
          <div className="h-5 w-48 skeleton rounded" />
          <div className="h-12 w-48 skeleton rounded-lg" />
        </div>
      </div>
    );
  }

  if (!movie) {
    // Distinguish "TMDB doesn't have this id" (true 404) from "we
    // can't reach the backend" (connection error). Previously both
    // rendered "Movie not found" which gaslit users during outages.
    const msg = isError ? (error instanceof Error ? error.message : 'Request failed') : null;
    return (
      <div className="flex items-center justify-center min-h-[50vh] text-[var(--text-muted)] text-center px-6">
        {msg ? (
          <div>
            <p className="mb-1 text-white">Couldn't load movie</p>
            <p className="text-xs">{msg}</p>
          </div>
        ) : (
          'Movie not found'
        )}
      </div>
    );
  }

  const title = String(movie.title ?? '');
  const releaseDate = movie.release_date as string | undefined;
  const year = releaseDate?.slice(0, 4);
  const runtime = movie.runtime as number | undefined;
  const rating = movie.vote_average as number | undefined;
  const genres = movie.genres as Array<{ id: number; name: string }> | undefined;
  const videos = movie.videos as
    | { results?: Array<{ key: string; site: string; type: string }> }
    | undefined;
  const trailer = videos?.results?.find((v) => v.site === 'YouTube' && v.type === 'Trailer');
  const hours = runtime ? Math.floor(runtime / 60) : 0;
  const mins = runtime ? runtime % 60 : 0;

  const meta: Array<{ icon?: ReactNode; label: string; href?: string }> = [
    ...(year
      ? [{ icon: <Calendar size={13} className="text-[var(--text-muted)]" />, label: year }]
      : []),
    ...(runtime
      ? [
          {
            icon: <Clock size={13} className="text-[var(--text-muted)]" />,
            label: `${hours}h ${mins}m`,
          },
        ]
      : []),
    ...(rating
      ? [
          {
            icon: <Star size={13} className="text-yellow-500 fill-yellow-500" />,
            label: rating.toFixed(1),
          },
        ]
      : []),
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

  // One Play button, state-aware behind the scenes. Same logic as
  // `TmdbMovieCard` — library → local playback; active download →
  // canonical play URL (dispatcher picks torrent/library bytes);
  // cold → watch-now which grabs first. Keeps every play path
  // funneled through a single handler.
  const handlePlay = () => {
    if (state.canPlay && state.libraryId) {
      void playMovie(state.libraryId);
      return;
    }
    if (activeDownload && state.libraryId) {
      if (state.phase === 'paused') state.resume();
      void playMovie(state.libraryId);
      return;
    }
    watchNow({ kind: 'movie', tmdbId: id, title });
  };

  return (
    <>
      <DetailLayout
        title={title}
        tagline={movie.tagline as string | undefined}
        overview={movie.overview as string | undefined}
        backdropUrl={tmdbImage(movie.backdrop_path as string | undefined, 'w1280')}
        posterUrl={tmdbImage(movie.poster_path as string | undefined, 'w500')}
        meta={meta}
        // The poster is the whole control surface — Play, Add,
        // Remove, Pause/Resume, download progress all render here.
        // No right-side action row, no status strings duplicating
        // the poster's visual state. Same `PosterCard` component
        // the library grid uses; scales to the detail-page width.
        poster={
          <PosterCard
            title={title}
            posterUrl={tmdbImage(movie.poster_path as string | undefined, 'w500')}
            phase={state.phase}
            quality={state.quality}
            watchProgress={state.watchProgress}
            downloadProgress={state.downloadProgress}
            downloadSpeed={state.downloadSpeed}
            onPlay={handlePlay}
            onAdd={state.canAdd ? state.add : undefined}
            onRemove={state.canRemove ? () => setConfirmRemove(true) : undefined}
            onPause={state.canPause ? state.pause : undefined}
            onResume={state.canResume ? state.resume : undefined}
            isAdding={state.isAdding}
          />
        }
        belowActions={<MovieRatingRow tmdbId={id} />}
      />
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

/** Renders the rate widget only when the movie is in the library —
 *  rating something you haven't added would require materialising
 *  the local row first, and we don't want to silently add-on-rate. */
function MovieRatingRow({ tmdbId }: { tmdbId: number }) {
  const movie = useMovieByTmdbId(tmdbId);
  if (!movie) return null;
  return <RateWidget kind="movie" id={movie.id} value={movie.user_rating} />;
}
