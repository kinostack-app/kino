import { VideoLogo } from '@/components/VideoLogo';
import { cn } from '@/lib/utils';
import type { LoadingOverlay as LoadingOverlayData } from '../types';
import { ErrorCard } from './ErrorCard';
import { StageStepper } from './StageStepper';

/**
 * Identity-reveal overlay — the journey card shown while
 * resolving a source (finding release, downloading
 * metadata, transcoding first segments). Fades out when
 * playback actually produces frames.
 *
 * Keeps its own mount lifecycle so the logo's clip-path
 * sweep can complete after first-play before unmounting —
 * the `fading` prop triggers a CSS opacity transition and
 * the parent handles the unmount after the transition
 * window.
 */
export function LoadingOverlayView({
  overlay,
  fading,
}: {
  overlay: LoadingOverlayData;
  fading: boolean;
}) {
  return (
    <div
      className={cn(
        'absolute inset-0 z-20 flex flex-col items-center justify-center gap-5 px-8 pointer-events-none transition-opacity duration-500 ease-out',
        fading && 'opacity-0'
      )}
    >
      <div className="w-full max-w-xl flex items-center justify-center">
        {overlay.logo && (
          <VideoLogo
            contentType={overlay.logo.contentType}
            entityId={overlay.logo.entityId}
            palette={overlay.logo.palette}
            title={overlay.title}
            sweepGap={`${Math.max(0, Math.min(100, 100 - overlay.progress))}%`}
            className="w-full"
          />
        )}
      </div>
      {overlay.error ? (
        <ErrorCard error={overlay.error} />
      ) : (
        overlay.status && (
          <p className="text-sm font-medium tracking-wide text-white/85">{overlay.status}</p>
        )
      )}
      {!overlay.error &&
        overlay.stages &&
        overlay.stages.length > 1 &&
        typeof overlay.currentStage === 'number' && (
          <StageStepper stages={overlay.stages} current={overlay.currentStage} />
        )}
      {/* Thin progress bar at the very bottom edge. */}
      <div
        className={cn('absolute bottom-0 left-0 right-0 h-[2px] bg-white/10')}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.max(0, Math.min(100, overlay.progress))}
      >
        <div
          className="h-full bg-[var(--accent)] transition-all duration-[700ms] ease-out"
          style={{
            width: `${Math.max(0, Math.min(100, overlay.progress))}%`,
          }}
        />
      </div>
    </div>
  );
}
