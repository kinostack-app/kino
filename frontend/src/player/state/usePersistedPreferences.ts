import { useEffect } from 'react';

/**
 * Surviving-reload player preferences. Wrapped in try/catch
 * because Safari private browsing throws on localStorage.
 */
const LS_VOLUME = 'kino.player.volume';
const LS_PLAYBACK_RATE = 'kino.player.rate';
/** Legacy key — muted was persisted previously. Autoplay
 *  policies can force first-load mute, and persisting that
 *  silently silenced every subsequent session. We wipe this
 *  on mount so recovered users start unmuted. */
const LS_MUTED = 'kino.player.muted';

export function readStoredNumber(key: string, fallback: number): number {
  try {
    const raw = localStorage.getItem(key);
    if (raw == null) return fallback;
    const n = Number(raw);
    return Number.isFinite(n) ? n : fallback;
  } catch {
    return fallback;
  }
}

function writeStored(key: string, value: string | number | boolean) {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // Quota / private mode — silently ignore.
  }
}

/**
 * Restore persisted volume/rate onto the `<video>` on every
 * source swap (the browser recreates element state on `src`
 * change), and write them back when React state updates.
 *
 * Muted is intentionally session-only — see LS_MUTED comment.
 */
export function usePersistedPreferences(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  sourceUrl: string | undefined,
  volume: number,
  muted: boolean,
  playbackRate: number
) {
  // Apply to the live <video> on mount / source change.
  // The sourceUrl dep is intentional — we re-apply
  // preferences to every new <video> the browser builds
  // when `src` changes. Without this, a persisted volume
  // is only read once on first mount and gets reset by
  // the browser's default on every source swap.
  // biome-ignore lint/correctness/useExhaustiveDependencies: sourceUrl is the trigger that rebuilds <video>.
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    v.volume = volume;
    v.muted = muted;
    v.playbackRate = playbackRate;
  }, [videoRef, sourceUrl, volume, muted, playbackRate]);

  useEffect(() => {
    writeStored(LS_VOLUME, volume);
  }, [volume]);
  useEffect(() => {
    writeStored(LS_PLAYBACK_RATE, playbackRate);
  }, [playbackRate]);

  // One-time migration: wipe the old muted key.
  useEffect(() => {
    try {
      localStorage.removeItem(LS_MUTED);
    } catch {
      // ignore
    }
  }, []);
}

export const PREFERENCE_DEFAULTS = {
  volume: () => readStoredNumber(LS_VOLUME, 1),
  playbackRate: () => readStoredNumber(LS_PLAYBACK_RATE, 1),
};
