import { useQueryClient } from '@tanstack/react-query';
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  ChevronDown,
  ChevronUp,
  Download,
  Pause,
  Play,
  RotateCcw,
  Search,
  Trash2,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  blocklistAndSearch,
  cancelDownload,
  pauseDownload,
  resumeDownload,
  retryDownload,
} from '@/api/generated/sdk.gen';
import { cn } from '@/lib/utils';
import {
  type ActiveDownload,
  DOWNLOADS_KEY,
  LIBRARY_MOVIES_KEY,
  useDownloads,
} from '@/state/library-cache';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';
import { DownloadsDetailPane } from './downloads/DownloadsDetailPane';
import {
  ACTIVE_STATES,
  formatBytes,
  formatEta,
  formatRelativeTime,
  formatSpeed,
  stateDisplay,
} from './downloads/formatters';

type FilterId = 'all' | 'active' | 'paused' | 'seeding' | 'failed' | 'imported';
type SortColumn =
  | 'title'
  | 'state'
  | 'progress'
  | 'size'
  | 'down_speed'
  | 'up_speed'
  | 'eta'
  | 'peers'
  | 'added';
type SortDir = 'asc' | 'desc';

const FILTER_OPTIONS: { id: FilterId; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'active', label: 'Active' },
  { id: 'paused', label: 'Paused' },
  { id: 'seeding', label: 'Seeding' },
  { id: 'failed', label: 'Failed' },
  { id: 'imported', label: 'Imported' },
];

function matchesFilter(download: ActiveDownload, filter: FilterId): boolean {
  switch (filter) {
    case 'all':
      return true;
    case 'active':
      return ACTIVE_STATES.has(download.state);
    case 'paused':
      return download.state === 'paused';
    case 'seeding':
      return download.state === 'seeding';
    case 'failed':
      return download.state === 'failed';
    case 'imported':
      return download.state === 'imported';
  }
}

function sortValue(d: ActiveDownload, column: SortColumn): number | string {
  switch (column) {
    case 'title':
      return d.title.toLowerCase();
    case 'state':
      return d.state;
    case 'progress':
      return d.size && d.size > 0 ? d.downloaded / d.size : 0;
    case 'size':
      return d.size ?? 0;
    case 'down_speed':
      return d.download_speed ?? 0;
    case 'up_speed':
      return d.upload_speed ?? 0;
    case 'eta':
      return d.eta ?? Number.POSITIVE_INFINITY;
    case 'peers':
      return (d.seeders ?? 0) * 1000 + (d.leechers ?? 0);
    case 'added':
      return Date.parse(d.added_at);
  }
}

