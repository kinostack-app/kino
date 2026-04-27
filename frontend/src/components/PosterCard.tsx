import {
  ArrowDown,
  Check,
  Download,
  Eye,
  Loader2,
  Pause,
  Play,
  TriangleAlert,
  X,
} from 'lucide-react';
import { BlurhashImg } from '@/components/BlurhashImg';
import { cn } from '@/lib/utils';
import type { ContentPhase } from '@/state/use-content-state';

interface PosterCardProps {
  title: string;
  year?: number;
  /** Optional short label rendered below the title in place of (or
   *  alongside) the year. Used by show cards to surface the episode
   *  a Play click will target ("S02E05 · Next up"). Keep this to
   *  ~16 chars so it doesn't wrap on narrow grids. */
  sublabel?: string;
  posterUrl?: string;
  /** BlurHash placeholder — rendered behind the poster until it loads. */
  blurhash?: string | null;
  quality?: string;
  phase?: ContentPhase;
  watchProgress?: number; // 0-100, continue watching
  downloadProgress?: number; // 0-100, download in progress
  downloadSpeed?: number; // bytes/s
  /** Compact "X/Y"-style summary for collection progress (used by
   *  TV show cards: "1/8" available/aired). Shown bottom-left in
   *  place of or alongside the available-check indicator. */
  progressBadge?: string;
  /** Show-only: total active-state downloads. When `> 1`, an "×N"
   *  segment is appended to the stats pill so users know multiple
   *  episodes are moving at once (and pause affects all of them). */
  activeDownloadCount?: number;
  onClick?: () => void;
  onPlay?: () => void;
  onAdd?: () => void;
  onRemove?: () => void;
  onPause?: () => void;
  onResume?: () => void;
  isAdding?: boolean;
}

