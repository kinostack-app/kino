import { useCallback, useEffect, useState } from 'react';

/**
 * Fullscreen state + toggle.
 *
 * Targets the `<video>`-containing element passed in, not
 * the `<video>` itself — fullscreening the video directly
 * hides our control overlay. Our controls live as sibling
 * chrome layered over the video, so the fullscreen target
 * must be the common ancestor.
 *
 * Firefox/Linux edge-bars fix: we set `background: black`
 * inline on the target during fullscreen + rely on the
 * element being a plain `<div>` (no `fixed inset-0` — those
 * interact badly with the UA-applied `:fullscreen`
 * styling). The parent is expected to have normal block
 * layout that fills the viewport.
 */
export function useFullscreen(targetRef: React.RefObject<HTMLElement | null>) {
  const [isFullscreen, setIsFullscreen] = useState(false);

  const toggle = useCallback(() => {
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => {});
    } else {
      const el = targetRef.current;
      if (!el) return;
      void el.requestFullscreen().catch(() => {});
    }
  }, [targetRef]);

  useEffect(() => {
    const handler = () => setIsFullscreen(!!document.fullscreenElement);
    document.addEventListener('fullscreenchange', handler);
    return () => document.removeEventListener('fullscreenchange', handler);
  }, []);

  return { isFullscreen, toggle };
}