export function DownloadingTab() {
  const { data, isLoading } = useDownloads();
  const downloads = data ?? [];

  const [filter, setFilter] = useState<FilterId>('all');
  const [query, setQuery] = useState('');
  const [sortColumn, setSortColumn] = useState<SortColumn>('state');
  const [sortDir, setSortDir] = useState<SortDir>('asc');
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [focusedId, setFocusedId] = useState<number | null>(null);
  const [lastClickId, setLastClickId] = useState<number | null>(null);

  const handleSort = (col: SortColumn) => {
    if (sortColumn === col) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortColumn(col);
      setSortDir(col === 'title' ? 'asc' : 'desc');
    }
  };

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return downloads
      .filter((d) => matchesFilter(d, filter))
      .filter((d) => (q ? d.title.toLowerCase().includes(q) : true));
  }, [downloads, filter, query]);

  const sorted = useMemo(() => {
    const copy = [...filtered];
    copy.sort((a, b) => {
      const av = sortValue(a, sortColumn);
      const bv = sortValue(b, sortColumn);
      const cmp =
        typeof av === 'string' ? av.localeCompare(bv as string) : (av as number) - (bv as number);
      return sortDir === 'asc' ? cmp : -cmp;
    });
    return copy;
  }, [filtered, sortColumn, sortDir]);

  // Clean up selection when rows disappear from the filtered list.
  useEffect(() => {
    setSelectedIds((prev) => {
      const visibleIds = new Set(sorted.map((d) => d.id));
      const next = new Set<number>();
      for (const id of prev) if (visibleIds.has(id)) next.add(id);
      return next.size === prev.size ? prev : next;
    });
  }, [sorted]);

  const focusedDownload = useMemo(
    () => downloads.find((d) => d.id === focusedId) ?? null,
    [downloads, focusedId]
  );

  const toggleRow = (id: number, event: React.MouseEvent) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (event.shiftKey && lastClickId != null) {
        // Range select from lastClick → id on the visible-sorted list.
        const ids = sorted.map((d) => d.id);
        const from = ids.indexOf(lastClickId);
        const to = ids.indexOf(id);
        if (from !== -1 && to !== -1) {
          const [a, b] = from < to ? [from, to] : [to, from];
          for (let i = a; i <= b; i++) next.add(ids[i]);
        } else {
          next.add(id);
        }
      } else if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
    setLastClickId(id);
    setFocusedId(id);
  };

  // Only sum speeds for states where librqbit is actively transferring.
  // Paused / failed / imported rows retain their last-reported speed in
  // the DB (no UPDATE resets it), so summing every row left the totals
  // stuck at the last live number even when nothing was transferring.
  const { totalDown, totalUp } = useMemo(() => {
    let d = 0;
    let u = 0;
    for (const row of downloads) {
      if (row.state === 'downloading' || row.state === 'stalled' || row.state === 'seeding') {
        d += row.download_speed ?? 0;
        u += row.upload_speed ?? 0;
      }
    }
    return { totalDown: d, totalUp: u };
  }, [downloads]);

  // Action targets: ticked rows take priority, else the focused row.
  // Keeping the rule simple means the toolbar's enable/disable state
  // tracks what the user "pointed at last" without hidden modes.
  const targets = useMemo<ActiveDownload[]>(() => {
    if (selectedIds.size > 0) {
      return downloads.filter((d) => selectedIds.has(d.id));
    }
    return focusedDownload ? [focusedDownload] : [];
  }, [selectedIds, downloads, focusedDownload]);

  if (isLoading) {
    return (
      <div className="space-y-3">
        {Array.from({ length: 5 }, (_, i) => (
          <div key={String(i)} className="h-10 skeleton rounded" />
        ))}
      </div>
    );
  }
  if (downloads.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center min-h-[40vh] text-[var(--text-muted)]">
        <Download size={48} className="mb-4 opacity-30" />
        <p className="text-lg mb-2">Nothing downloading</p>
        <p className="text-sm">Active transfers will appear here</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-[calc(100vh-14rem)]">
      <Toolbar
        filter={filter}
        onFilterChange={setFilter}
        query={query}
        onQueryChange={setQuery}
        totalDown={totalDown}
        totalUp={totalUp}
        targets={targets}
        ticked={selectedIds.size > 0}
      />
      <div className="flex-1 min-h-0 overflow-auto rounded-lg ring-1 ring-white/5">
        <table className="w-full text-xs">
          <thead className="sticky top-0 bg-[var(--bg-secondary)] z-10">
            <tr className="border-b border-white/10 text-[10px] uppercase tracking-wide text-[var(--text-muted)]">
              <th className="w-8 px-2 py-2">
                <HeaderCheckbox
                  visible={sorted}
                  selectedIds={selectedIds}
                  onToggleAll={(check) =>
                    setSelectedIds(check ? new Set(sorted.map((d) => d.id)) : new Set())
                  }
                />
              </th>
              <ThSort
                label="Name"
                column="title"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="text-left min-w-0"
              />
              <ThSort
                label="State"
                column="state"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-28 text-left"
              />
              <ThSort
                label="Progress"
                column="progress"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-48"
              />
              <ThSort
                label="Size"
                column="size"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-24 text-right"
              />
              <ThSort
                label="↓"
                column="down_speed"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-20 text-right"
              />
              <ThSort
                label="↑"
                column="up_speed"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-20 text-right"
              />
              <ThSort
                label="ETA"
                column="eta"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-16 text-right"
              />
              <ThSort
                label="Peers"
                column="peers"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-20 text-center"
              />
              <ThSort
                label="Added"
                column="added"
                sortColumn={sortColumn}
                sortDir={sortDir}
                onSort={handleSort}
                className="w-24 text-right"
              />
              <th className="w-32 px-2 py-2 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {sorted.length === 0 && (
              <tr>
                <td
                  colSpan={11}
                  className="px-6 py-12 text-center text-sm text-[var(--text-muted)]"
                >
                  No downloads match this filter
                </td>
              </tr>
            )}
            {sorted.map((d) => (
              <DownloadRow
                key={d.id}
                download={d}
                selected={selectedIds.has(d.id)}
                focused={focusedId === d.id}
                onToggle={(e) => toggleRow(d.id, e)}
                onFocus={() => setFocusedId(d.id)}
              />
            ))}
          </tbody>
        </table>
      </div>
      <div className="h-96 mt-3 rounded-lg ring-1 ring-white/5 overflow-hidden flex-shrink-0">
        <DownloadsDetailPane download={focusedDownload} />
      </div>
    </div>
  );
}

