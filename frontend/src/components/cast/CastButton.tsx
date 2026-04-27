import * as Popover from '@radix-ui/react-popover';
import { Cast } from 'lucide-react';
import { useEffect } from 'react';
import { CastPopover } from '@/components/cast/CastPopover';
import { cn } from '@/lib/utils';
import { useCastStore } from '@/state/cast-store';

/**
 * Cast button next to Settings in the TopNav. Subsystem 32: the
 * Cast protocol lives entirely on the backend, so this button is
 * just a Zustand-driven popover trigger — no Cast SDK in the
 * browser, no Chromium gating.
 *
 * On first mount it fetches the device list (`/api/v1/cast/devices`).
 * Click always opens the popover; the popover swaps between a device
 * picker (when idle) and a mini-controller (when a session is live).
 */
export function CastButton() {
  const ready = useCastStore((s) => s.ready);
  const state = useCastStore((s) => s.state);
  const devices = useCastStore((s) => s.devices);
  const bootstrap = useCastStore((s) => s.bootstrap);

  // Bootstrap on first mount. `bootstrap()` self-dedupes on the
  // `ready` flag so re-mounts are free.
  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  // Hide the button when discovery hasn't run yet (avoids a flash
  // of "no devices" before the first listDevices() resolves).
  if (!ready) return null;

  const connected = state === 'connected';
  const connecting = state === 'connecting';
  const hasDevices = devices.length > 0;

  // Stay visible-but-muted when the LAN has no Chromecasts so users
  // can still open the popover and add one manually by IP — that
  // path is the workaround for Docker bridge / AP isolation.
  const iconClass = cn(
    'transition',
    connected && 'text-[var(--accent)]',
    connecting && 'text-[var(--accent)] animate-pulse',
    !connected && !connecting && hasDevices && 'text-[var(--text-secondary)] hover:text-white',
    !connected && !connecting && !hasDevices && 'text-[var(--text-muted)] hover:text-white'
  );

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label={connected ? 'Cast session' : 'Cast'}
          title={connected ? 'Casting' : 'Cast'}
          className={cn('p-2 rounded-md transition', connected ? 'bg-white/5' : 'hover:bg-white/5')}
        >
          <Cast size={18} className={iconClass} />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          sideOffset={8}
          align="end"
          className="z-[80] w-80 rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl"
        >
          <CastPopover />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