function formatSpeed(bps: number): string {
  if (bps >= 1048576) return `${(bps / 1048576).toFixed(1)} MB/s`;
  if (bps >= 1024) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${bps} B/s`;
}

export function PosterCard({
  title,
  year,
  sublabel,
  posterUrl,
  blurhash,
  quality,
  phase = 'none',
  watchProgress,
  downloadProgress,
  downloadSpeed,
  progressBadge,
  activeDownloadCount,
  onClick,
  onPlay,
  onAdd,
  onRemove,
  onPause,
  onResume,
  isAdding,
}: PosterCardProps) {
  const isActiveDownload = phase === 'downloading' || phase === 'stalled';
  const isPaused = phase === 'paused';
  const isTransferring = isActiveDownload || isPaused;
  const isWaiting = phase === 'queued' || phase === 'searching' || phase === 'importing';
  // Play is the universal primary action — visible for every state
  // except the brief pre-torrent waits ('searching', 'queued') where
  // there's literally nothing to stream yet. Cards supply `onPlay` to
  // opt in; cards that can't resolve a sensible target (e.g. shows
  // with no aired episodes yet, upstream) simply omit it.
  const showPlay = !!onPlay && phase !== 'searching' && phase !== 'queued';
  const hasWatchBar =
    !isTransferring && watchProgress != null && watchProgress > 0 && watchProgress < 100;
  const inLibrary = phase !== 'none';
  const pct = downloadProgress ?? 0;

  return (
    // biome-ignore lint/a11y/useSemanticElements: the card contains overlay floats (play badge, library badge, watch progress bar) positioned absolutely against this wrapper — converting to <button> would inherit form-button defaults that fight the layout. role+tabIndex+onKeyDown gives the same a11y surface
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onClick?.();
        }
      }}
      className="group relative w-full cursor-pointer text-left overflow-visible"
    >
      {/* Outer — scales on hover */}
      <div className="relative aspect-poster rounded-lg ring-1 ring-white/5 transition-all duration-200 group-hover:ring-white/20 group-hover:scale-[1.03]">
        {/* Inner — clips content */}
        <div className="absolute inset-0 rounded-lg overflow-hidden bg-[var(--bg-card)]">
          {/* Poster image (with optional blurhash placeholder) */}
          {posterUrl ? (
            <BlurhashImg
              src={posterUrl}
              blurhash={blurhash}
              alt={title}
              className="w-full h-full"
            />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-[var(--text-muted)]">
              <svg
                width="40"
                height="40"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                role="img"
                aria-label="No poster"
              >
                <title>No poster available</title>
                <rect x="2" y="2" width="20" height="20" rx="2" />
                <path d="M10 9l5 3-5 3V9z" />
              </svg>
            </div>
          )}

          {/*
           * ── DOWNLOAD STATES: sweep overlay + progress pill + line ──
           * Applies to: downloading, stalled, paused
           */}
          {isTransferring && (
            <>
              {/* Sweep: dark overlay on un-downloaded portion (right side reveals) */}
              <div
                className="absolute inset-0 bg-black/40 transition-[clip-path] duration-700 ease-linear"
                style={{ clipPath: `inset(0 0 0 ${pct}%)` }}
              />

              {/* Frosted stats pill — bottom center, fixed width */}
              <div className="absolute inset-x-0 bottom-2 z-10 flex justify-center">
                <div className="flex items-center gap-2 px-2.5 py-1 rounded-full bg-black/60 backdrop-blur-md ring-1 ring-white/15 text-[10px] tabular-nums whitespace-nowrap">
                  <span className="font-bold text-white w-7 text-right">{pct}%</span>
                  <span className="text-white/30">·</span>
                  {isActiveDownload && (
                    <span className="flex items-center gap-0.5 text-white/70 w-[58px]">
                      <ArrowDown size={8} className="flex-shrink-0" />
                      {downloadSpeed != null && downloadSpeed > 0
                        ? formatSpeed(downloadSpeed)
                        : '-- MB/s'}
                    </span>
                  )}
                  {phase === 'stalled' && (!downloadSpeed || downloadSpeed === 0) && (
                    <span className="text-amber-400 font-medium text-[9px]">Stalled</span>
                  )}
                  {isPaused && (
                    <span className="text-amber-400 font-semibold text-[9px] tracking-wider uppercase">
                      Paused
                    </span>
                  )}
                  {/* Multi-torrent indicator — only when > 1 and
                      while transferring. Tells the user "pause-all
                      affects N torrents, not just the leader the
                      sweep is animating." Kept tiny so the pill
                      doesn't bloat when there's only one. */}
                  {activeDownloadCount != null && activeDownloadCount > 1 && (
                    <>
                      <span className="text-white/30">·</span>
                      <span className="text-white/80 font-semibold">×{activeDownloadCount}</span>
                    </>
                  )}
                </div>
              </div>

              {/* Progress line at very bottom */}
              <div className="absolute bottom-0 left-0 right-0 h-[3px]">
                <div
                  className={cn(
                    'h-full transition-all duration-700',
                    isPaused || phase === 'stalled' ? 'bg-amber-400' : 'bg-[var(--accent)]'
                  )}
                  style={{ width: `${pct}%` }}
                />
              </div>
            </>
          )}

          {/*
           * ── WAITING STATES: dim overlay + frosted pill ──
           * Applies to: searching, queued, importing, failed
           */}
          {(isWaiting || phase === 'failed') && (
            <>
              {/* Light dim */}
              {(phase === 'queued' || phase === 'failed') && (
                <div className="absolute inset-0 bg-black/30" />
              )}

              {/* Status pill — bottom right */}
              <div className="absolute bottom-2 right-2 z-10">
                <div className="flex items-center gap-1.5 h-6 px-2 rounded-full bg-black/60 backdrop-blur-sm ring-1 ring-white/15">
                  {(phase === 'searching' || phase === 'queued' || phase === 'importing') && (
                    <>
                      <div className="flex gap-[3px] dot-animation">
                        <span className="block w-1.5 h-1.5 rounded-full bg-white" />
                        <span className="block w-1.5 h-1.5 rounded-full bg-white" />
                        <span className="block w-1.5 h-1.5 rounded-full bg-white" />
                      </div>
                      {phase !== 'searching' && (
                        <span className="text-[9px] text-white/70 font-medium uppercase tracking-wider">
                          {phase === 'queued' ? 'Queued' : 'Importing'}
                        </span>
                      )}
                    </>
                  )}
                  {phase === 'failed' && (
                    <>
                      <TriangleAlert size={10} className="text-red-400" />
                      <span className="text-[9px] text-red-400 font-semibold uppercase tracking-wider">
                        Failed
                      </span>
                    </>
                  )}
                </div>
              </div>
            </>
          )}

          {/*
           * ── WATCH PROGRESS BAR ──
           * Shows on available/watched cards when partially watched
           */}
          {hasWatchBar && (
            <div className="absolute bottom-0 left-0 right-0 h-[3px] bg-white/15">
              <div className="h-full bg-[var(--accent)]" style={{ width: `${watchProgress}%` }} />
            </div>
          )}
        </div>

        {/*
         * ── TOP-RIGHT: Add or Remove (hover only) ──
         */}
        {!inLibrary && onAdd && (
          <button
            type="button"
            className={cn(
              'absolute -top-1.5 -right-1.5 z-30 w-7 h-7 rounded-full grid place-items-center shadow-lg transition-all opacity-0 group-hover:opacity-100',
              isAdding
                ? 'bg-[var(--accent)] text-white border border-[var(--accent)]'
                : 'bg-black/80 text-white border border-white/20 hover:bg-[var(--accent)] hover:border-[var(--accent)]'
            )}
            onClick={(e) => {
              e.stopPropagation();
              onAdd();
            }}
          >
            {isAdding ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
          </button>
        )}

        {inLibrary && onRemove && (
          <button
            type="button"
            className="absolute -top-1.5 -right-1.5 z-30 w-7 h-7 rounded-full bg-black/80 border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-opacity hover:bg-red-600 hover:border-red-600"
            onClick={(e) => {
              e.stopPropagation();
              onRemove();
            }}
          >
            <X size={13} strokeWidth={2.5} className="text-white" />
          </button>
        )}

        {/*
         * ── CENTER: Play button (hover). The container is
         * `pointer-events-none` so clicks on the rest of the card
         * pass through to the parent div's `onClick` (→ navigate to
         * detail); only the button itself intercepts. This fixes
         * the "click anywhere on the card and it starts playing"
         * UX — Play is explicit, everywhere else opens the detail.
         */}
        {showPlay && (
          <div className="absolute inset-0 z-10 grid place-items-center opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none">
            <button
              type="button"
              aria-label="Play"
              onClick={(e) => {
                e.stopPropagation();
                onPlay?.();
              }}
              className="pointer-events-auto w-12 h-12 rounded-full bg-white/90 grid place-items-center shadow-xl hover:scale-110 hover:bg-white transition"
            >
              <Play size={22} fill="black" stroke="black" className="ml-1" />
            </button>
          </div>
        )}

        {/*
         * ── TOP-LEFT: Pause / Resume (transfer states only) ──
         * Secondary affordance — keeps the center Play button as the
         * one-and-only primary verb while still giving users a way to
         * stop the background download without navigating into detail.
         */}
        {isActiveDownload && onPause && (
          <button
            type="button"
            aria-label="Pause download"
            className="absolute -top-1.5 -left-1.5 z-30 w-7 h-7 rounded-full bg-black/80 border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-[opacity,background-color] hover:bg-neutral-700 hover:border-white/40"
            onClick={(e) => {
              e.stopPropagation();
              onPause();
            }}
          >
            <Pause size={12} fill="white" className="text-white" />
          </button>
        )}
        {isPaused && onResume && (
          <button
            type="button"
            aria-label="Resume download"
            className="absolute -top-1.5 -left-1.5 z-30 w-7 h-7 rounded-full bg-black/80 border border-white/20 grid place-items-center shadow-lg opacity-0 group-hover:opacity-100 transition-[opacity,background-color] hover:bg-neutral-700 hover:border-white/40"
            onClick={(e) => {
              e.stopPropagation();
              onResume();
            }}
          >
            <Play size={12} fill="white" className="text-white ml-0.5" />
          </button>
        )}

        {/*
         * ── BOTTOM-RIGHT: Quality badge (available/watched only, not during transfer) ──
         */}
        {quality && !isTransferring && !isWaiting && phase !== 'none' && phase !== 'failed' && (
          <div className="absolute bottom-2 right-2 z-10 px-1.5 py-0.5 rounded bg-black/70 backdrop-blur-sm text-[10px] font-medium text-white">
            {quality}
          </div>
        )}

        {/*
         * ── BOTTOM-LEFT: Library state indicator ──
         * TV shows: "X/Y" progress pill replaces the binary check
         * because "one episode available" ≠ "whole show available".
         * Movies (no progressBadge): Available = green check,
         * Watched = eye icon.
         */}
        {progressBadge && !isTransferring && !isWaiting ? (
          <div className="absolute bottom-2 left-2 z-10 px-1.5 py-0.5 rounded bg-black/70 backdrop-blur-sm text-[10px] font-semibold text-white tabular-nums ring-1 ring-white/10">
            {progressBadge}
          </div>
        ) : (
          <>
            {phase === 'available' && (
              <div className="absolute bottom-2 left-2 z-10 w-6 h-6 rounded-full bg-black/60 backdrop-blur-sm ring-1 ring-white/15 grid place-items-center">
                <Check size={13} strokeWidth={3} className="text-green-400" />
              </div>
            )}
            {phase === 'watched' && (
              <div className="absolute bottom-2 left-2 z-10 w-6 h-6 rounded-full bg-black/60 backdrop-blur-sm ring-1 ring-white/15 grid place-items-center">
                <Eye size={12} className="text-blue-400" />
              </div>
            )}
          </>
        )}
      </div>

      {/* Title + year/sublabel below. Sublabel wins over year when
          present (show cards set it to surface the target episode);
          the year line is lower-priority context. */}
      <p className="mt-2 text-xs font-medium text-[var(--text-secondary)] truncate group-hover:text-white transition-colors">
        {title}
      </p>
      {sublabel ? (
        <p className="text-[10px] text-[var(--text-muted)] truncate">{sublabel}</p>
      ) : (
        year && <p className="text-[10px] text-[var(--text-muted)]">{year}</p>
      )}
    </div>
  );
}
