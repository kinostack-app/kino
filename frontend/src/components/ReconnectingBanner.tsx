/**
 * Slim top-banner for transient backend failures.
 *
 * Renders while `connection.phase === 'reconnecting'` (< 30s of
 * failures). Keeps the app shell interactive — cached queries still
 * render, navigation still works — but surfaces "something's not
 * right." On the next successful request the phase flips back to
 * `healthy` and the banner unmounts.
 *
 * Styled amber (caution, not critical) to distinguish from the full
 * offline takeover. Persists at `top-0` over the TopNav so it's
 * visible regardless of scroll position.
 */

import { Loader2 } from 'lucide-react';
import { useConnectionPhase } from '@/state/connection';

export function ReconnectingBanner() {
  const phase = useConnectionPhase();
  if (phase !== 'reconnecting') return null;
  return (
    <div className="fixed top-0 inset-x-0 z-[60] flex items-center justify-center gap-2 px-4 py-1.5 bg-amber-500/15 text-amber-200 text-xs font-medium backdrop-blur-sm border-b border-amber-500/20">
      <Loader2 size={12} className="animate-spin" />
      <span>Reconnecting to kino…</span>
    </div>
  );
}
