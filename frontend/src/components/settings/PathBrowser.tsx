import * as Dialog from '@radix-ui/react-dialog';
import { useQuery } from '@tanstack/react-query';
import {
  ChevronRight,
  FolderOpen,
  FolderPlus,
  HardDrive,
  Home,
  Loader2,
  Network,
  Search,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { browse, mkdir, mounts } from '@/api/generated/sdk.gen';
import { cn } from '@/lib/utils';

/**
 * Server-side directory picker.
 *
 * Navigates the host filesystem (the kino server's, not the browser's)
 * via `GET /api/v1/fs/browse`. Used everywhere the user has to pick a
 * folder — setup wizard's Storage step, Settings → Library (media +
 * download paths), Settings → Backup (backup location).
 *
 * Visual contract:
 *   - Two-column body: left sidebar (Home + auto-detected mounts +
 *     common locations) over a right pane (breadcrumb + listing +
 *     selection footer).
 *   - Sidebar entries jump straight to a path; the right pane is the
 *     standard click-into-subfolder navigation.
 *   - Search filters the current directory's listing client-side.
 *   - Keyboard: ↑/↓ highlights, Enter navigates into highlight,
 *     Backspace goes to parent, Esc closes (Radix Dialog primitive
 *     handles Esc + focus restoration; we layer the rest on top).
 *   - "+ New folder" calls `POST /api/v1/fs/mkdir` and re-browses
 *     so the new folder appears in the listing without a manual
 *     refresh.
 *
 * The host's kernel pseudo-filesystems (proc, sysfs, tmpfs) are NOT
 * surfaced — the backend's `/fs/mounts` endpoint already filters
 * those out via a fs-type allowlist.
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
  const [filter, setFilter] = useState('');
  const [highlight, setHighlight] = useState<number>(-1);
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [newFolderName, setNewFolderName] = useState('');
  const [mkdirError, setMkdirError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const newFolderInputRef = useRef<HTMLInputElement | null>(null);

  // Auto-detected mounts (Linux-only on the backend today; macOS /
  // Windows return an empty list). Cached for the open session;
  // changes during the modal's life are exceedingly rare.
  const { data: mountsData } = useQuery({
    queryKey: ['kino', 'fs', 'mounts'],
    queryFn: async () => (await mounts()).data?.mounts ?? [],
    staleTime: 60_000,
    enabled: open,
  });

  const navigate = useCallback(async (path: string) => {
    setLoading(true);
    setError(null);
    setHighlight(-1);
    setFilter('');
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
    setCreatingFolder(false);
    setNewFolderName('');
    setMkdirError(null);
  }, [open, startPath, navigate]);

  const dirs = useMemo(() => entries.filter((e) => e.is_dir), [entries]);
  const filtered = useMemo(() => {
    if (!filter.trim()) return dirs;
    const q = filter.toLowerCase();
    return dirs.filter((e) => e.name.toLowerCase().includes(q));
  }, [dirs, filter]);

  // Breadcrumb segments — each clickable to jump up the tree. Built
  // off the canonical path the backend returned, so multi-segment
  // names (`home/robertsmith/Movies and TV`) come out right without
  // us second-guessing the platform separator.
  const breadcrumbs = useMemo(() => {
    if (!currentPath) return [] as Array<{ label: string; path: string }>;
    if (currentPath === '/') return [{ label: '/', path: '/' }];
    const parts = currentPath.split('/').filter(Boolean);
    const out: Array<{ label: string; path: string }> = [{ label: '/', path: '/' }];
    let acc = '';
    for (const p of parts) {
      acc = `${acc}/${p}`;
      out.push({ label: p, path: acc });
    }
    return out;
  }, [currentPath]);

  // Keyboard handling on the listing region. Radix's Dialog already
  // owns Esc + focus trap; we add ↑/↓/Enter/Backspace.
  const onListKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (filtered.length === 0) return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setHighlight((h) => Math.min(filtered.length - 1, h < 0 ? 0 : h + 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setHighlight((h) => Math.max(0, h - 1));
      } else if (e.key === 'Enter' && highlight >= 0) {
        e.preventDefault();
        const target = filtered[highlight];
        if (target) navigate(target.path);
      } else if (e.key === 'Backspace' && parent) {
        e.preventDefault();
        navigate(parent);
      }
    },
    [filtered, highlight, navigate, parent]
  );

  // Scroll the highlighted row into view when keyboard nav moves it.
  useEffect(() => {
    if (highlight < 0 || !listRef.current) return;
    const row = listRef.current.querySelector<HTMLElement>(`[data-row="${highlight}"]`);
    row?.scrollIntoView({ block: 'nearest' });
  }, [highlight]);

  // Focus the new-folder input the moment its row appears. Done via
  // ref + effect rather than the autoFocus attribute so biome's
  // a11y rule (which warns on autoFocus because of screen-reader
  // surprise) stays clean — and so the focus only fires when the
  // user explicitly opened the row.
  useEffect(() => {
    if (creatingFolder) newFolderInputRef.current?.focus();
  }, [creatingFolder]);

  const select = () => {
    onSelect(currentPath);
    onOpenChange(false);
  };

  const createFolder = useCallback(async () => {
    const name = newFolderName.trim();
    if (!name) return;
    if (name.includes('/') || name === '.' || name === '..') {
      setMkdirError('Folder name cannot contain slashes or be . / ..');
      return;
    }
    const target = currentPath.endsWith('/') ? `${currentPath}${name}` : `${currentPath}/${name}`;
    try {
      await mkdir({ body: { path: target } });
      setCreatingFolder(false);
      setNewFolderName('');
      setMkdirError(null);
      // Re-browse so the new folder appears + immediately navigate
      // into it (likely what the user wants — they made it to use it).
      await navigate(target);
    } catch (e) {
      setMkdirError(e instanceof Error ? e.message : 'mkdir failed');
    }
  }, [currentPath, newFolderName, navigate]);

  // Common-locations list. We only show entries that exist on the
  // host — which we can't probe synchronously, so emit them all
  // and let the click → navigate's error path handle missing ones
  // (they fall through to the "browse failed" empty state).
  // Cheap UX win on macOS / Windows where mount enumeration is
  // empty: users still get one-click access to /Volumes etc.
  const commonLocations = [
    { label: '/media', path: '/media' },
    { label: '/mnt', path: '/mnt' },
    { label: '/Volumes', path: '/Volumes' },
    { label: '/var/lib/kino', path: '/var/lib/kino' },
  ];

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100vh-4rem)] w-[min(820px,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-white/10 bg-[var(--bg-primary)] shadow-2xl data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95">
          <div className="flex items-center justify-between gap-3 border-b border-white/5 px-4 py-3">
            <Dialog.Title className="truncate text-sm font-semibold text-white">
              {title}
            </Dialog.Title>
            <Dialog.Close asChild>
              <button
                type="button"
                className="text-[var(--text-muted)] transition hover:text-white"
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>

          {/* Breadcrumb path — each segment is clickable. */}
          <nav
            aria-label="Path breadcrumbs"
            className="flex items-center gap-0.5 overflow-x-auto border-b border-white/5 px-4 py-2 text-xs"
          >
            {breadcrumbs.map((b, i) => (
              <div key={b.path} className="flex shrink-0 items-center gap-0.5">
                {i > 0 && (
                  <ChevronRight
                    size={12}
                    className="shrink-0 text-[var(--text-muted)] opacity-60"
                  />
                )}
                <button
                  type="button"
                  onClick={() => navigate(b.path)}
                  disabled={b.path === currentPath || loading}
                  className={cn(
                    'rounded px-1.5 py-0.5 font-mono transition',
                    b.path === currentPath
                      ? 'text-white'
                      : 'text-[var(--text-muted)] hover:bg-white/5 hover:text-white'
                  )}
                >
                  {b.label}
                </button>
              </div>
            ))}
          </nav>

          {/* Two-column body: sidebar + listing. */}
          <div className="grid min-h-0 flex-1 grid-cols-1 sm:grid-cols-[200px_1fr]">
            <aside className="hidden min-h-0 flex-col overflow-y-auto border-r border-white/5 bg-white/[0.02] py-3 text-xs sm:flex">
              <SidebarSection title="Quick">
                <SidebarRow
                  icon={<Home size={12} />}
                  label="Home"
                  onClick={() => navigate('/root')}
                />
                <SidebarRow
                  icon={<HardDrive size={12} />}
                  label="/ (root)"
                  onClick={() => navigate('/')}
                />
              </SidebarSection>

              {mountsData && mountsData.length > 0 && (
                <SidebarSection title="Drives">
                  {mountsData.map((m) => (
                    <SidebarRow
                      key={m.path}
                      icon={
                        m.fs_type === 'nfs' ||
                        m.fs_type === 'nfs4' ||
                        m.fs_type === 'cifs' ||
                        m.fs_type === 'smbfs' ||
                        m.fs_type === 'smb3' ? (
                          <Network size={12} />
                        ) : (
                          <HardDrive size={12} />
                        )
                      }
                      label={m.label}
                      sublabel={
                        m.free_bytes != null
                          ? `${formatBytes(m.free_bytes)} free · ${m.fs_type}`
                          : m.fs_type
                      }
                      onClick={() => navigate(m.path)}
                    />
                  ))}
                </SidebarSection>
              )}

              <SidebarSection title="Common">
                {commonLocations.map((c) => (
                  <SidebarRow
                    key={c.path}
                    icon={<FolderOpen size={12} />}
                    label={c.label}
                    onClick={() => navigate(c.path)}
                    muted
                  />
                ))}
              </SidebarSection>
            </aside>

            <div className="flex min-h-0 min-w-0 flex-col">
              {/* Listing toolbar — search + new-folder. */}
              <div className="flex items-center gap-2 border-b border-white/5 px-3 py-2">
                <div className="relative flex-1">
                  <Search
                    size={12}
                    className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
                  />
                  <input
                    type="text"
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                    placeholder="Filter folders…"
                    className="h-7 w-full rounded-md bg-white/5 pl-7 pr-2 text-xs text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                  />
                </div>
                <button
                  type="button"
                  onClick={() => setCreatingFolder((v) => !v)}
                  title="Create a new folder here"
                  className="flex h-7 items-center gap-1 rounded-md bg-white/5 px-2 text-[11px] text-[var(--text-muted)] ring-1 ring-white/10 transition hover:bg-white/10 hover:text-white"
                >
                  <FolderPlus size={12} />
                  New folder
                </button>
              </div>

              {/* Inline create-folder row. Slides in below the toolbar
                  so users keep their place in the listing while typing. */}
              {creatingFolder && (
                <div className="flex items-center gap-2 border-b border-white/5 bg-[var(--accent)]/5 px-3 py-2">
                  <FolderPlus size={12} className="text-[var(--accent)]" />
                  <input
                    ref={newFolderInputRef}
                    type="text"
                    value={newFolderName}
                    onChange={(e) => {
                      setNewFolderName(e.target.value);
                      if (mkdirError) setMkdirError(null);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') void createFolder();
                      else if (e.key === 'Escape') {
                        setCreatingFolder(false);
                        setNewFolderName('');
                        setMkdirError(null);
                      }
                    }}
                    placeholder="folder name"
                    className="h-7 flex-1 rounded bg-white/5 px-2 font-mono text-xs text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                  />
                  <button
                    type="button"
                    onClick={() => void createFolder()}
                    disabled={!newFolderName.trim()}
                    className="h-7 rounded-md bg-[var(--accent)] px-2 text-[11px] font-semibold text-white transition hover:bg-[var(--accent)]/90 disabled:opacity-50"
                  >
                    Create
                  </button>
                  {mkdirError && <p className="text-[11px] text-red-400">{mkdirError}</p>}
                </div>
              )}

              {/* Listing — keyboard-nav region. */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: container holds buttons, keydown drives the same interactions screen readers reach via Tab into the buttons. */}
              <div
                ref={listRef}
                tabIndex={-1}
                onKeyDown={onListKeyDown}
                className="min-h-0 flex-1 overflow-auto focus:outline-none"
              >
                {loading ? (
                  <div className="flex items-center justify-center py-12 text-[var(--text-muted)]">
                    <Loader2 size={16} className="animate-spin" />
                  </div>
                ) : error ? (
                  <p className="p-6 text-center text-sm text-red-400">{error}</p>
                ) : filtered.length === 0 ? (
                  <div className="p-6 text-center">
                    <p className="text-sm text-[var(--text-muted)]">
                      {filter ? `No folders match "${filter}".` : 'No subfolders here.'}
                    </p>
                    {!filter && (
                      <p className="mt-1 text-xs text-[var(--text-muted)]/60">
                        Use "Use this folder" below to pick this directory, or "New folder" to
                        create one.
                      </p>
                    )}
                  </div>
                ) : (
                  <div className="divide-y divide-white/5">
                    {filtered.map((e, i) => (
                      <button
                        key={e.path}
                        type="button"
                        data-row={i}
                        onClick={() => navigate(e.path)}
                        onMouseEnter={() => setHighlight(i)}
                        className={cn(
                          'flex w-full items-center gap-2 px-4 py-2 text-left transition',
                          i === highlight ? 'bg-white/[0.05]' : 'hover:bg-white/[0.03]'
                        )}
                      >
                        <FolderOpen size={14} className="shrink-0 text-[var(--text-muted)]" />
                        <span className="truncate text-sm text-white">{e.name}</span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Selection footer. */}
          <div className="flex items-center justify-between gap-3 border-t border-white/5 px-4 py-3">
            <div className="min-w-0">
              <p className="text-[10px] uppercase tracking-wide text-[var(--text-muted)]">
                Selected
              </p>
              <p className="truncate font-mono text-xs text-white">{currentPath || '—'}</p>
            </div>
            <div className="flex items-center gap-2">
              <Dialog.Close asChild>
                <button
                  type="button"
                  className="h-8 rounded-lg bg-white/5 px-3 text-sm text-[var(--text-secondary)] transition hover:bg-white/10 hover:text-white"
                >
                  Cancel
                </button>
              </Dialog.Close>
              <button
                type="button"
                onClick={select}
                disabled={loading || Boolean(error)}
                className="h-8 rounded-lg bg-[var(--accent)] px-3 text-sm font-medium text-white transition hover:bg-[var(--accent)]/90 disabled:cursor-not-allowed disabled:opacity-50"
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

function SidebarSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mb-3 last:mb-0">
      <p className="px-3 pb-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-muted)]/70">
        {title}
      </p>
      <div className="space-y-px">{children}</div>
    </div>
  );
}

function SidebarRow({
  icon,
  label,
  sublabel,
  onClick,
  muted = false,
}: {
  icon: React.ReactNode;
  label: string;
  sublabel?: string;
  onClick: () => void;
  muted?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'flex w-full items-center gap-2 px-3 py-1.5 text-left transition hover:bg-white/[0.04]',
        muted && 'opacity-70'
      )}
    >
      <span className="shrink-0 text-[var(--text-muted)]">{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-xs text-white">{label}</span>
        {sublabel && (
          <span className="block truncate text-[10px] text-[var(--text-muted)]">{sublabel}</span>
        )}
      </span>
    </button>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}
