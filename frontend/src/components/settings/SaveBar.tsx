import { AlertTriangle, ChevronDown, ChevronUp, Loader2, Save, Undo2 } from 'lucide-react';
import { useState } from 'react';

interface SaveBarProps {
  hasChanges: boolean;
  changes: Record<string, unknown>;
  onSave: () => void;
  onDiscard: () => void;
  isSaving?: boolean;
  /** Error from the save mutation. Rendered inline so the user sees
   *  *why* the save failed instead of watching the spinner clear
   *  with no feedback. Ignored while `isSaving` is true (an in-
   *  flight save supersedes the prior error). */
  saveError?: { message?: string } | null;
}

export function SaveBar({
  hasChanges,
  changes,
  onSave,
  onDiscard,
  isSaving,
  saveError,
}: SaveBarProps) {
  const [showDiff, setShowDiff] = useState(false);
  if (!hasChanges) return null;

  const changedKeys = Object.keys(changes);
  const errorText = !isSaving && saveError ? (saveError.message ?? 'Save failed') : null;

  return (
    <div className="fixed bottom-0 left-0 right-0 z-40 md:left-52 lg:left-56">
      <div className="mx-auto max-w-3xl px-4 md:px-8 py-3">
        <div className="rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-xl backdrop-blur-sm overflow-hidden">
          {errorText && (
            <div className="flex items-start gap-2 px-4 py-2 bg-red-500/10 text-red-200 border-b border-red-500/20 text-sm">
              <AlertTriangle size={14} className="mt-0.5 flex-shrink-0" aria-hidden="true" />
              <span>
                <span className="font-semibold">Save failed.</span> {errorText}
              </span>
            </div>
          )}
          <div className="flex items-center justify-between gap-4 px-4 py-2.5">
            <button
              type="button"
              onClick={() => setShowDiff(!showDiff)}
              className="flex items-center gap-1.5 text-sm text-[var(--text-secondary)] hover:text-white transition"
            >
              {changedKeys.length} unsaved change{changedKeys.length !== 1 ? 's' : ''}
              {showDiff ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
            </button>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={onDiscard}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition"
              >
                <Undo2 size={14} />
                Discard
              </button>
              <button
                type="button"
                onClick={onSave}
                disabled={isSaving}
                className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
              >
                {isSaving ? <Loader2 size={14} className="animate-spin" /> : <Save size={14} />}
                {isSaving ? 'Saving...' : 'Save'}
              </button>
            </div>
          </div>

          {showDiff && (
            <div className="px-4 pb-3 border-t border-white/5 pt-2">
              <div className="space-y-1 max-h-32 overflow-y-auto">
                {changedKeys.map((key) => (
                  <div key={key} className="flex items-center gap-2 text-xs">
                    <span className="text-[var(--text-muted)] font-mono w-48 truncate">
                      {key.replace(/_/g, ' ')}
                    </span>
                    <span className="text-[var(--accent)]">→</span>
                    <span className="text-white font-mono truncate">
                      {formatValue(changes[key])}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function formatValue(v: unknown): string {
  if (typeof v === 'boolean') return v ? 'Yes' : 'No';
  if (typeof v === 'string' && v.length > 30) return `${v.slice(0, 30)}...`;
  if (typeof v === 'string' && v === '') return '(empty)';
  return String(v);
}
