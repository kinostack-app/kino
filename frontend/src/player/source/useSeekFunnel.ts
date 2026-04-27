import { useCallback } from 'react';
import type { VideoSource } from '../types';

/**
 * Single funnel for anything that wants to move the
 * playhead. Works in **source-time** (what the user sees
 * on the scrubber / clock) and handles three cases:
 *
 * 1. **Target inside buffered range** → native `.currentTime`
 *    seek.
 * 2. **Target past buffered (or backward of the HLS
 *    offset) with `onSeekReload` available** → delegate to
 *    the parent. Parent bumps HLS offset, rebuilds the URL
 *    with `?at=<secs>`, and the source-URL-change effect
 *    re-arms the loading overlay while ffmpeg restarts at
 *    the new `-ss`.
 * 3. **Target past buffered with no reload capability** →
 *    native seek + re-show the loading overlay (the browser
 *    may snap back for MKVs whose cue index doesn't cover
 *    the target, but at least we surfaced the wait).
 */
export interface UseSeekFunnelOptions {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  source: VideoSource | null;
  setIsLoading: (v: boolean) => void;
  setHasPlayedOnce: (v: boolean) => void;
  /** Called with the new source-time so the shell can keep
   *  its displayed currentTime in sync between the seek
   *  request and the `timeupdate` that follows. */
  onSourceTimeChange: (sourceSec: number) => void;
}

export function useSeekFunnel({
  videoRef,
  source,
  setIsLoading,
  setHasPlayedOnce,
  onSourceTimeChange,
}: UseSeekFunnelOptions) {
  return useCallback(
    (targetSourceSec: number) => {
      const v = videoRef.current;
      if (!v) return;
      const offset = source?.hlsSourceOffsetSec ?? 0;
      const internalTime = targetSourceSec - offset;
      const pastBufferedGrace = 3;
      const bufferedEnd = v.buffered.length > 0 ? v.buffered.end(v.buffered.length - 1) : 0;
      const pastBuffered = internalTime > bufferedEnd + pastBufferedGrace;
      const backwardOfOffset = internalTime < 0;

      if ((pastBuffered || backwardOfOffset) && source?.onSeekReload) {
        source.onSeekReload(Math.max(0, targetSourceSec));
        return;
      }

      if (pastBuffered) {
        setHasPlayedOnce(false);
        setIsLoading(true);
      }

      v.currentTime = Math.max(0, internalTime);
      onSourceTimeChange(targetSourceSec);
    },
    [videoRef, source, setIsLoading, setHasPlayedOnce, onSourceTimeChange]
  );
}
