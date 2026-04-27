import type { CSSProperties } from 'react';
import { useState } from 'react';
import { cn } from '@/lib/utils';

/**
 * Clearlogo identity — two stacked `<img>` copies (dim back, bright
 * front with a clip-path sweep driven by `sweepGap`). Uses <img>
 * for both SVG and PNG responses from the backend `/logo` endpoint.
 *
 * No text fallback: when the logo 404s or errors, the space stays
 * empty. The user knows what they clicked — the text is redundant
 * next to the status line.
 *
 * Both images are always in the DOM so the container reserves its
 * space; opacity transitions handle the reveal so there's no
 * layout shift when the image lands.
 */

export interface VideoLogoProps {
  contentType: 'movies' | 'shows';
  entityId: number;
  /** Optimistic palette hint; reserved for future outline mode. */
  palette: string | null | undefined;
  /** Used for `aria-label` / `alt`. */
  title: string;
  className?: string;
  /** Percentage (e.g. `"35%"`) of the logo still hidden on the
   *  right. 100% = not yet revealed. 0% = fully revealed. */
  sweepGap: string;
}

export function VideoLogo({
  contentType,
  entityId,
  palette: _palette,
  title,
  className,
  sweepGap,
}: VideoLogoProps) {
  const [state, setState] = useState<'pending' | 'loaded' | 'error'>('pending');

  // Same-origin: the browser auto-attaches the `kino-session`
  // cookie on the `<img>` request. Cross-origin deploys go through
  // `mediaUrl()` which swaps in a signed URL — wired upstream by
  // the consumer that knows the deploy mode.
  const src = `/api/v1/images/${contentType}/${entityId}/logo`;
  const style: CSSProperties = { ['--sweep-gap' as never]: sweepGap } as CSSProperties;

  return (
    <div
      className={cn('video-logo', state === 'loaded' && 'video-logo--loaded', className)}
      aria-label={title}
      role="img"
      style={style}
    >
      {state !== 'error' && (
        <>
          <img
            className="video-logo__layer video-logo__layer--back"
            src={src}
            alt=""
            aria-hidden
            onLoad={() => setState('loaded')}
            onError={() => setState('error')}
          />
          <img
            className="video-logo__layer video-logo__layer--front"
            src={src}
            alt=""
            aria-hidden
          />
        </>
      )}
    </div>
  );
}
