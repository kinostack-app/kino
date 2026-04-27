import { Play } from 'lucide-react';
import type { ReactNode } from 'react';
import { ContentActions } from '@/components/ContentActions';
import type { ContentState } from '@/state/use-content-state';

interface MetaItem {
  icon?: ReactNode;
  label: string;
  href?: string;
}

interface DetailLayoutProps {
  title: string;
  tagline?: string;
  overview?: string;
  backdropUrl?: string;
  posterUrl?: string;
  meta: MetaItem[];
  /** Optional status-style chips rendered inline at the end of the
   *  meta row (e.g. "Returning Series"). Keeps colored status pills
   *  aligned with year / rating / runtime instead of floating under
   *  the hero. */
  badges?: ReactNode;
  /** When present, the right-side action row (`ContentActions`) is
   *  rendered with these handlers. Omit for surfaces whose actions
   *  live entirely on the poster (e.g. movie detail) — no row at all
   *  shows up. */
  state?: ContentState;
  onAddOverride?: () => void;
  addLabel?: string;
  /** Show-only: when followed, clicking this opens the Manage
   *  dialog. Passed through to ContentActions where it sits next to
   *  Remove in the primary action row. */
  onManageDownloads?: () => void;
  /** Show-only: next unwatched aired episode. Renders as a primary
   *  Play button + caption inside the action row. */
  nextUp?: {
    label: string;
    caption?: string;
    onPlay: () => void;
  };
  /** Replace the default poster rendering (plain img / Next-Up
   *  overlay) with a fully-custom element — typically a
   *  `<PosterCard>` so the poster carries every in-library control
   *  (Play, Add, Remove, Pause/Resume, download progress). When set,
   *  `posterUrl` / `nextUp` are ignored; the caller owns the slot. */
  poster?: ReactNode;
  /** Optional slot rendered between the action row and the overview.
   *  Used for the rate widget + Next Up card so they sit with the
   *  title block instead of being banished below the page. */
  belowActions?: ReactNode;
  children?: ReactNode;
}

export function DetailLayout({
  title,
  tagline,
  overview,
  backdropUrl,
  posterUrl,
  meta,
  badges,
  state,
  onAddOverride,
  addLabel,
  onManageDownloads,
  nextUp,
  poster,
  belowActions,
  children,
}: DetailLayoutProps) {
  return (
    <div className="min-h-screen pb-24 md:pb-8">
      {/* Backdrop */}
      <div className="relative h-[55vh] min-h-[350px] max-h-[600px]">
        {backdropUrl ? (
          <img src={backdropUrl} alt="" className="absolute inset-0 w-full h-full object-cover" />
        ) : (
          <div className="absolute inset-0 bg-gradient-to-br from-[#1a1a2e] via-[#16213e] to-[#0f3460]" />
        )}
        <div className="hero-gradient absolute inset-0" />
      </div>

      {/* Content */}
      <div className="relative z-10 -mt-48 px-4 md:px-12">
        <div className="flex flex-col md:flex-row gap-6 lg:gap-10 max-w-6xl">
          {/* Poster. Priority: explicit `poster` override (movie
              detail passes a full PosterCard here) > Next Up overlay
              (show detail with an episode to play) > plain img. */}
          {poster ? (
            <div className="flex-shrink-0 hidden md:block w-52 lg:w-60">{poster}</div>
          ) : (
            posterUrl && (
              <div className="flex-shrink-0 hidden md:block">
                {nextUp ? (
                  <button
                    type="button"
                    onClick={nextUp.onPlay}
                    aria-label={`Play ${nextUp.label}`}
                    className="group block w-52 lg:w-60 text-left cursor-pointer"
                  >
                    <div className="relative aspect-poster rounded-xl ring-1 ring-white/10 shadow-2xl transition-all duration-200 group-hover:ring-white/20 group-hover:scale-[1.02]">
                      <div className="absolute inset-0 rounded-xl overflow-hidden bg-[var(--bg-card)]">
                        <img src={posterUrl} alt={title} className="w-full h-full object-cover" />
                        {/* Next Up overlay — pinned to the bottom of
                          the poster so the whole control reads as a
                          single integrated card. Solid semi-
                          transparent black + a backdrop blur keeps
                          the text legible against any underlying
                          poster art. */}
                        <div className="absolute inset-x-0 bottom-0 p-3 bg-black/75 backdrop-blur-sm">
                          <p className="text-[10px] uppercase tracking-wider text-[var(--accent)] font-semibold">
                            Next up
                          </p>
                          <p className="text-sm font-medium text-white truncate">{nextUp.label}</p>
                          {nextUp.caption && (
                            <p className="text-[11px] text-white/70 mt-0.5 truncate">
                              {nextUp.caption}
                            </p>
                          )}
                        </div>
                        {/* Center Play on hover — mirrors PosterCard */}
                        <div className="absolute inset-0 grid place-items-center opacity-0 group-hover:opacity-100 transition-opacity bg-black/30">
                          <div className="w-14 h-14 rounded-full bg-white/90 grid place-items-center shadow-xl group-hover:scale-110 transition">
                            <Play size={26} fill="black" stroke="black" className="ml-1" />
                          </div>
                        </div>
                      </div>
                    </div>
                  </button>
                ) : (
                  <img
                    src={posterUrl}
                    alt={title}
                    className="w-52 lg:w-60 rounded-xl shadow-2xl ring-1 ring-white/10"
                  />
                )}
              </div>
            )
          )}

          {/* Info */}
          <div className="flex-1 min-w-0">
            <h1 className="text-3xl md:text-4xl lg:text-5xl font-bold tracking-tight leading-tight">
              {title}
            </h1>

            {tagline && <p className="mt-1 text-[var(--text-muted)] italic text-sm">{tagline}</p>}

            {/* Meta row */}
            <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-[var(--text-secondary)]">
              {meta.map((m) =>
                m.href ? (
                  <a
                    key={m.label}
                    href={m.href}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="flex items-center gap-1 text-[var(--text-muted)] hover:text-white transition"
                  >
                    {m.icon}
                    {m.label}
                  </a>
                ) : (
                  <span key={m.label} className="flex items-center gap-1">
                    {m.icon}
                    {m.label}
                  </span>
                )
              )}
              {badges}
            </div>

            {/* Actions row — only rendered when a `state` is passed.
                Movie detail omits this entirely; all its controls
                live on the poster via the `poster` override. When
                `poster` IS provided (show detail), auto-hide Remove
                from the action row — the poster's × already carries
                that job. */}
            {state && (
              <div className="mt-4">
                <ContentActions
                  state={state}
                  onAddOverride={onAddOverride}
                  addLabel={addLabel}
                  title={title}
                  onManageDownloads={onManageDownloads}
                  hideRemove={!!poster}
                />
              </div>
            )}

            {/* Optional inline slot — used for the rate widget so it
                sits next to the action row instead of further down
                the page. */}
            {belowActions && <div className="mt-5">{belowActions}</div>}

            {/* Overview */}
            {overview && (
              <div className="mt-4">
                <h2 className="text-xs font-semibold text-[var(--text-muted)] uppercase tracking-wider mb-1.5">
                  Overview
                </h2>
                <p className="text-sm text-[var(--text-secondary)] leading-relaxed max-w-2xl">
                  {overview}
                </p>
              </div>
            )}
          </div>
        </div>

        {/* Extra content (seasons, recommendations, etc.) */}
        {children}
      </div>
    </div>
  );
}
