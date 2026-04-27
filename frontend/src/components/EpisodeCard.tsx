import {
  ArrowDown,
  Check,
  Download,
  Eye,
  EyeOff,
  ListOrdered,
  Pause,
  Play,
  RotateCcw,
  TriangleAlert,
  X,
} from 'lucide-react';
import { useState } from 'react';
import type { EpisodeView } from '@/api/generated/types.gen';
import { RateWidget } from '@/components/RateWidget';
import { ReleasesDialog } from '@/components/ReleasesDialog';
import { tmdbImage } from '@/lib/api';
import { cn } from '@/lib/utils';
import { useDownloads } from '@/state/library-cache';

/**
 * Single episode row — TMDB metadata plus (when the show is in the
 * library) per-episode state: download progress, imported file,
 * watched mark, etc. The parent passes an `EpisodeView` straight
 * from the unified `show_season_episodes_by_tmdb` endpoint so we
 * never branch on follow status here.
 */

interface EpisodeCardProps {
  episode: EpisodeView;
  isUpNext?: boolean;
  /** Fired on the center Play button. Should kick watch-now; the
   *  hook behind it handles route decisions (imported → local player,
   *  downloading → /watch/{id}, else watch-now search). */
  onPlay?: () => void;
  /** Toggle watched mark. Null when the episode isn't in the library
   *  yet — the action isn't applicable. */
  onToggleWatched?: (watched: boolean) => void;
  /** Reset the episode so the scheduler re-searches. */
  onRedownload?: () => void;
  /** Flip `acquire = 1` so the scheduler picks this specific episode
   *  up. Used when the show was followed with "future only" and the
   *  user now wants a back-catalog episode without widening the
   *  monitor scope. Parallel to movie/show card "+" button. */
  onGet?: () => void;
  /** Discard — symmetric inverse of `onGet`. Cancels in-flight
   *  downloads + removes linked media + sets acquire=0. Parallel
   *  to movie/show card "X" button (remove from library). */
  onRemove?: () => void;
  /** Pause an in-flight download. */
  onPause?: () => void;
  /** Resume a paused download. */
  onResume?: () => void;
}