function HeaderCheckbox({
  visible,
  selectedIds,
  onToggleAll,
}: {
  visible: ActiveDownload[];
  selectedIds: Set<number>;
  onToggleAll: (check: boolean) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const allSelected = visible.length > 0 && visible.every((d) => selectedIds.has(d.id));
  const someSelected = visible.some((d) => selectedIds.has(d.id));
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = someSelected && !allSelected;
  }, [someSelected, allSelected]);
  return (
    <input
      ref={ref}
      type="checkbox"
      checked={allSelected}
      onChange={(e) => onToggleAll(e.target.checked)}
      aria-label={allSelected ? 'Deselect all' : 'Select all'}
      className="h-3.5 w-3.5 rounded border-white/20 bg-white/5 text-[var(--accent)] focus:ring-[var(--accent)]"
    />
  );
}

function ThSort({
  label,
  column,
  sortColumn,
  sortDir,
  onSort,
  className,
}: {
  label: string;
  column: SortColumn;
  sortColumn: SortColumn;
  sortDir: SortDir;
  onSort: (c: SortColumn) => void;
  className?: string;
}) {
  const active = sortColumn === column;
  return (
    <th className={cn('px-2 py-2 font-medium select-none', className)}>
      <button
        type="button"
        onClick={() => onSort(column)}
        className={cn(
          'inline-flex items-center gap-1 hover:text-white transition',
          active && 'text-white'
        )}
      >
        {label}
        {active ? (
          sortDir === 'asc' ? (
            <ChevronUp size={12} />
          ) : (
            <ChevronDown size={12} />
          )
        ) : (
          <ArrowUpDown size={10} className="opacity-40" />
        )}
      </button>
    </th>
  );
}

