import { Link } from '@tanstack/react-router';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { Children, isValidElement, type ReactNode, useRef, useState } from 'react';

interface MediaRowProps {
  title: string;
  children: ReactNode;
  /**
   * Client-side route for the row header link. Kept as a single
   * string since most callers use plain paths like `/discover?tab=x`;
   * TanStack Router's Link handles the query-string fine with `to` +
   * `search` but the simpler form is preserved here via Link's `to`
   * + no-preload. Pass a leading slash.
   */
  href?: string;
  /**
   * Optional small node rendered before the title — used by the Lists
   * subsystem to stamp a source-origin mark (Trakt circle, MDBList /
   * TMDB pill) so users can see which rows are list-sourced at a
   * glance without a separate "Your lists" section header.
   */
  titlePrefix?: ReactNode;
}

export function MediaRow({ title, children, href, titlePrefix }: MediaRowProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [canScrollLeft, setCanScrollLeft] = useState(false);
  const [canScrollRight, setCanScrollRight] = useState(true);

  const scroll = (direction: 'left' | 'right') => {
    const el = scrollRef.current;
    if (!el) return;
    const amount = el.clientWidth * 0.8;
    el.scrollBy({ left: direction === 'left' ? -amount : amount, behavior: 'smooth' });
  };

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    setCanScrollLeft(el.scrollLeft > 10);
    setCanScrollRight(el.scrollLeft < el.scrollWidth - el.clientWidth - 10);
  };

  return (
    <section className="relative py-4">
      {/* Row title — inset from edge */}
      <div className="px-6 md:px-12 mb-2 flex items-center justify-between">
        <div className="flex items-center gap-2 min-w-0">
          {titlePrefix}
          {href ? (
            <Link
              to={href}
              className="text-base md:text-lg font-semibold text-white hover:text-[var(--text-secondary)] transition-colors truncate"
            >
              {title}
              <ChevronRight
                size={16}
                className="inline ml-1 opacity-0 group-hover/row:opacity-100"
              />
            </Link>
          ) : (
            <h2 className="text-base md:text-lg font-semibold text-white truncate">{title}</h2>
          )}
        </div>
      </div>

      {/* Scroll container goes edge-to-edge. A leading spacer provides the
          initial gutter so the first card doesn't touch the screen edge,
          while cards can still bleed off both edges during scroll — the
          hint that there's more content. */}
      <div className="relative group/row">
        {/* Left chevron */}
        {canScrollLeft && (
          <button
            type="button"
            onClick={() => scroll('left')}
            className="hidden md:flex absolute left-3 top-1/2 -translate-y-1/2 z-10 w-10 h-10 items-center justify-center rounded-full bg-black/60 backdrop-blur text-white opacity-0 group-hover/row:opacity-100 transition-opacity hover:bg-black/80"
          >
            <ChevronLeft size={20} />
          </button>
        )}

        {/* Right chevron */}
        {canScrollRight && (
          <button
            type="button"
            onClick={() => scroll('right')}
            className="hidden md:flex absolute right-3 top-1/2 -translate-y-1/2 z-10 w-10 h-10 items-center justify-center rounded-full bg-black/60 backdrop-blur text-white opacity-0 group-hover/row:opacity-100 transition-opacity hover:bg-black/80"
          >
            <ChevronRight size={20} />
          </button>
        )}

        <div
          ref={scrollRef}
          onScroll={onScroll}
          className="flex gap-3 pt-3 pb-2 -mt-3 -mb-2 overflow-x-auto overflow-y-visible scrollbar-hide snap-x"
          style={{ scrollPaddingLeft: '1.5rem' }}
        >
          {/* Leading gutter spacer — keeps first card away from screen edge */}
          <div className="flex-shrink-0 w-6 md:w-12" aria-hidden="true" />

          {Children.map(children, (child, i) => {
            // Reuse the child's own key for the wrapper so reconciliation
            // lines up when the list reorders.
            const key = isValidElement(child) && child.key != null ? child.key : `slot-${i}`;
            return (
              <div
                key={key}
                className="flex-shrink-0 w-[140px] sm:w-[160px] md:w-[180px] snap-start"
              >
                {child}
              </div>
            );
          })}

          {/* Trailing spacer so last card has a small gutter when scrolled fully right */}
          <div className="flex-shrink-0 w-6 md:w-12" aria-hidden="true" />
        </div>
      </div>
    </section>
  );
}
