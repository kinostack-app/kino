/**
 * kinoToast — unified toast API across the app.
 *
 * Thin wrapper over sonner that enforces a single visual language
 * across six intents (success / info / warning / error / cancelled /
 * hero). Every variant renders the same card skeleton: left-aligned
 * icon, title, optional description, optional action button,
 * dismiss affordance top-right. Widths, radii, fonts, and dismiss
 * behaviour are all defined here so call sites can't drift.
 *
 * Hero (import ready) + failure (download errored) keep their
 * richer layouts but conform to the header/dismiss conventions.
 *
 * Every call returns the toast id so callers can programmatically
 * dismiss or upgrade a pending toast.
 */

import { AlertTriangle, Check, CircleAlert, Copy, Info, Play, X, XCircle } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';

type Variant = 'success' | 'info' | 'warning' | 'error' | 'cancelled';

interface BaseOpts {
  description?: string;
  action?: { label: string; onClick: () => void };
  /** Sonner's de-dup key. Same id collapses duplicate toasts. */
  id?: string | number;
  /** Override the variant's default duration (ms). */
  duration?: number;
}

/** Default durations per variant — calibrated to the weight of the
 *  message. Cancelled is deliberately brief; errors linger so the
 *  user can read + act. */
const DEFAULT_DURATION: Record<Variant, number> = {
  success: 4000,
  info: 5000,
  warning: 6000,
  error: 8000,
  cancelled: 2500,
};

/** Ring colour per variant — the one visual signal that differs,
 *  everything else is uniform. Keep this sparse: colour is for
 *  categorising, not decorating. */
const RING_CLASS: Record<Variant, string> = {
  success: 'ring-emerald-500/30',
  info: 'ring-white/10',
  warning: 'ring-amber-500/40',
  error: 'ring-red-500/40',
  cancelled: 'ring-white/10',
};

/** Icon + icon colour per variant. */
function VariantIcon({ variant }: { variant: Variant }) {
  const cls = 'flex-shrink-0 mt-0.5';
  switch (variant) {
    case 'success':
      return <Check size={16} className={cn(cls, 'text-emerald-400')} />;
    case 'info':
      return <Info size={16} className={cn(cls, 'text-[var(--accent)]')} />;
    case 'warning':
      return <AlertTriangle size={16} className={cn(cls, 'text-amber-400')} />;
    case 'error':
      return <XCircle size={16} className={cn(cls, 'text-red-400')} />;
    case 'cancelled':
      return <CircleAlert size={16} className={cn(cls, 'text-[var(--text-muted)]')} />;
  }
}

function ToastCard({
  variant,
  title,
  description,
  action,
  toastId,
}: {
  variant: Variant;
  title: string;
  description?: string;
  action?: { label: string; onClick: () => void };
  toastId: string | number;
}) {
  return (
    <div
      className={cn(
        'w-[380px] max-w-[92vw] rounded-xl bg-[var(--bg-secondary)] ring-1 shadow-2xl overflow-hidden',
        RING_CLASS[variant]
      )}
    >
      <div className="px-4 py-3 flex items-start gap-3">
        <VariantIcon variant={variant} />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-white leading-snug">{title}</p>
          {description && (
            <p className="mt-0.5 text-xs text-[var(--text-muted)] leading-relaxed break-words">
              {description}
            </p>
          )}
          {action && (
            <button
              type="button"
              onClick={() => {
                toast.dismiss(toastId);
                action.onClick();
              }}
              className="mt-2 inline-flex items-center px-2.5 py-1 rounded-md bg-white/10 hover:bg-white/15 text-xs font-medium text-white transition"
            >
              {action.label}
            </button>
          )}
        </div>
        <button
          type="button"
          onClick={() => toast.dismiss(toastId)}
          aria-label="Dismiss"
          className="p-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white flex-shrink-0"
        >
          <X size={12} />
        </button>
      </div>
    </div>
  );
}

function makeVariant(variant: Variant) {
  return (title: string, opts?: BaseOpts): string | number => {
    const duration = opts?.duration ?? DEFAULT_DURATION[variant];
    // `unstyled: true` is on the Toaster too, but pass it per-call as
    // belt-and-suspenders — older Sonner builds didn't propagate the
    // global option to `toast.custom()` and the default wrapper would
    // leak its background + padding through our card's rounded corners.
    return toast.custom(
      (toastId) => (
        <ToastCard
          variant={variant}
          title={title}
          description={opts?.description}
          action={opts?.action}
          toastId={toastId}
        />
      ),
      { id: opts?.id, duration, unstyled: true }
    );
  };
}

// ── Hero (import-ready) ─────────────────────────────────────────

interface HeroOpts {
  title: string;
  quality: string | null;
  movieId: number | undefined;
  episodeId: number | undefined;
  showId: number | undefined;
}

