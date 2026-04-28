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
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertCircle,
  Check,
  ChevronLeft,
  Database,
  Download,
  Edit2,
  ExternalLink,
  Github,
  Globe,
  GripVertical,
  Library,
  Loader2,
  Lock,
  Plus,
  RefreshCw,
  Search,
  Shield,
  Trash2,
  Wifi,
  X,
  Zap,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { getRefreshState, refreshDefinitions } from '@/api/generated/sdk.gen';
import type { DefinitionsRefreshState } from '@/api/generated/types.gen';
import { FormField, SecretInput, TextInput, Toggle } from '@/components/settings/FormField';
import { cn } from '@/lib/utils';
import { INDEXERS_KEY } from '@/state/library-cache';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

// ── Types ──

const headers: Record<string, string> = {
  'Content-Type': 'application/json',
};
// Auth flows through cookies (cookie mode) or the SDK's pre-set
// Authorization header (bearer mode). Direct fetch calls in this
// file use `credentials: 'include'` to opt the browser into the
// cookie attach.

interface IndexerDefinition {
  id: string;
  name: string;
  description: string;
  indexer_type: string;
  language: string;
  /** Top-level categories derived from the definition's caps — used for filtering. */
  categories: string[];
}

interface DefinitionSetting {
  name: string;
  type: string;
  label: string;
  default?: string;
  options?: Record<string, string>;
}

interface DefinitionDetail extends IndexerDefinition {
  links: string[];
  settings: DefinitionSetting[];
}

interface ConfiguredIndexer {
  id: number;
  name: string;
  url: string;
  api_key?: string;
  priority: number;
  enabled: boolean;
  indexer_type: string;
  definition_id?: string;
  settings_json?: string;
  escalation_level: number;
  disabled_until?: string;
}

interface TestResult {
  success: boolean;
  message: string;
  result_count: number;
}

// ── API helpers ──

async function apiFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    ...init,
    credentials: 'include',
    headers: { ...headers, ...init?.headers },
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(text || `HTTP ${res.status}`);
  }
  // 204 No Content (and other empty responses) — parsing the body would
  // throw "Unexpected end of JSON input"; return undefined cast to T.
  if (res.status === 204 || res.headers.get('content-length') === '0') {
    return undefined as T;
  }
  return res.json();
}

// ── Type badge component ──

function TypeBadge({ type }: { type: string }) {
  const t = type.toLowerCase();
  let color: string;
  let icon: React.ReactNode;
  let label: string;

  if (t === 'public') {
    color = 'bg-green-500/15 text-green-400 ring-green-500/20';
    icon = <Globe size={11} />;
    label = 'Public';
  } else if (t === 'semi-private') {
    color = 'bg-yellow-500/15 text-yellow-400 ring-yellow-500/20';
    icon = <Shield size={11} />;
    label = 'Semi-Private';
  } else if (t === 'private') {
    color = 'bg-red-500/15 text-red-400 ring-red-500/20';
    icon = <Lock size={11} />;
    label = 'Private';
  } else if (t === 'torznab') {
    color = 'bg-blue-500/15 text-blue-400 ring-blue-500/20';
    icon = <Zap size={11} />;
    label = 'Torznab';
  } else {
    color = 'bg-white/10 text-[var(--text-secondary)] ring-white/10';
    icon = null;
    label = type;
  }

  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[10px] font-semibold uppercase tracking-wide ring-1',
        color
      )}
    >
      {icon}
      {label}
    </span>
  );
}

// ── Status dot ──

function StatusDot({ indexer }: { indexer: ConfiguredIndexer }) {
  if (!indexer.enabled) {
    return <span className="w-2.5 h-2.5 rounded-full bg-white/20" title="Disabled" />;
  }
  if (indexer.disabled_until) {
    return <span className="w-2.5 h-2.5 rounded-full bg-red-500" title="Failing" />;
  }
  return <span className="w-2.5 h-2.5 rounded-full bg-green-500" title="Healthy" />;
}

// ── Inline test button ──
//
// Mirrors the shared TestButton visual contract (static label, only the
// icon changes, sticky result, 300ms min testing duration). Kept
// separate from the FormField TestButton because this one sits inside
// a row and needs a tighter size + stopPropagation on click.

const MIN_TESTING_MS = 300;

function InlineTestButton({ indexerId }: { indexerId: number }) {
  const [state, setState] = useState<'idle' | 'testing' | 'success' | 'failed'>('idle');
  const [message, setMessage] = useState('');

  const handleTest = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setState('testing');
    setMessage('');
    const start = Date.now();
    let result: TestResult | null = null;
    try {
      result = await apiFetch<TestResult>(`/api/v1/indexers/${indexerId}/test`, { method: 'POST' });
    } catch (err) {
      result = {
        success: false,
        message: err instanceof Error ? err.message : 'Test failed',
        result_count: 0,
      };
    }
    const elapsed = Date.now() - start;
    if (elapsed < MIN_TESTING_MS) {
      await new Promise((r) => setTimeout(r, MIN_TESTING_MS - elapsed));
    }
    setState(result.success ? 'success' : 'failed');
    setMessage(result.success ? `OK · ${result.result_count} results` : result.message);
  };

  // Tooltip carries the detail (result count / error message) while the
  // button stays "Test" in all states.
  const tooltip = message || (state === 'idle' ? 'Test connection' : 'Test');

  return (
    <button
      type="button"
      onClick={handleTest}
      disabled={state === 'testing'}
      title={tooltip}
      data-state={state}
      className={cn(
        'inline-flex items-center gap-1 px-2.5 py-1 rounded-lg text-xs font-medium transition ring-1',
        state === 'idle' &&
          'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white ring-white/10',
        state === 'testing' && 'bg-white/5 text-[var(--text-muted)] ring-white/10 cursor-progress',
        state === 'success' && 'bg-green-500/10 text-green-300 ring-green-500/20',
        state === 'failed' && 'bg-red-500/10 text-red-300 ring-red-500/20'
      )}
    >
      <span className="w-3 h-3 inline-flex items-center justify-center flex-shrink-0">
        {state === 'idle' && <Wifi size={12} />}
        {state === 'testing' && <Loader2 size={12} className="animate-spin" />}
        {state === 'success' && <Check size={12} />}
        {state === 'failed' && <AlertCircle size={12} />}
      </span>
      Test
    </button>
  );
}

