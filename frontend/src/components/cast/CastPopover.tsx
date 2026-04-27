import { Pause, Play, Plus, Trash2, X } from 'lucide-react';
import { useState } from 'react';
import { useCastStore } from '@/state/cast-store';

/**
 * Popover content for the TopNav Cast button. Two modes:
 *
 *   - **Idle** — device picker. Lists devices discovered via mDNS
 *     plus any manually-added rows; user picks one to start a
 *     session against the currently-active media (when there is
 *     one — selecting from this popover with no media in scope
 *     is a no-op until the user navigates to a detail page).
 *   - **Active session** — mini-controller (device name, media
 *     title, current position, play/pause, stop casting).
 *
 * Volume + queue controls land with Phase 2.
 */
export function CastPopover({ onClose }: { onClose?: () => void } = {}) {
  const state = useCastStore((s) => s.state);
  if (state === 'connected' || state === 'connecting' || state === 'ending') {
    return <ActiveSession onClose={onClose} />;
  }
  return <DevicePicker onClose={onClose} />;
}

// ─── Idle: device picker ─────────────────────────────────────────

function DevicePicker({ onClose }: { onClose?: () => void }) {
  const devices = useCastStore((s) => s.devices);
  const refreshDevices = useCastStore((s) => s.refreshDevices);
  const forgetDevice = useCastStore((s) => s.forgetDevice);
  const [showAdd, setShowAdd] = useState(false);

  return (
    <div className="p-4">
      <Header title="Cast to a device" onClose={onClose} />
      {devices.length === 0 ? (
        <p className="mt-3 text-xs text-[var(--text-muted)]">
          No devices found on the LAN. Add one by IP if mDNS is blocked.
        </p>
      ) : (
        <ul className="mt-3 space-y-1">
          {devices.map((d) => (
            <li key={d.id} className="flex items-center gap-2">
              <button
                type="button"
                disabled
                title="Open a movie or episode and use the Cast button there to choose a target"
                className="flex-1 text-left px-3 py-2 rounded-lg bg-white/[0.03] text-sm text-white/80 cursor-not-allowed"
              >
                <span className="block truncate">{d.name}</span>
                <span className="block text-[11px] text-[var(--text-muted)]">{d.ip}</span>
              </button>
              {d.source === 'manual' && (
                <button
                  type="button"
                  onClick={() => void forgetDevice(d.id)}
                  aria-label={`Forget ${d.name}`}
                  className="p-2 rounded text-[var(--text-muted)] hover:text-white hover:bg-white/5"
                >
                  <Trash2 size={14} />
                </button>
              )}
            </li>
          ))}
        </ul>
      )}

      <div className="mt-4">
        {showAdd ? (
          <AddDeviceForm
            onCancel={() => setShowAdd(false)}
            onAdded={() => {
              setShowAdd(false);
              void refreshDevices();
            }}
          />
        ) : (
          <button
            type="button"
            onClick={() => setShowAdd(true)}
            className="flex w-full items-center gap-2 px-3 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)]"
          >
            <Plus size={14} />
            Add device by IP
          </button>
        )}
      </div>
    </div>
  );
}

function AddDeviceForm({ onCancel, onAdded }: { onCancel: () => void; onAdded: () => void }) {
  const addDevice = useCastStore((s) => s.addDevice);
  const [ip, setIp] = useState('');
  const [name, setName] = useState('');
  const [submitting, setSubmitting] = useState(false);

  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault();
        if (!ip.trim()) return;
        setSubmitting(true);
        const result = await addDevice({ ip: ip.trim(), name: name.trim() || undefined });
        setSubmitting(false);
        if (result) onAdded();
      }}
      className="space-y-2"
    >
      <input
        type="text"
        inputMode="numeric"
        placeholder="192.168.1.42"
        value={ip}
        onChange={(e) => setIp(e.target.value)}
        className="w-full px-3 py-2 rounded-lg bg-black/40 ring-1 ring-white/10 text-sm text-white placeholder:text-[var(--text-muted)] focus:ring-[var(--accent)] outline-none"
      />
      <input
        type="text"
        placeholder="Living room TV (optional)"
        value={name}
        onChange={(e) => setName(e.target.value)}
        className="w-full px-3 py-2 rounded-lg bg-black/40 ring-1 ring-white/10 text-sm text-white placeholder:text-[var(--text-muted)] focus:ring-[var(--accent)] outline-none"
      />
      <div className="flex gap-2">
        <button
          type="submit"
          disabled={submitting || !ip.trim()}
          className="flex-1 px-3 py-2 rounded-lg bg-[var(--accent)] text-white text-sm font-medium disabled:opacity-50"
        >
          {submitting ? 'Adding…' : 'Add'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="px-3 py-2 rounded-lg bg-white/5 text-sm text-[var(--text-secondary)] hover:bg-white/10"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

// ─── Active: controls ────────────────────────────────────────────

function ActiveSession({ onClose }: { onClose?: () => void }) {
  const deviceName = useCastStore((s) => s.deviceName);
  const media = useCastStore((s) => s.media);
  const isPaused = useCastStore((s) => s.isPaused);
  const currentTimeSec = useCastStore((s) => s.currentTimeSec);
  const playOrPause = useCastStore((s) => s.playOrPause);
  const endSession = useCastStore((s) => s.endSession);
  const state = useCastStore((s) => s.state);

  return (
    <div className="p-4">
      <Header title="Casting" subtitle={deviceName ?? '—'} onClose={onClose} />

      {media && (
        <div className="mt-3">
          <p className="text-[13px] text-white truncate">{media.title}</p>
          <p className="text-[11px] text-[var(--text-muted)] tabular-nums mt-0.5">
            {formatTime(currentTimeSec)}
          </p>
        </div>
      )}

      <div className="mt-3 flex items-center gap-2">
        <button
          type="button"
          onClick={() => void playOrPause()}
          aria-label={isPaused ? 'Play' : 'Pause'}
          disabled={state === 'connecting' || state === 'ending'}
          className="w-9 h-9 rounded-full bg-white text-black grid place-items-center hover:bg-white/90 disabled:opacity-50"
        >
          {isPaused ? <Play size={16} fill="black" /> : <Pause size={16} />}
        </button>
        <p className="text-[11px] text-[var(--text-muted)]">
          {state === 'connecting' ? 'Connecting…' : state === 'ending' ? 'Stopping…' : 'Connected'}
        </p>
      </div>

      <button
        type="button"
        onClick={() => void endSession()}
        disabled={state === 'ending'}
        className="mt-4 w-full px-3 py-2 rounded-lg bg-white/5 hover:bg-red-600/15 text-sm text-[var(--text-secondary)] hover:text-red-300 transition disabled:opacity-50"
      >
        Stop casting
      </button>
    </div>
  );
}

function Header({
  title,
  subtitle,
  onClose,
}: {
  title: string;
  subtitle?: string;
  onClose?: () => void;
}) {
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="min-w-0">
        <p className="text-[10px] uppercase tracking-wider text-[var(--accent)] font-semibold">
          {title}
        </p>
        {subtitle && <p className="text-sm font-medium text-white truncate">{subtitle}</p>}
      </div>
      {onClose && (
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          className="p-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white"
        >
          <X size={14} />
        </button>
      )}
    </div>
  );
}

function formatTime(sec: number): string {
  const s = Math.floor(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  const pad = (n: number) => n.toString().padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(ss)}` : `${m}:${pad(ss)}`;
}
