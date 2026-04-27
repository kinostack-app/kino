import { ChevronRight } from 'lucide-react';
import { useEffect } from 'react';
import type { VideoSource } from '../types';

/**
 * Skip Intro / Skip Credits button (subsystem 15).
 *
 * Renders whenever the playhead is inside an intro or
 * credits range on the current source. Clicking it seeks
 * past the segment; keyboard 'S' does the same. Stays
 * always-visible — rendered as a sibling to
 * `<MediaController>` so Media Chrome's autohide doesn't
 * eat it.
 */
export function SkipButton({
  currentTime,
  source,
  onSeek,
}: {
  currentTime: number;
  source: VideoSource | null;
  onSeek: (seconds: number) => void;
}) {
  const introStart = source?.introStartMs != null ? source.introStartMs / 1000 : null;
  const introEnd = source?.introEndMs != null ? source.introEndMs / 1000 : null;
  const creditsStart = source?.creditsStartMs != null ? source.creditsStartMs / 1000 : null;
  const creditsEnd = source?.creditsEndMs != null ? source.creditsEndMs / 1000 : null;
  const showEnabled = source?.skipEnabledForShow ?? true;

  // Decide which skip the button represents right now.
  // Intro wins on the rare overlap case.
  let mode: 'intro' | 'credits' | null = null;
  let seekTo = 0;
  if (
    showEnabled &&
    introEnd != null &&
    introStart != null &&
    currentTime >= introStart &&
    currentTime < introEnd
  ) {
    mode = 'intro';
    seekTo = introEnd;
  } else if (creditsStart != null && currentTime >= creditsStart) {
    mode = 'credits';
    seekTo = creditsEnd ?? Number.POSITIVE_INFINITY;
  }

  useEffect(() => {
    if (!mode) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (e.key === 's' || e.key === 'S') {
        e.preventDefault();
        onSeek(seekTo);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [mode, seekTo, onSeek]);

  if (!mode) return null;

  const label = mode === 'intro' ? 'Skip Intro' : 'Skip Credits';
  return (
    <button
      type="button"
      onClick={() => onSeek(seekTo)}
      aria-label={label}
      className="absolute bottom-24 right-6 md:bottom-28 md:right-10 z-30 inline-flex items-center gap-1.5 px-4 py-2 rounded-lg bg-black/75 backdrop-blur-sm text-white text-sm font-semibold ring-1 ring-white/10 shadow-lg hover:bg-black/90 transition motion-safe:animate-[fadein_200ms_ease-out]"
    >
      {label}
      <ChevronRight size={16} />
    </button>
  );
}