function formatSpeed(bps: number | undefined): string {
  // Match PosterCard's placeholder so "Queued / stalled with 0 B/s"
  // still shows a stable label rather than an empty span that makes
  // the pill collapse horizontally between ticks.
  if (!bps || bps <= 0) return '-- MB/s';
  if (bps >= 1048576) return `${(bps / 1048576).toFixed(1)} MB/s`;
  if (bps >= 1024) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${bps} B/s`;
}

export function EpisodeCard({
  episode,
  isUpNext,
  onPlay,
  onToggleWatched,
  onRedownload,
  onGet,
  onRemove,
  onPause,
  onResume,
}: EpisodeCardProps) {
  const hasAired = episode.air_date ? new Date(episode.air_date) <= new Date() : false;
  const stillUrl = tmdbImage(episode.still_path, 'w300');
  const playable = hasAired && onPlay;
  const watched = !!episode.watched_at;
  // Partial watch progress (0–100). Runtime is in minutes on the
  // TMDB side; `playback_position_ticks` is libvlc-style (100ns
  // units); converting both to seconds.
  const watchProgress = (() => {
    if (!episode.playback_position_ticks || !episode.runtime) return undefined;
    const playedSec = episode.playback_position_ticks / 10_000_000;
    const runtimeSec = episode.runtime * 60;
    if (runtimeSec <= 0) return undefined;
    const pct = Math.round((playedSec / runtimeSec) * 100);
    return pct > 2 && pct < 99 ? pct : undefined;
  })();

  // Join live download state from the downloads cache when this
  // episode has an active download. The season-episodes query has a
  // long stale time + doesn't invalidate on per-tick download_progress
  // events (those are high-frequency). Reading from the downloads
  // cache directly gives us live percent + speed without thrashing
  // the episode list refetch.
  const { data: allDownloads } = useDownloads();
  const liveDownload = episode.download_id
    ? allDownloads?.find((d) => d.id === episode.download_id)
    : undefined;
  const phase = derivePhase(episode, liveDownload?.state);
  const downloadPct = liveDownload?.size
    ? Math.round((liveDownload.downloaded / liveDownload.size) * 100)
    : (episode.download_percent ?? 0);
  const downloadSpeed = liveDownload?.download_speed;

  // ReleasesDialog is modal — no more outside-click / Escape wiring
  // needed now that the overflow menu is gone (actions moved inline
  // alongside the episode metadata).
  const [releasesOpen, setReleasesOpen] = useState(false);

  return (
    <div
      className={cn(
        'group flex gap-4 p-3 rounded-lg transition-colors',
        isUpNext ? 'bg-white/5 ring-1 ring-[var(--accent)]/30' : 'hover:bg-white/5'
      )}
    >
      {/* Thumbnail — outer wrapper is `relative` but NOT `overflow-
          hidden` so the corner action buttons can overlap the edge
          the same way they do on PosterCard. Inner wrapper clips the
          image + overlays. Structure mirrors PosterCard exactly. */}
      <div className="relative flex-shrink-0 w-40 sm:w-48 aspect-video">
        <div className="absolute inset-0 rounded-md overflow-hidden bg-[var(--bg-card)]">
          {stillUrl ? (
            <img src={stillUrl} alt="" className="w-full h-full object-cover" loading="lazy" />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-[var(--text-muted)] text-xs">
              E{episode.episode_number}
            </div>
          )}

          {/* Download sweep + progress pill — only during active transfer */}
          {(phase === 'downloading' || phase === 'paused' || phase === 'stalled') && (
            <>
              <div
                className="absolute inset-0 bg-black/40 transition-[clip-path] duration-700 ease-linear"
                style={{ clipPath: `inset(0 0 0 ${downloadPct}%)` }}
              />
              <div className="absolute inset-x-0 bottom-1.5 z-10 flex justify-center">
                <div className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-black/70 backdrop-blur-sm ring-1 ring-white/15 text-[10px] tabular-nums whitespace-nowrap">
                  <span className="font-bold text-white">{downloadPct}%</span>
                  {phase === 'downloading' && (
                    <span className="flex items-center gap-0.5 text-white/70">
                      <ArrowDown size={8} />
                      {formatSpeed(downloadSpeed)}
                    </span>
                  )}
                  {phase === 'stalled' && (
                    <span className="text-amber-400 font-medium">Stalled</span>
                  )}
                  {phase === 'paused' && (
                    <span className="text-amber-400 font-semibold uppercase tracking-wider">
                      Paused
                    </span>
                  )}
                </div>
              </div>
              <div className="absolute bottom-0 left-0 right-0 h-[3px]">
                <div
                  className={cn(
                    'h-full transition-all duration-700',
                    phase === 'paused' || phase === 'stalled'
                      ? 'bg-amber-400'
                      : 'bg-[var(--accent)]'
                  )}
                  style={{ width: `${downloadPct}%` }}
                />
              </div>
            </>
          )}

          {/* Waiting state pill (searching / queued / importing) */}
          {(phase === 'searching' || phase === 'queued' || phase === 'importing') && (
            <div className="absolute bottom-2 right-2 z-10 flex items-center gap-1.5 h-6 px-2 rounded-full bg-black/70 backdrop-blur-sm ring-1 ring-white/15">
              <div className="flex gap-[3px] dot-animation">
                <span className="block w-1 h-1 rounded-full bg-white" />
                <span className="block w-1 h-1 rounded-full bg-white" />
                <span className="block w-1 h-1 rounded-full bg-white" />
              </div>
              <span className="text-[9px] text-white/70 font-medium uppercase tracking-wider">
                {phase === 'searching' ? 'Searching' : phase === 'queued' ? 'Queued' : 'Importing'}
              </span>
            </div>
          )}

          {/* Failed badge */}
          {phase === 'failed' && (
            <div className="absolute bottom-2 right-2 z-10 flex items-center gap-1 h-6 px-2 rounded-full bg-black/70 backdrop-blur-sm ring-1 ring-red-500/30">
              <TriangleAlert size={10} className="text-red-400" />
              <span className="text-[9px] text-red-400 font-semibold uppercase tracking-wider">
                Failed
              </span>
            </div>
          )}

          {/* Watch progress bar (partial viewing). 3px matches
            PosterCard so scrolling a library → episode list doesn't
            flicker between bar thicknesses. */}
          {watchProgress != null && phase === 'available' && (
            <div className="absolute bottom-0 left-0 right-0 h-[3px] bg-white/15">
              <div className="h-full bg-[var(--accent)]" style={{ width: `${watchProgress}%` }} />
            </div>
          )}

          {/* Resolution badge when imported (bottom-left, beside the
            status dot) */}
          {phase === 'available' && episode.resolution && (
            <div className="absolute top-1.5 left-1.5 z-10 px-1.5 py-0.5 rounded bg-black/70 backdrop-blur-sm text-[10px] font-medium text-white">
              {episode.resolution}p
            </div>
          )}

          {/* Status pip (bottom-left) — available check vs watched eye */}
          {(phase === 'available' || phase === 'watched') && (
            <div className="absolute bottom-2 left-2 z-10 w-5 h-5 rounded-full bg-black/70 backdrop-blur-sm ring-1 ring-white/15 grid place-items-center">
              {watched ? (
                <Eye size={10} className="text-blue-400" />
              ) : (
                <Check size={10} strokeWidth={3} className="text-green-400" />
              )}
            </div>
          )}

          {/* Play overlay — any state where there's something we can do
            (acquire if not started, stream while downloading, play
            imported file). Button is always white: primary action
            shouldn't change colour with content state. The sweep
            overlay below already communicates "this is downloading."
            Matches PosterCard for cross-card consistency. */}
          {playable && phase !== 'searching' && phase !== 'queued' && (
            <button
              type="button"
              onClick={onPlay}
              aria-label="Play"
              className="absolute inset-0 flex items-center justify-center opacity-0 hover:opacity-100 focus-visible:opacity-100 transition-opacity bg-black/40"
            >
              <span className="w-10 h-10 rounded-full grid place-items-center shadow-xl bg-white/90 text-black">
                <Play size={18} fill="currentColor" className="ml-0.5" />
              </span>
            </button>
          )}

          {/* Not-aired overlay */}
          {!hasAired && (
            <div className="absolute inset-0 bg-black/55 flex items-center justify-center">
              <span className="text-xs text-white/75 font-medium">
                {episode.air_date
                  ? new Date(episode.air_date).toLocaleDateString(undefined, {
                      month: 'short',
                      day: 'numeric',
                      year: 'numeric',
                    })
                  : 'TBA'}
              </span>
            </div>
          )}
        </div>

        {/* ── TOP-RIGHT corner overlap: Get / Remove. Mirrors
            PosterCard's +/- pattern exactly so cross-card UX is
            consistent: hover-only, circular, pops out of the edge.
            Visibility: idle → `+` (Download icon, hover accent);
            anything else in-library → `X` (hover red). */}
        {phase === 'idle' && hasAired && onGet && (
          <button
            type="button"
            aria-label="Get episode"
            title="Get episode"
            className="absolute -top-1.5 -right-1.5 z-30 w-7 h-7 rounded-full bg-black/80 text-white border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-all hover:bg-[var(--accent)] hover:border-[var(--accent)]"
            onClick={(e) => {
              e.stopPropagation();
              onGet();
            }}
          >
            <Download size={13} />
          </button>
        )}
        {phase !== 'idle' && hasAired && onRemove && episode.episode_id != null && (
          <button
            type="button"
            aria-label="Remove episode"
            title="Remove episode"
            className="absolute -top-1.5 -right-1.5 z-30 w-7 h-7 rounded-full bg-black/80 border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-opacity hover:bg-red-600 hover:border-red-600"
            onClick={(e) => {
              e.stopPropagation();
              onRemove();
            }}
          >
            <X size={13} strokeWidth={2.5} className="text-white" />
          </button>
        )}

        {/* ── TOP-LEFT corner overlap: Pause / Resume. Mirrors
            PosterCard's transfer-state affordance. */}
        {(phase === 'downloading' || phase === 'stalled') && onPause && (
          <button
            type="button"
            aria-label="Pause download"
            title="Pause download"
            className="absolute -top-1.5 -left-1.5 z-30 w-7 h-7 rounded-full bg-black/80 border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-[opacity,background-color] hover:bg-neutral-700 hover:border-white/40"
            onClick={(e) => {
              e.stopPropagation();
              onPause();
            }}
          >
            <Pause size={12} fill="white" className="text-white" />
          </button>
        )}
        {phase === 'paused' && onResume && (
          <button
            type="button"
            aria-label="Resume download"
            title="Resume download"
            className="absolute -top-1.5 -left-1.5 z-30 w-7 h-7 rounded-full bg-black/80 border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-[opacity,background-color] hover:bg-neutral-700 hover:border-white/40"
            onClick={(e) => {
              e.stopPropagation();
              onResume();
            }}
          >
            <Play size={12} fill="white" className="text-white ml-0.5" />
          </button>
        )}
      </div>

      {/* Info */}
      <div className="flex-1 min-w-0 py-0.5 relative">
        <div className="flex items-start gap-2">
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <p className="text-sm font-medium truncate">
                {episode.episode_number}. {episode.name ?? `Episode ${episode.episode_number}`}
              </p>
              {isUpNext && (
                <span className="flex-shrink-0 px-1.5 py-0.5 rounded text-[10px] font-semibold bg-[var(--accent)]/20 text-[var(--accent)] uppercase">
                  Up Next
                </span>
              )}
            </div>
            <div className="flex items-center gap-2 mt-0.5 text-xs text-[var(--text-muted)]">
              {episode.runtime && <span>{episode.runtime}m</span>}
              {episode.air_date && hasAired && (
                <span>
                  {new Date(episode.air_date).toLocaleDateString(undefined, {
                    month: 'short',
                    day: 'numeric',
                    year: 'numeric',
                  })}
                </span>
              )}
              {episode.vote_average != null && episode.vote_average > 0 && (
                <span>★ {episode.vote_average.toFixed(1)}</span>
              )}
            </div>
            {episode.overview && (
              <p className="mt-1.5 text-xs text-[var(--text-secondary)] line-clamp-2 leading-relaxed">
                {episode.overview}
              </p>
            )}
            {episode.episode_id != null && (
              <div className="mt-2">
                <RateWidget
                  kind="episode"
                  id={episode.episode_id}
                  value={episode.user_rating}
                  compact
                />
              </div>
            )}
          </div>

          {/* Inline action cluster — per-episode actions, replaces
              the old hover-revealed overflow menu. Episode cards
              always display metadata so there's no image-only
              constraint forcing a menu; flat icon buttons next to
              the title read better + match the movie-card
              affordance model (hover button, not menu). Visibility
              rules are state-driven — see table in the PR note. */}
          {episode.episode_id != null && hasAired && (
            <div className="flex-shrink-0 flex items-center gap-0.5 -mt-0.5 opacity-0 group-hover:opacity-100 sm:opacity-100 transition-opacity">
              {/* Pause/Resume live at the thumbnail's top-left
                  corner (matches PosterCard). Everything below is
                  episode-specific actions with no movie parallel. */}
              {/* Retry — failed downloads. Distinct accent colour so
                  it draws the eye (the state needs a user action to
                  recover, unlike the recoverable pause/stall). */}
              {phase === 'failed' && onRedownload && (
                <IconButton
                  label="Retry download"
                  icon={<Download size={14} />}
                  onClick={onRedownload}
                  tone="accent"
                />
              )}
              {/* Watched toggle — available / watched states only
                  (marking not-yet-imported content watched makes no
                  sense). */}
              {(phase === 'available' || phase === 'watched') && onToggleWatched && (
                <IconButton
                  label={watched ? 'Mark as unwatched' : 'Mark as watched'}
                  icon={watched ? <EyeOff size={14} /> : <Check size={14} />}
                  onClick={() => onToggleWatched(!watched)}
                />
              )}
              {/* Re-download — imported file replacement path. */}
              {phase === 'available' && onRedownload && (
                <IconButton
                  label="Re-download"
                  icon={<RotateCcw size={14} />}
                  onClick={onRedownload}
                />
              )}
              {/* Browse releases — always available once the episode
                  exists in the library. Modal, not navigate. */}
              <IconButton
                label="Browse releases"
                icon={<ListOrdered size={14} />}
                onClick={() => setReleasesOpen(true)}
              />
            </div>
          )}
        </div>
      </div>
      {episode.episode_id != null && (
        <ReleasesDialog
          open={releasesOpen}
          onClose={() => setReleasesOpen(false)}
          scope={{
            kind: 'episode',
            id: episode.episode_id,
            subtitle: `S${String(episode.season_number).padStart(2, '0')}E${String(episode.episode_number).padStart(2, '0')}${episode.name ? ` · ${episode.name}` : ''}`,
          }}
        />
      )}
    </div>
  );
}

type EpisodePhase =
  | 'idle' // acquire=0, aired, no media/download — user can click Get
  | 'searching' // acquire=1, aired, no media/download — scheduler will grab
  | 'queued'
  | 'downloading'
  | 'stalled'
  | 'paused'
  | 'importing'
  | 'failed'
  | 'available'
  | 'watched';

function derivePhase(ep: EpisodeView, liveDownloadState?: string): EpisodePhase {
  // Not in library — `episode_id` is null when the show isn't
  // followed (or was just removed). No acquisition state exists, so
  // return the inert `idle` phase. The Get / Remove buttons won't
  // render (their handlers are gated on `episode_id != null` at the
  // parent), and the searching/queued pills stay hidden. The card
  // just shows TMDB metadata + the Play overlay.
  if (ep.episode_id == null) return 'idle';

  if (ep.watched_at) return 'watched';
  if (ep.media_id != null) return 'available';
  // Prefer the live download cache state (tick-updated by WS
  // download_progress patches) over the snapshot from the show-
  // episodes query, which can be seconds stale.
  const state = liveDownloadState ?? ep.download_state;
  if (state) {
    switch (state) {
      case 'queued':
      case 'grabbing':
        return 'queued';
      case 'downloading':
        return 'downloading';
      case 'stalled':
        return 'stalled';
      case 'paused':
        return 'paused';
      case 'importing':
        return 'importing';
      case 'failed':
        return 'failed';
    }
  }
  // No media, no download — split by the `acquire` flag. Backend's
  // `status = 'wanted'` collapses both cases into one string which
  // can't be distinguished here; `acquire` is the authoritative bit.
  //   acquire = 0 → user hasn't asked for this episode → idle
  //   acquire = 1 → scheduler will pick up on next sweep → searching
  if (ep.acquire === false) return 'idle';
  return 'searching';
}

/** Compact inline action button used in the episode card cluster.
 *  Icon-only with an `aria-label` + native tooltip. Tone=accent draws
 *  attention for state-recovery actions like Retry. */
function IconButton({
  label,
  icon,
  onClick,
  tone = 'default',
}: {
  label: string;
  icon: React.ReactNode;
  onClick: () => void;
  tone?: 'default' | 'accent';
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={cn(
        'w-7 h-7 grid place-items-center rounded-md transition',
        tone === 'accent'
          ? 'text-[var(--accent)] hover:bg-[var(--accent)]/10'
          : 'text-[var(--text-muted)] hover:text-white hover:bg-white/10'
      )}
    >
      {icon}
    </button>
  );
}

/** Skeleton placeholder for loading episodes */
export function EpisodeCardSkeleton() {
  return (
    <div className="flex gap-4 p-3">
      <div className="flex-shrink-0 w-40 sm:w-48 aspect-video rounded-md skeleton" />
      <div className="flex-1 space-y-2 py-1">
        <div className="h-4 w-48 skeleton rounded" />
        <div className="h-3 w-24 skeleton rounded" />
        <div className="h-3 w-full max-w-sm skeleton rounded" />
      </div>
    </div>
  );
}