function showHeroToast(p: HeroOpts): string | number {
  const posterUrl = posterUrlFor(p.movieId, p.showId);
  // Unified player route is `/play/$kind/$entityId` — `media_id` is
  // a `media` table id (file row), not a routable entity. Use the
  // movie/episode id as appropriate.
  const playHref =
    p.movieId != null
      ? `/play/movie/${p.movieId}`
      : p.episodeId != null
        ? `/play/episode/${p.episodeId}`
        : null;
  return toast.custom(
    (toastId) => (
      <div className="w-[380px] max-w-[92vw] flex rounded-xl bg-[var(--bg-secondary)] ring-1 ring-[var(--accent)]/30 shadow-2xl overflow-hidden">
        {posterUrl && (
          <div className="w-[72px] flex-shrink-0 self-stretch bg-black/40">
            <img
              src={posterUrl}
              alt=""
              className="w-full h-full object-cover"
              onError={(e) => {
                (e.currentTarget as HTMLImageElement).style.display = 'none';
              }}
            />
          </div>
        )}
        <div className="flex-1 min-w-0 p-3 flex flex-col gap-2">
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">
              <p className="text-[11px] font-semibold uppercase tracking-wider text-[var(--accent)]">
                Ready to play
              </p>
              <p className="text-sm font-semibold text-white truncate">{p.title}</p>
              {p.quality && (
                <p className="text-xs text-[var(--text-muted)] mt-0.5 truncate">{p.quality}</p>
              )}
            </div>
            <button
              type="button"
              onClick={() => toast.dismiss(toastId)}
              aria-label="Dismiss"
              className="p-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white flex-shrink-0"
            >
              <X size={12} />
            </button>
          </div>
          {playHref && (
            <button
              type="button"
              onClick={() => {
                toast.dismiss(toastId);
                window.location.assign(playHref);
              }}
              className="self-start inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-xs font-semibold"
            >
              <Play size={12} fill="white" stroke="white" />
              Play now
            </button>
          )}
        </div>
      </div>
    ),
    { duration: 8000, unstyled: true }
  );
}

// ── Failure card (download errored, actionable) ─────────────────

interface FailureOpts {
  title: string;
  error: string;
  downloadId: number | undefined;
  movieId: number | undefined;
  episodeId: number | undefined;
  showId: number | undefined;
}

function showFailureToast(p: FailureOpts): string | number {
  const alternateHref = alternateReleasesHrefFor(p);
  return toast.custom(
    (toastId) => <FailureCardBody params={p} alternateHref={alternateHref} toastId={toastId} />,
    { duration: 15000, unstyled: true }
  );
}

function FailureCardBody({
  params,
  alternateHref,
  toastId,
}: {
  params: FailureOpts;
  alternateHref: string | null;
  toastId: string | number;
}) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard
      .writeText(params.error)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {});
  };
  return (
    <div className="w-[380px] max-w-[92vw] rounded-xl bg-[var(--bg-secondary)] ring-1 ring-red-500/40 shadow-2xl overflow-hidden">
      <div className="px-4 py-3 flex items-start gap-3">
        <XCircle size={16} className="flex-shrink-0 mt-0.5 text-red-400" />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-white leading-snug">Download failed</p>
          <p className="mt-0.5 text-xs text-[var(--text-muted)] truncate">{params.title}</p>
        </div>
        <button
          type="button"
          onClick={() => toast.dismiss(toastId)}
          aria-label="Dismiss"
          className="p-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white flex-shrink-0"
        >
          <X size={12} />
        </button>
      </div>
      <div className="px-4 pb-3">
        <div className="relative rounded-md bg-red-950/40 ring-1 ring-red-500/20 px-3 py-2 pr-9">
          <p className="text-xs text-amber-100 font-mono leading-relaxed select-text whitespace-pre-wrap break-words max-h-24 overflow-auto">
            {params.error}
          </p>
          <button
            type="button"
            onClick={copy}
            aria-label={copied ? 'Copied' : 'Copy error'}
            className="absolute top-1.5 right-1.5 w-6 h-6 grid place-items-center rounded bg-white/5 hover:bg-white/15 text-white/70 hover:text-white transition"
          >
            {copied ? <Check size={12} /> : <Copy size={12} />}
          </button>
        </div>
      </div>
      {alternateHref && (
        <div className="px-4 pb-3 flex justify-end">
          <button
            type="button"
            onClick={() => {
              toast.dismiss(toastId);
              window.location.assign(alternateHref);
            }}
            className="px-3 py-1.5 rounded-md bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-xs font-semibold"
          >
            Pick alternate
          </button>
        </div>
      )}
    </div>
  );
}

// ── Public surface ──────────────────────────────────────────────

export const kinoToast = {
  success: makeVariant('success'),
  info: makeVariant('info'),
  warning: makeVariant('warning'),
  error: makeVariant('error'),
  cancelled: makeVariant('cancelled'),
  hero: showHeroToast,
  failure: showFailureToast,
  dismiss: toast.dismiss,
};

// ── Helpers ─────────────────────────────────────────────────────

function posterUrlFor(movieId: number | undefined, showId: number | undefined): string | null {
  // Same-origin: cookie auto-sends. Cross-origin deploys would hit
  // `mediaUrl()` upstream — the toast images don't carry headers.
  if (movieId != null) return `/api/v1/images/movies/${movieId}/poster`;
  if (showId != null) return `/api/v1/images/shows/${showId}/poster`;
  return null;
}

function alternateReleasesHrefFor(p: FailureOpts): string | null {
  // Prefer the most specific detail page — that's where the user
  // finds the Releases dialog + alternate-grab UX. Episode-level
  // detail doesn't have its own route, so episode grabs land on the
  // parent show's page.
  if (p.movieId != null) return `/movies/${p.movieId}`;
  if (p.showId != null) return `/shows/${p.showId}`;
  return null;
}
