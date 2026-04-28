import * as Dialog from '@radix-ui/react-dialog';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertTriangle,
  CheckCircle,
  ChevronRight,
  ExternalLink,
  FolderOpen,
  FolderPlus,
  HardDrive,
  Home,
  Info,
  Loader2,
  Network,
  RefreshCw,
  Search,
  ShieldAlert,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { browse, mkdir, places, testPath } from '@/api/generated/sdk.gen';
import { cn } from '@/lib/utils';

interface PathBrowserProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Starting directory. Falls back gracefully on the server side. */
  startPath?: string;
  title: string;
  onSelect: (path: string) => void;
  /**
   * When true, the "Use this folder" button refuses paths the kino
   * service user can't write to. Set on Storage / Backup pickers
   * where kino has to write; off for read-only paths.
   */
  requireWritable?: boolean;
}

interface Entry {
  name: string;
  path: string;
  is_dir: boolean;
}

type ErrorState =
  | { kind: 'none' }
  | { kind: 'permission'; path: string; message: string }
  | { kind: 'other'; message: string };

export function PathBrowser({
  open,
  onOpenChange,
  startPath,
  title,
  onSelect,
  requireWritable = true,
}: PathBrowserProps) {
  const qc = useQueryClient();

  const [currentPath, setCurrentPath] = useState<string>(startPath ?? '/');
  const [parent, setParent] = useState<string | null>(null);
  const [fallbackFrom, setFallbackFrom] = useState<string | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorState>({ kind: 'none' });
  const [filter, setFilter] = useState('');
  const [highlight, setHighlight] = useState<number>(-1);
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [newFolderName, setNewFolderName] = useState('');
  const [mkdirError, setMkdirError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const newFolderInputRef = useRef<HTMLInputElement | null>(null);

  const { data: placesData } = useQuery({
    queryKey: ['kino', 'fs', 'places'],
    queryFn: async () => (await places()).data?.places ?? [],
    staleTime: 60_000,
    enabled: open,
  });

  const navigate = useCallback(async (path: string) => {
    setLoading(true);
    setError({ kind: 'none' });
    setHighlight(-1);
    setFilter('');
    try {
      const res = await browse({ query: { path } });
      const data = res.data;
      if (!data) throw new Error('empty response');
      setCurrentPath(data.path);
      setParent(data.parent ?? null);
      setEntries((data.entries ?? []).filter((e): e is Entry => Boolean(e)));
      setFallbackFrom(data.fallback_from ?? null);
    } catch (e) {
      const status = (e as { response?: { status?: number } })?.response?.status;
      if (status === 403) {
        setError({
          kind: 'permission',
          path,
          message: `kino doesn't have permission to read ${path}.`,
        });
      } else {
        setError({
          kind: 'other',
          message: e instanceof Error ? e.message : 'browse failed',
        });
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    navigate(startPath && startPath.length > 0 ? startPath : '/');
    setCreatingFolder(false);
    setNewFolderName('');
    setMkdirError(null);
  }, [open, startPath, navigate]);

  useEffect(() => {
    if (creatingFolder) newFolderInputRef.current?.focus();
  }, [creatingFolder]);

  const dirs = useMemo(() => entries.filter((e) => e.is_dir), [entries]);
  const filtered = useMemo(() => {
    if (!filter.trim()) return dirs;
    const q = filter.toLowerCase();
    return dirs.filter((e) => e.name.toLowerCase().includes(q));
  }, [dirs, filter]);

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

  // biome-ignore lint/correctness/useExhaustiveDependencies: re-runs whenever the path changes is the point
  const writability = useQuery({
    queryKey: ['kino', 'fs', 'test', currentPath],
    queryFn: async () => {
      const { data } = await testPath({ query: { path: currentPath } });
      return data ?? null;
    },
    enabled: open && !loading && error.kind === 'none' && currentPath.length > 0,
    staleTime: 5_000,
  });

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

  useEffect(() => {
    if (highlight < 0 || !listRef.current) return;
    const row = listRef.current.querySelector<HTMLElement>(`[data-row="${highlight}"]`);
    row?.scrollIntoView({ block: 'nearest' });
  }, [highlight]);

  const mkdirMutation = useMutation({
    mutationFn: async (target: string) => {
      const { data } = await mkdir({ body: { path: target } });
      return data;
    },
    onSuccess: async (data) => {
      setCreatingFolder(false);
      setNewFolderName('');
      setMkdirError(null);
      qc.invalidateQueries({ queryKey: ['kino', 'fs', 'places'] });
      if (data?.canonical) await navigate(data.canonical);
    },
    onError: (e) => {
      const status = (e as { response?: { status?: number } })?.response?.status;
      if (status === 403) {
        setMkdirError(
          'kino service user lacks write permission here. Try /var/lib/kino, or run `sudo kino setup-permissions <path>`.'
        );
      } else {
        setMkdirError(e instanceof Error ? e.message : 'create folder failed');
      }
    },
  });

  const createFolder = useCallback(() => {
    const name = newFolderName.trim();
    if (!name) return;
    if (name.includes('/') || name === '.' || name === '..') {
      setMkdirError('Folder name cannot contain slashes or be . / ..');
      return;
    }
    const target = currentPath.endsWith('/') ? `${currentPath}${name}` : `${currentPath}/${name}`;
    mkdirMutation.mutate(target);
  }, [currentPath, newFolderName, mkdirMutation]);

  const select = () => {
    onSelect(currentPath);
    onOpenChange(false);
  };

  const writable = writability.data?.writable === true;
  const writeBlocked = requireWritable && writability.data && !writable;
  const canPick = error.kind === 'none' && !loading && !writeBlocked;

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 grid h-[min(640px,calc(100vh-3rem))] w-[min(820px,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 grid-rows-[auto_auto_1fr_auto] overflow-hidden rounded-xl border border-white/10 bg-[var(--bg-primary)] shadow-2xl data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95">
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

          <div className="grid min-h-0 grid-cols-1 sm:grid-cols-[200px_1fr]">
            <aside className="hidden min-h-0 flex-col overflow-y-auto border-r border-white/5 bg-white/[0.02] py-3 text-xs sm:flex">
              <SidebarSection title="Quick">
                {(placesData ?? [])
                  .filter((p) => p.kind === 'home' || p.kind === 'root')
                  .map((p) => (
                    <SidebarRow
                      key={p.path}
                      icon={p.kind === 'home' ? <Home size={12} /> : <HardDrive size={12} />}
                      label={p.label}
                      onClick={() => navigate(p.path)}
                    />
                  ))}
              </SidebarSection>

              {(placesData ?? []).some((p) => p.kind === 'drive' || p.kind === 'network') && (
                <SidebarSection title="Drives">
                  {(placesData ?? [])
                    .filter((p) => p.kind === 'drive' || p.kind === 'network')
                    .map((p) => (
                      <SidebarRow
                        key={p.path}
                        icon={
                          p.kind === 'network' ? <Network size={12} /> : <HardDrive size={12} />
                        }
                        label={p.label}
                        sublabel={p.sublabel ?? undefined}
                        onClick={() => navigate(p.path)}
                      />
                    ))}
                </SidebarSection>
              )}

              {(placesData ?? []).some((p) => p.kind === 'system') && (
                <SidebarSection title="Other">
                  {(placesData ?? [])
                    .filter((p) => p.kind === 'system')
                    .map((p) => (
                      <SidebarRow
                        key={p.path}
                        icon={<FolderOpen size={12} />}
                        label={p.label}
                        onClick={() => navigate(p.path)}
                        muted
                      />
                    ))}
                </SidebarSection>
              )}
            </aside>

            <div className="flex min-h-0 min-w-0 flex-col">
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

              {creatingFolder && (
                <div className="flex flex-col gap-1.5 border-b border-white/5 bg-[var(--accent)]/5 px-3 py-2">
                  <div className="flex items-center gap-2">
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
                        if (e.key === 'Enter') createFolder();
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
                      onClick={() => {
                        setCreatingFolder(false);
                        setNewFolderName('');
                        setMkdirError(null);
                      }}
                      className="h-7 rounded-md px-2 text-[11px] text-[var(--text-muted)] transition hover:text-white"
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      onClick={createFolder}
                      disabled={!newFolderName.trim() || mkdirMutation.isPending}
                      className="h-7 rounded-md bg-[var(--accent)] px-2 text-[11px] font-semibold text-white transition hover:bg-[var(--accent)]/90 disabled:opacity-50"
                    >
                      {mkdirMutation.isPending ? (
                        <Loader2 size={11} className="animate-spin" />
                      ) : (
                        'Create'
                      )}
                    </button>
                  </div>
                  {mkdirError && <p className="ml-5 text-[11px] text-red-300">{mkdirError}</p>}
                </div>
              )}

              {fallbackFrom && (
                <div className="flex items-start gap-2 border-b border-white/5 bg-amber-500/5 px-3 py-2 text-[11px] text-amber-200/90">
                  <Info size={12} className="mt-0.5 shrink-0 text-amber-300" />
                  <span>
                    <span className="font-mono">{fallbackFrom}</span> doesn't exist; showing the
                    nearest readable folder above.
                  </span>
                </div>
              )}

              {/* biome-ignore lint/a11y/noStaticElementInteractions: container holds buttons; keydown drives the same interactions screen readers reach via Tab */}
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
                ) : error.kind === 'permission' ? (
                  <PermissionBanner path={error.path} onUp={() => parent && navigate(parent)} />
                ) : error.kind === 'other' ? (
                  <div className="p-6 text-center">
                    <p className="text-sm text-red-300">{error.message}</p>
                    {parent && (
                      <button
                        type="button"
                        onClick={() => navigate(parent)}
                        className="mt-3 text-xs text-[var(--accent)] hover:underline"
                      >
                        Go up one level
                      </button>
                    )}
                  </div>
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

          <div className="flex items-center justify-between gap-3 border-t border-white/5 px-4 py-3">
            <div className="min-w-0 flex-1">
              <p className="text-[10px] uppercase tracking-wide text-[var(--text-muted)]">
                Selected
              </p>
              <p className="truncate font-mono text-xs text-white">{currentPath || '—'}</p>
              {writability.data?.exists && requireWritable && (
                <p
                  className={cn(
                    'mt-0.5 flex items-center gap-1 text-[11px]',
                    writable ? 'text-green-300/80' : 'text-amber-300/90'
                  )}
                >
                  {writable ? (
                    <>
                      <CheckCircle size={11} />
                      kino can write here
                      {writability.data.free_bytes != null && (
                        <span className="text-[var(--text-muted)]">
                          {' · '}
                          {formatBytes(writability.data.free_bytes)} free
                        </span>
                      )}
                    </>
                  ) : (
                    <>
                      <AlertTriangle size={11} />
                      kino can't write here — see help below
                    </>
                  )}
                </p>
              )}
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
                disabled={!canPick}
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

function PermissionBanner({ path, onUp }: { path: string; onUp: () => void }) {
  const cmd = `sudo kino setup-permissions ${path}`;
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard.writeText(cmd).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      },
      () => undefined
    );
  };
  return (
    <div className="m-4 rounded-lg bg-amber-500/5 p-4 ring-1 ring-amber-500/20">
      <div className="flex items-start gap-2">
        <ShieldAlert size={14} className="mt-0.5 shrink-0 text-amber-300" />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-amber-200">Permission denied</p>
          <p className="mt-1 text-xs text-amber-200/80">
            kino runs as a separate <span className="font-mono">kino</span> service user and doesn't
            have permission to read{' '}
            <span className="break-all font-mono text-amber-100">{path}</span>. This is the default
            behaviour for external drives that auto-mounted with your desktop user's permissions.
          </p>
          <p className="mt-2 text-xs text-amber-200/80">Grant kino access (one-time):</p>
          <div className="mt-1.5 flex items-center gap-2">
            <code className="block flex-1 overflow-x-auto rounded bg-black/40 px-2 py-1.5 font-mono text-[11px] text-amber-100">
              {cmd}
            </code>
            <button
              type="button"
              onClick={copy}
              className="rounded-md bg-amber-500/15 px-2 py-1 text-[11px] font-semibold text-amber-200 transition hover:bg-amber-500/25"
            >
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <p className="mt-2 text-[11px] text-amber-200/60">
            Reversible: <span className="font-mono">sudo setfacl -R -x u:kino {path}</span>.{' '}
            <a
              href="https://docs.kinostack.app/setup/external-drives"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-0.5 underline hover:text-amber-100"
            >
              More options <ExternalLink size={9} />
            </a>
          </p>
          <button
            type="button"
            onClick={onUp}
            className="mt-3 inline-flex items-center gap-1 text-[11px] font-medium text-amber-300 hover:text-amber-200"
          >
            <RefreshCw size={11} />
            Go up one level
          </button>
        </div>
      </div>
    </div>
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
