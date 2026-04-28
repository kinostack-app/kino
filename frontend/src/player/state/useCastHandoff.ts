import { useEffect, useRef } from 'react';
import { useCastStore } from '@/state/cast-store';
import type { VideoSource } from '../types';

/**
 * Handoff to a Cast session when one's been started for the
 * currently-playing media. Two paths feed in:
 *
 *   1. **Explicit pick** — user clicks the in-player Cast button,
 *      which calls `selectDevice` directly. This hook then watches
 *      the resulting session state.
 *   2. **Pre-connect** — user picked a target device from the
 *      header popover (BBC iPlayer flow) BEFORE opening any media.
 *      `preselectedDeviceId` carries that pick (persisted across
 *      reloads). When a source with `castMediaId` mounts and there's
 *      no active session, we call `selectDevice` with the
 *      preselected target automatically — the user never has to
 *      open the in-player Cast menu.
 *
 * Once a session for *this* mediaId becomes connected, fires
 * `onHandoff` to pause the local `<video>` element so the receiver
 * owns the playback experience without two pipelines fighting.
 */
export function useCastHandoff(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  source: VideoSource | null,
  onHandoff: () => void
) {
  const castState = useCastStore((s) => s.state);
  const castDeviceName = useCastStore((s) => s.deviceName);
  const castSessionMediaId = useCastStore((s) => s.media?.mediaId ?? null);
  const preselectedDeviceId = useCastStore((s) => s.preselectedDeviceId);
  const devices = useCastStore((s) => s.devices);
  const selectDevice = useCastStore((s) => s.selectDevice);
  const isCasting = castState === 'connected';
  const isIdle = castState === 'idle' || castState === 'no_devices';

  const lastHandoffIdRef = useRef<number | null>(null);
  const lastAutoStartIdRef = useRef<number | null>(null);

  // Path 1 + 2 finalisation: pause the local element when the
  // receiver becomes the source of truth for this mediaId.
  useEffect(() => {
    if (!isCasting || !source?.castMediaId) return;
    if (castSessionMediaId !== source.castMediaId) return;
    if (lastHandoffIdRef.current === source.castMediaId) return;
    lastHandoffIdRef.current = source.castMediaId;
    videoRef.current?.pause();
    onHandoff();
  }, [isCasting, source?.castMediaId, castSessionMediaId, videoRef, onHandoff]);

  // Path 2 trigger: auto-start a session against the preselected
  // device when this player mounts a castable source and nothing's
  // running. Guarded by a ref so we don't restart after the user
  // explicitly stops the session for this mediaId.
  useEffect(() => {
    if (!isIdle) return;
    if (!preselectedDeviceId) return;
    if (!source?.castMediaId) return;
    if (lastAutoStartIdRef.current === source.castMediaId) return;
    // The device must still be on the LAN — a stale preselection
    // (router reboot, device unplugged) shouldn't error-spam.
    if (!devices.some((d) => d.id === preselectedDeviceId)) return;
    lastAutoStartIdRef.current = source.castMediaId;
    void selectDevice(preselectedDeviceId, {
      mediaId: source.castMediaId,
      title: source.title ?? 'Now playing',
      startTimeSec: videoRef.current?.currentTime ?? 0,
    });
  }, [isIdle, preselectedDeviceId, source, devices, selectDevice, videoRef]);

  return {
    isCasting,
    castDeviceName,
    castMediaId: source?.castMediaId ?? null,
  };
}