function Toolbar({
  filter,
  onFilterChange,
  query,
  onQueryChange,
  totalDown,
  totalUp,
  targets,
  ticked,
}: {
  filter: FilterId;
  onFilterChange: (f: FilterId) => void;
  query: string;
  onQueryChange: (s: string) => void;
  totalDown: number;
  totalUp: number;
  /** Rows the toolbar actions should operate on. Ticked rows take
   *  priority; otherwise the click-focused row. Always reflects the
   *  current selection so buttons light up / grey out in real time. */
  targets: ActiveDownload[];
  /** Whether any rows are ticked — affects the label next to the
   *  action buttons so the user can see whether an action will hit
   *  multiple rows or the one they clicked. */
  ticked: boolean;
}) {
  const qc = useQueryClient();
  const invalidate = () => {
    qc.invalidateQueries({ queryKey: [...DOWNLOADS_KEY] });
    qc.invalidateQueries({ queryKey: [...LIBRARY_MOVIES_KEY] });
  };
  const apply = async (action: (id: number) => Promise<unknown>) => {
    await Promise.allSettled(targets.map((d) => action(d.id)));
    invalidate();
  };

  // Enablement derived from target states. Each button is enabled
  // when at least one target is in a state where that action makes
  // sense. Remove works on any state — handy for clearing imported
  // rows out of the table when the filter is "All".
  const canPause = targets.some((d) => d.state === 'downloading' || d.state === 'stalled');
  const canResume = targets.some((d) => d.state === 'paused');
  const canRetry = targets.some((d) => d.state === 'failed');
  const canRemove = targets.length > 0;

  const targetLabel =
    targets.length === 0
      ? ''
      : ticked
        ? `${targets.length} ticked`
        : targets.length === 1
          ? 'focused row'
          : `${targets.length} rows`;

  return (
    <div className="flex flex-wrap items-center gap-2 mb-3">
      <div className="flex items-center gap-1 rounded-lg ring-1 ring-white/5 p-0.5">
        {FILTER_OPTIONS.map((opt) => (
          <button
            key={opt.id}
            type="button"
            onClick={() => onFilterChange(opt.id)}
            className={cn(
              'px-2.5 py-1 rounded-md text-xs font-medium transition',
              filter === opt.id
                ? 'bg-white/10 text-white'
                : 'text-[var(--text-muted)] hover:text-white'
            )}
          >
            {opt.label}
          </button>
        ))}
      </div>

      <div className="relative">
        <Search
          size={12}
          className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
        />
        <input
          type="search"
          placeholder="Search downloads…"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          className="pl-7 pr-3 py-1.5 rounded-md bg-white/5 ring-1 ring-white/5 text-xs w-56 focus:outline-none focus:ring-[var(--accent)]/40"
        />
      </div>

      <div className="flex items-center gap-1 ml-1 pl-2 border-l border-white/10">
        <ToolbarAction
          icon={<Pause size={12} />}
          label="Pause"
          enabled={canPause}
          onClick={() => apply((id) => pauseDownload({ path: { id } }))}
        />
        <ToolbarAction
          icon={<Play size={12} />}
          label="Resume"
          enabled={canResume}
          onClick={() => apply((id) => resumeDownload({ path: { id } }))}
        />
        <ToolbarAction
          icon={<RotateCcw size={12} />}
          label="Retry"
          enabled={canRetry}
          onClick={() => apply((id) => retryDownload({ path: { id } }))}
        />
        <ToolbarAction
          icon={<Trash2 size={12} />}
          label="Remove"
          enabled={canRemove}
          danger
          onClick={() => apply((id) => cancelDownload({ path: { id } }))}
        />
        {targetLabel && (
          <span className="ml-2 text-[10px] text-[var(--text-muted)]">{targetLabel}</span>
        )}
      </div>

      <div className="flex items-center gap-3 text-xs text-[var(--text-muted)] ml-auto">
        <span className="flex items-center gap-1 text-sky-400">
          <ArrowDown size={12} />
          {formatSpeed(totalDown)}
        </span>
        <span className="flex items-center gap-1 text-emerald-400">
          <ArrowUp size={12} />
          {formatSpeed(totalUp)}
        </span>
      </div>
    </div>
  );
}

function ToolbarAction({
  icon,
  label,
  enabled,
  danger,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  enabled: boolean;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={enabled ? onClick : undefined}
      disabled={!enabled}
      title={label}
      className={cn(
        'inline-flex items-center gap-1 px-2.5 py-1 rounded text-xs font-medium transition',
        enabled
          ? danger
            ? 'bg-rose-500/15 hover:bg-rose-500/25 text-rose-200'
            : 'bg-white/5 hover:bg-white/10 text-white'
          : 'bg-white/[0.02] text-[var(--text-muted)]/60 cursor-not-allowed'
      )}
    >
      {icon}
      {label}
    </button>
  );
}

