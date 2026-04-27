/**
 * "You were here" prompt shown when a library media has a meaningful
 * saved position and the user didn't arrive via a `?resume_at=` deep
 * link or the in-place stream→library handoff. Extracted from the
 * old Player route so PlayerRoot can render it without dragging the
 * whole library-only file along.
 */

import { ArrowLeft, Play, RotateCcw } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { tmdbImage } from '@/lib/api';

interface ResumeDialogProps {
  resumeSec: number;
  resumePct: number;
  /** Total runtime in seconds. When known, the dialog shows a
   *  "played / remaining" line next to the percentage so the
   *  user can tell how much of the film is left before deciding
   *  Resume vs Start over. Defaults to computing remaining from
   *  `resumeSec / resumePct` when unset. */
  durationSec?: number | null;
  title: string;
  /** Trickplay tile at the resume position. `undefined` while the
   *  VTT is still loading or when no trickplay was generated — the
   *  `backdropPath` fallback renders underneath, so the card is
   *  never an empty black rectangle. */
  thumbCue: { src: string; x: number; y: number; w: number; h: number } | undefined;
  /** TMDB backdrop path (e.g. `/abc.jpg`). Used as a fallback
   *  poster below the trickplay tile so the dialog looks
   *  cinematic even before (or without) trickplay. */
  backdropPath?: string | null;
  onResume: () => void;
  onStartOver: () => void;
  /** Navigate away without starting playback. Wired from
   *  PlayerRoot's `goBack` — mirrors the player's back button.
   *  Also fired on Escape / backdrop click so an accidental
   *  click doesn't reset the saved position (the previous
   *  behaviour was "start over", which destroyed progress). */
  onBack: () => void;
}

export function ResumeDialog({
  resumeSec,
  resumePct,
  durationSec,
  title,
  thumbCue,
  backdropPath,
  onResume,
  onStartOver,
  onBack,
}: ResumeDialogProps) {
  // Restore focus to whatever had it before the dialog opened,
  // so dismissing with Escape doesn't leave the user stranded
  // on the document root.
  const priorFocus = useRef<HTMLElement | null>(
    typeof document !== 'undefined' ? (document.activeElement as HTMLElement | null) : null
  );
  const resumeBtnRef = useRef<HTMLButtonElement | null>(null);
  const dialogRef = useRef<HTMLDivElement | null>(null);

  // Mount: focus the primary action. Unmount: return focus to
  // the trigger. This is the bare minimum a11y contract for a
  // modal — a proper focus trap would also cycle Tab inside
  // the dialog, but two buttons + the close-on-Escape handler
  // keep the trap surface small enough that basic focus
  // management suffices.
  useEffect(() => {
    resumeBtnRef.current?.focus();
    const captured = priorFocus.current;
    return () => {
      captured?.focus?.();
    };
  }, []);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="resume-dialog-title"
      ref={dialogRef}
      onClick={(e) => {
        // Backdrop click dismisses by going back — previously it
        // called `onStartOver` which silently reset the saved
        // position on a stray click. Navigating away is the safe
        // default: if the user actually wanted to start over,
        // that's what the explicit button is for.
        if (e.target === e.currentTarget) onBack();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onBack();
      }}
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/85 backdrop-blur-sm p-4"
    >
      <div className="w-[min(440px,100%)] rounded-2xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl overflow-hidden">
        <div className="relative w-full aspect-video bg-black/60 overflow-hidden">
          {/* Backdrop fallback layer — always renders when we have
              a TMDB path, underneath the trickplay tile. Keeps the
              card cinematic during the VTT-load window (and gives
              a sensible fallback for sources where trickplay never
              generated). */}
          {backdropPath && (
            <img
              src={tmdbImage(backdropPath, 'w780')}
              alt=""
              loading="eager"
              className="absolute inset-0 w-full h-full object-cover"
            />
          )}
          {/* Trickplay tile layer. Sprite sheet is 1600×900 (10×10
              grid of 160×90 tiles). The `<img>` stretches the whole
              sheet to 10× the container, then shifts via
              `left` / `top` so the selected tile lands in the
              visible viewport. `<img>` + `onError` gives us
              observability for sprite-fetch failures; a plain
              `<div>` with `background-image` can fail silently. */}
          {thumbCue && (
            <img
              src={thumbCue.src}
              alt=""
              loading="eager"
              style={{
                position: 'absolute',
                width: '1000%',
                height: '1000%',
                maxWidth: 'none',
                left: `-${(thumbCue.x / thumbCue.w) * 100}%`,
                top: `-${(thumbCue.y / thumbCue.h) * 100}%`,
              }}
            />
          )}
          <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-transparent to-black/20 pointer-events-none" />
          {/* Back control — lets the user bail without committing
              to play. Matches the player's back-button affordance
              so the dialog is never a trap. Same handler fires on
              Escape + backdrop click. */}
          <button
            type="button"
            onClick={onBack}
            aria-label="Go back"
            className="absolute top-3 left-3 w-9 h-9 grid place-items-center rounded-full bg-black/50 backdrop-blur-sm ring-1 ring-white/15 text-white/80 hover:text-white hover:bg-black/70 transition"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="absolute bottom-0 left-0 right-0 px-5 pb-4">
            <p className="text-[10px] uppercase tracking-wider text-[var(--accent)] font-semibold mb-1">
              Resume
            </p>
            <p id="resume-dialog-title" className="text-base font-semibold text-white truncate">
              {title}
            </p>
          </div>
        </div>

        <div className="px-5 py-4 space-y-4">
          <div>
            <div className="flex items-baseline justify-between gap-3 text-xs text-white/60 mb-2 tabular-nums">
              {/* "12:34 / 47:14" — played over total. Falls back
                  to just played when we don't know the runtime. */}
              <span>
                <span className="text-white/90">{formatResumeTime(resumeSec)}</span>
                {durationSec ? (
                  <span className="text-white/40"> / {formatResumeTime(durationSec)}</span>
                ) : null}
              </span>
              <span className="flex items-baseline gap-2">
                {/* Remaining time. Only shows when we know the
                    runtime — otherwise percentage alone is already
                    the "how much is left" signal. */}
                {durationSec && durationSec > resumeSec ? (
                  <span className="text-white/50">
                    −{formatResumeTime(durationSec - resumeSec)}
                  </span>
                ) : null}
                <span>{Math.round(resumePct * 100)}%</span>
              </span>
            </div>
            <div className="h-1.5 rounded-full bg-white/10 overflow-hidden">
              <div
                className="h-full bg-[var(--accent)] rounded-full"
                style={{ width: `${resumePct * 100}%` }}
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-2">
            <button
              ref={resumeBtnRef}
              type="button"
              onClick={onResume}
              className="px-4 py-2.5 rounded-lg bg-white text-black font-semibold hover:bg-white/90 transition inline-flex items-center justify-center gap-2"
            >
              <Play size={14} fill="currentColor" />
              Resume
            </button>
            <button
              type="button"
              onClick={onStartOver}
              className="px-4 py-2.5 rounded-lg bg-white/10 text-white font-medium hover:bg-white/20 transition inline-flex items-center justify-center gap-2"
            >
              <RotateCcw size={14} />
              Start over
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function formatResumeTime(seconds: number): string {
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => n.toString().padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}
