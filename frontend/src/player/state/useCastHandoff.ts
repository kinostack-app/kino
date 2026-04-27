import { useEffect, useRef } from 'react';
import { useCastStore } from '@/state/cast-store';
import type { VideoSource } from '../types';

/**
 * Handoff to a Cast session when one's been started for the
 * currently-playing media. With the Phase-2 server-side cast model
 * the user picks a device via the player's `<CastButton>` (which
 * triggers `selectDevice` directly with the current source +
 * snap-time); this hook is the bookkeeping side, returning the
 * casting state so the player can render its overlay.
 *
 * Fires `onHandoff` once when a session for *this* mediaId becomes
 * connected — pauses the local element so the receiver owns the
 * playback experience without two pipelines fighting.
 */
export function useCastHandoff(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  source: VideoSource | null,
  onHandoff: () => void
) {
  const castState = useCastStore((s) => s.state);
  const castDeviceName = useCastStore((s) => s.deviceName);
  const castSessionMediaId = useCastStore((s) => s.media?.mediaId ?? null);
  const isCasting = castState === 'connected';

  const lastHandoffIdRef = useRef<number | null>(null);

  useEffect(() => {
    if (!isCasting || !source?.castMediaId) return;
    if (castSessionMediaId !== source.castMediaId) return;
    if (lastHandoffIdRef.current === source.castMediaId) return;
    lastHandoffIdRef.current = source.castMediaId;
    videoRef.current?.pause();
    onHandoff();
  }, [isCasting, source?.castMediaId, castSessionMediaId, videoRef, onHandoff]);

  return {
    isCasting,
    castDeviceName,
    castMediaId: source?.castMediaId ?? null,
  };
}