function DownloadRow({
  download: d,
  selected,
  focused,
  onToggle,
  onFocus,
}: {
  download: ActiveDownload;
  selected: boolean;
  focused: boolean;
  onToggle: (e: React.MouseEvent) => void;
  onFocus: () => void;
}) {
  const qc = useQueryClient();
  const [showDelete, setShowDelete] = useState(false);
  const invalidate = () => {
    qc.invalidateQueries({ queryKey: [...DOWNLOADS_KEY] });
    qc.invalidateQueries({ queryKey: [...LIBRARY_MOVIES_KEY] });
  };

  const pauseMut = useMutationWithToast({
    verb: 'pause download',
    mutationFn: () => pauseDownload({ path: { id: d.id } }),
    onSuccess: invalidate,
  });
  const resumeMut = useMutationWithToast({
    verb: 'resume download',
    mutationFn: () => resumeDownload({ path: { id: d.id } }),
    onSuccess: invalidate,
  });
  const retryMut = useMutationWithToast({
    verb: 'retry download',
    mutationFn: () => retryDownload({ path: { id: d.id } }),
    onSuccess: invalidate,
  });
  const cancelMut = useMutationWithToast({
    verb: 'cancel download',
    mutationFn: () => cancelDownload({ path: { id: d.id } }),
    onSuccess: invalidate,
  });
  const blocklistMut = useMutationWithToast({
    verb: 'blocklist release',
    mutationFn: () => blocklistAndSearch({ path: { id: d.id } }),
    onSuccess: invalidate,
  });

  const state = stateDisplay(d.state);
  const progress = d.size && d.size > 0 ? (d.downloaded / d.size) * 100 : 0;

  return (
    <>
      <tr
        onClick={onFocus}
        className={cn(
          'border-b border-white/5 transition cursor-default',
          focused ? 'bg-white/[0.05]' : selected ? 'bg-white/[0.03]' : 'hover:bg-white/[0.02]'
        )}
      >
        {/* biome-ignore lint/a11y/useKeyWithClickEvents: stopPropagation prevents the row's focus-on-click from firing when the user clicks the checkbox. Keyboard navigation reaches the checkbox via tab order and space-toggles directly — no row-level keyboard interaction to gate */}
        <td className="px-2 py-1.5" onClick={(e) => e.stopPropagation()}>
          <input
            type="checkbox"
            checked={selected}
            onClick={(e) => onToggle(e)}
            onChange={() => {}}
            aria-label={`Select ${d.title}`}
            className="h-3.5 w-3.5 rounded border-white/20 bg-white/5 text-[var(--accent)] focus:ring-[var(--accent)]"
          />
        </td>
        <td className="px-2 py-1.5 max-w-0 truncate" title={d.title}>
          <span className="text-white">{d.title}</span>
          {d.error_message && (
            <span className="ml-2 text-[10px] text-rose-300 truncate" title={d.error_message}>
              · {d.error_message}
            </span>
          )}
        </td>
        <td className="px-2 py-1.5">
          <span
            className={cn(
              'inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium',
              state.chipClass
            )}
          >
            {state.label}
          </span>
        </td>
        <td className="px-2 py-1.5">
          <div className="flex items-center gap-2">
            <div className="flex-1 h-1.5 rounded-full bg-white/10 overflow-hidden">
              <div
                className={cn('h-full transition-all', state.barClass)}
                style={{ width: `${Math.min(progress, 100).toFixed(1)}%` }}
              />
            </div>
            <span className="w-10 text-right text-[var(--text-secondary)] tabular-nums">
              {progress.toFixed(0)}%
            </span>
          </div>
        </td>
        <td className="px-2 py-1.5 text-right tabular-nums text-[var(--text-secondary)]">
          {formatBytes(d.size)}
        </td>
        <td className="px-2 py-1.5 text-right tabular-nums">
          <span className={d.download_speed > 0 ? 'text-sky-300' : 'text-[var(--text-muted)]'}>
            {formatSpeed(d.download_speed)}
          </span>
        </td>
        <td className="px-2 py-1.5 text-right tabular-nums">
          <span className={d.upload_speed > 0 ? 'text-emerald-300' : 'text-[var(--text-muted)]'}>
            {formatSpeed(d.upload_speed)}
          </span>
        </td>
        <td className="px-2 py-1.5 text-right tabular-nums text-[var(--text-secondary)]">
          {formatEta(d.eta)}
        </td>
        <td className="px-2 py-1.5 text-center tabular-nums text-[var(--text-secondary)]">
          {(d.seeders ?? 0) === 0 && (d.leechers ?? 0) === 0
            ? '—'
            : `${d.seeders ?? 0}/${d.leechers ?? 0}`}
        </td>
        <td className="px-2 py-1.5 text-right text-[var(--text-muted)]">
          {formatRelativeTime(d.added_at)}
        </td>
        {/* biome-ignore lint/a11y/useKeyWithClickEvents: stopPropagation prevents the row's focus-on-click from firing when the user clicks the action buttons. The buttons themselves handle keyboard activation; this <td> only swallows mouse-event bubbling */}
        <td className="px-2 py-1.5" onClick={(e) => e.stopPropagation()}>
          <div className="flex items-center justify-end gap-1">
            {d.state === 'downloading' || d.state === 'stalled' ? (
              <IconBtn onClick={() => pauseMut.mutate()} title="Pause">
                <Pause size={13} />
              </IconBtn>
            ) : null}
            {d.state === 'paused' && (
              <IconBtn onClick={() => resumeMut.mutate()} title="Resume">
                <Play size={13} />
              </IconBtn>
            )}
            {d.state === 'failed' && (
              <IconBtn onClick={() => retryMut.mutate()} title="Retry">
                <RotateCcw size={13} />
              </IconBtn>
            )}
            <IconBtn
              onClick={() => setShowDelete(true)}
              title="Remove"
              className="hover:text-rose-300"
            >
              <Trash2 size={13} />
            </IconBtn>
          </div>
        </td>
      </tr>
      {showDelete &&
        createPortal(
          <RemoveDialog
            title={d.title}
            onClose={() => setShowDelete(false)}
            onBlocklistAndSearch={() => {
              blocklistMut.mutate();
              setShowDelete(false);
            }}
            onCancel={() => {
              cancelMut.mutate();
              setShowDelete(false);
            }}
          />,
          document.body
        )}
    </>
  );
}

