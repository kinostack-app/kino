import { Info, Play } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

export interface HeroItem {
  /** TMDB id — the Home page uses this to navigate when the user clicks. */
  tmdbId?: number;
  kind?: 'movie' | 'show';
  title: string;
  year?: number;
  overview?: string | null;
  quality?: string;
  backdropUrl?: string;
}

interface HeroBannerProps {
  items: HeroItem[];
  /** Rotation interval in ms. 0 = no rotation. Default 8000. */
  interval?: number;
  /**
   * Called when the user clicks the Play button. Receives the active
   * hero item. The caller decides whether to jump to the detail page,
   * start an add-to-library flow, or play a real library entry.
   */
  onPlay?: (item: HeroItem) => void;
  /**
   * Called when the user clicks More Info. Same shape as `onPlay` —
   * usually a navigate to /movie or /show.
   */
  onMoreInfo?: (item: HeroItem) => void;
}

export function HeroBanner({ items, interval = 8000, onPlay, onMoreInfo }: HeroBannerProps) {
  const [activeIndex, setActiveIndex] = useState(0);
  // Pause rotation while the user hovers the banner — rotating the
  // backdrop mid-sentence while they read the overview is annoying.
  // We also pause when the tab is backgrounded so a carousel doesn't
  // burn through items nobody's looking at.
  const [paused, setPaused] = useState(false);
  const rootRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (items.length <= 1 || interval === 0 || paused) return;
    const timer = setInterval(() => {
      setActiveIndex((prev) => (prev + 1) % items.length);
    }, interval);
    return () => clearInterval(timer);
  }, [items.length, interval, paused]);

  // Clamp index if items shrink
  const index = activeIndex < items.length ? activeIndex : 0;
  const current = items[index];

  if (!current) {
    return (
      <section className="relative w-full h-[50vh] min-h-[320px] max-h-[700px] md:h-[70vh] md:min-h-[400px]">
        <div className="absolute inset-0 bg-gradient-to-br from-[#1a1a2e] via-[#16213e] to-[#0f3460]" />
        <div className="hero-gradient absolute inset-0" />
        <div className="absolute bottom-0 left-0 right-0 px-4 md:px-12 pb-12 md:pb-16">
          <h1 className="text-3xl md:text-5xl font-bold">Welcome to kino</h1>
          <p className="mt-3 text-[var(--text-secondary)]">Your personal media server</p>
        </div>
      </section>
    );
  }

  return (
    <section
      ref={rootRef}
      aria-label="Featured content"
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      className="relative w-full h-[50vh] min-h-[320px] max-h-[700px] md:h-[70vh] md:min-h-[400px]"
    >
      {/* Backdrop layers — all rendered, only active one visible */}
      {items.map((item, i) => (
        <div
          key={`${item.kind ?? 'x'}-${item.tmdbId ?? item.title}`}
          className={cn(
            'absolute inset-0 transition-opacity duration-1000',
            i === index ? 'opacity-100' : 'opacity-0'
          )}
        >
          {item.backdropUrl ? (
            <img
              src={item.backdropUrl}
              alt={item.title}
              className="absolute inset-0 w-full h-full object-cover"
            />
          ) : (
            <div className="absolute inset-0 bg-gradient-to-br from-[#1a1a2e] via-[#16213e] to-[#0f3460]" />
          )}
        </div>
      ))}

      {/* Gradient overlay */}
      <div className="hero-gradient absolute inset-0" />

      {/* Content — crossfades with backdrop */}
      <div className="absolute bottom-0 left-0 right-0 px-4 md:px-12 pb-12 md:pb-16">
        <div className="max-w-2xl">
          {current.quality && (
            <span className="inline-block px-2 py-0.5 mb-3 text-xs font-semibold rounded bg-[var(--accent)] text-white uppercase tracking-wide">
              {current.quality}
            </span>
          )}
          <h1
            key={current.title}
            className="text-3xl md:text-5xl lg:text-6xl font-bold tracking-tight leading-tight animate-fade-in"
          >
            {current.title}
          </h1>
          {current.year && (
            <p className="mt-2 text-lg text-[var(--text-secondary)]">{current.year}</p>
          )}
          {current.overview && (
            <p className="mt-3 text-sm md:text-base text-[var(--text-secondary)] line-clamp-3 max-w-lg">
              {current.overview}
            </p>
          )}
          <div className="mt-5 flex items-center gap-3">
            <button
              type="button"
              onClick={() => onPlay?.(current)}
              className="flex items-center gap-2 px-6 py-2.5 rounded-lg bg-white text-black font-semibold text-sm hover:bg-white/90 transition"
            >
              <Play size={18} fill="black" />
              Play
            </button>
            <button
              type="button"
              onClick={() => onMoreInfo?.(current)}
              className="flex items-center gap-2 px-5 py-2.5 rounded-lg bg-white/20 backdrop-blur text-white font-medium text-sm hover:bg-white/30 transition"
            >
              <Info size={18} />
              More Info
            </button>
          </div>
        </div>

        {/* Navigation dots */}
        {items.length > 1 && (
          <div className="flex items-center justify-center gap-2 mt-6">
            {items.map((item, i) => (
              <button
                key={`${item.kind ?? 'x'}-dot-${item.tmdbId ?? item.title}`}
                type="button"
                onClick={() => setActiveIndex(i)}
                className={cn(
                  'h-1 rounded-full transition-all duration-300',
                  i === index ? 'w-8 bg-white/70' : 'w-2 bg-white/20 hover:bg-white/40'
                )}
                aria-label={`Show ${item.title}`}
              />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
