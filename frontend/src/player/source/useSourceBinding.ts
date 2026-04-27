import { useEffect, useRef, useState } from 'react';
import type { HlsSession } from '../hls/useHlsSession';
import type { VideoSource } from '../types';

/**
 * Bind `source.url` to the underlying `<video>`, choosing
 * between direct playback and hls.js based on
 * `source.mode`. Handles three subtleties the naïve
 * "set video.src" pattern gets wrong:
 *
 * 1. **Force-HLS fallback.** Direct play can fail with
 *    `MediaError::SRC_NOT_SUPPORTED` (code 4) on a
 *    container/codec the browser can't decode. We can't
 *    recover here alone — flipping the local mode to
 *    `'hls'` would feed hls.js the byte-range `/direct`
 *    URL it can't parse. The parent owns URL construction,
 *    so we delegate via `source.onForceHls`; the parent
 *    rebuilds the URL with `/master.m3u8` and feeds back
 *    a new `VideoSource` whose `mode` is `'hls'`. Mode +
 *    URL change together.
 *
 * 2. **Autoplay-gesture handoff.** Modern browsers
 *    silently force-mute autoplay without a user gesture.
 *    Once muted, they stay muted until user interaction —
 *    a trap that produced the "no sound anywhere" bug.
 *    Calling `play()` in JS inherits the gesture from the
 *    route navigation; if rejected, we fall back to muted
 *    playback so the user sees video + an unmute button.
 *
 * 3. **URL-keyed restart.** The parent rebuilds the source
 *    object on every prepare refetch (download_progress WS
 *    events fire every few seconds). If we depend on the
 *    object identity we'd reassign `video.src` each time,
 *    nuking any user-set pause. Re-gate on the observable
 *    URL + mode.
 */
export interface UseSourceBindingReturn {
  /** Call from the `<video>`'s `onError` handler with the
   *  element's `.error.code`. Escalates `SRC_NOT_SUPPORTED`
   *  to HLS fallback (via `source.onForceHls`) when possible,
   *  or surfaces a user message otherwise. */
  handleVideoError: (code: number | undefined) => void;
  /** Set by the error handler; shell reads to render the
   *  error overlay. */
  mediaError: string | null;
  clearMediaError: () => void;
  /** Whether the shell should show its "has not played yet"
   *  loading affordance. Reset to `true` on source swap;
   *  flipped to `false` when playback produces frames. */
  hasPlayedOnce: boolean;
  setHasPlayedOnce: (value: boolean) => void;
  /** True between source-change and first `canplay`. */
  isLoading: boolean;
  setIsLoading: (value: boolean) => void;
}

export function useSourceBinding(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  source: VideoSource | null,
  hls: HlsSession,
  /** Called once per source for the resume-at seek in
   *  `onDurationChange`. The parent tracks whether it's
   *  fired so we don't re-seek on every metadata refresh. */
  resumeSeekAppliedRef: React.RefObject<boolean>
): UseSourceBindingReturn {
  const [mediaError, setMediaError] = useState<string | null>(null);
  const [hasPlayedOnce, setHasPlayedOnce] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  // URL-keyed source binding — depending on the source
  // object identity would cause this effect to re-fire on
  // every parent re-render (PlayerRoot rebuilds `source`
  // on every prepare poll, every download_progress WS
  // event, every few seconds). Each re-fire would
  // reassign video.src → reset playback → re-render →
  // loop. Gate strictly on the observable URL + mode.
  //
  // The deps we omit (`source` object, `videoRef`,
  // `resumeSeekAppliedRef`, `hls`, the three setters) are
  // all stable references or intentionally-read-through —
  // source's non-URL fields (audio_stream_index etc.)
  // shouldn't trigger a re-mount, and hls is a useMemo'd
  // object whose identity is pinned to its useCallback
  // children.
  // biome-ignore lint/correctness/useExhaustiveDependencies: URL-keyed on purpose; see docstring.
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !source) return;
    resumeSeekAppliedRef.current = false;
    setHasPlayedOnce(false);
    setMediaError(null);
    setIsLoading(true);

    if (source.mode === 'direct') {
      video.src = source.url;
    } else {
      hls.start(source.url);
    }

    void video.play().catch(() => {
      // Unmuted autoplay rejected. Try muted — the user can
      // click the volume icon to get audio back.
      video.muted = true;
      void video.play().catch(() => {
        // Still can't play (rare — probably a codec issue);
        // leave paused so the user sees a Play button.
      });
    });

    return () => {
      hls.stop();
    };
  }, [source?.url, source?.mode]);

  const handleVideoError = (code: number | undefined) => {
    if (!code) return;
    // MEDIA_ERR_SRC_NOT_SUPPORTED (4) → ask the parent to rebuild the
    // source as HLS. We can't flip mode locally because the URL is
    // built by the parent; flipping mode without the URL leaves hls.js
    // trying to load the byte-range `/direct` endpoint and the
    // recovery never happens.
    if (code === 4 && source?.mode === 'direct') {
      if (source.onForceHls && source.castMediaId != null) {
        if (import.meta.env.DEV) {
          console.warn('[player] direct play failed, asking parent for HLS fallback');
        }
        source.onForceHls();
        return;
      }
      setMediaError(
        "This file's container isn't playable in the browser. It'll play from the library once import completes."
      );
    } else {
      setMediaError(`Playback error (code ${code}).`);
    }
  };

  return {
    handleVideoError,
    mediaError,
    clearMediaError: () => setMediaError(null),
    hasPlayedOnce,
    setHasPlayedOnce,
    isLoading,
    setIsLoading,
  };
}

/** Ref helper used by useSourceBinding for the one-shot
 *  resume-at seek. Kept separate so the shell can read the
 *  current value in the `onDurationChange` handler without
 *  closing over state. */
export function useResumeSeekRef() {
  return useRef(false);
}
