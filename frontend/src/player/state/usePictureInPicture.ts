import { useCallback, useEffect, useState } from 'react';

/**
 * Picture-in-picture state + toggle. Browsers without PiP
 * support (Firefox on Linux has partial support with
 * quirks) silently no-op; the consumer can consult
 * `supported` to hide the button entirely.
 */
export function usePictureInPicture(videoRef: React.RefObject<HTMLVideoElement | null>) {
  const [isPip, setIsPip] = useState(false);
  const supported = typeof document !== 'undefined' && document.pictureInPictureEnabled;

  const toggle = useCallback(async () => {
    const v = videoRef.current;
    if (!v) return;
    try {
      if (document.pictureInPictureElement) {
        await document.exitPictureInPicture();
      } else if (document.pictureInPictureEnabled) {
        await v.requestPictureInPicture();
      }
    } catch {
      // User denied / unsupported codec combo — fail quiet.
    }
  }, [videoRef]);

  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const enter = () => setIsPip(true);
    const leave = () => setIsPip(false);
    v.addEventListener('enterpictureinpicture', enter);
    v.addEventListener('leavepictureinpicture', leave);
    return () => {
      v.removeEventListener('enterpictureinpicture', enter);
      v.removeEventListener('leavepictureinpicture', leave);
    };
  }, [videoRef]);

  return { isPip, supported, toggle };
}
