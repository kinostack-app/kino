import { Check, Copy } from 'lucide-react';
import { useState } from 'react';
import type { LoadingOverlay } from '../types';

/**
 * Loading-overlay error surface. Shown in place of the
 * stage stepper when the download row reports
 * `state='failed'` or prepare errored out.
 *
 * The message sits in a copy-friendly panel:
 * `user-select: text` enabled, plus an explicit Copy
 * button so reporting the error in chat is one click away.
 * Action + retry buttons sit underneath.
 */
export function ErrorCard({ error }: { error: NonNullable<LoadingOverlay['error']> }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard
      .writeText(error.message)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {
        // Clipboard permission denied / insecure context
        // (http:// over LAN). The error text is still
        // selectable; user can copy manually.
      });
  };
  return (
    <div className="pointer-events-auto flex flex-col items-center gap-3 max-w-lg w-full">
      <div className="relative w-full rounded-lg bg-red-950/40 ring-1 ring-red-500/30 px-4 py-3 pr-12 backdrop-blur-sm">
        <p className="text-sm text-amber-100 font-mono leading-relaxed select-text whitespace-pre-wrap break-words">
          {error.message}
        </p>
        <button
          type="button"
          onClick={copy}
          aria-label={copied ? 'Copied' : 'Copy error message'}
          className="absolute top-2 right-2 w-7 h-7 grid place-items-center rounded-md bg-white/5 hover:bg-white/15 text-white/70 hover:text-white transition"
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </div>
      {(error.action || error.retry) && (
        <div className="flex gap-2">
          {error.action && (
            <button
              type="button"
              onClick={error.action.onClick}
              className="px-4 py-2 rounded-lg bg-white/10 hover:bg-white/15 text-white text-sm font-semibold backdrop-blur-sm"
            >
              {error.action.label}
            </button>
          )}
          {error.retry && (
            <button
              type="button"
              onClick={error.retry}
              className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold"
            >
              Try again
            </button>
          )}
        </div>
      )}
    </div>
  );
}
