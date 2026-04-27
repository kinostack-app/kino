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
import { useQuery } from '@tanstack/react-query';
import { AlertCircle, GripVertical, Loader2, Plus, Sliders, Trash2, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  createQualityProfile,
  deleteQualityProfile,
  setDefaultQualityProfile,
  updateQualityProfile,
} from '@/api/generated/sdk.gen';
import { FormField, TextInput, Toggle } from '@/components/settings/FormField';
import { cn } from '@/lib/utils';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

// ── Types & constants ─────────────────────────────────────────────

interface QualityTier {
  quality_id: string;
  name: string;
  allowed: boolean;
  rank: number;
}

interface QualityProfile {
  id: number;
  name: string;
  upgrade_allowed: boolean;
  cutoff: string;
  /** JSON-encoded QualityTier[] */
  items: string;
  /** JSON-encoded string[] of ISO language codes. */
  accepted_languages: string;
  is_default: boolean;
}

interface QualityProfileWithUsage extends QualityProfile {
  usage_count: number;
}

// Default tier ordering — mirrors `default_quality_items` in the Rust
// crate. We use this to populate a new profile; once saved, the server's
// items become the source of truth.
const DEFAULT_TIERS: QualityTier[] = [
  { quality_id: 'remux_2160p', name: 'Remux 2160p', allowed: true, rank: 18 },
  { quality_id: 'bluray_2160p', name: 'Bluray 2160p', allowed: true, rank: 17 },
  { quality_id: 'web_2160p', name: 'WEB 2160p', allowed: true, rank: 16 },
  { quality_id: 'hdtv_2160p', name: 'HDTV 2160p', allowed: true, rank: 15 },
  { quality_id: 'remux_1080p', name: 'Remux 1080p', allowed: true, rank: 14 },
  { quality_id: 'bluray_1080p', name: 'Bluray 1080p', allowed: true, rank: 13 },
  { quality_id: 'web_1080p', name: 'WEB 1080p', allowed: true, rank: 12 },
  { quality_id: 'hdtv_1080p', name: 'HDTV 1080p', allowed: true, rank: 11 },
  { quality_id: 'bluray_720p', name: 'Bluray 720p', allowed: true, rank: 10 },
  { quality_id: 'web_720p', name: 'WEB 720p', allowed: true, rank: 9 },
  { quality_id: 'hdtv_720p', name: 'HDTV 720p', allowed: true, rank: 8 },
  { quality_id: 'bluray_480p', name: 'Bluray 480p', allowed: false, rank: 7 },
  { quality_id: 'web_480p', name: 'WEB 480p', allowed: false, rank: 6 },
  { quality_id: 'dvd', name: 'DVD', allowed: false, rank: 5 },
  { quality_id: 'sdtv', name: 'SDTV', allowed: false, rank: 4 },
  { quality_id: 'telecine', name: 'Telecine', allowed: false, rank: 3 },
  { quality_id: 'telesync', name: 'Telesync', allowed: false, rank: 2 },
  { quality_id: 'cam', name: 'CAM', allowed: false, rank: 1 },
];

const LANGUAGE_CHOICES: Array<{ code: string; label: string }> = [
  { code: 'en', label: 'English' },
  { code: 'es', label: 'Spanish' },
  { code: 'fr', label: 'French' },
  { code: 'de', label: 'German' },
  { code: 'it', label: 'Italian' },
  { code: 'pt', label: 'Portuguese' },
  { code: 'ru', label: 'Russian' },
  { code: 'zh', label: 'Chinese' },
  { code: 'ja', label: 'Japanese' },
  { code: 'ko', label: 'Korean' },
  { code: 'nl', label: 'Dutch' },
  { code: 'sv', label: 'Swedish' },
  { code: 'no', label: 'Norwegian' },
  { code: 'da', label: 'Danish' },
  { code: 'fi', label: 'Finnish' },
  { code: 'pl', label: 'Polish' },
  { code: 'tr', label: 'Turkish' },
  { code: 'ar', label: 'Arabic' },
  { code: 'hi', label: 'Hindi' },
];

