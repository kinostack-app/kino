import { Check, Gauge } from 'lucide-react';
import { useState } from 'react';
import { cn } from '@/lib/utils';
import { SPEED_OPTIONS } from '../types';

/**
 * Playback rate popover. Gear icon at 1.0x; rate number
 * when sped up/slowed — visible at a glance. Escape /
 * outside-click close via a backdrop button.
 */
export function SpeedMenu({
  playbackRate,
  onChange,
}: {
  playbackRate: number;
  onChange: (rate: number) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-label="Playback speed"
        aria-haspopup="menu"
        aria-expanded={open}
        className={cn(
          'h-9 px-2 grid place-items-center rounded-lg transition text-xs font-semibold tabular-nums',
          playbackRate !== 1 ? 'bg-white/15 text-white' : 'hover:bg-white/10 text-white/70'
        )}
      >
        {playbackRate === 1 ? <Gauge size={18} /> : `${playbackRate}×`}
      </button>
      {open && (
        <>
          <button
            type="button"
            aria-label="Close speed menu"
            className="fixed inset-0 z-40 cursor-default"
            onClick={() => setOpen(false)}
          />
          <div
            role="menu"
            className="absolute bottom-full right-0 mb-2 w-36 rounded-lg bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-xl py-1 text-sm z-50"
          >
            {SPEED_OPTIONS.map((opt) => (
              <button
                key={opt}
                type="button"
                role="menuitem"
                onClick={() => {
                  onChange(opt);
                  setOpen(false);
                }}
                className="w-full text-left px-3 py-2 hover:bg-white/5 flex items-center gap-2"
              >
                <span className="w-4">{playbackRate === opt && <Check size={12} />}</span>
                <span className="tabular-nums">{opt === 1 ? 'Normal' : `${opt}×`}</span>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
