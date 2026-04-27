import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { ConfirmDialog } from '@/components/ConfirmDialog';
import { PosterCard } from '@/components/PosterCard';
import { usePlayMedia } from '@/hooks/usePlayMedia';
import { useWatchNow } from '@/hooks/useWatchNow';
import { tmdbImage } from '@/lib/api';
import { useDownloadForMovie } from '@/state/library-cache';
import { useContentState } from '@/state/use-content-state';

interface TmdbMovieCardProps {
  id: number;
  title: string;
  releaseDate?: string | null;
  posterPath?: string | null;
  /** Pre-computed year (from library cache — avoids parsing releaseDate) */
  year?: number;
  /** Optional BlurHash for the poster (library items only). */
  blurhash?: string | null;
}

export function TmdbMovieCard({
  id,
  title,
  releaseDate,
  posterPath,
  year,
  blurhash,
}: TmdbMovieCardProps) {
  const navigate = useNavigate();
  const state = useContentState(id, 'movie');
  const { playMovie } = usePlayMedia();
  const { watchNow } = useWatchNow();
  const activeDownload = useDownloadForMovie(state.libraryId);
  const displayYear =
    year ?? (releaseDate ? Number.parseInt(releaseDate.slice(0, 4), 10) : undefined);
  const [confirmRemove, setConfirmRemove] = useState(false);

  // One Play button, state-aware behind the scenes:
  //   - imported           → local playback (fastest path)
  //   - active download    → /watch/{id} directly (no stepper flash)
  //   - paused             → resume first, then /watch/{id}
  //   - importing          → /watch/{id} — stream still works during
  //                          the import window
  //   - everything else    → watchNow (cold, failed, retry-etc.)
  const handlePlay = () => {
    if (state.canPlay && state.libraryId) {
      void playMovie(state.libraryId);
      return;
    }
    if (activeDownload && state.libraryId) {
      // Canonical URL regardless of import status — the dispatcher
      // picks torrent bytes vs library bytes per request.
      if (state.phase === 'paused') state.resume();
      void playMovie(state.libraryId);
      return;
    }
    watchNow({ kind: 'movie', tmdbId: id, title });
  };

  return (
    <>
      <PosterCard
        title={title}
        year={displayYear}
        posterUrl={tmdbImage(posterPath)}
        blurhash={blurhash}
        phase={state.phase}
        quality={state.quality}
        watchProgress={state.watchProgress}
        downloadProgress={state.downloadProgress}
        downloadSpeed={state.downloadSpeed}
        onClick={() => navigate({ to: '/movie/$tmdbId', params: { tmdbId: String(id) } })}
        onPlay={handlePlay}
        onAdd={state.canAdd ? state.add : undefined}
        onRemove={state.canRemove ? () => setConfirmRemove(true) : undefined}
        onPause={state.canPause ? state.pause : undefined}
        onResume={state.canResume ? state.resume : undefined}
        isAdding={state.isAdding}
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