// ── Retry (reset escalation) ──

function RetryButton({ indexerId }: { indexerId: number }) {
  const qc = useQueryClient();
  const [state, setState] = useState<'idle' | 'running'>('idle');
  const handle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setState('running');
    try {
      await apiFetch(`/api/v1/indexers/${indexerId}/retry`, { method: 'POST' });
      qc.invalidateQueries({ queryKey: [...INDEXERS_KEY] });
    } catch {
      // Swallow — the escalation warning on the row will still be visible.
    } finally {
      setState('idle');
    }
  };
  return (
    <button
      type="button"
      onClick={handle}
      disabled={state === 'running'}
      className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs bg-yellow-500/10 text-yellow-300 hover:bg-yellow-500/20 hover:text-yellow-200 transition ring-1 ring-yellow-500/20"
    >
      {state === 'running' ? <Loader2 size={11} className="animate-spin" /> : null}
      Retry now
    </button>
  );
}

// ── Add Indexer Modal ──

type AddStep = 'choose' | 'browse' | 'torznab' | 'configure';

interface AddIndexerModalProps {
  onClose: () => void;
  onSaved: () => void;
}

// Top-level category taxonomy — must match the backend `top_level_categories`
// in indexers/loader.rs.
const CATEGORY_FILTERS = [
  // Kino is a TV+movie app, so the Movies/TV tabs are first and "TV+Movies"
  // is the default on open.
  { value: 'Movies+TV', label: 'TV + Movies' },
  { value: 'Movies', label: 'Movies' },
  { value: 'TV', label: 'TV' },
  { value: 'Anime', label: 'Anime' },
  { value: 'Audio', label: 'Music' },
  { value: 'Books', label: 'Books' },
  { value: 'all', label: 'All' },
] as const;

const LANGUAGE_OPTIONS = [
  { value: 'all', label: 'Any language' },
  { value: 'en', label: 'English' },
  { value: 'fr', label: 'French' },
  { value: 'de', label: 'German' },
  { value: 'es', label: 'Spanish' },
  { value: 'it', label: 'Italian' },
  { value: 'pt', label: 'Portuguese' },
  { value: 'ru', label: 'Russian' },
  { value: 'zh', label: 'Chinese' },
  { value: 'ja', label: 'Japanese' },
  { value: 'ko', label: 'Korean' },
] as const;

