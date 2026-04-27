import { useCallback, useState } from 'react';
import type { TrickplayCue } from '@/hooks/useTrickplay';
import { useTrickplay } from '@/hooks/useTrickplay';
import { cn } from '@/lib/utils';
import type { VideoSource } from '../types';
import { formatTime } from '../types';

/**
 * Custom scrubber with layered feedback:
 *
 *   background track
 *   └ extra layer (torrent download stripe — optional)
 *     └ buffered range
 *       └ progress
 *         └ thumb (visible on hover / scrub)
 *
 * Trickplay hover tooltip resolves three states from the
 * `useTrickplay` hook + source coverage:
 *
 *   1. **Cue exists** → real thumbnail + timestamp
 *   2. **No cues yet** → skeleton + spinner (streaming
 *      source, still waiting on first sprite sheet)
 *   3. **Past covered range** → time + "Generating…" hint
 *      (streaming source, later part of file not sheeted
 *      yet)
 *
 * Pointer-capture drag keeps the scrub alive even when the
 * cursor leaves the thin track. Keyboard arrows on focus
 * don't fight the global hotkey — stopPropagation on the
 * track's key handler.
 */
export function ScrubBar({
  source,
  currentTime,
  duration,
  buffered,
  seekExtraLayer,
  onSeek,
}: {
  source: VideoSource | null;
  currentTime: number;
  duration: number;
  buffered: number;
  /** Optional extra layer drawn beneath the buffered bar
   *  — used by the torrent wrapper for its
   *  download-percent stripe. */
  seekExtraLayer?: React.ReactNode;
  onSeek: (sourceSec: number) => void;
}) {
  const [hoverPct, setHoverPct] = useState<number | null>(null);
  const [isScrubbing, setIsScrubbing] = useState(false);

  const trickplay = useTrickplay(source?.trickplayUrl ?? null, {
    refreshSignal: source?.trickplayRefreshSignal ?? 0,
  });

  const seekTo = useCallback(
    (clientX: number, rect: DOMRect) => {
      if (!duration) return;
      const pct = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
      onSeek(pct * duration);
    },
    [duration, onSeek]
  );

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    setIsScrubbing(true);
    seekTo(e.clientX, el.getBoundingClientRect());
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    const rect = el.getBoundingClientRect();
    const pct = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    setHoverPct(pct);
    if (isScrubbing && el.hasPointerCapture(e.pointerId)) {
      seekTo(e.clientX, rect);
    }
  };
  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
    setIsScrubbing(false);
  };

  return (
    <div
      role="slider"
      tabIndex={0}
      aria-label="Seek"
      aria-valuemin={0}
      aria-valuemax={Math.floor(duration)}
      aria-valuenow={Math.floor(currentTime)}
      className={cn(
        'group/seek relative h-1 transition-all cursor-pointer mb-3 touch-none',
        isScrubbing ? 'h-2' : 'hover:h-2'
      )}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onPointerLeave={() => {
        if (!isScrubbing) setHoverPct(null);
      }}
      onKeyDown={(e) => {
        // Don't fight the global arrow-seek hotkey.
        if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') e.stopPropagation();
      }}
    >
      <div className="absolute inset-0 rounded-full bg-white/20" />
      {seekExtraLayer}
      <div
        className="absolute inset-y-0 left-0 rounded-full bg-white/30"
        style={{ width: duration ? `${(buffered / duration) * 100}%` : '0%' }}
      />
      <div
        className="absolute inset-y-0 left-0 rounded-full bg-[var(--accent)]"
        style={{ width: duration ? `${(currentTime / duration) * 100}%` : '0%' }}
      />
      <div
        className={cn(
          'absolute top-1/2 -translate-y-1/2 w-3 h-3 rounded-full bg-[var(--accent)] transition-opacity',
          isScrubbing ? 'opacity-100' : 'opacity-0 group-hover/seek:opacity-100'
        )}
        style={{ left: duration ? `calc(${(currentTime / duration) * 100}% - 6px)` : '0' }}
      />

      {trickplay && hoverPct !== null && duration > 0 && (
        <TrickplayHover
          hoverPct={hoverPct}
          duration={duration}
          cue={trickplay.cueAt(hoverPct * duration)}
          hasCues={trickplay.cues.length > 0}
        />
      )}
    </div>
  );
}

function TrickplayHover({
  hoverPct,
  duration,
  cue,
  hasCues,
}: {
  hoverPct: number;
  duration: number;
  cue: TrickplayCue | undefined;
  hasCues: boolean;
}) {
  const t = hoverPct * duration;

  if (cue) {
    // State 1: real thumbnail.
    return (
      <div
        className="absolute bottom-[calc(100%+12px)] pointer-events-none"
        style={{
          // Clamp to seek-bar width so hovering extremes
          // doesn't push the tooltip off screen.
          left: `clamp(8px, calc(${hoverPct * 100}% - ${cue.w / 2}px), calc(100% - ${cue.w + 8}px))`,
          width: cue.w,
          height: cue.h + 18,
        }}
      >
        <div
          className="rounded-md ring-1 ring-white/30 overflow-hidden shadow-lg"
          style={{
            width: cue.w,
            height: cue.h,
            backgroundImage: `url(${cue.src})`,
            backgroundPosition: `-${cue.x}px -${cue.y}px`,
            backgroundRepeat: 'no-repeat',
          }}
        />
        <div className="text-center text-xs text-white/80 mt-1 tabular-nums">{formatTime(t)}</div>
      </div>
    );
  }

  if (!hasCues) {
    // State 2: no coverage yet — skeleton with spinner.
    // Fixed 160×90 matches library thumbnail default so
    // the tooltip doesn't jump once real sprites arrive.
    return (
      <div
        className="absolute bottom-[calc(100%+12px)] pointer-events-none"
        style={{
          left: `clamp(8px, calc(${hoverPct * 100}% - 80px), calc(100% - 168px))`,
          width: 160,
          height: 90 + 18,
        }}
      >
        <div
          className="rounded-md ring-1 ring-white/10 bg-white/5 grid place-items-center"
          style={{ width: 160, height: 90 }}
        >
          <div className="w-4 h-4 border-2 border-white/20 border-t-white/70 rounded-full animate-spin" />
        </div>
        <div className="text-center text-xs text-white/80 mt-1 tabular-nums">{formatTime(t)}</div>
      </div>
    );
  }

  // State 3: past the covered range.
  return (
    <div
      className="absolute bottom-[calc(100%+12px)] pointer-events-none"
      style={{
        left: `clamp(8px, calc(${hoverPct * 100}% - 50px), calc(100% - 108px))`,
        width: 100,
      }}
    >
      <div className="text-center text-xs text-white/80 tabular-nums">{formatTime(t)}</div>
      <div className="text-center text-[10px] text-white/40 mt-0.5">Generating…</div>
    </div>
  );
}
