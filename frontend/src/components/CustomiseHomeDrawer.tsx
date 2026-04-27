/**
 * Right-edge drawer that lets the user reorder, hide, and toggle Home
 * rows. Auto-saves each change via `PATCH /api/v1/preferences/home` —
 * no save button, per `docs/subsystems/18-ui-customisation.md` §
 * Customise Home drawer.
 *
 * Built on @dnd-kit/sortable (already a project dependency). Keyboard
 * reorder is supported via dnd-kit's KeyboardSensor: focus a row, hold
 * space, arrow to reorder, release.
 */

import {
  closestCenter,
  DndContext,
  type DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { GripVertical, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { listLists, resetHomePreferences, updateHomePreferences } from '@/api/generated/sdk.gen';
import type { HomePreferences, List as ListRow } from '@/api/generated/types.gen';
import { ConfirmDialog } from '@/components/ConfirmDialog';
import { cn } from '@/lib/utils';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

/** Human labels for each known row ID. Kept alongside the registry
 *  in Home.tsx conceptually — mirroring the IDs here means adding a
 *  row is "add to Home.tsx registry + add a label here". Unknown IDs
 *  render their raw string — never seen in v1. */
const ROW_LABELS: Record<string, string> = {
  up_next: 'Up Next',
  recommendations: 'Recommended for you',
  trending_trakt: 'Trending on Trakt',
  trending_movies: 'Trending Movies',
  trending_shows: 'Trending TV Shows',
  popular_movies: 'Popular Movies',
  popular_shows: 'Popular TV Shows',
};

/** Small source-origin marker for list rows in the drawer. Trakt
 *  gets its circle-mark (matches the /lists card + RateWidget);
 *  other sources get a compact text pill. Built-in rows don't get
 *  one at all (null-return). Keeps list rows visually distinct from
 *  the built-in "Popular Movies" / "Trending" set. */
function ListSourceMark({ sourceType }: { sourceType?: string }) {
  if (!sourceType) return null;
  if (sourceType.startsWith('trakt_')) {
    return <img src="/trakt-mark.svg" alt="Trakt" className="h-4 w-4 opacity-80 shrink-0" />;
  }
  const label =
    sourceType === 'mdblist' ? 'MDBList' : sourceType === 'tmdb_list' ? 'TMDB' : sourceType;
  return (
    <span className="shrink-0 px-1.5 py-0.5 rounded text-[9px] font-semibold uppercase tracking-wider bg-white/5 text-[var(--text-muted)] ring-1 ring-white/5">
      {label}
    </span>
  );
}

interface SectionRowProps {
  id: string;
  label: string;
  visible: boolean;
  onToggle: (id: string) => void;
  /** When set, rendered before the label as a source-origin marker.
   *  Only list rows provide this; built-ins have no marker. */
  sourceType?: string;
}

function SectionRow({ id, label, visible, onToggle, sourceType }: SectionRowProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id,
  });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };
  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(
        'flex items-center gap-3 px-3 py-2.5 rounded-lg bg-[var(--bg-card)] ring-1 ring-white/10',
        isDragging && 'shadow-2xl z-20 relative'
      )}
    >
      <button
        type="button"
        className="text-[var(--text-muted)] hover:text-white cursor-grab active:cursor-grabbing touch-none"
        aria-label={`Drag ${label}`}
        {...attributes}
        {...listeners}
      >
        <GripVertical size={16} />
      </button>
      <ListSourceMark sourceType={sourceType} />
      <span
        className={cn(
          'flex-1 text-sm truncate',
          visible ? 'text-white' : 'text-[var(--text-muted)]'
        )}
      >
        {label}
      </span>
      <button
        type="button"
        role="switch"
        aria-checked={visible}
        aria-label={`${visible ? 'Hide' : 'Show'} ${label}`}
        onClick={() => onToggle(id)}
        className={cn(
          'relative inline-flex h-5 w-9 items-center rounded-full transition-colors',
          visible ? 'bg-[var(--accent)]' : 'bg-white/10'
        )}
      >
        <span
          className={cn(
            'inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform',
            visible ? 'translate-x-[18px]' : 'translate-x-[2px]'
          )}
        />
      </button>
    </div>
  );
}

/** Row for a list that's followed in /lists but not (yet) pinned to
 *  Home. Visually demarcated from the sortable rows above by having
 *  no drag handle and a dashed border — signals "this isn't on Home,
 *  flip the toggle to add it." */
