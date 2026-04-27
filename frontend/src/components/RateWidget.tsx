/**
 * Trakt-style 10-star rating control.
 *
 * Trakt's scale is 1–10 whole points; one star = one point. Click
 * the same value to clear. Hover preview drives a brighter highlight
 * so the user sees what they'd commit before committing.
 *
 * Compact mode (`compact`) keeps the same 10-star model with a
 * smaller star size for inline use on episode cards. The Trakt mark
 * is hidden in compact mode (it's already shown on the parent
 * detail page).
 */

import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { rate } from '@/api/generated/sdk.gen';
import { cn } from '@/lib/utils';
import { LIBRARY_MOVIES_KEY, LIBRARY_SHOWS_KEY } from '@/state/library-cache';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

type Kind = 'movie' | 'show' | 'episode';

interface RateWidgetProps {
  kind: Kind;
  /** Local library entity id (movie.id / show.id / episode.id), not tmdb_id. */
  id: number;
  /** Current user rating; undefined/null means unrated. */
  value: number | null | undefined;
  /** Compact = small stars, no Trakt mark. For episode rows. */
  compact?: boolean;
  /** Hook called after the rating successfully persists. Used by the
   *  episode card to optimistically refresh its own watch-state cache. */
  onChanged?: (next: number | null) => void;
}

export function RateWidget({ kind, id, value, compact = false, onChanged }: RateWidgetProps) {
  const qc = useQueryClient();
  const [hover, setHover] = useState<number | null>(null);

  const saveMutation = useMutationWithToast({
    verb: 'save rating',
    mutationFn: async (next: number | null) => {
      await rate({
        path: { kind, id },
        body: { rating: next },
      });
    },
    onMutate: async (next) => {
      // Optimistic patch — library lists carry user_rating so the
      // visual update happens before the server round-trip. Only
      // movie / show caches are touched here; episode rating
      // optimistic updates flow through `onChanged` since their
      // shape lives inside per-show queries.
      const key = kind === 'show' ? LIBRARY_SHOWS_KEY : LIBRARY_MOVIES_KEY;
      if (kind === 'movie' || kind === 'show') {
        await qc.cancelQueries({ queryKey: [...key] });
        const prev = qc.getQueryData<Array<{ id: number; user_rating?: number | null }>>([...key]);
        if (prev) {
          qc.setQueryData(
            [...key],
            prev.map((row) => (row.id === id ? { ...row, user_rating: next } : row))
          );
        }
        return { prev, key };
      }
      return { prev: undefined, key } as { prev: undefined; key: typeof key };
    },
    onError: (_err, _next, ctx) => {
      if (ctx?.prev) qc.setQueryData([...ctx.key], ctx.prev);
    },
    onSettled: (_data, _err, next) => {
      // Backend emits `Rated` → meta dispatcher refreshes library
      // rows + Trakt recommendations + show-episodes. No manual
      // invalidations needed here — the optimistic patch above
      // already covered the snappy path.
      onChanged?.(next ?? null);
    },
  });

  const current = value ?? null;
  const displayed = hover ?? current ?? 0;
  const previewing = hover != null && hover !== current;

  const onPick = (n: number) => {
    // Click the same value to clear — self-explanatory without a
    // separate clear button.
    const next = n === current ? null : n;
    saveMutation.mutate(next);
  };

  const starSize = compact ? 18 : 26;
  const gap = compact ? 'gap-0.5' : 'gap-1';

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: onMouseLeave clears hover preview — purely visual, not an interactive control. Real interactivity is on the per-star <button> children.
    <div
      className={cn('inline-flex items-center', compact ? 'gap-2' : 'gap-3')}
      onMouseLeave={() => setHover(null)}
    >
      {!compact && <TraktMark />}
      {/* biome-ignore lint/a11y/useSemanticElements: <fieldset> would impose default border + spacing that conflict with the inline rating layout; role="group" is the documented escape hatch for grouping form controls without the fieldset visual */}
      <div className={cn('inline-flex', gap)} role="group" aria-label="Rate 1 to 10">
        {Array.from({ length: 10 }, (_, i) => {
          const n = i + 1;
          const filled = n <= displayed;
          return (
            <button
              key={n}
              type="button"
              disabled={saveMutation.isPending}
              aria-label={`Rate ${n} out of 10`}
              onMouseEnter={() => setHover(n)}
              onClick={() => onPick(n)}
              className={cn(
                'bg-transparent p-0 cursor-pointer disabled:cursor-not-allowed transition-transform hover:scale-110',
                filled
                  ? previewing
                    ? 'text-amber-300'
                    : 'text-amber-400'
                  : 'text-white/15 hover:text-white/30'
              )}
            >
              <Star size={starSize} filled={filled} />
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** Small Trakt circle-mark so users know where the rating goes. */
function TraktMark() {
  return (
    <a
      href="https://trakt.tv"
      target="_blank"
      rel="noopener noreferrer"
      title="Ratings sync to Trakt"
      className="inline-flex items-center opacity-60 hover:opacity-100 transition-opacity"
    >
      <img src="/trakt-mark.svg" alt="Trakt" className="h-5 w-5" />
    </a>
  );
}

/** SVG star — `filled=true` for solid, `filled=false` for outline.
 *  Centralised so the overlay + outline are pixel-aligned. */
function Star({ size, filled = false }: { size: number; filled?: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill={filled ? 'currentColor' : 'none'}
      stroke="currentColor"
      strokeWidth={filled ? 0 : 1.5}
      strokeLinejoin="round"
      strokeLinecap="round"
      aria-hidden
    >
      <title>Star</title>
      <path d="M12 2.5l3.0 6.4 7.0 0.9-5.1 4.8 1.3 7-6.2-3.4-6.2 3.4 1.3-7-5.1-4.8 7.0-0.9z" />
    </svg>
  );
}
