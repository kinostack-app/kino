import * as Dialog from '@radix-ui/react-dialog';
import { ChevronLeft, Folder, Loader2, X } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { browse } from '@/api/generated/sdk.gen';

/**
 * Server-side directory picker. Navigates the host filesystem (not the
 * browser's) so the user can pick media library / download paths
 * without having to paste from a terminal.
 */
interface PathBrowserProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Starting directory. Falls back to `/` on failure. */
  startPath?: string;
  title: string;
  onSelect: (path: string) => void;
}

interface Entry {
  name: string;
  path: string;
  is_dir: boolean;
}

export function PathBrowser({ open, onOpenChange, startPath, title, onSelect }: PathBrowserProps) {
  const [currentPath, setCurrentPath] = useState<string>(startPath ?? '/');
  const [parent, setParent] = useState<string | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const navigate = useCallback(async (path: string) => {
    setLoading(true);
    setError(null);
    try {
      const { data } = await browse({ query: { path } });
      if (!data) throw new Error('no response');
      setCurrentPath(data.path);
      setParent(data.parent ?? null);
      setEntries((data.entries ?? []).filter((e): e is Entry => Boolean(e)));
    } catch (e) {
      setError(e instanceof Error ? e.message : 'browse failed');
    } finally {
      setLoading(false);
    }
  }, []);

  // Re-browse whenever the modal opens (or its starting path changes).
  useEffect(() => {
    if (!open) return;
    navigate(startPath && startPath.length > 0 ? startPath : '/');
  }, [open, startPath, navigate]);

  const select = () => {
    onSelect(currentPath);
    onOpenChange(false);
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2 w-[min(600px,calc(100vw-2rem))] max-h-[calc(100vh-4rem)] flex flex-col rounded-xl border border-white/10 bg-[var(--bg-primary)] shadow-2xl data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95">
          <div className="flex items-center justify-between gap-3 px-4 py-3 border-b border-white/5">
            <Dialog.Title className="text-sm font-semibold text-white truncate">
              {title}
            </Dialog.Title>
            <Dialog.Close asChild>
              <button
                type="button"
                className="text-[var(--text-muted)] hover:text-white transition"
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>

          <div className="flex items-center gap-2 px-4 py-2 border-b border-white/5">
            <button
              type="button"
              onClick={() => parent && navigate(parent)}
              disabled={!parent || loading}
              className="h-7 w-7 rounded-md text-[var(--text-muted)] hover:text-white hover:bg-white/5 transition disabled:opacity-30 disabled:cursor-not-allowed flex items-center justify-center flex-shrink-0"
              aria-label="Parent directory"
              title="Parent directory"
            >
              <ChevronLeft size={14} />
            </button>
            <code className="flex-1 text-xs font-mono text-[var(--text-secondary)] truncate">
              {currentPath}
            </code>
          </div>

          <div className="flex-1 overflow-auto">
            {loading ? (
              <div className="flex items-center justify-center py-12 text-[var(--text-muted)]">
                <Loader2 size={16} className="animate-spin" />
              </div>
            ) : error ? (
              <p className="p-6 text-center text-sm text-red-400">{error}</p>
            ) : entries.length === 0 ? (
              <p className="p-6 text-center text-sm text-[var(--text-muted)]">
                No subdirectories here.
              </p>
            ) : (
              <div className="divide-y divide-white/5">
                {entries
                  .filter((e) => e.is_dir)
                  .map((e) => (
                    <button
                      key={e.path}
                      type="button"
                      onClick={() => navigate(e.path)}
                      className="w-full flex items-center gap-2 px-4 py-2 text-left hover:bg-white/[0.03] transition"
                    >
                      <Folder size={14} className="text-[var(--text-muted)] flex-shrink-0" />
                      <span className="text-sm text-white truncate">{e.name}</span>
                    </button>
                  ))}
              </div>
            )}
          </div>

          <div className="flex items-center justify-between gap-3 px-4 py-3 border-t border-white/5">
            <p className="text-xs text-[var(--text-muted)]">Select the folder shown above.</p>
            <div className="flex items-center gap-2">
              <Dialog.Close asChild>
                <button
                  type="button"
                  className="h-8 px-3 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] hover:text-white transition"
                >
                  Cancel
                </button>
              </Dialog.Close>
              <button
                type="button"
                onClick={select}
                disabled={loading || Boolean(error)}
                className="h-8 px-3 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent)]/90 text-sm font-medium text-white transition disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Use this folder
              </button>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
