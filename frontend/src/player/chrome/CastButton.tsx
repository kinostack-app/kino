import * as Popover from '@radix-ui/react-popover';
import { Cast } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useCastStore } from '@/state/cast-store';
import type { VideoSource } from '../types';

/**
 * Player-chrome Cast button. Subsystem 32: the Cast protocol lives
 * on the backend, so this is a pure trigger — open a small device
 * picker on click; on selection, hand the current playhead off as
 * the `start_position_ms` of a new cast session.
 *
 * We deliberately don't use Media Chrome's `<media-cast-button>`:
 * it dispatches through `navigator.remote.prompt()` (Chromium only).
 * Going through our own popover keeps the behaviour identical
 * across Firefox, Safari, and Chrome — which is the whole point of
 * subsystem 32.
 *
 * Hidden when the source has no `castMediaId` (torrent streams
 * pre-import can't cast — the receiver needs a library media id).
 */
export function CastButton({
  source,
  videoRef,
  hlsOffsetRef,
}: {
  source: VideoSource | null;
  videoRef: React.RefObject<HTMLVideoElement | null>;
  hlsOffsetRef: React.RefObject<number>;
}) {
  const state = useCastStore((s) => s.state);
  const devices = useCastStore((s) => s.devices);
  const selectDevice = useCastStore((s) => s.selectDevice);
  const endSession = useCastStore((s) => s.endSession);

  if (!source?.castMediaId) return null;

  const connected = state === 'connected';
  const connecting = state === 'connecting';
  const hasDevices = devices.length > 0;

  // Already casting — clicking ends the session.
  if (connected || connecting) {
    return (
      <button
        type="button"
        onClick={() => void endSession()}
        aria-label="Stop casting"
        className={cn(
          'relative w-9 h-9 grid place-items-center rounded-lg transition',
          'bg-[var(--accent)]/25 text-[var(--accent)]',
          connecting && 'animate-pulse'
        )}
      >
        <Cast size={18} />
        <span
          aria-hidden="true"
          className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-[var(--accent)]"
        />
      </button>
    );
  }

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label="Cast to a device"
          disabled={!hasDevices}
          className={cn(
            'w-9 h-9 grid place-items-center rounded-lg transition',
            'hover:bg-white/10 text-white/70',
            !hasDevices && 'opacity-40 cursor-not-allowed'
          )}
        >
          <Cast size={18} />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          sideOffset={8}
          align="end"
          className="z-[80] w-72 rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl p-3"
        >
          <p className="text-[10px] uppercase tracking-wider text-[var(--accent)] font-semibold mb-2">
            Cast to
          </p>
          <ul className="space-y-1">
            {devices.map((d) => (
              <li key={d.id}>
                <button
                  type="button"
                  onClick={() => {
                    if (!source.castMediaId) return;
                    const video = videoRef.current;
                    const snapTimeSec = (video?.currentTime ?? 0) + hlsOffsetRef.current;
                    void selectDevice(d.id, {
                      mediaId: source.castMediaId,
                      title: source.title,
                      startTimeSec: snapTimeSec,
                    });
                  }}
                  className="w-full text-left px-3 py-2 rounded-lg bg-white/[0.03] hover:bg-white/[0.06] text-sm text-white/90"
                >
                  <span className="block truncate">{d.name}</span>
                  <span className="block text-[11px] text-[var(--text-muted)]">{d.ip}</span>
                </button>
              </li>
            ))}
          </ul>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
