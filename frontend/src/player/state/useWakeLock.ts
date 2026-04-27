import { useEffect, useRef } from 'react';

/**
 * Keep the screen awake while actively playing.
 *
 * `navigator.wakeLock` isn't universal (Firefox desktop
 * lacks it as of 2026); the try/catch silently degrades
 * where missing. A fire-and-forget async effect reads the
 * latest `playing` boolean, acquires the lock on `true`,
 * and releases on `false` or unmount.
 */
export function useWakeLock(playing: boolean) {
  const lockRef = useRef<WakeLockSentinel | null>(null);
  useEffect(() => {
    if (!playing) {
      lockRef.current?.release().catch(() => {});
      lockRef.current = null;
      return;
    }
    let released = false;
    (async () => {
      try {
        const lock = await navigator.wakeLock?.request('screen');
        if (released) {
          await lock?.release().catch(() => {});
        } else {
          lockRef.current = lock ?? null;
        }
      } catch {
        // Permission denied / unsupported — no-op.
      }
    })();
    return () => {
      released = true;
      lockRef.current?.release().catch(() => {});
      lockRef.current = null;
    };
  }, [playing]);
}
