import { Volume2, VolumeX } from 'lucide-react';
import { cn } from '@/lib/utils';

/**
 * Mute button + inline volume slider. Slider hides on
 * narrow viewports (below `sm`); mute button always
 * visible. Click-and-drag behavior on the slider uses
 * native click coordinates — a lighter affordance than
 * pointer capture for a UI element this small.
 */
export function VolumeControl({
  volume,
  muted,
  onVolumeChange,
  onMuteToggle,
}: {
  volume: number;
  muted: boolean;
  onVolumeChange: (v: number) => void;
  onMuteToggle: () => void;
}) {
  return (
    <div className="flex items-center gap-1 group/vol">
      <button
        type="button"
        onClick={onMuteToggle}
        className="w-9 h-9 grid place-items-center hover:bg-white/10 rounded-lg transition"
        aria-label={muted ? 'Unmute' : 'Mute'}
      >
        {muted || volume === 0 ? <VolumeX size={18} /> : <Volume2 size={18} />}
      </button>
      <div
        role="slider"
        tabIndex={0}
        aria-label="Volume"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(volume * 100)}
        className="w-20 h-1 rounded-full bg-white/20 cursor-pointer hidden sm:block"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const pct = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
          onVolumeChange(pct);
        }}
        onKeyDown={(e) => {
          // Don't fight the global arrow-volume hotkey.
          if (e.key === 'ArrowUp' || e.key === 'ArrowDown') e.stopPropagation();
        }}
      >
        <div
          className={cn('h-full rounded-full bg-white')}
          style={{ width: `${muted ? 0 : volume * 100}%` }}
        />
      </div>
    </div>
  );
}
