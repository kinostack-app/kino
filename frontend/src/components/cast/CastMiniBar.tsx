import { useLocation } from '@tanstack/react-router';
import { Cast, Pause, Play } from 'lucide-react';
import { useState } from 'react';
import { CastPopover } from '@/components/cast/CastPopover';
import { useCastStore } from '@/state/cast-store';

/**
 * Persistent bar shown at the bottom of the viewport while a Cast
 * session is active, on every route except the Player itself (the
 * Player owns the full-screen cast UX). Sits above the mobile
 * BottomNav.
 *
 * Lets users pause / resume / open the popover without hunting back
 * to the TopNav button or reopening the source detail page.
 */
export function CastMiniBar() {
  const state = useCastStore((s) => s.state);
  const deviceName = useCastStore((s) => s.deviceName);
  const media = useCastStore((s) => s.media);
  const isPaused = useCastStore((s) => s.isPaused);
  const playOrPause = useCastStore((s) => s.playOrPause);
  const location = useLocation();
  const [expanded, setExpanded] = useState(false);

  if (state !== 'connected') return null;
  // The player page has its own full-screen Cast overlay; suppress
  // the mini-bar there to avoid two competing controllers.
  if (location.pathname.startsWith('/play/') || location.pathname.startsWith('/watch/')) {
    return null;
  }

  return (
    <div className="fixed bottom-16 md:bottom-4 left-4 right-4 z-50 flex justify-center pointer-events-none">
      <div className="w-full max-w-md pointer-events-auto rounded-2xl bg-[var(--bg-secondary)]/95 backdrop-blur-xl ring-1 ring-white/10 shadow-2xl overflow-hidden">
        {!expanded ? (
          <div className="flex items-center gap-3 px-3 py-2">
            <button
              type="button"
              onClick={() => setExpanded(true)}
              aria-label="Cast controls"
              className="flex items-center gap-3 flex-1 min-w-0 text-left hover:bg-white/5 rounded-lg px-2 -mx-2 py-1"
            >
              <Cast size={16} className="text-[var(--accent)] flex-shrink-0" />
              <div className="flex-1 min-w-0">
                <p className="text-[10px] uppercase tracking-wider text-[var(--accent)] font-semibold leading-none">
                  Casting · {deviceName ?? ''}
                </p>
                <p className="text-[13px] text-white truncate mt-0.5">
                  {media?.title ?? 'Nothing loaded'}
                </p>
              </div>
            </button>
            <button
              type="button"
              onClick={() => void playOrPause()}
              aria-label={isPaused ? 'Play' : 'Pause'}
              className="w-8 h-8 rounded-full bg-white text-black grid place-items-center hover:bg-white/90"
            >
              {isPaused ? <Play size={14} fill="black" /> : <Pause size={14} />}
            </button>
          </div>
        ) : (
          <CastPopover onClose={() => setExpanded(false)} />
        )}
      </div>
    </div>
  );
}
