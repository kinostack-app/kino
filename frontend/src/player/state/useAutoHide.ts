import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * Auto-hide controls + cursor after N seconds of inactivity
 * while playing. Hold-off while paused — a paused player
 * with invisible controls is a UX dead-end ("how do I
 * resume?"). Mouse move / touch / key-press all reset the
 * timer; hover over a control prevents the hide outright.
 *
 * The `container` ref is the element we track
 * mouse-move on. The `videoRef` is consulted so we don't
 * hide while paused.
 */
export function useAutoHide(
  container: React.RefObject<HTMLElement | null>,
  videoRef: React.RefObject<HTMLVideoElement | null>,
  delayMs: number = 3000
) {
  const [visible, setVisible] = useState(true);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  const show = useCallback(() => {
    setVisible(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      if (videoRef.current && !videoRef.current.paused) {
        setVisible(false);
      }
    }, delayMs);
  }, [videoRef, delayMs]);

  useEffect(() => {
    const el = container.current;
    if (!el) return;
    const move = () => show();
    el.addEventListener('mousemove', move);
    el.addEventListener('touchstart', move);
    return () => {
      el.removeEventListener('mousemove', move);
      el.removeEventListener('touchstart', move);
      if (timer.current) clearTimeout(timer.current);
    };
  }, [container, show]);

  return { visible, show };
}