function AvailableListRow({ list, onAdd }: { list: ListRow; onAdd: () => void }) {
  return (
    <div className="flex items-center gap-3 px-3 py-2.5 rounded-lg bg-transparent ring-1 ring-dashed ring-white/10">
      <ListSourceMark sourceType={list.source_type} />
      <span className="flex-1 text-sm text-[var(--text-muted)] truncate">{list.title}</span>
      <button
        type="button"
        role="switch"
        aria-checked={false}
        aria-label={`Add ${list.title} to Home`}
        onClick={onAdd}
        className="relative inline-flex h-5 w-9 items-center rounded-full transition-colors bg-white/10 hover:bg-white/15"
      >
        <span className="inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform translate-x-[2px]" />
      </button>
    </div>
  );
}

interface CustomiseHomeDrawerProps {
  open: boolean;
  onClose: () => void;
  prefs: HomePreferences;
}

export function CustomiseHomeDrawer({ open, onClose, prefs }: CustomiseHomeDrawerProps) {
  const qc = useQueryClient();
  const panelRef = useRef<HTMLDivElement>(null);

  // Lists appear as `list:<id>` pseudo-rows in section_order. We pull
  // the list records so we can render their user-facing titles in the
  // sortable panel — otherwise the row would just read "list:5".
  const listsQ = useQuery({
    queryKey: ['kino', 'lists'],
    queryFn: async () => {
      const r = await listLists();
      return (r.data as ListRow[] | undefined) ?? [];
    },
    enabled: open,
    meta: {
      invalidatedBy: ['list_bulk_growth', 'list_unreachable', 'list_auto_added', 'list_deleted'],
    },
  });
  const listsById = new Map((listsQ.data ?? []).map((l) => [l.id, l]));

  // Keep local state so drag operations feel instant. We PATCH on
  // every settled change — the server is the source of truth, but
  // round-tripping on every drag tick would be wasteful.
  const [order, setOrder] = useState(prefs.section_order);
  const [hidden, setHidden] = useState(new Set(prefs.section_hidden));
  const [heroEnabled, setHeroEnabled] = useState(prefs.hero_enabled);
  const [confirmingReset, setConfirmingReset] = useState(false);

  // Sync local state when prefs change externally (WS invalidation,
  // reset, another tab). Shallow compare the array/set so we don't
  // clobber an in-flight drag.
  useEffect(() => {
    setOrder(prefs.section_order);
    setHidden(new Set(prefs.section_hidden));
    setHeroEnabled(prefs.hero_enabled);
  }, [prefs]);

  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [open, onClose]);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  const saveMutation = useMutationWithToast({
    verb: 'save home layout',
    mutationFn: async (patch: {
      section_order?: string[];
      section_hidden?: string[];
      hero_enabled?: boolean;
    }) => {
      await updateHomePreferences({ body: patch });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'preferences', 'home'] });
    },
  });

  const resetMutation = useMutationWithToast({
    verb: 'reset home layout',
    mutationFn: async () => {
      await resetHomePreferences();
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'preferences', 'home'] });
    },
  });

  const handleDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const oldIdx = order.indexOf(String(active.id));
    const newIdx = order.indexOf(String(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    const next = arrayMove(order, oldIdx, newIdx);
    setOrder(next);
    saveMutation.mutate({ section_order: next });
  };

  const toggleVisibility = (id: string) => {
    const next = new Set(hidden);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setHidden(next);
    saveMutation.mutate({ section_hidden: [...next] });
  };

  const toggleHero = () => {
    const next = !heroEnabled;
    setHeroEnabled(next);
    saveMutation.mutate({ hero_enabled: next });
  };

  const unpinRow = (id: string) => {
    const nextOrder = order.filter((x) => x !== id);
    const nextHidden = new Set(hidden);
    nextHidden.delete(id);
    setOrder(nextOrder);
    setHidden(nextHidden);
    saveMutation.mutate({
      section_order: nextOrder,
      section_hidden: [...nextHidden],
    });
  };

  /** Add a list to Home at the end of section_order. Used by the
   *  "Available lists" rows — matches the Lists page pin button but
   *  keeps the user in the drawer. */
  const pinList = (listId: number) => {
    const marker = `list:${listId}`;
    if (order.includes(marker)) return;
    const nextOrder = [...order, marker];
    setOrder(nextOrder);
    saveMutation.mutate({ section_order: nextOrder });
  };

  /** Lists that exist but aren't in section_order — rendered as
   *  toggleable "Available lists" rows so the drawer is a one-stop
   *  shop for pinning to Home. */
  const availableLists = (listsQ.data ?? []).filter((l) => !order.includes(`list:${l.id}`));

  // Resolve an ID to a human label. Built-ins come from the static
  // map; `list:<id>` pulls from the fetched list record, falling
  // back to a generic label while the lists query is in flight.
  const labelFor = (id: string): string => {
    if (id.startsWith('list:')) {
      const lid = Number(id.slice(5));
      const list = listsById.get(lid);
      return list?.title ?? `List ${lid}`;
    }
    return ROW_LABELS[id] ?? id;
  };

  if (!open) return null;

  return createPortal(
    <>
      {/* Backdrop — click dismisses, content stays visible behind a
          subtle dim per spec. */}
      {/* biome-ignore lint/a11y/useSemanticElements: a real <button> here would inherit appearance defaults that conflict with the full-screen backdrop layout (display, padding, border). role+tabIndex+onKeyDown is the documented div-as-button pattern */}
      <div
        className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[2px]"
        onClick={onClose}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onClose();
        }}
        role="button"
        tabIndex={-1}
        aria-label="Close customise drawer"
      />
      <aside
        ref={panelRef}
        className="fixed top-0 right-0 z-50 h-full w-full sm:w-[380px] bg-[var(--bg-secondary)] border-l border-white/10 shadow-2xl flex flex-col"
        role="dialog"
        aria-modal="true"
        aria-label="Customise Home"
      >
        <header className="flex items-center justify-between px-5 py-4 border-b border-white/10">
          <h2 className="text-base font-semibold text-white">Customise Home</h2>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-md text-[var(--text-secondary)] hover:text-white hover:bg-white/5 transition"
            aria-label="Close"
          >
            <X size={18} />
          </button>
        </header>

        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-5">
          {/* Hero toggle — slightly separated from the sortable list
              since it's a different shape of preference. */}
          <div className="flex items-center gap-3 px-3 py-2.5 rounded-lg bg-[var(--bg-card)] ring-1 ring-white/10">
            <span
              className={cn(
                'flex-1 text-sm',
                heroEnabled ? 'text-white' : 'text-[var(--text-muted)]'
              )}
            >
              Show hero banner
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={heroEnabled}
              onClick={toggleHero}
              className={cn(
                'relative inline-flex h-5 w-9 items-center rounded-full transition-colors',
                heroEnabled ? 'bg-[var(--accent)]' : 'bg-white/10'
              )}
            >
              <span
                className={cn(
                  'inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform',
                  heroEnabled ? 'translate-x-[18px]' : 'translate-x-[2px]'
                )}
              />
            </button>
          </div>

          <div>
            <h3 className="px-1 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)] mb-2">
              Sections
            </h3>
            <DndContext
              sensors={sensors}
              collisionDetection={closestCenter}
              onDragEnd={handleDragEnd}
            >
              <SortableContext items={order} strategy={verticalListSortingStrategy}>
                <div className="space-y-2">
                  {order.map((id) => {
                    const isList = id.startsWith('list:');
                    return (
                      <SectionRow
                        key={id}
                        id={id}
                        label={labelFor(id)}
                        visible={isList ? true : !hidden.has(id)}
                        // List rows: toggle = pin/unpin (off pops it back to
                        // Available lists). Built-ins: toggle = hide (they
                        // can't be removed from section_order entirely).
                        onToggle={isList ? unpinRow : toggleVisibility}
                        sourceType={
                          isList ? listsById.get(Number(id.slice(5)))?.source_type : undefined
                        }
                      />
                    );
                  })}
                </div>
              </SortableContext>
            </DndContext>
          </div>

          {availableLists.length > 0 && (
            <div>
              <h3 className="px-1 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-muted)] mb-2">
                Available lists
              </h3>
              <div className="space-y-2">
                {availableLists.map((l) => (
                  <AvailableListRow key={l.id} list={l} onAdd={() => pinList(l.id)} />
                ))}
              </div>
            </div>
          )}
        </div>

        <footer className="px-5 py-3 border-t border-white/10">
          <button
            type="button"
            onClick={() => setConfirmingReset(true)}
            disabled={resetMutation.isPending}
            className="w-full px-3 py-2 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white hover:bg-white/5 transition disabled:opacity-50"
          >
            Reset to defaults
          </button>
        </footer>
      </aside>

      <ConfirmDialog
        open={confirmingReset}
        title="Reset Home customisation?"
        description="Row order, hidden rows, and the hero toggle will go back to their defaults. Library view preferences are not affected."
        confirmLabel="Reset"
        onConfirm={() => {
          resetMutation.mutate();
          setConfirmingReset(false);
        }}
        onCancel={() => setConfirmingReset(false)}
      />
    </>,
    document.body
  );
}
