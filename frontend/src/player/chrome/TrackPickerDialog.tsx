import { Check, Search, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

/**
 * Generic track-picker dialog — centered modal with
 * search + keyboard navigation. Built to serve both the
 * subtitle picker and (eventually) audio picker with the
 * same interaction model.
 *
 * Interaction shape:
 *
 *   * Typing filters the list by `searchText(item)` substring.
 *   * Arrow keys move the highlight; Enter selects.
 *   * `/` refocuses the search input.
 *   * Escape closes without changing the current selection.
 *   * Click-through on backdrop also closes.
 *
 * Accessibility:
 *
 *   * `role="dialog"` + `aria-modal` on the panel, labelled
 *     by its title.
 *   * Arrow-key navigation updates `aria-activedescendant`
 *     so screen readers announce the highlighted option.
 *   * Initial focus goes to the search input; restored to
 *     the trigger element on close by the caller.
 */
export interface TrackPickerDialogItem<T> {
  /** Stable key (usually `stream_index`). */
  key: number | string;
  /** The underlying item — opaque to the dialog, fed to
   *  `render` / `searchText`. */
  data: T;
  /** Optional group label. Items with the same group
   *  render under a shared section header. Groups render
   *  in first-seen order. */
  group?: string;
}

export interface TrackPickerDialogProps<T> {
  title: string;
  items: TrackPickerDialogItem<T>[];
  selectedKey: number | string | null;
  /** Plain-text search haystack for each item. Kept
   *  independent of `render` so the rendered row can be
   *  icon-heavy without confusing the matcher. */
  searchText: (item: T) => string;
  /** How each row renders inside the button. The dialog
   *  owns the click handler, highlight state, and check
   *  mark — `render` produces the content only. */
  render: (item: T, active: boolean) => React.ReactNode;
  /** Called with the selected key (or `null` for the
   *  "Off" row). */
  onSelect: (key: number | string | null) => void;
  /** Label of the "Off" row when present. Pass `null` to
   *  omit the row entirely (e.g. when a selection is
   *  mandatory). */
  offLabel?: string | null;
  onClose: () => void;
}

export function TrackPickerDialog<T>({
  title,
  items,
  selectedKey,
  searchText,
  render,
  onSelect,
  offLabel = 'Off',
  onClose,
}: TrackPickerDialogProps<T>) {
  const [query, setQuery] = useState('');
  const [activeIdx, setActiveIdx] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Flat list of "rows" the user can navigate through.
  // Off (when present) goes first; then items in group
  // order.
  const rows = useMemo(() => {
    type Row = { kind: 'off' } | { kind: 'item'; item: TrackPickerDialogItem<T>; group?: string };
    const out: Row[] = [];
    if (offLabel !== null) out.push({ kind: 'off' });
    const q = query.trim().toLowerCase();
    const filtered = q
      ? items.filter((it) => searchText(it.data).toLowerCase().includes(q))
      : items;
    const seen = new Set<string>();
    const order: string[] = [];
    for (const it of filtered) {
      const g = it.group ?? '';
      if (!seen.has(g)) {
        seen.add(g);
        order.push(g);
      }
    }
    for (const group of order) {
      for (const it of filtered) {
        if ((it.group ?? '') === group) {
          out.push({ kind: 'item', item: it, group: it.group });
        }
      }
    }
    return out;
  }, [items, offLabel, query, searchText]);

  // Clamp the active index whenever the filtered rows
  // length changes (including going to zero on no matches).
  useEffect(() => {
    if (activeIdx >= rows.length) setActiveIdx(Math.max(0, rows.length - 1));
  }, [rows.length, activeIdx]);

  // Autofocus the search input on mount. We also restore
  // focus to whatever triggered the open on unmount —
  // captured at mount time so the caller doesn't have to
  // plumb a ref.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    searchRef.current?.focus();
    return () => {
      prev?.focus?.();
    };
  }, []);

  // Scroll the active row into view as the highlight
  // moves — especially useful on long lists where the
  // highlighted row can drift off-screen. `activeIdx` is
  // the trigger; the selector looks up the live DOM
  // rather than reading the index directly so we don't
  // close over stale row refs.
  // biome-ignore lint/correctness/useExhaustiveDependencies: activeIdx is the intentional re-run trigger.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const active = list.querySelector<HTMLElement>('[data-active="true"]');
    active?.scrollIntoView({ block: 'nearest' });
  }, [activeIdx]);

  const commit = useCallback(
    (rowIdx: number) => {
      const row = rows[rowIdx];
      if (!row) return;
      if (row.kind === 'off') onSelect(null);
      else onSelect(row.item.key);
      onClose();
    },
    [onClose, onSelect, rows]
  );

  const onKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActiveIdx((i) => Math.min(rows.length - 1, i + 1));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setActiveIdx((i) => Math.max(0, i - 1));
        break;
      case 'Home':
        e.preventDefault();
        setActiveIdx(0);
        break;
      case 'End':
        e.preventDefault();
        setActiveIdx(rows.length - 1);
        break;
      case 'Enter':
        e.preventDefault();
        commit(activeIdx);
        break;
      case 'Escape':
        e.preventDefault();
        onClose();
        break;
      case '/':
        if (document.activeElement !== searchRef.current) {
          e.preventDefault();
          searchRef.current?.focus();
        }
        break;
    }
  };

  // Track group headers so we render them inline with
  // their first item. First-pass: build index → header
  // string map.
  const groupHeaderBefore = useMemo(() => {
    const map = new Map<number, string>();
    let lastGroup: string | undefined;
    rows.forEach((row, i) => {
      if (row.kind !== 'item') return;
      const g = row.group;
      if (g && g !== lastGroup) {
        map.set(i, g);
        lastGroup = g;
      } else if (!g) {
        lastGroup = undefined;
      }
    });
    return map;
  }, [rows]);

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-dismiss is the standard modal affordance; all interactive controls live inside the dialog panel.
    <div
      className="absolute inset-0 z-[60] bg-black/70 backdrop-blur-sm grid place-items-center p-4"
      onClick={onClose}
      onKeyDown={onKeyDown}
      role="presentation"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="track-picker-title"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
        className="w-[min(480px,90vw)] max-h-[min(560px,85vh)] rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl flex flex-col overflow-hidden"
      >
        <div className="flex items-center justify-between px-4 pt-4 pb-3 border-b border-white/5">
          <h2 id="track-picker-title" className="text-sm font-semibold">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="w-7 h-7 grid place-items-center rounded hover:bg-white/10 text-white/60"
          >
            <X size={14} />
          </button>
        </div>
        <div className="relative px-4 pt-3 pb-2">
          <Search
            size={14}
            className="absolute left-6 top-1/2 -translate-y-1/2 text-white/40 pointer-events-none"
          />
          <input
            ref={searchRef}
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActiveIdx(0);
            }}
            placeholder="Search"
            aria-label="Search tracks"
            className="w-full h-9 pl-8 pr-3 rounded-md bg-[var(--bg-primary)] ring-1 ring-white/5 focus:ring-[var(--accent)]/50 focus:outline-none text-sm placeholder:text-white/30"
          />
        </div>
        <div ref={listRef} className="flex-1 overflow-y-auto px-2 pb-3">
          {rows.length === 0 ? (
            <p className="px-3 py-8 text-center text-sm text-white/40">No matches</p>
          ) : (
            rows.map((row, i) => {
              const header = groupHeaderBefore.get(i);
              const active = i === activeIdx;
              const highlighted = active ? 'bg-white/10' : 'hover:bg-white/5';
              if (row.kind === 'off') {
                const isSelected = selectedKey === null;
                return (
                  <button
                    key="__off__"
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    data-active={active}
                    onMouseEnter={() => setActiveIdx(i)}
                    onClick={() => commit(i)}
                    className={cn(
                      'w-full text-left px-3 py-2.5 rounded flex items-center gap-3 text-sm',
                      highlighted
                    )}
                  >
                    <span className="w-4 shrink-0">{isSelected && <Check size={14} />}</span>
                    <span className="font-medium">{offLabel}</span>
                  </button>
                );
              }
              const it = row.item;
              const isSelected = selectedKey === it.key;
              return (
                <div key={it.key} className="contents">
                  {header && (
                    <p className="px-3 pt-3 pb-1 text-[11px] uppercase tracking-wider text-white/40 font-semibold">
                      {header}
                    </p>
                  )}
                  <button
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    data-active={active}
                    onMouseEnter={() => setActiveIdx(i)}
                    onClick={() => commit(i)}
                    className={cn(
                      'w-full text-left px-3 py-2.5 rounded flex items-center gap-3 text-sm',
                      highlighted
                    )}
                  >
                    <span className="w-4 shrink-0">{isSelected && <Check size={14} />}</span>
                    <span className="flex-1 min-w-0">{render(it.data, active)}</span>
                  </button>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