function languageLabel(code: string): string {
  return LANGUAGE_CHOICES.find((l) => l.code === code)?.label ?? code;
}

function parseTiers(items: string): QualityTier[] {
  try {
    const parsed = JSON.parse(items) as QualityTier[];
    if (Array.isArray(parsed) && parsed.length > 0) return parsed;
  } catch {
    // fallthrough
  }
  return [...DEFAULT_TIERS];
}

function parseLanguages(raw: string): string[] {
  try {
    const parsed = JSON.parse(raw) as string[];
    if (Array.isArray(parsed)) return parsed;
  } catch {
    // fallthrough
  }
  return ['en'];
}

// ── Sortable tier row ─────────────────────────────────────────────

interface SortableTierProps {
  tier: QualityTier;
  onToggle: (allowed: boolean) => void;
}

function SortableTier({ tier, onToggle }: SortableTierProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: tier.quality_id,
  });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };
  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(
        'flex items-center gap-2 px-2.5 py-1.5 rounded-md bg-white/[0.03] ring-1 ring-white/5',
        isDragging && 'opacity-60 ring-white/15 z-10 relative'
      )}
    >
      <button
        type="button"
        {...attributes}
        {...listeners}
        aria-label={`Reorder ${tier.name}`}
        className="p-0.5 -ml-1 rounded text-[var(--text-muted)] hover:text-white cursor-grab active:cursor-grabbing touch-none flex-shrink-0"
      >
        <GripVertical size={13} />
      </button>
      <input
        type="checkbox"
        checked={tier.allowed}
        onChange={(e) => onToggle(e.target.checked)}
        className="accent-[var(--accent)] cursor-pointer"
      />
      <span
        className={cn(
          'text-sm flex-1 truncate',
          tier.allowed ? 'text-white' : 'text-[var(--text-muted)] line-through'
        )}
      >
        {tier.name}
      </span>
    </div>
  );
}

// ── Language chip picker ──────────────────────────────────────────

function LanguageChips({
  value,
  onChange,
}: {
  value: string[];
  onChange: (langs: string[]) => void;
}) {
  const toggle = (code: string) => {
    if (value.includes(code)) {
      onChange(value.filter((c) => c !== code));
    } else {
      onChange([...value, code]);
    }
  };
  return (
    <div className="flex flex-wrap gap-1.5">
      {LANGUAGE_CHOICES.map((l) => {
        const on = value.includes(l.code);
        return (
          <button
            key={l.code}
            type="button"
            onClick={() => toggle(l.code)}
            className={cn(
              'px-2.5 py-1 rounded-md text-xs font-medium transition ring-1',
              on
                ? 'bg-[var(--accent)]/15 text-[var(--accent)] ring-[var(--accent)]/30'
                : 'bg-white/5 text-[var(--text-muted)] ring-white/10 hover:text-white hover:bg-white/10'
            )}
          >
            {l.label}
          </button>
        );
      })}
    </div>
  );
}

// ── Profile modal ────────────────────────────────────────────────

interface ProfileModalProps {
  profile: QualityProfile | null; // null → create
  onClose: () => void;
  onSaved: () => void;
  onDelete?: (id: number) => void;
  deleting?: boolean;
}

