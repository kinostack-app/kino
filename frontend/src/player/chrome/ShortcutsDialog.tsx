import { X } from 'lucide-react';

const ROWS: Array<[string, string]> = [
  ['Space / K', 'Play / pause'],
  ['← / →', 'Skip 10 seconds'],
  ['↑ / ↓', 'Volume'],
  ['M', 'Mute'],
  ['F', 'Fullscreen'],
  ['P', 'Picture-in-picture'],
  ['C', 'Subtitles'],
  ['S', 'Skip intro / credits'],
  ['< / >', 'Playback speed'],
  ['?', 'This help'],
  ['Esc', 'Close / back'],
];

/**
 * Keyboard shortcuts overlay. Backdrop click + Escape
 * dismiss. Proper `role=dialog` + `aria-modal` +
 * `aria-labelledby` so screen readers announce it and
 * trap focus.
 */
export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click/keydown dismiss is the standard modal affordance; real controls live inside.
    <div
      role="presentation"
      onClick={onClose}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onClose();
      }}
      className="absolute inset-0 z-50 bg-black/70 backdrop-blur-sm grid place-items-center"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-title"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onClose();
        }}
        className="relative w-[min(420px,90vw)] rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 p-5 text-left cursor-default"
      >
        <div className="flex items-center justify-between mb-4">
          <p id="shortcuts-title" className="text-sm font-semibold">
            Keyboard shortcuts
          </p>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="w-7 h-7 grid place-items-center rounded hover:bg-white/10 text-white/60"
          >
            <X size={14} />
          </button>
        </div>
        <dl className="grid grid-cols-[auto_1fr] gap-y-2 gap-x-4 text-sm">
          {ROWS.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-white/60 tabular-nums">
                <kbd className="px-1.5 py-0.5 rounded bg-white/10 text-xs font-mono">{k}</kbd>
              </dt>
              <dd className="text-white/90">{v}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
