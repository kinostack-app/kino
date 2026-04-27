import { Download, Loader2, Pause, Play, RotateCcw, Settings2, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { ConfirmDialog } from '@/components/ConfirmDialog';
import { usePlayMedia } from '@/hooks/usePlayMedia';
import { useWatchNow } from '@/hooks/useWatchNow';
import { cn } from '@/lib/utils';
import { useDownloadForMovie } from '@/state/library-cache';
import type { ContentState } from '@/state/use-content-state';

interface ContentActionsProps {
  state: ContentState;
  /** Override the add action (e.g. to show season selection dialog for shows) */
  onAddOverride?: () => void;
  addLabel?: string;
  /** Movie/show title — used for the watch-now overlay status line. */
  title?: string;
  /** Show-only: callback for "Manage downloads" which opens the
   *  re-follow dialog. Surfacing it in this row keeps the primary
   *  affordances (Follow / Manage / Remove) clustered instead of
   *  buried below a status badge. */
  onManageDownloads?: () => void;
  /** Hide the Remove button when the caller's poster already carries
   *  an `×` affordance. Avoids rendering Remove twice on the show
   *  detail page, whose poster is a full PosterCard. */
  hideRemove?: boolean;
}

export function ContentActions({
  state,
  onAddOverride,
  addLabel,
  title,
  onManageDownloads,
  hideRemove,
}: ContentActionsProps) {
  const [confirmRemove, setConfirmRemove] = useState(false);
  const { playMovie, isLoading: isPlayLoading } = usePlayMedia();
  const { watchNow } = useWatchNow();
  // Narrow once at the top so the compiler enforces movie-only
  // field access below. `movie` is null for shows, forcing every
  // movie-specific branch to gate on it first.
  const movie = state.type === 'movie' ? state : null;
  const activeDownload = useDownloadForMovie(movie?.libraryId);

  // One Play button for movies, state-aware under the hood. Shows
  // get no Play here — their detail page uses the Next Up CTA and
  // per-episode rows for granular control.
  //
  // Hidden only in the pure pre-torrent waits ('searching', 'queued')
  // where there's genuinely nothing to stream yet, and in 'none' where
  // the primary verb is Add (Play is offered as a secondary — see
  // below). Failed renders Play because re-kicking watch-now gets a
  // fresh search + release.
  const showPlayPrimary =
    !!movie && movie.phase !== 'none' && movie.phase !== 'searching' && movie.phase !== 'queued';

  const showPlaySecondary = !!movie && movie.phase === 'none';

  const handleMoviePlay = () => {
    if (!movie) return;
    if (movie.canPlay && movie.libraryId) {
      void playMovie(movie.libraryId);
      return;
    }
    if (activeDownload && movie.libraryId) {
      // Same canonical URL the imported path uses — backend
      // dispatcher picks the in-progress torrent as the byte source
      // until the import lands, then transparently swaps to the
      // library file.
      if (movie.phase === 'paused') movie.resume();
      void playMovie(movie.libraryId);
      return;
    }
    watchNow({
      kind: 'movie',
      tmdbId: movie.tmdbId,
      title: title ?? 'movie',
    });
  };

  return (
    <div className="space-y-4">
      {/* Status label — movie-only. The show-level "status" concept
          is meaningless (the show either is or isn't followed; actual
          state lives per-episode on the detail page below). */}
      {movie && <StatusLabel phase={movie.phase} quality={movie.quality} />}

      {/* Action buttons */}
      <div className="flex items-center gap-3 flex-wrap">
        {/* Play (primary, movies) — always labelled "Play". The
            button exists in every state except the cold pre-torrent
            waits. Handles local playback, torrent streaming, and
            watch-now re-acquisition under one verb. */}
        {showPlayPrimary && (
          <button
            type="button"
            disabled={isPlayLoading}
            onClick={handleMoviePlay}
            className="flex items-center gap-2 px-6 py-3 rounded-lg bg-white text-black font-semibold hover:bg-white/90 disabled:opacity-50 transition"
          >
            {isPlayLoading ? (
              <Loader2 size={18} className="animate-spin" />
            ) : (
              <Play size={18} fill="black" />
            )}
            Play
          </button>
        )}

        {/* Add to Library — primary for cold (phase='none') items,
            also offered when the parent needs a per-season dialog
            (shows use onAddOverride). */}
        {state.canAdd && (
          <button
            type="button"
            onClick={onAddOverride ?? (() => state.add())}
            disabled={state.isAdding}
            className={cn(
              'flex items-center gap-2 px-6 py-3 rounded-lg disabled:opacity-50 font-semibold transition',
              // Demote Add to secondary styling when Play is the
              // cold-start primary (movies only — shows have no Play
              // at the item level so Add stays primary).
              showPlaySecondary
                ? 'bg-white/10 hover:bg-white/15 text-white'
                : 'bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white'
            )}
          >
            {state.isAdding ? (
              <Loader2 size={18} className="animate-spin" />
            ) : (
              <Download size={18} />
            )}
            {state.isAdding ? 'Adding...' : (addLabel ?? 'Add to Library')}
          </button>
        )}

        {/* Play (secondary, cold movie) — kicks watch-now directly
            so the user can start watching without the Add-first step.
            Separate from the Add path which just queues. */}
        {showPlaySecondary && (
          <button
            type="button"
            onClick={handleMoviePlay}
            className="flex items-center gap-2 px-6 py-3 rounded-lg bg-white text-black font-semibold hover:bg-white/90 transition"
          >
            <Play size={18} fill="black" />
            Play
          </button>
        )}

        {/* Pause / Resume / Retry — all movie-only. Shows don't have
            per-entity acquisition state (it's per episode). */}
        {movie?.canPause && (
          <button
            type="button"
            onClick={() => movie.pause()}
            className="flex items-center gap-2 px-5 py-3 rounded-lg bg-white/10 hover:bg-white/20 text-white font-medium transition"
          >
            <Pause size={18} />
            Pause
          </button>
        )}

        {movie?.canResume && (
          <button
            type="button"
            onClick={() => movie.resume()}
            className="flex items-center gap-2 px-5 py-3 rounded-lg bg-white/10 hover:bg-white/20 text-white font-medium transition"
          >
            <Play size={18} />
            Resume
          </button>
        )}

        {movie?.canRetry && (
          <button
            type="button"
            onClick={() =>
              watchNow({ kind: 'movie', tmdbId: movie.tmdbId, title: title ?? 'movie' })
            }
            className="flex items-center gap-2 px-5 py-3 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white font-semibold transition"
          >
            <RotateCcw size={18} />
            Retry
          </button>
        )}

        {/* Manage downloads — shows only. Lives in the main action
            row next to Remove so it's obvious this is how you change
            what's being pulled (seasons, future-episode policy). */}
        {onManageDownloads && state.canRemove && (
          <button
            type="button"
            onClick={onManageDownloads}
            className="flex items-center gap-2 px-5 py-3 rounded-lg bg-white/10 hover:bg-white/15 text-white font-medium transition"
          >
            <Settings2 size={16} />
            Manage
          </button>
        )}

        {/* Remove — suppressed when the caller's poster carries its
            own × affordance (see `hideRemove`). */}
        {state.canRemove && !hideRemove && (
          <button
            type="button"
            onClick={() => setConfirmRemove(true)}
            className="flex items-center gap-2 px-5 py-3 rounded-lg bg-white/5 hover:bg-red-600/20 text-[var(--text-secondary)] hover:text-red-400 font-medium transition"
          >
            <Trash2 size={16} />
            Remove
          </button>
        )}
      </div>

      {/* Download progress detail — movie-only */}
      {movie &&
        (movie.phase === 'downloading' ||
          movie.phase === 'stalled' ||
          movie.phase === 'paused') && (
          <div className="mt-2">
            <div className="h-2 rounded-full bg-white/10 overflow-hidden">
              <div
                className={cn(
                  'h-full rounded-full transition-all duration-500',
                  movie.phase === 'paused' || movie.phase === 'stalled'
                    ? 'bg-amber-500'
                    : 'bg-[var(--accent)]'
                )}
                style={{ width: `${movie.downloadProgress ?? 0}%` }}
              />
            </div>
            <p className="mt-1.5 text-xs text-[var(--text-muted)]">
              {movie.downloadProgress ?? 0}% downloaded
              {movie.phase === 'paused' && ' · Paused'}
            </p>
          </div>
        )}

      {/* Confirm remove modal */}
      <ConfirmDialog
        open={confirmRemove}
        title="Remove from library?"
        description="This will remove the content and cancel any active downloads. Downloaded files will be deleted."
        confirmLabel="Remove"
        onConfirm={() => {
          state.remove();
          setConfirmRemove(false);
        }}
        onCancel={() => setConfirmRemove(false)}
      />
    </div>
  );
}

function StatusLabel({ phase, quality }: { phase: string; quality?: string }) {
  switch (phase) {
    case 'none':
      return <p className="text-sm text-[var(--text-muted)]">Not in your library</p>;
    case 'searching':
      return (
        <div className="flex items-center gap-2">
          <div className="flex gap-[3px] dot-animation">
            <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
          </div>
          <p className="text-sm text-[var(--text-secondary)]">Searching for releases...</p>
        </div>
      );
    case 'queued':
      return <p className="text-sm text-[var(--text-muted)]">Queued for download</p>;
    case 'downloading':
      return <p className="text-sm text-[var(--accent)]">Downloading...</p>;
    case 'stalled':
      return <p className="text-sm text-amber-400">Stalled — waiting for peers</p>;
    case 'paused':
      return <p className="text-sm text-amber-400">Download paused</p>;
    case 'failed':
      return <p className="text-sm text-red-400">Download failed</p>;
    case 'importing':
      return (
        <div className="flex items-center gap-2">
          <div className="flex gap-[3px] dot-animation">
            <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
          </div>
          <p className="text-sm text-[var(--text-secondary)]">Importing...</p>
        </div>
      );
    case 'available':
      return (
        <div className="flex items-center gap-2">
          <span className="w-2 h-2 rounded-full bg-green-500" />
          <p className="text-sm text-[var(--text-secondary)]">
            Ready to play
            {quality && (
              <span className="ml-2 px-1.5 py-0.5 rounded bg-white/10 text-xs font-medium">
                {quality}
              </span>
            )}
          </p>
        </div>
      );
    case 'watched':
      return (
        <div className="flex items-center gap-2">
          <span className="w-2 h-2 rounded-full bg-green-500" />
          <p className="text-sm text-[var(--text-secondary)]">
            Watched
            {quality && (
              <span className="ml-2 px-1.5 py-0.5 rounded bg-white/10 text-xs font-medium">
                {quality}
              </span>
            )}
          </p>
        </div>
      );
    default:
      return null;
  }
}
