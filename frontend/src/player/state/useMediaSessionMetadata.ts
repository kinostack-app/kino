import { useEffect } from 'react';
import type { VideoSource } from '../types';

/**
 * Wire `navigator.mediaSession` for OS-level media keys
 * (Bluetooth remotes, macOS Now Playing, keyboard
 * media keys) + lockscreen controls.
 *
 * Metadata + action handlers are set on source change;
 * `playbackState` + `setPositionState` update in lockstep
 * with the element via the returned `syncPosition`
 * callback (wire it to `durationchange` / `ratechange` /
 * `timeupdate`).
 */
export function useMediaSessionMetadata(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  source: VideoSource | null,
  playing: boolean
) {
  // Metadata + action handlers.
  useEffect(() => {
    const ms = navigator.mediaSession;
    if (!ms || !source) return;
    ms.metadata = new MediaMetadata({
      title: source.title,
      artist: 'kino',
      // Artwork left empty until we thread backdrop URLs
      // through VideoSource — lockscreen falls back to the
      // video first-frame when artwork is absent.
      artwork: [],
    });
    ms.setActionHandler('play', () => {
      videoRef.current?.play().catch(() => {});
    });
    ms.setActionHandler('pause', () => {
      videoRef.current?.pause();
    });
    ms.setActionHandler('seekbackward', (details) => {
      const v = videoRef.current;
      if (!v) return;
      v.currentTime = Math.max(0, v.currentTime - (details.seekOffset ?? 10));
    });
    ms.setActionHandler('seekforward', (details) => {
      const v = videoRef.current;
      if (!v) return;
      v.currentTime = v.currentTime + (details.seekOffset ?? 10);
    });
    ms.setActionHandler('seekto', (details) => {
      const v = videoRef.current;
      if (!v || details.seekTime == null) return;
      v.currentTime = details.seekTime;
    });
    return () => {
      ms.metadata = null;
      for (const action of ['play', 'pause', 'seekbackward', 'seekforward', 'seekto'] as const) {
        ms.setActionHandler(action, null);
      }
    };
  }, [videoRef, source]);

  // Keep OS-level play/pause state synchronised so the
  // lockscreen icon matches reality.
  useEffect(() => {
    if (!navigator.mediaSession) return;
    navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
  }, [playing]);

  // Position state — attached to the `<video>`'s native
  // cadence so the OS indicator updates ~4×/s without a
  // separate timer.
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !navigator.mediaSession) return;
    const handler = () => {
      if (!video.duration || Number.isNaN(video.duration)) return;
      try {
        navigator.mediaSession.setPositionState({
          duration: video.duration,
          playbackRate: video.playbackRate,
          position: Math.min(video.currentTime, video.duration),
        });
      } catch {
        // Some browsers throw on bad values — non-fatal.
      }
    };
    video.addEventListener('durationchange', handler);
    video.addEventListener('ratechange', handler);
    video.addEventListener('timeupdate', handler);
    return () => {
      video.removeEventListener('durationchange', handler);
      video.removeEventListener('ratechange', handler);
      video.removeEventListener('timeupdate', handler);
    };
  }, [videoRef]);
}
