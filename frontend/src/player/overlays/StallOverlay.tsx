import { VideoLogo } from '@/components/VideoLogo';
import type { StallOverlay as StallOverlayData } from '../types';

/**
 * Mid-stream recovery overlay. Rendered only when
 * playback has actually stalled (video in `waiting` state
 * after first play) AND the caller has given us a stall
 * reason — i.e. we know WHY the data stopped and can
 * offer a fix. Built-in Media Chrome loading spinner
 * handles the transient "just a segment hiccup" case;
 * this is for the "the bitstream is never coming back
 * unless you click something" case.
 */
export function StallOverlayView({ overlay }: { overlay: StallOverlayData }) {
  return (
    <div className="absolute inset-0 z-30 flex items-center justify-center bg-black/70 backdrop-blur-sm p-8 pointer-events-auto">
      <div className="flex flex-col items-center gap-5 max-w-md text-center">
        {overlay.logo && (
          <div className="w-full max-w-sm">
            <VideoLogo
              contentType={overlay.logo.contentType}
              entityId={overlay.logo.entityId}
              palette={overlay.logo.palette}
              title={overlay.title}
              sweepGap="0%"
              className="w-full"
            />
          </div>
        )}
        <p className="text-sm text-white/85 font-medium tracking-wide">{overlay.message}</p>
        <button
          type="button"
          onClick={overlay.action.onClick}
          className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold"
        >
          {overlay.action.label}
        </button>
      </div>
    </div>
  );
}
