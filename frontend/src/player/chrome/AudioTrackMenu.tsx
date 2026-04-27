import { Check } from 'lucide-react';
import { useState } from 'react';
import { cn } from '@/lib/utils';
import type { VideoSource } from '../types';

/**
 * Custom audio-track picker.
 *
 * Media Chrome's built-in `<media-audio-track-menu>` reads
 * native `HTMLMediaElement.audioTracks`. Our switcher
 * rebuilds the HLS URL with `?audio_stream=N` (forces
 * ffmpeg to re-select the source stream), so native tracks
 * never populate and the built-in is useless for us.
 *
 * Keep our own popover — same design language as the old
 * hand-rolled one. Closes on outside click or Escape via a
 * document listener tied to `open`.
 */
export function AudioTrackMenu({ source }: { source: VideoSource | null }) {
  const [open, setOpen] = useState(false);
  if (!source?.audioTracks || source.audioTracks.length < 2) return null;
  const active = source.audioStreamIndex;

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-label="Audio track"
        aria-haspopup="menu"
        aria-expanded={open}
        className={cn(
          'h-9 px-2 grid place-items-center rounded-lg transition text-[10px] font-semibold uppercase tracking-wider',
          active != null ? 'bg-white/15 text-white' : 'hover:bg-white/10 text-white/70'
        )}
      >
        AUD
      </button>
      {open && (
        <>
          {/* Invisible backdrop catches outside clicks. */}
          <button
            type="button"
            aria-label="Close audio menu"
            className="fixed inset-0 z-40 cursor-default"
            onClick={() => setOpen(false)}
          />
          <div
            role="menu"
            className="absolute bottom-full right-0 mb-2 w-56 rounded-lg bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-xl py-1 text-sm z-50"
          >
            {source.audioTracks.map((track) => (
              <button
                key={track.stream_index}
                type="button"
                role="menuitem"
                onClick={() => {
                  source.onAudioStreamChange?.(track.stream_index);
                  setOpen(false);
                }}
                className="w-full text-left px-3 py-2 hover:bg-white/5 flex items-center gap-2"
              >
                <span className="w-4">{active === track.stream_index && <Check size={12} />}</span>
                <span className="truncate">{track.label}</span>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