function IconBtn({
  children,
  onClick,
  title,
  className,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  className?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-label={title}
      className={cn(
        'p-1.5 rounded text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition',
        className
      )}
    >
      {children}
    </button>
  );
}

function RemoveDialog({
  title,
  onClose,
  onBlocklistAndSearch,
  onCancel,
}: {
  title: string;
  onClose: () => void;
  onBlocklistAndSearch: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onClose();
      }}
      role="dialog"
      aria-modal="true"
    >
      <div className="bg-[var(--bg-secondary)] rounded-xl p-6 max-w-sm w-full border border-white/10 shadow-2xl">
        <h3 className="font-semibold text-white mb-1">Remove download</h3>
        <p className="text-sm text-[var(--text-muted)] mb-5 truncate">{title}</p>
        <div className="space-y-2">
          <button
            type="button"
            onClick={onBlocklistAndSearch}
            className="w-full flex items-center gap-3 px-4 py-3 rounded-lg bg-white/5 hover:bg-white/10 text-left transition"
          >
            <RotateCcw size={16} className="text-[var(--accent)] flex-shrink-0" />
            <div>
              <p className="text-sm font-medium text-white">Search for another release</p>
              <p className="text-xs text-[var(--text-muted)]">
                Block this release and find a different one
              </p>
            </div>
          </button>
          <button
            type="button"
            onClick={onCancel}
            className="w-full flex items-center gap-3 px-4 py-3 rounded-lg bg-white/5 hover:bg-rose-600/10 text-left transition"
          >
            <Trash2 size={16} className="text-rose-400 flex-shrink-0" />
            <div>
              <p className="text-sm font-medium text-white">Remove from library</p>
              <p className="text-xs text-[var(--text-muted)]">Cancel and delete the download</p>
            </div>
          </button>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="w-full mt-3 px-4 py-2 rounded-lg text-sm text-[var(--text-muted)] hover:text-white hover:bg-white/5 transition text-center"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}