function AddIndexerModal({ onClose, onSaved }: AddIndexerModalProps) {
  const [step, setStep] = useState<AddStep>('choose');
  const [searchText, setSearchText] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const [typeFilter, setTypeFilter] = useState<string>('all');
  const [categoryFilter, setCategoryFilter] = useState<string>('Movies+TV');
  const [languageFilter, setLanguageFilter] = useState<string>('en');
  const [selectedDef, setSelectedDef] = useState<IndexerDefinition | null>(null);
  const [defDetail, setDefDetail] = useState<DefinitionDetail | null>(null);
  const [defDetailLoading, setDefDetailLoading] = useState(false);
  const [settingsValues, setSettingsValues] = useState<Record<string, string>>({});
  const [torznabForm, setTorznabForm] = useState({ name: '', url: '', api_key: '' });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [testResult, setTestResult] = useState<TestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const overlayRef = useRef<HTMLDivElement>(null);

  // Debounce search input
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(searchText), 300);
    return () => clearTimeout(timer);
  }, [searchText]);

  // Always-live count of locally-installed definitions. Drives the
  // choose-step gating: when 0, the "Browse Indexers" card hides and
  // the catalogue-fetch tile takes its place (mirrors the setup
  // wizard's behaviour). Re-fetched whenever the definitions
  // refresh job completes so the choose-step swaps from "fetch" to
  // "browse" without the user navigating away.
  const { data: definitionCount } = useQuery({
    queryKey: ['kino', 'indexer-definitions', 'count'],
    queryFn: async () => {
      const list = await apiFetch<IndexerDefinition[]>('/api/v1/indexer-definitions');
      return list.length;
    },
    meta: {
      invalidatedBy: ['indexer_definitions_refresh_completed'],
    },
  });

  // Fetch definitions for browse step. Server handles search/type/language;
  // the "Movies+TV" pseudo-category is expanded client-side to the union
  // (shown as a single filter to the user because it's the kino default).
  const categoryQueryParam =
    categoryFilter === 'all' || categoryFilter === 'Movies+TV' ? undefined : categoryFilter;
  const { data: rawDefinitions, isLoading: defsLoading } = useQuery({
    queryKey: [
      'kino',
      'indexer-definitions',
      debouncedSearch,
      typeFilter,
      categoryQueryParam,
      languageFilter,
    ],
    queryFn: async () => {
      const params = new URLSearchParams();
      if (debouncedSearch) params.set('search', debouncedSearch);
      if (typeFilter !== 'all') params.set('type', typeFilter);
      if (categoryQueryParam) params.set('category', categoryQueryParam);
      if (languageFilter !== 'all') params.set('language', languageFilter);
      const qs = params.toString();
      return apiFetch<IndexerDefinition[]>(`/api/v1/indexer-definitions${qs ? `?${qs}` : ''}`);
    },
    enabled: step === 'browse',
  });
  const definitions = useMemo(() => {
    if (!rawDefinitions) return rawDefinitions;
    if (categoryFilter !== 'Movies+TV') return rawDefinitions;
    return rawDefinitions.filter(
      (d) => d.categories.includes('Movies') || d.categories.includes('TV')
    );
  }, [rawDefinitions, categoryFilter]);

  // Fetch definition detail
  const loadDefinitionDetail = useCallback(async (def: IndexerDefinition) => {
    setSelectedDef(def);
    setDefDetailLoading(true);
    setSaveError('');
    setTestResult(null);
    try {
      const detail = await apiFetch<DefinitionDetail>(`/api/v1/indexer-definitions/${def.id}`);
      setDefDetail(detail);
      // Initialize settings with defaults
      const defaults: Record<string, string> = {};
      for (const s of detail.settings) {
        if (s.default !== undefined) {
          defaults[s.name] = s.default;
        } else if (s.type === 'checkbox') {
          defaults[s.name] = 'false';
        } else {
          defaults[s.name] = '';
        }
      }
      setSettingsValues(defaults);
    } catch {
      // If we can't load details, just show an empty form
      setDefDetail({ ...def, links: [], settings: [] });
    } finally {
      setDefDetailLoading(false);
    }
  }, []);

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);

  const handleSaveCardigann = async () => {
    if (!selectedDef || !defDetail) return;
    setSaving(true);
    setSaveError('');
    try {
      const body = {
        name: selectedDef.name,
        url: defDetail.links[0] || `https://${selectedDef.id}`,
        indexer_type: 'cardigann',
        definition_id: selectedDef.id,
        settings_json: JSON.stringify(settingsValues),
        priority: 25,
        enabled: true,
      };
      await apiFetch('/api/v1/indexers', { method: 'POST', body: JSON.stringify(body) });
      onSaved();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  const handleSaveTorznab = async () => {
    if (!torznabForm.name || !torznabForm.url) return;
    setSaving(true);
    setSaveError('');
    try {
      const body = {
        name: torznabForm.name,
        url: torznabForm.url,
        api_key: torznabForm.api_key || undefined,
        indexer_type: 'torznab',
        priority: 25,
        enabled: true,
      };
      await apiFetch('/api/v1/indexers', { method: 'POST', body: JSON.stringify(body) });
      onSaved();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  const handleTestAfterSave = async (saveFn: () => Promise<void>) => {
    setTesting(true);
    setTestResult(null);
    setSaveError('');
    try {
      await saveFn();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : 'Failed to save');
      setTesting(false);
    }
  };

  const haveDefs = (definitionCount ?? 0) > 0;

  const renderChooseStep = () => (
    <div className="p-5 space-y-3">
      {/* Catalogue-fetch tile: matches the wizard's heuristic — when
          there are 0 local definitions, this REPLACES the "Browse
          Indexers" button (which would just show an empty grid).
          Once defs land, the tile collapses into a small pill and
          the Browse button appears, giving the user a clean two-
          step flow: fetch → browse. */}
      <CatalogueTile localCount={definitionCount ?? 0} />
      {haveDefs && (
        <button
          type="button"
          onClick={() => setStep('browse')}
          className="w-full p-5 rounded-xl bg-white/[0.03] ring-1 ring-white/5 hover:ring-white/15 hover:bg-white/[0.05] transition text-left group"
        >
          <div className="flex items-start gap-4">
            <div className="w-10 h-10 rounded-lg bg-[var(--accent)]/15 grid place-items-center flex-shrink-0">
              <Database size={20} className="text-[var(--accent)]" />
            </div>
            <div>
              <p className="text-sm font-semibold text-white group-hover:text-[var(--accent)] transition">
                Browse Indexers
              </p>
              <p className="text-xs text-[var(--text-muted)] mt-1">
                Choose from {definitionCount}+ supported sites with built-in Cardigann definitions
              </p>
            </div>
          </div>
        </button>
      )}
      <button
        type="button"
        onClick={() => {
          setStep('torznab');
          setSaveError('');
          setTestResult(null);
        }}
        className="w-full p-5 rounded-xl bg-white/[0.03] ring-1 ring-white/5 hover:ring-white/15 hover:bg-white/[0.05] transition text-left group"
      >
        <div className="flex items-start gap-4">
          <div className="w-10 h-10 rounded-lg bg-blue-500/15 grid place-items-center flex-shrink-0">
            <ExternalLink size={20} className="text-blue-400" />
          </div>
          <div>
            <p className="text-sm font-semibold text-white group-hover:text-blue-400 transition">
              Torznab / Newznab
            </p>
            <p className="text-xs text-[var(--text-muted)] mt-1">
              Manual URL entry for Prowlarr, Jackett, or other Torznab-compatible endpoints
            </p>
          </div>
        </div>
      </button>
    </div>
  );

  const renderBrowseStep = () => (
    <div className="flex flex-col h-[60vh]">
      {/* Search bar */}
      <div className="px-5 pt-4 pb-3 space-y-3 border-b border-white/5">
        <div className="relative">
          <Search
            size={16}
            className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
          />
          <input
            // biome-ignore lint/a11y/noAutofocus: focuses when the browse step becomes active
            autoFocus
            type="text"
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            placeholder="Search indexers..."
            className="w-full h-9 pl-9 pr-3 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
          />
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {CATEGORY_FILTERS.map((c) => (
            <button
              key={c.value}
              type="button"
              onClick={() => setCategoryFilter(c.value)}
              className={cn(
                'px-3 py-1 rounded-lg text-xs font-medium transition',
                categoryFilter === c.value
                  ? 'bg-[var(--accent)] text-white'
                  : 'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white'
              )}
            >
              {c.label}
            </button>
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex gap-1">
            {['all', 'public', 'semi-private', 'private'].map((t) => (
              <button
                key={t}
                type="button"
                onClick={() => setTypeFilter(t)}
                className={cn(
                  'px-2.5 py-0.5 rounded-md text-[11px] font-medium transition',
                  typeFilter === t
                    ? 'bg-white/10 text-white'
                    : 'text-[var(--text-muted)] hover:text-white'
                )}
              >
                {t === 'all'
                  ? 'Any'
                  : t === 'semi-private'
                    ? 'Semi'
                    : t.charAt(0).toUpperCase() + t.slice(1)}
              </button>
            ))}
          </div>
          <select
            value={languageFilter}
            onChange={(e) => setLanguageFilter(e.target.value)}
            className="h-7 pl-2 pr-7 rounded-md bg-white/5 text-xs text-[var(--text-secondary)] hover:bg-white/10 focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
          >
            {LANGUAGE_OPTIONS.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </select>
          {definitions && !defsLoading && (
            <span className="text-[11px] text-[var(--text-muted)] ml-auto">
              {definitions.length} of {rawDefinitions?.length ?? 0}
            </span>
          )}
        </div>
      </div>

      {/* Definition grid */}
      <div className="flex-1 overflow-y-auto px-5 py-3">
        {defsLoading && (
          <div className="flex items-center justify-center py-12 text-[var(--text-muted)]">
            <Loader2 size={20} className="animate-spin" />
          </div>
        )}
        {!defsLoading && rawDefinitions && rawDefinitions.length === 0 && (
          <div className="text-center py-12 text-[var(--text-muted)]">
            <Search size={24} className="mx-auto mb-2 opacity-30" />
            <p className="text-sm">No indexers match your filters</p>
          </div>
        )}
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          {definitions?.map((def) => (
            <button
              key={def.id}
              type="button"
              onClick={() => {
                loadDefinitionDetail(def);
                setStep('configure');
              }}
              className="p-3 rounded-lg bg-white/[0.03] ring-1 ring-white/5 hover:ring-white/15 hover:bg-white/[0.05] transition text-left"
            >
              <div className="flex items-start justify-between gap-2">
                <p className="text-sm font-medium text-white truncate">{def.name}</p>
                <TypeBadge type={def.indexer_type} />
              </div>
              {def.description && (
                <p className="text-xs text-[var(--text-muted)] mt-1 line-clamp-2">
                  {def.description}
                </p>
              )}
              {def.language && def.language !== 'en-US' && (
                <p className="text-[10px] text-[var(--text-muted)] mt-1">{def.language}</p>
              )}
            </button>
          ))}
        </div>
      </div>
    </div>
  );

  const renderTorznabStep = () => (
    <div className="px-5 py-4 space-y-1">
      <FormField label="Name">
        <TextInput
          autoFocus
          value={torznabForm.name}
          onChange={(v) => setTorznabForm((f) => ({ ...f, name: v }))}
          placeholder="My Indexer"
        />
      </FormField>
      <FormField label="URL" description="Torznab base URL">
        <TextInput
          value={torznabForm.url}
          onChange={(v) => setTorznabForm((f) => ({ ...f, url: v }))}
          placeholder="https://..."
          type="url"
        />
      </FormField>
      <FormField label="API Key">
        <SecretInput
          value={torznabForm.api_key}
          onChange={(v) => setTorznabForm((f) => ({ ...f, api_key: v }))}
        />
      </FormField>
      {saveError && (
        <p className="flex items-center gap-1.5 text-xs text-red-400 pt-2">
          <AlertCircle size={12} />
          {saveError}
        </p>
      )}
      {testResult && (
        <p
          className={cn(
            'flex items-center gap-1.5 text-xs pt-2',
            testResult.success ? 'text-green-400' : 'text-red-400'
          )}
        >
          {testResult.success ? <Check size={12} /> : <AlertCircle size={12} />}
          {testResult.message}
          {testResult.success && ` (${testResult.result_count} results)`}
        </p>
      )}
    </div>
  );

  const renderConfigureStep = () => (
    <div className="px-5 py-4 max-h-[60vh] overflow-y-auto">
      {defDetailLoading && (
        <div className="flex items-center justify-center py-12 text-[var(--text-muted)]">
          <Loader2 size={20} className="animate-spin" />
        </div>
      )}
      {!defDetailLoading && defDetail && (
        <div className="space-y-1">
          {/* Definition info header */}
          <div className="pb-3 mb-2 border-b border-white/5">
            <div className="flex items-center gap-2">
              <p className="text-sm font-semibold text-white">{defDetail.name}</p>
              <TypeBadge type={defDetail.indexer_type} />
            </div>
            {defDetail.description && (
              <p className="text-xs text-[var(--text-muted)] mt-1">{defDetail.description}</p>
            )}
          </div>

          {/* Settings fields */}
          {defDetail.settings.length === 0 && (
            <p className="text-sm text-[var(--text-muted)] py-4 text-center">
              No configuration needed. This indexer is ready to use.
            </p>
          )}
          {defDetail.settings.map((setting) => {
            if (setting.type === 'info') {
              return (
                <div
                  key={setting.name}
                  className="py-3 text-xs text-[var(--text-secondary)] bg-white/5 rounded-lg p-3"
                >
                  {setting.label}
                </div>
              );
            }

            if (setting.type === 'checkbox') {
              return (
                <FormField key={setting.name} label={setting.label}>
                  <Toggle
                    checked={settingsValues[setting.name] === 'true'}
                    onChange={(v) =>
                      setSettingsValues((s) => ({
                        ...s,
                        [setting.name]: v ? 'true' : 'false',
                      }))
                    }
                  />
                </FormField>
              );
            }

            if (setting.type === 'select' && setting.options) {
              const options = Object.entries(setting.options).map(([value, label]) => ({
                value,
                label,
              }));
              return (
                <FormField key={setting.name} label={setting.label}>
                  <select
                    value={settingsValues[setting.name] ?? ''}
                    onChange={(e) =>
                      setSettingsValues((s) => ({
                        ...s,
                        [setting.name]: e.target.value,
                      }))
                    }
                    className="h-9 px-3 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                  >
                    {options.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                </FormField>
              );
            }

            if (setting.type === 'password') {
              return (
                <FormField key={setting.name} label={setting.label}>
                  <SecretInput
                    value={settingsValues[setting.name] ?? ''}
                    onChange={(v) =>
                      setSettingsValues((s) => ({
                        ...s,
                        [setting.name]: v,
                      }))
                    }
                  />
                </FormField>
              );
            }

            // Default: text input
            return (
              <FormField key={setting.name} label={setting.label}>
                <TextInput
                  value={settingsValues[setting.name] ?? ''}
                  onChange={(v) =>
                    setSettingsValues((s) => ({
                      ...s,
                      [setting.name]: v,
                    }))
                  }
                  placeholder={setting.default ?? ''}
                />
              </FormField>
            );
          })}

          {saveError && (
            <p className="flex items-center gap-1.5 text-xs text-red-400 pt-2">
              <AlertCircle size={12} />
              {saveError}
            </p>
          )}
          {testResult && (
            <p
              className={cn(
                'flex items-center gap-1.5 text-xs pt-2',
                testResult.success ? 'text-green-400' : 'text-red-400'
              )}
            >
              {testResult.success ? <Check size={12} /> : <AlertCircle size={12} />}
              {testResult.message}
              {testResult.success && ` (${testResult.result_count} results)`}
            </p>
          )}
        </div>
      )}
    </div>
  );

  const getStepTitle = (): string => {
    switch (step) {
      case 'choose':
        return 'Add Indexer';
      case 'browse':
        return 'Browse Indexers';
      case 'torznab':
        return 'Add Torznab Indexer';
      case 'configure':
        return selectedDef ? `Configure ${selectedDef.name}` : 'Configure Indexer';
    }
  };

  const canGoBack = step !== 'choose';
  const handleBack = () => {
    setSaveError('');
    setTestResult(null);
    if (step === 'browse' || step === 'torznab') {
      setStep('choose');
    } else if (step === 'configure') {
      setStep('browse');
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
      <div className="relative w-full max-w-lg bg-[var(--bg-secondary)] rounded-xl ring-1 ring-white/10 shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="flex items-center gap-3 px-5 py-4 border-b border-white/5">
          {canGoBack && (
            <button
              type="button"
              onClick={handleBack}
              className="p-1 rounded-lg text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition"
            >
              <ChevronLeft size={18} />
            </button>
          )}
          <h2 className="text-lg font-semibold flex-1">{getStepTitle()}</h2>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded-lg text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition"
          >
            <X size={18} />
          </button>
        </div>

        {/* Content */}
        {step === 'choose' && renderChooseStep()}
        {step === 'browse' && renderBrowseStep()}
        {step === 'torznab' && renderTorznabStep()}
        {step === 'configure' && renderConfigureStep()}

        {/* Footer for saveable steps */}
        {(step === 'torznab' || step === 'configure') && (
          <div className="flex items-center justify-end gap-2 px-5 py-4 border-t border-white/5">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={() => {
                if (step === 'torznab') {
                  handleTestAfterSave(handleSaveTorznab);
                } else {
                  handleTestAfterSave(handleSaveCardigann);
                }
              }}
              disabled={
                saving || testing || (step === 'torznab' && (!torznabForm.name || !torznabForm.url))
              }
              className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
            >
              {(saving || testing) && <Loader2 size={14} className="animate-spin" />}
              Save
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body
  );
}

// ── Edit Indexer Modal ──

interface EditIndexerModalProps {
  indexer: ConfiguredIndexer;
  onClose: () => void;
  onSaved: () => void;
  onDelete: (id: number) => void;
  deleting: boolean;
}

function EditIndexerModal({
  indexer,
  onClose,
  onSaved,
  onDelete,
  deleting,
}: EditIndexerModalProps) {
  const [form, setForm] = useState({
    name: indexer.name,
    url: indexer.url,
    api_key: indexer.api_key ?? '',
    priority: indexer.priority,
    enabled: indexer.enabled,
    settings_json: indexer.settings_json ?? '{}',
  });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const overlayRef = useRef<HTMLDivElement>(null);

  // Parse settings for cardigann indexers
  const settingsObj = useMemo(() => {
    try {
      return JSON.parse(form.settings_json) as Record<string, string>;
    } catch {
      return {};
    }
  }, [form.settings_json]);

  // If Cardigann, load definition detail
  const { data: defDetail } = useQuery({
    queryKey: ['kino', 'indexer-definition-detail', indexer.definition_id],
    queryFn: async () => {
      return apiFetch<DefinitionDetail>(`/api/v1/indexer-definitions/${indexer.definition_id}`);
    },
    enabled: !!indexer.definition_id,
  });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);

  const handleSave = async () => {
    setSaving(true);
    setSaveError('');
    try {
      const body: Record<string, unknown> = {
        name: form.name,
        url: form.url,
        api_key: form.api_key || undefined,
        priority: form.priority,
        enabled: form.enabled,
      };
      if (indexer.definition_id) {
        body.settings_json = form.settings_json;
      }
      await apiFetch(`/api/v1/indexers/${indexer.id}`, {
        method: 'PUT',
        body: JSON.stringify(body),
      });
      onSaved();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  const updateSetting = (name: string, value: string) => {
    const updated = { ...settingsObj, [name]: value };
    setForm((f) => ({ ...f, settings_json: JSON.stringify(updated) }));
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
      <div className="relative w-full max-w-md bg-[var(--bg-secondary)] rounded-xl ring-1 ring-white/10 shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-5 py-4 border-b border-white/5">
          <h2 className="text-lg font-semibold">Edit Indexer</h2>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded-lg text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition"
          >
            <X size={18} />
          </button>
        </div>
        <div className="px-5 py-4 space-y-1 max-h-[60vh] overflow-y-auto">
          <FormField label="Name">
            <TextInput
              value={form.name}
              onChange={(v) => setForm((f) => ({ ...f, name: v }))}
              placeholder="My Indexer"
            />
          </FormField>

          {indexer.indexer_type === 'torznab' && (
            <>
              <FormField label="URL" description="Torznab base URL">
                <TextInput
                  value={form.url}
                  onChange={(v) => setForm((f) => ({ ...f, url: v }))}
                  placeholder="https://..."
                  type="url"
                />
              </FormField>
              <FormField label="API Key">
                <SecretInput
                  value={form.api_key}
                  onChange={(v) => setForm((f) => ({ ...f, api_key: v }))}
                />
              </FormField>
            </>
          )}

          {/* Cardigann settings */}
          {indexer.definition_id && defDetail?.settings && (
            <div className="border-t border-white/5 pt-3 mt-3">
              <p className="text-xs font-semibold text-[var(--text-muted)] uppercase tracking-wider mb-2">
                Indexer Settings
              </p>
              {defDetail.settings.map((setting) => {
                if (setting.type === 'info') {
                  return (
                    <div
                      key={setting.name}
                      className="py-3 text-xs text-[var(--text-secondary)] bg-white/5 rounded-lg p-3 my-2"
                    >
                      {setting.label}
                    </div>
                  );
                }
                if (setting.type === 'checkbox') {
                  return (
                    <FormField key={setting.name} label={setting.label}>
                      <Toggle
                        checked={settingsObj[setting.name] === 'true'}
                        onChange={(v) => updateSetting(setting.name, v ? 'true' : 'false')}
                      />
                    </FormField>
                  );
                }
                if (setting.type === 'select' && setting.options) {
                  const opts = Object.entries(setting.options).map(([value, label]) => ({
                    value,
                    label,
                  }));
                  return (
                    <FormField key={setting.name} label={setting.label}>
                      <select
                        value={settingsObj[setting.name] ?? ''}
                        onChange={(e) => updateSetting(setting.name, e.target.value)}
                        className="h-9 px-3 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                      >
                        {opts.map((o) => (
                          <option key={o.value} value={o.value}>
                            {o.label}
                          </option>
                        ))}
                      </select>
                    </FormField>
                  );
                }
                if (setting.type === 'password') {
                  return (
                    <FormField key={setting.name} label={setting.label}>
                      <SecretInput
                        value={settingsObj[setting.name] ?? ''}
                        onChange={(v) => updateSetting(setting.name, v)}
                      />
                    </FormField>
                  );
                }
                return (
                  <FormField key={setting.name} label={setting.label}>
                    <TextInput
                      value={settingsObj[setting.name] ?? ''}
                      onChange={(v) => updateSetting(setting.name, v)}
                      placeholder={setting.default ?? ''}
                    />
                  </FormField>
                );
              })}
            </div>
          )}

          {/* Priority is now controlled by drag-and-drop in the list view. */}
          <FormField label="Enabled">
            <Toggle
              checked={form.enabled}
              onChange={(v) => setForm((f) => ({ ...f, enabled: v }))}
            />
          </FormField>

          {saveError && (
            <p className="flex items-center gap-1.5 text-xs text-red-400 pt-2">
              <AlertCircle size={12} />
              {saveError}
            </p>
          )}
        </div>
        <div className="flex items-center justify-between px-5 py-4 border-t border-white/5">
          <div>
            {!confirmDelete ? (
              <button
                type="button"
                onClick={() => setConfirmDelete(true)}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm text-red-400 hover:bg-red-600/10 transition"
              >
                <Trash2 size={14} />
                Delete
              </button>
            ) : (
              <div className="flex items-center gap-2">
                <span className="text-xs text-red-400">Are you sure?</span>
                <button
                  type="button"
                  onClick={() => onDelete(indexer.id)}
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
              disabled={saving || !form.name}
              className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
            >
              {saving && <Loader2 size={14} className="animate-spin" />}
              Save
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body
  );
}

// ── Sortable row ──
//
// Wraps the indexer row with dnd-kit's `useSortable`. The drag handle
// on the left owns the drag listeners so clicks on the rest of the row
// (Test, Edit, Toggle, URL link) keep working without accidentally
// starting a drag.

interface SortableRowProps {
  indexer: ConfiguredIndexer;
  onEdit: () => void;
  onToggle: (enabled: boolean) => void;
}

function SortableIndexerRow({ indexer, onEdit, onToggle }: SortableRowProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: indexer.id,
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
        'p-4 rounded-xl bg-white/[0.03] ring-1 ring-white/5 hover:ring-white/10 transition-colors',
        isDragging && 'opacity-50 ring-white/20 z-10 relative'
      )}
    >
      <div className="flex items-center gap-3">
        {/* Drag handle */}
        <button
          type="button"
          {...attributes}
          {...listeners}
          aria-label={`Reorder ${indexer.name}`}
          className="p-1 -ml-1 rounded text-[var(--text-muted)] hover:text-white hover:bg-white/5 cursor-grab active:cursor-grabbing touch-none flex-shrink-0"
        >
          <GripVertical size={14} />
        </button>

        {/* Priority badge — the number under which this indexer is queried */}
        <span
          className="text-[10px] font-mono tabular-nums text-[var(--text-muted)] w-6 text-right flex-shrink-0"
          title={`Priority ${indexer.priority} — drag to reorder`}
        >
          #{indexer.priority}
        </span>

        {/* Status and info */}
        <StatusDot indexer={indexer} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <p className="text-sm font-medium text-white truncate">{indexer.name}</p>
            <TypeBadge type={indexer.indexer_type} />
          </div>
          {indexer.url ? (
            <a
              href={indexer.url}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              className="inline-flex items-center gap-1 text-xs text-[var(--text-muted)] hover:text-white mt-0.5 truncate max-w-full"
              title={indexer.url}
            >
              <span className="truncate">{indexer.url}</span>
              <ExternalLink size={10} className="flex-shrink-0 opacity-60" />
            </a>
          ) : null}
        </div>

        {/* Actions */}
        <div className="flex items-center gap-2 flex-shrink-0">
          <InlineTestButton indexerId={indexer.id} />

          <button
            type="button"
            onClick={onEdit}
            className="p-1.5 rounded-lg text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition"
            title="Edit"
          >
            <Edit2 size={14} />
          </button>

          <Toggle checked={indexer.enabled} onChange={onToggle} />
        </div>
      </div>

      {/* Escalation warning with Retry Now */}
      {indexer.escalation_level > 0 && indexer.disabled_until && (
        <div className="mt-2 ml-6 flex items-center gap-2 text-xs">
          <AlertCircle size={12} className="text-yellow-400 flex-shrink-0" />
          <span className="text-yellow-400 flex-1 min-w-0 truncate">
            Backoff level {indexer.escalation_level} — disabled until{' '}
            {new Date(indexer.disabled_until).toLocaleString()}
          </span>
          <RetryButton indexerId={indexer.id} />
        </div>
      )}
    </div>
  );
}

// ── Main component ──

export function IndexerSettings() {
  const queryClient = useQueryClient();
  const [showAddModal, setShowAddModal] = useState(false);
  const [editingIndexer, setEditingIndexer] = useState<ConfiguredIndexer | null>(null);

  // Fetch configured indexers
  const { data, isLoading } = useQuery({
    queryKey: [...INDEXERS_KEY],
    queryFn: async () => {
      return apiFetch<ConfiguredIndexer[]>('/api/v1/indexers');
    },
    meta: { invalidatedBy: ['indexer_changed'] },
  });

  // Toggle enable/disable — backend emits `IndexerChanged` on every
  // PUT, which the meta dispatcher routes to INDEXERS_KEY via the
  // useQuery's invalidatedBy tag. No explicit invalidate needed.
  const toggleMutation = useMutationWithToast({
    verb: 'toggle indexer',
    mutationFn: async ({ id, enabled }: { id: number; enabled: boolean }) => {
      await apiFetch(`/api/v1/indexers/${id}`, {
        method: 'PUT',
        body: JSON.stringify({ enabled }),
      });
    },
  });

  // Delete indexer
  const deleteMutation = useMutationWithToast({
    verb: 'delete indexer',
    mutationFn: async (id: number) => {
      await apiFetch(`/api/v1/indexers/${id}`, { method: 'DELETE' });
    },
    onSuccess: () => {
      setEditingIndexer(null);
    },
  });

  const handleSaved = () => {
    setShowAddModal(false);
    setEditingIndexer(null);
  };

  const indexers = data ?? [];

  // ── Drag-to-reorder ─────────────────────────────────────────────
  const sensors = useSensors(
    // 4px drag before the drag starts — lets the click events on the
    // handle itself still feel responsive without accidentally dragging.
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    const oldIndex = indexers.findIndex((i) => i.id === active.id);
    const newIndex = indexers.findIndex((i) => i.id === over.id);
    if (oldIndex < 0 || newIndex < 0) return;

    const reordered = arrayMove(indexers, oldIndex, newIndex);

    // Optimistically update the cache with the new order + recomputed
    // priorities so the UI reflects the drop instantly.
    const withPriority = reordered.map((ind, i) => ({ ...ind, priority: i + 1 }));
    queryClient.setQueryData<ConfiguredIndexer[]>([...INDEXERS_KEY], withPriority);

    // Fire a PUT for each row whose priority actually changed. The WS
    // IndexerChanged events that come back will refetch — no need to
    // invalidate here. On failure, invalidate to snap back to server.
    const changed = withPriority.filter((ind, i) => indexers[i]?.id !== ind.id);
    Promise.all(
      changed.map((ind) =>
        apiFetch(`/api/v1/indexers/${ind.id}`, {
          method: 'PUT',
          body: JSON.stringify({ priority: ind.priority }),
        })
      )
    ).catch(() => {
      queryClient.invalidateQueries({ queryKey: [...INDEXERS_KEY] });
    });
  };

  return (
    <div>
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-bold">Indexers</h1>
          <p className="text-sm text-[var(--text-muted)]">
            Configure torrent indexer sources for searching releases
          </p>
        </div>
        <button
          type="button"
          onClick={() => setShowAddModal(true)}
          className="flex items-center gap-1.5 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold transition"
        >
          <Plus size={16} />
          Add Indexer
        </button>
      </div>

      {/* Loading skeleton */}
      {isLoading && (
        <div className="space-y-2">
          <div className="h-20 skeleton rounded-xl" />
          <div className="h-20 skeleton rounded-xl" />
        </div>
      )}

      {/* Empty state — the header already has an Add button, so just point to it. */}
      {!isLoading && indexers.length === 0 && (
        <div className="text-center py-16 text-[var(--text-muted)]">
          <Wifi size={36} className="mx-auto mb-3 opacity-30" />
          <p className="text-sm font-medium">No indexers configured</p>
          <p className="text-xs mt-1">Use the Add Indexer button above to get started.</p>
        </div>
      )}

      {/* Indexer list — drag rows to reorder; priority is position + 1. */}
      {!isLoading && indexers.length > 0 && (
        <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
          <SortableContext items={indexers.map((i) => i.id)} strategy={verticalListSortingStrategy}>
            <div className="space-y-2">
              {indexers.map((idx) => (
                <SortableIndexerRow
                  key={idx.id}
                  indexer={idx}
                  onEdit={() => setEditingIndexer(idx)}
                  onToggle={(v) => toggleMutation.mutate({ id: idx.id, enabled: v })}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      )}

      {/* Add indexer modal */}
      {showAddModal && (
        <AddIndexerModal onClose={() => setShowAddModal(false)} onSaved={handleSaved} />
      )}

      {/* Edit indexer modal */}
      {editingIndexer && (
        <EditIndexerModal
          indexer={editingIndexer}
          onClose={() => setEditingIndexer(null)}
          onSaved={handleSaved}
          onDelete={(id) => deleteMutation.mutate(id)}
          deleting={deleteMutation.isPending}
        />
      )}
    </div>
  );
}

const PROWLARR_REPO_URL = 'https://github.com/Prowlarr/Indexers';

/**
 * Settings-page port of the wizard's `DefinitionsCatalogueTile`.
 *
 * - 0 local defs + idle → big "Fetch catalogue (~30s)" CTA card.
 * - Running → progress bar with `fetched / total`.
 * - Already loaded → small pill ("X loaded · Refresh · Source").
 * - Failed → retry CTA with the upstream error verbatim.
 *
 * Same `useQuery` polling cadence (500ms while running, idle
 * otherwise) and `meta.invalidatedBy` tags as the wizard so cross-
 * tab refreshes propagate. The mutation re-invalidates the
 * indexer-definitions list query on success, which triggers the
 * grid to refetch once the new tarball lands.
 */
function CatalogueTile({ localCount }: { localCount: number }) {
  const qc = useQueryClient();
  const { data: state } = useQuery({
    queryKey: ['kino', 'indexer-definitions', 'refresh'],
    queryFn: async () => {
      const { data } = await getRefreshState();
      return data ?? ({ status: 'idle' } satisfies DefinitionsRefreshState);
    },
    refetchInterval: (q) => (q.state.data?.status === 'running' ? 500 : false),
    meta: {
      invalidatedBy: [
        'indexer_definitions_refresh_completed',
        'indexer_definitions_refresh_failed',
      ],
    },
  });

  const startMutation = useMutation({
    mutationFn: async () => {
      await refreshDefinitions();
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'indexer-definitions', 'refresh'] });
    },
  });

  const status = state?.status ?? 'idle';
  const running = status === 'running';
  const completed = status === 'completed';
  const failed = status === 'failed';
  const effectiveCount =
    state?.status === 'completed' ? Math.max(localCount, state.count) : localCount;

  // When a run completes, refetch the definitions grid so the new
  // entries land without a hard reload. The grid's query key is
  // `['kino', 'indexer-definitions', <search>, <type>, <cat>, <lang>]`
  // — invalidating the prefix `['kino', 'indexer-definitions']`
  // matches every variant (default tanstack-query behaviour for
  // hierarchical key matching). Without the `kino` prefix, no
  // queries match and the grid stays stale until the user
  // navigates away and back.
  const prevCompletedRef = useRef(completed);
  useEffect(() => {
    if (completed && !prevCompletedRef.current) {
      // Hierarchical invalidation: every query whose key starts
      // with `['kino', 'indexer-definitions', ...]` refetches.
      // Covers both the grid (key includes filters) and the
      // count query that drives the choose-step's tile-vs-button
      // toggle. Without that prefix match, the UI would stay
      // stuck on "Fetch catalogue" until a manual reload.
      qc.invalidateQueries({ queryKey: ['kino', 'indexer-definitions'] });
    }
    prevCompletedRef.current = completed;
  }, [completed, qc]);

  if (running) {
    const fetched = state?.status === 'running' ? state.fetched : 0;
    const total = state?.status === 'running' ? state.total : 0;
    const pct = total > 0 ? Math.min(100, Math.round((fetched / total) * 100)) : 0;
    return (
      <div className="mb-3 rounded-lg bg-white/[0.03] ring-1 ring-white/5 p-3">
        <div className="flex items-center gap-2 mb-2">
          <Loader2 size={14} className="animate-spin text-[var(--accent)]" />
          <span className="text-sm font-semibold text-white">Fetching indexer catalogue…</span>
          <span className="ml-auto text-xs text-[var(--text-muted)]">
            {fetched}/{total > 0 ? total : '?'}
          </span>
        </div>
        <div className="h-1.5 rounded-full bg-white/5 overflow-hidden">
          <div className="h-full bg-[var(--accent)] transition-all" style={{ width: `${pct}%` }} />
        </div>
      </div>
    );
  }

  if (effectiveCount > 0) {
    return (
      <div
        className="mb-3 flex items-center justify-between rounded-lg bg-white/[0.02] px-3 py-2 text-xs ring-1 ring-white/5"
        aria-live="polite"
      >
        <span className="flex items-center gap-2 text-[var(--text-muted)]">
          <Check size={12} className="text-green-400" />
          {effectiveCount} indexer definitions loaded
        </span>
        <div className="flex items-center gap-3">
          <a
            href={PROWLARR_REPO_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-[var(--text-muted)] transition hover:text-white"
            title="Source: Prowlarr/Indexers on GitHub"
          >
            <Github size={11} />
            Source
          </a>
          <button
            type="button"
            onClick={() => startMutation.mutate()}
            disabled={startMutation.isPending}
            className="flex items-center gap-1 text-[var(--text-muted)] transition hover:text-white disabled:opacity-50"
          >
            <RefreshCw size={11} className={startMutation.isPending ? 'animate-spin' : ''} />
            Refresh
          </button>
        </div>
      </div>
    );
  }

  if (failed) {
    const message = state?.status === 'failed' ? state.reason : 'Unknown error';
    return (
      <div className="mb-3 rounded-lg bg-red-500/10 ring-1 ring-red-500/20 p-3">
        <div className="flex items-start gap-2 mb-2">
          <AlertCircle size={14} className="text-red-400 mt-0.5 flex-shrink-0" />
          <div className="flex-1 min-w-0">
            <p className="text-sm font-semibold text-red-300">Catalogue fetch failed</p>
            <p className="text-xs text-[var(--text-muted)] mt-0.5">{message}</p>
          </div>
        </div>
        <button
          type="button"
          onClick={() => startMutation.mutate()}
          disabled={startMutation.isPending}
          className="ml-6 flex items-center gap-1 text-xs text-[var(--text-muted)] transition hover:text-white disabled:opacity-50"
        >
          <RefreshCw size={11} className={startMutation.isPending ? 'animate-spin' : ''} />
          Retry
        </button>
      </div>
    );
  }

  return (
    <div className="mb-4 rounded-xl bg-gradient-to-br from-[var(--accent)]/10 to-white/[0.02] p-4 ring-1 ring-[var(--accent)]/20">
      <div className="flex items-start gap-3">
        <div className="grid h-10 w-10 flex-shrink-0 place-items-center rounded-lg bg-[var(--accent)]/15">
          <Library size={20} className="text-[var(--accent)]" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-white">Indexer catalogue</p>
          <p className="mt-0.5 text-xs text-[var(--text-muted)]">
            kino can browse a community-maintained set of pre-configured indexers from the
            Prowlarr/Indexers repository on GitHub. Fetching is opt-in and takes about 30 seconds.
            Skip if you'd rather plug in indexer URLs by hand.
          </p>
          <div className="mt-3 flex flex-wrap items-center gap-3">
            <button
              type="button"
              onClick={() => startMutation.mutate()}
              disabled={startMutation.isPending}
              className="inline-flex items-center gap-2 rounded-md bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-white transition hover:bg-[var(--accent-hover)] disabled:opacity-50"
            >
              <Download size={12} />
              Fetch catalogue
            </button>
            <a
              href={PROWLARR_REPO_URL}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-xs text-[var(--text-muted)] transition hover:text-white"
            >
              <Github size={12} />
              Prowlarr/Indexers
              <ExternalLink size={10} className="opacity-60" />
            </a>
          </div>
        </div>
      </div>
    </div>
  );
}
