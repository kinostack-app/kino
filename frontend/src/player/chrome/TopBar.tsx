import { ArrowLeft } from 'lucide-react';

/**
 * Back button + title + optional top overlay.
 *
 * Top gradient fades into the video so the controls don't
 * float over a bright scene. Autohides in lockstep with
 * the bottom control bar via the parent's visibility
 * prop — a single fade handles the whole chrome.
 */
export function TopBar({
  title,
  onBack,
  topOverlay,
}: {
  title?: string;
  onBack: () => void;
  topOverlay?: React.ReactNode;
}) {
  return (
    <div className="bg-gradient-to-b from-black/80 via-black/30 to-transparent px-4 pt-4 pb-12">
      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={onBack}
          className="w-10 h-10 rounded-full bg-white/10 hover:bg-white/20 grid place-items-center transition"
          aria-label="Back"
        >
          <ArrowLeft size={20} />
        </button>
        <span className="text-sm font-medium text-white/80 truncate">{title}</span>
        {topOverlay && <div className="ml-auto">{topOverlay}</div>}
      </div>
    </div>
  );
}