function ProfileModal({ profile, onClose, onSaved, onDelete, deleting }: ProfileModalProps) {
  const [name, setName] = useState(profile?.name ?? 'New Profile');
  const [upgradeAllowed, setUpgradeAllowed] = useState(profile?.upgrade_allowed ?? true);
  const [tiers, setTiers] = useState<QualityTier[]>(() =>
    profile ? parseTiers(profile.items) : [...DEFAULT_TIERS]
  );
  const [cutoff, setCutoff] = useState<string>(profile?.cutoff ?? 'bluray_1080p');
  const [languages, setLanguages] = useState<string[]>(() =>
    profile ? parseLanguages(profile.accepted_languages) : ['en']
  );
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const overlayRef = useRef<HTMLDivElement>(null);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);

  const allowedTiers = useMemo(() => tiers.filter((t) => t.allowed), [tiers]);
  // If the currently-selected cutoff is no longer allowed, snap to the
  // first allowed tier so the server never receives a stale value.
  useEffect(() => {
    if (allowedTiers.length === 0) return;
    if (!allowedTiers.some((t) => t.quality_id === cutoff)) {
      setCutoff(allowedTiers[0].quality_id);
    }
  }, [allowedTiers, cutoff]);

  const onDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = tiers.findIndex((t) => t.quality_id === active.id);
    const newIndex = tiers.findIndex((t) => t.quality_id === over.id);
    if (oldIndex < 0 || newIndex < 0) return;
    // Keep rank aligned with display order — highest rank at the top so
    // search results pick the most-preferred first.
    const reordered = arrayMove(tiers, oldIndex, newIndex);
    const withRanks = reordered.map((t, i) => ({ ...t, rank: reordered.length - i }));
    setTiers(withRanks);
  };

  const toggleTier = (quality_id: string, allowed: boolean) => {
    setTiers((prev) => prev.map((t) => (t.quality_id === quality_id ? { ...t, allowed } : t)));
  };

  const handleSave = async () => {
    setSaving(true);
    setSaveError('');
    const body = {
      name,
      upgrade_allowed: upgradeAllowed,
      cutoff,
      items: JSON.stringify(tiers),
      accepted_languages: JSON.stringify(languages),
    };
    try {
      if (profile) {
        await updateQualityProfile({ path: { id: profile.id }, body });
      } else {
        await createQualityProfile({ body });
      }
      onSaved();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  return createPortal(
    <div
      ref={overlayRef}
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      onClick={(e) => {
        if (e.target === overlayRef.current) onClose();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onClose();
      }}
      role="dialog"
      aria-modal="true"
    >
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" />
      <div className="relative w-full max-w-2xl max-h-[calc(100vh-4rem)] flex flex-col bg-[var(--bg-secondary)] rounded-xl ring-1 ring-white/10 shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="flex items-center justify-between gap-3 px-5 py-4 border-b border-white/5">
          <h2 className="text-lg font-semibold">
            {profile ? `Edit: ${profile.name}` : 'New Quality Profile'}
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded-lg text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition"
          >
            <X size={18} />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-auto px-5 py-4 space-y-4">
          <FormField label="Name">
            <TextInput value={name} onChange={setName} placeholder="e.g. HD Bluray" />
          </FormField>

          <FormField
            label="Upgrade allowed"
            description="Replace existing copies with better quality"
            help="When on, grabbing a higher-ranked allowed tier replaces an already-imported lower-ranked copy. Stops once the Cutoff tier is reached."
          >
            <Toggle checked={upgradeAllowed} onChange={setUpgradeAllowed} />
          </FormField>

          <FormField
            label="Qualities"
            description="Tick qualities to accept; drag to reorder preference"
            help="Highest-ranked allowed tier is preferred. Disallowed tiers are never grabbed."
          >
            <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
              <SortableContext
                items={tiers.map((t) => t.quality_id)}
                strategy={verticalListSortingStrategy}
              >
                <div className="space-y-1">
                  {tiers.map((tier) => (
                    <SortableTier
                      key={tier.quality_id}
                      tier={tier}
                      onToggle={(v) => toggleTier(tier.quality_id, v)}
                    />
                  ))}
                </div>
              </SortableContext>
            </DndContext>
          </FormField>

          <FormField
            label="Cutoff"
            description="Stop upgrading once we have this"
            help="Only allowed tiers are available. Once we import a file at the cutoff tier (or better), no more upgrades will be attempted."
          >
            <select
              value={cutoff}
              onChange={(e) => setCutoff(e.target.value)}
              className="h-9 px-3 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
            >
              {allowedTiers.length === 0 ? (
                <option>(no allowed tiers)</option>
              ) : (
                allowedTiers.map((t) => (
                  <option key={t.quality_id} value={t.quality_id}>
                    {t.name}
                  </option>
                ))
              )}
            </select>
          </FormField>

          <FormField
            label="Languages"
            description="Tracks + subtitles to prefer"
            help="Used for release scoring and subtitle fetch. Multiple languages rank by preference order."
          >
            <LanguageChips value={languages} onChange={setLanguages} />
          </FormField>

          {saveError && (
            <p className="flex items-center gap-1.5 text-xs text-red-400 pt-2">
              <AlertCircle size={12} />
              {saveError}
            </p>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between gap-3 px-5 py-4 border-t border-white/5">
          <div>
            {profile && onDelete && !confirmDelete && (
              <button
                type="button"
                onClick={() => setConfirmDelete(true)}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm text-red-400 hover:bg-red-600/10 transition"
              >
                <Trash2 size={14} />
                Delete
              </button>
            )}
            {profile && onDelete && confirmDelete && (
              <div className="flex items-center gap-2">
                <span className="text-xs text-red-400">Are you sure?</span>
                <button
                  type="button"
                  onClick={() => onDelete(profile.id)}
                  disabled={deleting}
                  className="flex items-center gap-1.5 px-3 py-1 rounded-lg text-xs font-semibold bg-red-600 hover:bg-red-500 text-white disabled:opacity-50 transition"
                >
                  {deleting && <Loader2 size={12} className="animate-spin" />}
                  {deleting ? 'Deleting…' : 'Yes, delete'}
                </button>
                <button
                  type="button"
                  onClick={() => setConfirmDelete(false)}
                  disabled={deleting}
                  className="px-3 py-1 rounded-lg text-xs bg-white/10 hover:bg-white/15 disabled:opacity-50 transition"
                >
                  No
                </button>
              </div>
            )}
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleSave}
              disabled={saving || !name.trim() || allowedTiers.length === 0}
              className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
              title={allowedTiers.length === 0 ? 'Tick at least one quality to save' : undefined}
            >
              {saving && <Loader2 size={14} className="animate-spin" />}
              {profile ? 'Save' : 'Create'}
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body
  );
}

// ── Main page ────────────────────────────────────────────────────

export function QualitySettings() {
  const [editing, setEditing] = useState<QualityProfile | null | 'new'>(null);

  const { data, isLoading } = useQuery({
    queryKey: ['kino', 'quality-profiles'],
    queryFn: async () => {
      // Use raw fetch so we can type the response without wrestling with
      // the generated zod schemas for the cutoff enum.
      const res = await fetch('/api/v1/quality-profiles', {
        credentials: 'include',
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return (await res.json()) as QualityProfileWithUsage[];
    },
    meta: { invalidatedBy: ['quality_profile_changed'] },
  });

  // All quality-profile mutations emit `QualityProfileChanged` →
  // meta dispatcher refreshes the list. Mutations stay lean.
  const setDefaultMutation = useMutationWithToast({
    verb: 'set default profile',
    mutationFn: async (id: number) => {
      await setDefaultQualityProfile({ path: { id } });
    },
  });

  const deleteMutation = useMutationWithToast({
    verb: 'delete quality profile',
    mutationFn: async (id: number) => {
      await deleteQualityProfile({ path: { id } });
    },
    onSuccess: () => {
      setEditing(null);
    },
  });

  const profiles = data ?? [];

  const onSaved = () => {
    setEditing(null);
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-bold">Quality Profiles</h1>
          <p className="text-sm text-[var(--text-muted)]">
            Define quality preferences and upgrade rules
          </p>
        </div>
        <button
          type="button"
          onClick={() => setEditing('new')}
          className="flex items-center gap-1.5 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold transition"
        >
          <Plus size={16} />
          Add Profile
        </button>
      </div>

      {isLoading && (
        <div className="space-y-2">
          <div className="h-16 skeleton rounded-lg" />
          <div className="h-16 skeleton rounded-lg" />
        </div>
      )}

      {!isLoading && profiles.length === 0 && (
        <div className="text-center py-16 text-[var(--text-muted)]">
          <Sliders size={32} className="mx-auto mb-3 opacity-30" />
          <p className="text-sm font-medium">No quality profiles</p>
          <p className="text-xs mt-1">Use the Add Profile button above to create one.</p>
        </div>
      )}

      <div className="space-y-2">
        {profiles.map((p) => {
          const tiers = parseTiers(p.items);
          const allowed = tiers.filter((t) => t.allowed);
          const langs = parseLanguages(p.accepted_languages);
          const cutoffTier = tiers.find((t) => t.quality_id === p.cutoff);
          return (
            <div
              key={p.id}
              className="relative p-4 rounded-lg bg-[var(--bg-card)] ring-1 ring-white/5 hover:ring-white/15 transition"
            >
              <button
                type="button"
                onClick={() => setEditing(p)}
                className="absolute inset-0 rounded-lg focus:outline-none focus:ring-1 focus:ring-[var(--accent)]/40"
                aria-label={`Edit ${p.name}`}
              />
              <div className="relative flex items-start justify-between gap-4 pointer-events-none">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <p className="text-sm font-medium text-white">{p.name}</p>
                    {p.is_default && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--accent)]/15 text-[var(--accent)] ring-1 ring-[var(--accent)]/30 uppercase tracking-wide font-semibold">
                        Default
                      </span>
                    )}
                    {p.upgrade_allowed && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-300 ring-1 ring-sky-500/20 uppercase tracking-wide">
                        Upgrades
                      </span>
                    )}
                    {p.usage_count > 0 && (
                      <span
                        className="text-[10px] px-1.5 py-0.5 rounded bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10"
                        title="Number of movies + shows using this profile. Blocks delete."
                      >
                        {p.usage_count} in use
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-[var(--text-muted)] mt-1">
                    Cutoff: {cutoffTier?.name ?? p.cutoff} · {allowed.length} of {tiers.length}{' '}
                    qualities
                  </p>
                </div>
                <div className="flex flex-col items-end gap-1.5 flex-shrink-0 max-w-[40%]">
                  <div className="flex flex-wrap gap-1 justify-end">
                    {langs.length === 0 ? (
                      <span className="text-xs text-[var(--text-muted)]">no languages</span>
                    ) : (
                      langs.slice(0, 4).map((code) => (
                        <span
                          key={code}
                          className="text-[10px] px-1.5 py-0.5 rounded bg-white/5 text-[var(--text-secondary)] ring-1 ring-white/10"
                        >
                          {languageLabel(code)}
                        </span>
                      ))
                    )}
                    {langs.length > 4 && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10">
                        +{langs.length - 4}
                      </span>
                    )}
                  </div>
                  {!p.is_default && (
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        setDefaultMutation.mutate(p.id);
                      }}
                      disabled={setDefaultMutation.isPending}
                      className="pointer-events-auto relative text-[11px] px-2 py-0.5 rounded bg-white/5 hover:bg-white/10 text-[var(--text-muted)] hover:text-white transition disabled:opacity-50"
                      title="Make this the default profile for new movies + shows"
                    >
                      Set as default
                    </button>
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>

      {editing === 'new' && (
        <ProfileModal profile={null} onClose={() => setEditing(null)} onSaved={onSaved} />
      )}
      {editing && editing !== 'new' && (
        <ProfileModal
          profile={editing}
          onClose={() => setEditing(null)}
          onSaved={onSaved}
          onDelete={(id) => deleteMutation.mutate(id)}
          deleting={deleteMutation.isPending}
        />
      )}
    </div>
  );
}
