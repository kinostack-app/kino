import Hls from 'hls.js';
import { useCallback, useMemo, useRef } from 'react';
import { BACKOFF_BASE_MS, MAX_MEDIA_RECOVERIES, MAX_NETWORK_RETRIES } from './constants';

/**
 * hls.js session manager.
 *
 * Returns `start` / `stop` callbacks the shell wires to
 * source changes, and takes care of the two-tier retry
 * policy + authenticated request setup + known-stall
 * suppression.
 *
 * Keeping this isolated from React state is deliberate —
 * hls.js owns its own event loop, and the retry budget
 * lives in a ref so a re-render (e.g. from a stall
 * overlay transition) doesn't reset the counter.
 */
export interface UseHlsSessionOptions {
  /** Observes `<video>` element — must be attached before
   *  calling `start`. */
  videoRef: React.RefObject<HTMLVideoElement | null>;
  /** When a known stall is active (e.g. torrent paused),
   *  we suppress hls.js auto-recovery — retrying just
   *  thrashes the transcode and jumps playback back on
   *  every loop. The explicit Resume click is the real
   *  recovery path. Checked fresh on every error event. */
  isStalledRef: React.RefObject<boolean>;
  /** Called when retries are exhausted and playback can't
   *  continue. Fed a human-readable reason for the user
   *  error overlay. */
  onFatal: (message: string) => void;
}

export interface HlsSession {
  /** Build a new hls.js instance against `url`. Safe to
   *  call repeatedly — old sessions are torn down first. */
  start: (url: string) => void;
  /** Destroy the active session. Idempotent. */
  stop: () => void;
  /** When a known stall clears, nudge hls.js to re-request
   *  fragments so playback resumes. */
  poke: () => void;
}

export function useHlsSession({
  videoRef,
  isStalledRef,
  onFatal,
}: UseHlsSessionOptions): HlsSession {
  const hlsRef = useRef<Hls | null>(null);
  const retryStateRef = useRef({
    networkRetries: 0,
    mediaRecoveries: 0,
    lastRetryMs: 0,
  });

  // Pin the latest `onFatal` behind a ref so `start` is
  // a stable useCallback. Otherwise every caller re-render
  // would build a new onFatal closure → `start` identity
  // changes → the useMemo'd session object's identity
  // changes → consumer useEffects re-fire → video src
  // resets → render loop.
  const onFatalRef = useRef(onFatal);
  onFatalRef.current = onFatal;

  const stop = useCallback(() => {
    hlsRef.current?.destroy();
    hlsRef.current = null;
  }, []);

  // We intentionally don't depend on onFatal — it's read
  // through the ref above so this callback stays stable
  // across VideoShell re-renders.
  // biome-ignore lint/correctness/useExhaustiveDependencies: onFatal read via ref; see onFatalRef comment above.
  const start = useCallback(
    (url: string) => {
      stop();
      const video = videoRef.current;
      if (!video) return;
      if (!Hls.isSupported()) {
        // Safari native HLS fallback — no retry budget,
        // the browser handles it.
        if (video.canPlayType('application/vnd.apple.mpegurl')) {
          video.src = url;
        }
        return;
      }
      const hls = new Hls({
        maxBufferLength: 30,
        maxMaxBufferLength: 60,
        // Cookie auto-sends on hls.js's XHRs when the request is
        // same-origin; cross-origin deploys go through a signed URL
        // upstream so the bearer token doesn't need to be set here.
        xhrSetup: (xhr) => {
          xhr.withCredentials = true;
        },
      });
      hls.loadSource(url);
      hls.attachMedia(video);
      hlsRef.current = hls;

      const resetRetryState = () => {
        retryStateRef.current = {
          networkRetries: 0,
          mediaRecoveries: 0,
          lastRetryMs: 0,
        };
      };
      hls.on(Hls.Events.FRAG_LOADED, resetRetryState);
      hls.on(Hls.Events.MANIFEST_PARSED, resetRetryState);

      hls.on(Hls.Events.ERROR, (_e, data) => {
        if (!data.fatal) return;
        if (isStalledRef.current) {
          // Known stall in progress — suppress auto-recovery
          // so our stall overlay is the authoritative recovery
          // affordance.
          if (import.meta.env.DEV) {
            console.debug('[hls] fatal during known stall, deferring:', data.type);
          }
          return;
        }
        const state = retryStateRef.current;
        if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
          if (state.networkRetries >= MAX_NETWORK_RETRIES) {
            if (state.mediaRecoveries < MAX_MEDIA_RECOVERIES) {
              state.mediaRecoveries += 1;
              hls.recoverMediaError();
            } else {
              onFatalRef.current(`Playback stopped: ${data.details}. Try reloading.`);
              hls.destroy();
            }
            return;
          }
          const delay = BACKOFF_BASE_MS * 2 ** state.networkRetries;
          state.networkRetries += 1;
          state.lastRetryMs = Date.now();
          window.setTimeout(() => {
            if (hlsRef.current === hls) hls.startLoad();
          }, delay);
        } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
          if (state.mediaRecoveries >= MAX_MEDIA_RECOVERIES) {
            onFatal(`Playback stopped: ${data.details}. Try reloading.`);
            hls.destroy();
            return;
          }
          state.mediaRecoveries += 1;
          hls.recoverMediaError();
        } else {
          onFatal(`Playback stopped: ${data.details}. Try reloading.`);
          hls.destroy();
        }
      });
    },
    [videoRef, isStalledRef, stop]
  );

  const poke = useCallback(() => {
    hlsRef.current?.startLoad();
  }, []);

  // Return a stable object — if we returned `{ start,
  // stop, poke }` by literal, any useEffect in a consumer
  // that depends on `hls` would fire on every render and
  // tear down the HLS session mid-playback. useMemo pins
  // the identity to the stable useCallback references.
  return useMemo(() => ({ start, stop, poke }), [start, stop, poke]);
}
