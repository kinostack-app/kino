import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle,
  Cpu,
  Database,
  Download,
  Edit2,
  ExternalLink,
  FolderOpen,
  Github,
  Globe,
  Library,
  Loader2,
  Lock,
  RefreshCw,
  Search,
  Shield,
  Sparkles,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  cancelFfmpegDownload,
  getFfmpegDownload,
  getRefreshState,
  listIndexers,
  listQualityProfiles,
  refreshDefinitions,
  startFfmpegDownload,
  testTmdb,
  updateConfig,
  updateQualityProfile,
} from '@/api/generated/sdk.gen';
import type { DefinitionsRefreshState, FfmpegDownloadState } from '@/api/generated/types.gen';
import { FormField, SecretInput, TextInput } from '@/components/settings/FormField';
import { PathBrowser } from '@/components/settings/PathBrowser';
import { cn } from '@/lib/utils';

// The setup wizard runs against a backend that has just init'd its
// api_key but the user hasn't seen it yet. We rely on the AutoLocalhost
// cookie that `GET /api/v1/bootstrap` auto-issues for same-host
// browsers (see `auth_session/handlers.rs::bootstrap`) — by the time
// the wizard mounts, AuthGate has already called bootstrap, so the
// cookie is in the browser jar. `credentials: 'include'` is what tells
// fetch to send it.
const headers = {
  'Content-Type': 'application/json',
};

async function apiFetch<T>(url: string, opts?: RequestInit): Promise<T> {
  const res = await fetch(url, { ...opts, headers, credentials: 'include' });
  if (!res.ok) throw new Error(`${res.status}`);
  if (res.status === 204 || res.headers.get('content-length') === '0') return undefined as T;
  return res.json();
}

interface SetupWizardProps {
  onComplete: () => void;
  onSave: (config: Record<string, unknown>) => Promise<void>;
}

interface IndexerDefinition {
  id: string;
  name: string;
  description: string;
  indexer_type: string;
  language: string;
}

interface DefinitionDetail extends IndexerDefinition {
  links: string[];
  settings: Array<{
    name: string;
    type?: string;
    label?: string;
    default?: string;
    options?: Record<string, string>;
  }>;
}

const STEPS = [
  { title: 'Storage', description: 'Where your media lives' },
  { title: 'Metadata', description: 'TMDB API for content info' },
  { title: 'Languages', description: "Which languages you'll watch in" },
  { title: 'Indexers', description: 'Where to search for releases' },
  { title: 'Transcode', description: 'Optional: bundled ffmpeg' },
  { title: 'Done', description: "You're all set" },
];

/// Quality profile language picker — same ordered list as
/// `/settings/quality`. Default pick is English; the wizard seeds
/// the default quality profile on advance, which the search scorer
/// then hard-rejects any release that doesn't match.
const WIZARD_LANGUAGES: Array<{ code: string; label: string }> = [
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

type TestState = 'idle' | 'testing' | 'pass' | 'fail';

export function SetupWizard({ onComplete, onSave }: SetupWizardProps) {
  const [step, setStep] = useState(0);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  // Language selection — applied to the default quality profile on
  // advance from step 2. At least one language must be picked so the
  // hard-reject filter has something to accept.
  const [languages, setLanguages] = useState<string[]>(['en']);

  // Config
  const [config, setConfig] = useState({
    media_library_path: '/media/library',
    download_path: '/media/downloads',
    tmdb_api_key: '',
  });
  const [tmdbTest, setTmdbTest] = useState<TestState>('idle');

  // Path browser modal target — `'media'` / `'download'` opens the
  // server-side directory picker against the matching path. `null`
  // means closed. Mirrors LibrarySettings' pattern exactly so the
  // wizard's Storage step gives users the same point-and-click
  // experience as Settings.
  const [browserFor, setBrowserFor] = useState<'media' | 'download' | null>(null);

  // Indexer state
  interface AddedIndexer {
    id: number;
    name: string;
    indexer_type: string;
    definition_id?: string;
  }
  const [addedIndexers, setAddedIndexers] = useState<AddedIndexer[]>([]);
  const indexerCount = addedIndexers.length;
  const [indexerStep, setIndexerStep] = useState<'choose' | 'browse' | 'configure' | 'torznab'>(
    'choose'
  );
  const [searchText, setSearchText] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const [typeFilter, setTypeFilter] = useState('all');
  const [definitions, setDefinitions] = useState<IndexerDefinition[]>([]);
  const [defsLoading, setDefsLoading] = useState(false);
  const [selectedDef, setSelectedDef] = useState<DefinitionDetail | null>(null);
  const [settingsValues, setSettingsValues] = useState<Record<string, string>>({});
  const [torznabForm, setTorznabForm] = useState({ name: '', url: '', api_key: '' });
  const [indexerSaveError, setIndexerSaveError] = useState('');
  const [indexerSaving, setIndexerSaving] = useState(false);
  const [editingIndexerId, setEditingIndexerId] = useState<number | null>(null);

  const { data: catalogueDefs } = useQuery({
    queryKey: ['kino', 'indexer-definitions', 'count'],
    queryFn: async () => {
      const res = await fetch('/api/v1/indexer-definitions', { credentials: 'include' });
      if (!res.ok) return [] as unknown[];
      return (await res.json()) as unknown[];
    },
    meta: { invalidatedBy: ['indexer_definitions_refresh_completed'] },
  });
  const catalogueCount = catalogueDefs?.length ?? 0;

  const update = (key: string, value: string) => {
    setConfig((prev) => ({ ...prev, [key]: value }));
    setError('');
  };

  // Load existing config + indexer count on mount
  useEffect(() => {
    (async () => {
      try {
        // Pre-fill config from backend (env vars may have set some values)
        const configData = await apiFetch<Record<string, unknown>>('/api/v1/config');
        setConfig((prev) => ({
          media_library_path: String(configData.media_library_path ?? prev.media_library_path),
          download_path: String(configData.download_path ?? prev.download_path),
          tmdb_api_key: String(configData.tmdb_api_key ?? prev.tmdb_api_key),
        }));
        // If key is pre-filled, mark as idle — user can click Test to verify
        // (TMDB client may not be initialized if backend started before DB existed)
      } catch {
        // ignore — first run may not have config yet
      }
      try {
        const { data: idxData } = await listIndexers();
        const idxList = idxData as AddedIndexer[] | undefined;
        if (idxList) setAddedIndexers(idxList);
      } catch {
        // ignore
      }
    })();
  }, []);

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(searchText), 300);
    return () => clearTimeout(timer);
  }, [searchText]);

  // Fetch definitions when browsing
  useEffect(() => {
    if (indexerStep !== 'browse') return;
    setDefsLoading(true);
    const params = new URLSearchParams();
    if (debouncedSearch) params.set('search', debouncedSearch);
    if (typeFilter !== 'all') params.set('type', typeFilter);
    const qs = params.toString();
    apiFetch<IndexerDefinition[]>(`/api/v1/indexer-definitions${qs ? `?${qs}` : ''}`)
      .then(setDefinitions)
      .catch(() => setDefinitions([]))
      .finally(() => setDefsLoading(false));
  }, [indexerStep, debouncedSearch, typeFilter]);

  // TMDB test — saves the key, then hits the dedicated test
  // endpoint that constructs a fresh TmdbClient from the just-saved
  // DB value. Earlier versions called trendingMovies() which depends
  // on the at-boot-time cached state.tmdb client; on a fresh install
  // that client is None until the next restart, so the test would
  // always fail on the first save.
  const testTmdbKey = useCallback(async (key: string) => {
    if (!key.trim()) return;
    setTmdbTest('testing');
    try {
      await updateConfig({ body: { tmdb_api_key: key } });
      const { data } = await testTmdb();
      setTmdbTest(data?.ok ? 'pass' : 'fail');
    } catch {
      setTmdbTest('fail');
    }
  }, []);

  // Select a definition to configure (new or edit existing)
  const selectDefinition = async (def: IndexerDefinition, existingIndexer?: AddedIndexer) => {
    setIndexerSaveError('');
    setEditingIndexerId(existingIndexer?.id ?? null);
    try {
      const detail = await apiFetch<DefinitionDetail>(`/api/v1/indexer-definitions/${def.id}`);
      setSelectedDef(detail);

      // Pre-fill from existing settings_json if editing, otherwise use defaults
      let values: Record<string, string> = {};
      if (existingIndexer) {
        try {
          const full = await apiFetch<{ settings_json?: string }>(
            `/api/v1/indexers/${existingIndexer.id}`
          );
          if (full.settings_json) values = JSON.parse(full.settings_json);
        } catch {
          /* use defaults */
        }
      }
      // Fill any missing fields with defaults
      for (const s of detail.settings) {
        if (!(s.name in values)) {
          values[s.name] = s.default ?? (s.type === 'checkbox' ? 'false' : '');
        }
      }
      setSettingsValues(values);
      setIndexerStep('configure');
    } catch {
      setIndexerSaveError('Failed to load definition details');
    }
  };

  // Save a Cardigann indexer
  const saveCardigann = async () => {
    if (!selectedDef) return;
    setIndexerSaving(true);
    setIndexerSaveError('');
    try {
      if (editingIndexerId) {
        // Update existing
        const updated = await apiFetch<AddedIndexer>(`/api/v1/indexers/${editingIndexerId}`, {
          method: 'PUT',
          body: JSON.stringify({ settings_json: JSON.stringify(settingsValues) }),
        });
        setAddedIndexers((prev) => prev.map((i) => (i.id === editingIndexerId ? updated : i)));
      } else {
        // Create new
        const created = await apiFetch<AddedIndexer>('/api/v1/indexers', {
          method: 'POST',
          body: JSON.stringify({
            name: selectedDef.name,
            url: selectedDef.links[0] || '',
            indexer_type: 'cardigann',
            definition_id: selectedDef.id,
            settings_json: JSON.stringify(settingsValues),
            priority: 25,
            enabled: true,
          }),
        });
        setAddedIndexers((prev) => [...prev, created]);
      }
      setEditingIndexerId(null);
      setIndexerStep('choose');
      setSelectedDef(null);
    } catch (e) {
      setIndexerSaveError(e instanceof Error ? e.message : 'Failed to save');
    } finally {
      setIndexerSaving(false);
    }
  };

  // Save a Torznab indexer
  const saveTorznab = async () => {
    if (!torznabForm.name || !torznabForm.url) return;
    setIndexerSaving(true);
    setIndexerSaveError('');
    try {
      const created = await apiFetch<AddedIndexer>('/api/v1/indexers', {
        method: 'POST',
        body: JSON.stringify({
          name: torznabForm.name,
          url: torznabForm.url,
          api_key: torznabForm.api_key || undefined,
          indexer_type: 'torznab',
          priority: 25,
          enabled: true,
        }),
      });
      setAddedIndexers((prev) => [...prev, created]);
      setIndexerStep('choose');
      setTorznabForm({ name: '', url: '', api_key: '' });
    } catch (e) {
      setIndexerSaveError(e instanceof Error ? e.message : 'Failed to save');
    } finally {
      setIndexerSaving(false);
    }
  };

  const canAdvance = () => {
    switch (step) {
      case 0:
        return config.media_library_path.trim() !== '' && config.download_path.trim() !== '';
      case 1:
        // TMDB is technically optional — backend just disables metadata
        // features if absent. Wizard still soft-gates it as recommended.
        return config.tmdb_api_key.trim() !== '';
      case 2:
        return languages.length > 0;
      case 3:
        // Indexers require at least one to do anything useful, but we
        // allow proceeding with zero so users can come back later via
        // Settings → Indexers (the launchpad on step 5 surfaces it).
        return true;
      case 4:
        // Transcode bundle is optional — system ffmpeg works fine
        // for software transcoding on most desktops.
        return true;
      default:
        return true;
    }
  };

  const next = async () => {
    if (step === STEPS.length - 1) {
      onComplete();
      return;
    }

    setSaving(true);
    setError('');
    try {
      if (step === 0) {
        await onSave({
          media_library_path: config.media_library_path,
          download_path: config.download_path,
        });
      } else if (step === 1) {
        await onSave({ tmdb_api_key: config.tmdb_api_key });
      } else if (step === 2) {
        // Write the language picks into the default quality profile.
        // Loads the profile list rather than hard-coding id=1 so the
        // wizard still works if the default seed ever changes.
        const profiles = await listQualityProfiles();
        const defaultProfile = profiles.data?.find((p) => p.is_default) ?? profiles.data?.[0];
        if (defaultProfile) {
          await updateQualityProfile({
            path: { id: defaultProfile.id },
            body: { accepted_languages: JSON.stringify(languages) },
          });
        }
      }
      setStep((s) => s + 1);
    } catch {
      setError('Failed to save');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex bg-[var(--bg-primary)]">
      <aside className="hidden w-64 shrink-0 flex-col border-r border-white/5 bg-[var(--bg-secondary)] p-6 md:flex">
        <div className="mb-10 flex items-center gap-2.5">
          <img src="/kino-mark.svg" alt="kino" className="h-7 w-auto" />
          <span className="text-xl font-bold tracking-tight">kino</span>
        </div>
        <ol className="space-y-1">
          {STEPS.map((s, i) => {
            const state: 'done' | 'current' | 'pending' =
              i < step ? 'done' : i === step ? 'current' : 'pending';
            const clickable = i < step;
            return (
              <li key={s.title}>
                <button
                  type="button"
                  onClick={() => clickable && setStep(i)}
                  disabled={!clickable}
                  aria-current={state === 'current' ? 'step' : undefined}
                  className={cn(
                    'flex w-full items-center gap-3 rounded-lg px-2.5 py-2 text-left transition',
                    state === 'current'
                      ? 'bg-[var(--accent)]/10 ring-1 ring-[var(--accent)]/40'
                      : state === 'done'
                        ? 'hover:bg-white/[0.04]'
                        : 'opacity-60'
                  )}
                >
                  <span
                    className={cn(
                      'grid h-6 w-6 shrink-0 place-items-center rounded-full text-[11px] font-semibold transition-colors',
                      state === 'done'
                        ? 'bg-[var(--accent)] text-white'
                        : state === 'current'
                          ? 'bg-[var(--accent)] text-white ring-2 ring-[var(--accent)]/30'
                          : 'bg-white/5 text-[var(--text-muted)]'
                    )}
                  >
                    {state === 'done' ? <Check size={12} /> : i + 1}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span
                      className={cn(
                        'block truncate text-sm font-medium',
                        state === 'pending' ? 'text-[var(--text-muted)]' : 'text-white'
                      )}
                    >
                      {s.title}
                    </span>
                    <span className="block truncate text-[11px] text-[var(--text-muted)]">
                      {s.description}
                    </span>
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
        <div className="mt-auto pt-6 text-[11px] text-[var(--text-muted)]/70">
          Setup ~ 2 minutes. Everything is editable later in Settings.
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="md:hidden flex items-center gap-2 border-b border-white/5 px-4 py-3">
          <span className="grid h-6 w-6 place-items-center rounded-full bg-[var(--accent)] text-[11px] font-semibold text-white">
            {step + 1}
          </span>
          <span className="text-sm font-medium text-white">{STEPS[step].title}</span>
          <span className="ml-auto text-[11px] text-[var(--text-muted)]">
            {step + 1} of {STEPS.length}
          </span>
        </div>

        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
          <header className="border-b border-white/5 px-6 pt-8 pb-5 md:px-12 md:pt-12">
            <p className="mb-2 flex items-center gap-2 text-xs font-medium uppercase tracking-wider text-[var(--text-muted)]">
              <span>Setup</span>
              <span className="opacity-40">·</span>
              <span>
                {step + 1} of {STEPS.length}
              </span>
            </p>
            <h1 className="text-2xl font-semibold tracking-tight">{STEPS[step].title}</h1>
            <p className="mt-1 text-sm text-[var(--text-muted)]">{STEPS[step].description}</p>
          </header>

          <div className="min-h-0 flex-1 overflow-y-auto px-6 py-6 md:px-12 md:py-8">
            <div className="mx-auto max-w-2xl">
              {/* Step 0: Storage */}
              {step === 0 && (
                <div className="space-y-1">
                  <FormField label="Media Library" description="Where organized files are stored">
                    <div className="flex gap-2">
                      <div className="flex-1">
                        <TextInput
                          value={config.media_library_path}
                          onChange={(v) => update('media_library_path', v)}
                          placeholder="/media/library"
                        />
                      </div>
                      <BrowseButton
                        onClick={() => setBrowserFor('media')}
                        label="Browse server folders for media library"
                      />
                    </div>
                  </FormField>
                  <FormField label="Download Path" description="Where torrents download to">
                    <div className="flex gap-2">
                      <div className="flex-1">
                        <TextInput
                          value={config.download_path}
                          onChange={(v) => update('download_path', v)}
                          placeholder="/media/downloads"
                        />
                      </div>
                      <BrowseButton
                        onClick={() => setBrowserFor('download')}
                        label="Browse server folders for download path"
                      />
                    </div>
                  </FormField>
                  <PathBrowser
                    open={browserFor !== null}
                    onOpenChange={(o) => !o && setBrowserFor(null)}
                    title={browserFor === 'media' ? 'Select Media Library' : 'Select Download Path'}
                    startPath={
                      browserFor === 'media'
                        ? config.media_library_path || '/'
                        : config.download_path || '/'
                    }
                    onSelect={(picked) => {
                      if (browserFor === 'media') update('media_library_path', picked);
                      else if (browserFor === 'download') update('download_path', picked);
                    }}
                  />
                </div>
              )}

              {/* Step 1: TMDB */}
              {step === 1 && (
                <div className="space-y-1">
                  <FormField
                    label="API Read Access Token"
                    description="Required for content metadata"
                  >
                    <div className="flex gap-2">
                      <div className="flex-1">
                        <SecretInput
                          value={config.tmdb_api_key}
                          onChange={(v) => update('tmdb_api_key', v)}
                          placeholder="eyJ..."
                          // Backend redacts already-set secrets to "***"
                          // on /api/v1/config GETs (server never lets the
                          // SPA pull plaintext credentials back). `masked`
                          // tells SecretInput to render empty + a dotted
                          // placeholder when it sees the redaction
                          // sentinel, so the user sees "••••••••" + Test
                          // works against the real stored key. Typing
                          // anything replaces the sentinel and the new
                          // value is what gets submitted on Next.
                          masked
                        />
                      </div>
                      <button
                        type="button"
                        onClick={() => testTmdbKey(config.tmdb_api_key)}
                        disabled={!config.tmdb_api_key.trim() || tmdbTest === 'testing'}
                        className="px-3 py-2 rounded-lg bg-white/10 hover:bg-white/15 text-sm font-medium disabled:opacity-50 transition flex-shrink-0"
                      >
                        {tmdbTest === 'testing' ? (
                          <Loader2 size={16} className="animate-spin" />
                        ) : (
                          'Test'
                        )}
                      </button>
                    </div>
                  </FormField>
                  {tmdbTest === 'pass' && (
                    <p className="flex items-center gap-2 text-sm text-green-400">
                      <CheckCircle size={16} />
                      TMDB connection verified
                    </p>
                  )}
                  {tmdbTest === 'fail' && (
                    <p className="flex items-center gap-2 text-sm text-red-400">
                      <X size={16} />
                      Test failed — check the key
                    </p>
                  )}
                  <p className="mt-2 text-xs text-[var(--text-muted)]">
                    Use the long <strong className="font-mono text-white">eyJ…</strong> JWT labelled
                    "API Read Access Token" — not the short hex string labelled "API Key (v3 auth)".
                    They're on the same page; only the JWT works.
                  </p>
                  <a
                    href="https://www.themoviedb.org/settings/api"
                    target="_blank"
                    rel="noopener noreferrer"
                    className="mt-1 inline-flex items-center gap-1.5 text-xs text-[var(--accent)] hover:underline"
                  >
                    <ExternalLink size={12} />
                    Get a free key from TMDB
                  </a>
                </div>
              )}

              {/* Step 2: Languages.
              Two distinct controls so priority isn't a hidden
              ordering rule on a flat chip list. The preferred
              language has a single select (the one decision); the
              accept-list is a chip multi-select where order is
              irrelevant. The submitted array always starts with
              `preferred`, then the rest, so the backend's
              accepted_languages[0]-is-preferred contract is
              honoured without exposing the indexing to the user. */}
              {step === 2 && (
                <div className="space-y-6">
                  <div>
                    <label
                      htmlFor="wizard-preferred-language"
                      className="mb-1 block text-sm font-medium text-white"
                    >
                      Preferred language
                    </label>
                    <p className="mb-2 text-xs text-[var(--text-muted)]">
                      Used to break ties between releases in different languages, and as the assumed
                      language for untagged releases (scene convention).
                    </p>
                    <select
                      id="wizard-preferred-language"
                      value={languages[0] ?? 'en'}
                      onChange={(e) => {
                        const next = e.target.value;
                        setLanguages([next, ...languages.slice(1).filter((c) => c !== next)]);
                      }}
                      className="h-9 w-full rounded-lg border border-white/10 bg-[var(--bg-card)] px-3 text-sm text-white focus:border-[var(--accent)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                    >
                      {WIZARD_LANGUAGES.map((l) => (
                        <option key={l.code} value={l.code}>
                          {l.label}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div>
                    <p className="mb-1 text-sm font-medium text-white">Also acceptable</p>
                    <p className="mb-3 text-xs text-[var(--text-muted)]">
                      Releases in any of these languages will be kept too. Pick none if you only
                      want your preferred language.
                    </p>
                    <div className="flex flex-wrap gap-1.5">
                      {WIZARD_LANGUAGES.filter((l) => l.code !== (languages[0] ?? 'en')).map(
                        (l) => {
                          const on = languages.slice(1).includes(l.code);
                          return (
                            <button
                              key={l.code}
                              type="button"
                              aria-pressed={on}
                              onClick={() => {
                                if (on) {
                                  setLanguages(languages.filter((c) => c !== l.code));
                                } else {
                                  setLanguages([...languages, l.code]);
                                }
                              }}
                              className={cn(
                                'rounded-full px-3 py-1.5 text-xs font-medium transition-colors',
                                on
                                  ? 'bg-[var(--accent)]/15 text-[var(--accent)] ring-1 ring-[var(--accent)]/40'
                                  : 'bg-white/5 text-[var(--text-secondary)] ring-1 ring-white/10 hover:bg-white/10 hover:text-white'
                              )}
                            >
                              {l.label}
                            </button>
                          );
                        }
                      )}
                    </div>
                  </div>

                  <p className="text-xs text-[var(--text-muted)]">
                    You can change all of this any time in Settings → Quality.
                  </p>
                </div>
              )}

              {step === 3 && (
                <div className="min-h-[200px]">
                  <DefinitionsCatalogueTile
                    localCount={catalogueCount}
                    onLoaded={() => setIndexerStep('choose')}
                  />

                  {/* Choose type */}
                  {indexerStep === 'choose' && (
                    <div className="space-y-3">
                      {/* Added indexers list */}
                      {addedIndexers.length > 0 && (
                        <div className="space-y-1.5">
                          {addedIndexers.map((idx) => {
                            // Look up the definition to get the real type (public/private)
                            const defType = idx.definition_id
                              ? (definitions.find((d) => d.id === idx.definition_id)
                                  ?.indexer_type ?? 'public')
                              : idx.indexer_type;
                            const displayType =
                              idx.indexer_type === 'torznab' ? 'Torznab' : defType;

                            return (
                              <div
                                key={idx.id}
                                className="flex items-center justify-between p-2.5 rounded-lg bg-white/[0.03] ring-1 ring-white/5"
                              >
                                <div className="flex items-center gap-2 min-w-0">
                                  <CheckCircle size={14} className="text-green-400 flex-shrink-0" />
                                  <span className="text-sm font-medium truncate">{idx.name}</span>
                                  <TypeBadge type={displayType} />
                                </div>
                                <div className="flex items-center gap-1 flex-shrink-0">
                                  {idx.definition_id && (
                                    <button
                                      type="button"
                                      onClick={() => {
                                        selectDefinition(
                                          {
                                            id: idx.definition_id ?? '',
                                            name: idx.name,
                                            description: '',
                                            indexer_type: displayType,
                                            language: '',
                                          },
                                          idx
                                        );
                                      }}
                                      className="p-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white transition"
                                      title="Edit settings"
                                    >
                                      <Edit2 size={13} />
                                    </button>
                                  )}
                                  <button
                                    type="button"
                                    onClick={async () => {
                                      try {
                                        await apiFetch(`/api/v1/indexers/${idx.id}`, {
                                          method: 'DELETE',
                                        });
                                        setAddedIndexers((prev) =>
                                          prev.filter((i) => i.id !== idx.id)
                                        );
                                      } catch {
                                        // ignore
                                      }
                                    }}
                                    className="p-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-red-400 transition"
                                    title="Remove"
                                  >
                                    <X size={14} />
                                  </button>
                                </div>
                              </div>
                            );
                          })}
                        </div>
                      )}

                      {catalogueCount > 0 && (
                        <button
                          type="button"
                          onClick={() => setIndexerStep('browse')}
                          className="w-full p-4 rounded-xl bg-white/[0.03] ring-1 ring-white/10 hover:ring-[var(--accent)]/50 hover:bg-white/[0.05] transition text-left"
                        >
                          <div className="flex items-center gap-3">
                            <div className="w-10 h-10 rounded-lg bg-[var(--accent)]/10 grid place-items-center">
                              <Database size={20} className="text-[var(--accent)]" />
                            </div>
                            <div>
                              <p className="text-sm font-semibold text-white">Browse catalogue</p>
                              <p className="text-xs text-[var(--text-muted)]">
                                {catalogueCount} indexer definitions to choose from
                              </p>
                            </div>
                          </div>
                        </button>
                      )}
                      <button
                        type="button"
                        onClick={() => setIndexerStep('torznab')}
                        className="w-full p-4 rounded-xl bg-white/[0.03] ring-1 ring-white/10 hover:ring-white/20 hover:bg-white/[0.05] transition text-left"
                      >
                        <div className="flex items-center gap-3">
                          <div className="w-10 h-10 rounded-lg bg-white/5 grid place-items-center">
                            <Globe size={20} className="text-[var(--text-secondary)]" />
                          </div>
                          <div>
                            <p className="text-sm font-semibold text-white">Torznab / Newznab</p>
                            <p className="text-xs text-[var(--text-muted)]">
                              Manual URL entry (Prowlarr, Jackett, etc.)
                            </p>
                          </div>
                        </div>
                      </button>
                    </div>
                  )}

                  {/* Browse definitions */}
                  {indexerStep === 'browse' && (
                    <div className="space-y-3">
                      <button
                        type="button"
                        onClick={() => setIndexerStep('choose')}
                        className="flex items-center gap-1 text-xs text-[var(--text-muted)] hover:text-white transition"
                      >
                        <ArrowLeft size={12} />
                        Back
                      </button>
                      <div className="relative">
                        <Search
                          size={16}
                          className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
                        />
                        <input
                          type="text"
                          value={searchText}
                          onChange={(e) => setSearchText(e.target.value)}
                          placeholder="Search indexers..."
                          className="w-full h-9 pl-9 pr-3 rounded-lg bg-white/5 border border-white/10 text-sm placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                        />
                      </div>
                      <div className="flex gap-1.5">
                        {['all', 'public', 'semi-private', 'private'].map((t) => (
                          <button
                            key={t}
                            type="button"
                            onClick={() => setTypeFilter(t)}
                            className={cn(
                              'px-2.5 py-1 rounded-lg text-xs font-medium transition',
                              typeFilter === t
                                ? 'bg-[var(--accent)] text-white'
                                : 'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10'
                            )}
                          >
                            {t === 'all' ? 'All' : t.charAt(0).toUpperCase() + t.slice(1)}
                          </button>
                        ))}
                      </div>
                      <div className="max-h-[280px] overflow-y-auto space-y-1.5">
                        {defsLoading && (
                          <div className="flex items-center justify-center py-8">
                            <Loader2 size={20} className="animate-spin text-[var(--text-muted)]" />
                          </div>
                        )}
                        {!defsLoading && definitions.length === 0 && (
                          <p className="text-center text-sm text-[var(--text-muted)] py-8">
                            No indexers found
                          </p>
                        )}
                        {definitions.map((def) => {
                          const alreadyAdded = addedIndexers.some(
                            (i) => i.definition_id === def.id
                          );
                          return (
                            <button
                              key={def.id}
                              type="button"
                              disabled={alreadyAdded}
                              onClick={() => !alreadyAdded && selectDefinition(def)}
                              className={cn(
                                'w-full p-2.5 rounded-lg ring-1 transition text-left',
                                alreadyAdded
                                  ? 'bg-white/[0.01] ring-white/5 opacity-40 cursor-not-allowed'
                                  : 'bg-white/[0.03] ring-white/5 hover:ring-white/15'
                              )}
                            >
                              <div className="flex items-center justify-between gap-2">
                                <div className="flex items-center gap-2 min-w-0">
                                  {alreadyAdded && (
                                    <CheckCircle
                                      size={12}
                                      className="text-green-400 flex-shrink-0"
                                    />
                                  )}
                                  <span className="text-sm font-medium truncate">{def.name}</span>
                                </div>
                                <TypeBadge type={def.indexer_type} />
                              </div>
                              {def.description && (
                                <p className="text-xs text-[var(--text-muted)] mt-0.5 line-clamp-1">
                                  {def.description}
                                </p>
                              )}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}

                  {/* Configure Cardigann — full area with proper field types */}
                  {indexerStep === 'configure' && selectedDef && (
                    <div className="space-y-4">
                      <button
                        type="button"
                        onClick={() => {
                          setIndexerStep(editingIndexerId ? 'choose' : 'browse');
                          setEditingIndexerId(null);
                        }}
                        className="flex items-center gap-1 text-xs text-[var(--text-muted)] hover:text-white transition"
                      >
                        <ArrowLeft size={12} />
                        {editingIndexerId ? 'Back' : 'Back to browse'}
                      </button>

                      {/* Definition header */}
                      <div className="p-3 rounded-lg bg-white/[0.03] ring-1 ring-white/5">
                        <div className="flex items-center gap-2">
                          <span className="font-semibold">{selectedDef.name}</span>
                          <TypeBadge type={selectedDef.indexer_type} />
                        </div>
                        {selectedDef.description && (
                          <p className="text-xs text-[var(--text-muted)] mt-1">
                            {selectedDef.description}
                          </p>
                        )}
                      </div>

                      {/* Settings fields */}
                      {selectedDef.settings.length === 0 ? (
                        <div className="text-center py-6">
                          <CheckCircle size={24} className="mx-auto mb-2 text-green-400" />
                          <p className="text-sm text-[var(--text-secondary)]">
                            No configuration needed — ready to use.
                          </p>
                        </div>
                      ) : (
                        <div className="space-y-3">
                          {selectedDef.settings.map((s, _i, arr) => {
                            // Check if previous field was also info (for tighter grouping)
                            const prevIsInfo = _i > 0 && arr[_i - 1].type?.startsWith('info');
                            // All info_* types — rendered as compact helper text
                            // They cluster at the end of settings as general notes
                            if (s.type?.startsWith('info')) {
                              const builtinMessages: Record<string, string> = {
                                info_flaresolverr:
                                  'This indexer may use Cloudflare protection. If searches fail, a FlareSolverr proxy may be needed.',
                                info_cookie:
                                  'This indexer requires cookies for authentication. Log in to the site in your browser and copy your cookies.',
                                info_useragent:
                                  'This indexer requires a browser User-Agent header. Copy yours from your browser developer tools.',
                                info_category_8000:
                                  'This indexer uses non-standard categories. Some results may not be categorized correctly.',
                              };

                              let text = '';
                              if (s.type === 'info') {
                                text = (s.default ?? s.label ?? '')
                                  .replace(/<br\s*\/?>/gi, ' ')
                                  .replace(/<[^>]*>/g, '')
                                  .replace(/\s+/g, ' ')
                                  .trim();
                              } else {
                                text = builtinMessages[s.type ?? ''] ?? '';
                              }
                              if (!text) return null;

                              const isBuiltin = s.type !== 'info';
                              return (
                                <div
                                  key={s.name}
                                  className={cn(
                                    'p-2.5 rounded-lg text-[11px] leading-relaxed',
                                    prevIsInfo ? '-mt-1.5' : '', // tighter when consecutive
                                    isBuiltin
                                      ? 'bg-amber-500/5 ring-1 ring-amber-500/10 text-amber-300/80'
                                      : 'bg-white/[0.02] text-[var(--text-muted)]'
                                  )}
                                >
                                  {s.type === 'info' && s.label && s.default && (
                                    <span className="font-medium text-[var(--text-secondary)]">
                                      {s.label}:{' '}
                                    </span>
                                  )}
                                  {text}
                                </div>
                              );
                            }

                            // Checkbox — short label + long description split
                            if (s.type === 'checkbox') {
                              const label = s.label ?? s.name;
                              const dashIdx = label.indexOf(' - ');
                              const title = dashIdx > 0 ? label.slice(0, dashIdx) : label;
                              const desc = dashIdx > 0 ? label.slice(dashIdx + 3) : undefined;
                              return (
                                <div key={s.name} className="py-1">
                                  <label className="flex items-start justify-between gap-3">
                                    <div className="min-w-0">
                                      <span className="text-sm text-white">{title}</span>
                                      {desc && (
                                        <p className="text-[11px] text-[var(--text-muted)] mt-0.5 leading-relaxed">
                                          {desc}
                                        </p>
                                      )}
                                    </div>
                                    <input
                                      type="checkbox"
                                      checked={settingsValues[s.name] === 'true'}
                                      onChange={(e) =>
                                        setSettingsValues((prev) => ({
                                          ...prev,
                                          [s.name]: e.target.checked ? 'true' : 'false',
                                        }))
                                      }
                                      className="w-4 h-4 mt-0.5 rounded accent-[var(--accent)] flex-shrink-0"
                                    />
                                  </label>
                                </div>
                              );
                            }

                            // Select dropdown
                            if (s.type === 'select' && s.options) {
                              return (
                                <FormField key={s.name} label={s.label ?? s.name}>
                                  <select
                                    value={settingsValues[s.name] ?? ''}
                                    onChange={(e) =>
                                      setSettingsValues((prev) => ({
                                        ...prev,
                                        [s.name]: e.target.value,
                                      }))
                                    }
                                    className="w-full h-9 px-3 rounded-lg bg-white/5 border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
                                  >
                                    {Object.entries(s.options).map(([value, label]) => (
                                      <option key={value} value={value}>
                                        {label}
                                      </option>
                                    ))}
                                  </select>
                                </FormField>
                              );
                            }

                            // Text / password
                            return (
                              <FormField key={s.name} label={s.label ?? s.name}>
                                {s.type === 'password' ? (
                                  <SecretInput
                                    value={settingsValues[s.name] ?? ''}
                                    onChange={(v) =>
                                      setSettingsValues((prev) => ({ ...prev, [s.name]: v }))
                                    }
                                  />
                                ) : (
                                  <TextInput
                                    value={settingsValues[s.name] ?? ''}
                                    onChange={(v) =>
                                      setSettingsValues((prev) => ({ ...prev, [s.name]: v }))
                                    }
                                    placeholder={s.default ?? ''}
                                  />
                                )}
                              </FormField>
                            );
                          })}
                        </div>
                      )}

                      {indexerSaveError && (
                        <p className="text-xs text-red-400">{indexerSaveError}</p>
                      )}
                      <button
                        type="button"
                        onClick={saveCardigann}
                        disabled={indexerSaving}
                        className="w-full px-4 py-2.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50 transition"
                      >
                        {indexerSaving ? (
                          <Loader2 size={16} className="animate-spin mx-auto" />
                        ) : editingIndexerId ? (
                          'Save Changes'
                        ) : (
                          'Add Indexer'
                        )}
                      </button>
                    </div>
                  )}

                  {/* Torznab manual */}
                  {indexerStep === 'torznab' && (
                    <div className="space-y-3">
                      <button
                        type="button"
                        onClick={() => setIndexerStep('choose')}
                        className="flex items-center gap-1 text-xs text-[var(--text-muted)] hover:text-white transition"
                      >
                        <ArrowLeft size={12} />
                        Back
                      </button>
                      <div className="space-y-1">
                        <FormField label="Name">
                          <TextInput
                            value={torznabForm.name}
                            onChange={(v) => setTorznabForm((f) => ({ ...f, name: v }))}
                            placeholder="My Indexer"
                          />
                        </FormField>
                        <FormField label="Torznab URL">
                          <TextInput
                            value={torznabForm.url}
                            onChange={(v) => setTorznabForm((f) => ({ ...f, url: v }))}
                            placeholder="http://..."
                            type="url"
                          />
                        </FormField>
                        <FormField label="API Key">
                          <SecretInput
                            value={torznabForm.api_key}
                            onChange={(v) => setTorznabForm((f) => ({ ...f, api_key: v }))}
                          />
                        </FormField>
                      </div>
                      {indexerSaveError && (
                        <p className="text-xs text-red-400">{indexerSaveError}</p>
                      )}
                      <button
                        type="button"
                        onClick={saveTorznab}
                        disabled={indexerSaving || !torznabForm.name || !torznabForm.url}
                        className="w-full px-4 py-2.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50 transition"
                      >
                        {indexerSaving ? (
                          <Loader2 size={16} className="animate-spin mx-auto" />
                        ) : (
                          'Add Indexer'
                        )}
                      </button>
                    </div>
                  )}
                </div>
              )}

              {/* Step 4: Transcode (optional jellyfin-ffmpeg) */}
              {step === 4 && <FfmpegBundleStep />}

              {step === 5 && (
                <Launchpad
                  indexerCount={indexerCount}
                  onPick={(href) => {
                    if (typeof window !== 'undefined') {
                      window.history.replaceState({}, '', href);
                    }
                    onComplete();
                  }}
                />
              )}

              {error && <p className="mt-3 text-sm text-red-400">{error}</p>}
            </div>
          </div>

          <footer className="flex items-center justify-between gap-3 border-t border-white/5 px-6 py-4 md:px-12">
            <button
              type="button"
              onClick={() => setStep((s) => Math.max(0, s - 1))}
              disabled={step === 0 || saving}
              className="flex items-center gap-1 rounded-lg px-3 py-2 text-sm text-[var(--text-muted)] transition hover:text-white disabled:invisible"
            >
              <ArrowLeft size={14} />
              Back
            </button>
            <button
              type="button"
              onClick={next}
              disabled={saving || !canAdvance()}
              className="flex items-center gap-2 rounded-lg bg-[var(--accent)] px-6 py-2.5 text-sm font-semibold text-white transition hover:bg-[var(--accent-hover)] disabled:opacity-50"
            >
              {saving ? (
                <Loader2 size={16} className="animate-spin" />
              ) : step === STEPS.length - 1 ? (
                'Open library'
              ) : (
                <>
                  {(step === 1 && !config.tmdb_api_key.trim()) || (step === 3 && indexerCount === 0)
                    ? 'Skip for now'
                    : 'Next'}
                  <ArrowRight size={16} />
                </>
              )}
            </button>
          </footer>
        </div>
      </div>
    </div>
  );
}

/// Small icon-only "browse server folders" button. Mirrors the
/// LibrarySettings primitive (`routes/settings/LibrarySettings.tsx`)
/// so the visual contract stays identical between the wizard and
/// Settings — same shape, same hover, same icon.
function BrowseButton({ onClick, label }: { onClick: () => void; label: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-label={label}
      className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-white/5 text-[var(--text-muted)] ring-1 ring-white/10 transition hover:bg-white/10 hover:text-white"
    >
      <FolderOpen size={14} />
    </button>
  );
}

function TypeBadge({ type }: { type: string }) {
  const config: Record<string, { icon: typeof Globe; color: string }> = {
    public: { icon: Globe, color: 'text-green-400' },
    private: { icon: Lock, color: 'text-red-400' },
    'semi-private': { icon: Shield, color: 'text-amber-400' },
  };
  const { icon: Icon, color } = config[type.toLowerCase()] ?? config.public;
  return (
    <span className={cn('flex items-center gap-1 text-[10px] font-medium', color)}>
      <Icon size={10} />
      {type}
    </span>
  );
}

// ─── Indexer-definitions catalogue download ──────────────────────────
//
// Renders at the top of the Indexers step. Three states keyed on the
// `/api/v1/indexer-definitions/refresh` snapshot:
//   - `idle` + 0 local defs → "Download catalogue (~30s)" CTA
//   - `running` → progress bar with `fetched / total` count
//   - `completed` / `idle` with defs already loaded → small "X
//     definitions loaded · Refresh" pill
//
// Mirrors the FfmpegDownload modal pattern in PlaybackSettings —
// `useQuery` polls the tracker at 500ms while running, stops
// when terminal. `useMutation` kicks the POST that flips Idle →
// Running. The async download itself never blocks step navigation
// (`canAdvance()` returns true regardless), so a user who chose
// "Skip for now" can advance and come back to Settings later.
const PROWLARR_REPO_URL = 'https://github.com/Prowlarr/Indexers';

function DefinitionsCatalogueTile({
  localCount,
  onLoaded,
}: {
  localCount: number;
  onLoaded: () => void;
}) {
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

  const prevCompletedRef = useRef(completed);
  useEffect(() => {
    if (completed && !prevCompletedRef.current) {
      onLoaded();
      qc.invalidateQueries({ queryKey: ['kino', 'indexer-definitions', 'count'] });
    }
    prevCompletedRef.current = completed;
  }, [completed, onLoaded, qc]);

  if (effectiveCount > 0 && !running) {
    return (
      <div
        className="mb-3 flex items-center justify-between rounded-lg bg-white/[0.02] px-3 py-2 text-xs ring-1 ring-white/5"
        aria-live="polite"
      >
        <span className="flex items-center gap-2 text-[var(--text-muted)]">
          <CheckCircle size={12} className="text-green-400" />
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

  if (running && state?.status === 'running') {
    const pct =
      state.total > 0 ? Math.min(100, Math.round((state.fetched * 100) / state.total)) : 0;
    return (
      <div
        className="mb-4 rounded-xl bg-white/[0.03] p-4 ring-1 ring-[var(--accent)]/30"
        aria-live="polite"
      >
        <div className="mb-2 flex items-center gap-2">
          <Loader2 size={14} className="animate-spin text-[var(--accent)]" />
          <span className="text-sm font-semibold text-white">Fetching indexer catalogue…</span>
        </div>
        <p className="mb-3 text-xs text-[var(--text-muted)]">
          {state.fetched} of {state.total} definitions retrieved. You can keep configuring while
          this runs.
        </p>
        <div
          className="h-1.5 w-full overflow-hidden rounded-full bg-white/10"
          role="progressbar"
          aria-valuenow={pct}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label="Indexer definitions download progress"
        >
          <div className="h-full bg-[var(--accent)] transition-all" style={{ width: `${pct}%` }} />
        </div>
      </div>
    );
  }

  if (failed && state?.status === 'failed') {
    return (
      <div className="mb-4 rounded-xl bg-red-500/5 p-4 ring-1 ring-red-500/30">
        <p className="text-sm font-semibold text-red-300">Catalogue fetch failed</p>
        <p className="mt-1 text-xs text-red-300/70">{state.reason}</p>
        <button
          type="button"
          onClick={() => startMutation.mutate()}
          disabled={startMutation.isPending}
          className="mt-3 inline-flex items-center gap-1 rounded-md bg-red-500/15 px-3 py-1.5 text-xs font-semibold text-red-200 transition hover:bg-red-500/25 disabled:opacity-50"
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
            Skip this if you'd rather plug in indexer URLs by hand.
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

// ─── Step 4: Optional jellyfin-ffmpeg bundle ─────────────────────────
//
// Lightweight in-wizard surface for the same download flow that
// PlaybackSettings exposes. Skip-with-warning is acceptable — system
// ffmpeg works fine for software transcoding; the bundle is mainly
// for HW-accelerated transcoding (NVENC / VAAPI / VideoToolbox)
// where the system ffmpeg often lacks the required encoders.
function FfmpegBundleStep() {
  const qc = useQueryClient();
  const { data: state } = useQuery({
    queryKey: ['kino', 'playback', 'ffmpeg', 'download'],
    queryFn: async () => {
      const { data } = await getFfmpegDownload();
      return data ?? ({ status: 'idle' } satisfies FfmpegDownloadState);
    },
    refetchInterval: (q) => (q.state.data?.status === 'running' ? 500 : false),
    meta: {
      invalidatedBy: ['ffmpeg_download_completed', 'ffmpeg_download_failed'],
    },
  });

  const startMutation = useMutation({
    mutationFn: async () => {
      await startFfmpegDownload();
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'playback', 'ffmpeg', 'download'] });
    },
  });

  const cancelMutation = useMutation({
    mutationFn: async () => {
      await cancelFfmpegDownload();
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'playback', 'ffmpeg', 'download'] });
    },
  });

  const status = state?.status ?? 'idle';
  const running = status === 'running';
  const completed = status === 'completed';
  const failed = status === 'failed';

  return (
    <div className="space-y-3">
      <div className="rounded-xl bg-white/[0.02] p-4 ring-1 ring-white/5">
        <div className="flex items-start gap-3">
          <div className="grid h-10 w-10 flex-shrink-0 place-items-center rounded-lg bg-white/5">
            <Cpu size={20} className="text-[var(--text-secondary)]" />
          </div>
          <div className="min-w-0 flex-1">
            <p className="text-sm font-semibold text-white">jellyfin-ffmpeg</p>
            <p className="mt-0.5 text-xs text-[var(--text-muted)]">
              A bundled ffmpeg build with hardware acceleration support (NVENC, VAAPI, VideoToolbox)
              baked in. The system ffmpeg on your distro often lacks these encoders; installing the
              bundle gives Kino fast HW-accelerated transcoding without touching your system ffmpeg.
              Skip if you only stream to clients that play original files.
            </p>
            {!running && !completed && !failed && (
              <button
                type="button"
                onClick={() => startMutation.mutate()}
                disabled={startMutation.isPending}
                className="mt-3 inline-flex items-center gap-2 rounded-md bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-white transition hover:bg-[var(--accent-hover)] disabled:opacity-50"
              >
                <Download size={12} />
                Download bundled ffmpeg (~60 MB)
              </button>
            )}
          </div>
        </div>
      </div>

      {running && state?.status === 'running' && (
        <div
          className="rounded-xl bg-white/[0.03] p-4 ring-1 ring-[var(--accent)]/30"
          aria-live="polite"
        >
          <div className="mb-2 flex items-center justify-between gap-2">
            <span className="flex items-center gap-2 text-sm font-semibold text-white">
              <Loader2 size={14} className="animate-spin text-[var(--accent)]" />
              Downloading jellyfin-ffmpeg {state.version}
            </span>
            <button
              type="button"
              onClick={() => cancelMutation.mutate()}
              disabled={cancelMutation.isPending}
              className="text-xs text-[var(--text-muted)] hover:text-red-400"
            >
              Cancel
            </button>
          </div>
          <p className="mb-3 text-xs text-[var(--text-muted)]">
            {(state.bytes / 1024 / 1024).toFixed(1)} of {(state.total / 1024 / 1024).toFixed(1)} MB
          </p>
          <div
            className="h-1.5 w-full overflow-hidden rounded-full bg-white/10"
            role="progressbar"
            aria-valuenow={state.total > 0 ? Math.round((state.bytes * 100) / state.total) : 0}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <div
              className="h-full bg-[var(--accent)] transition-all"
              style={{
                width: `${state.total > 0 ? Math.min(100, (state.bytes * 100) / state.total) : 0}%`,
              }}
            />
          </div>
        </div>
      )}

      {completed && (
        <div className="rounded-xl bg-green-500/5 p-4 ring-1 ring-green-500/20" aria-live="polite">
          <p className="flex items-center gap-2 text-sm font-semibold text-green-300">
            <CheckCircle size={14} />
            jellyfin-ffmpeg ready
          </p>
          <p className="mt-1 text-xs text-[var(--text-muted)]">
            Kino will use the bundled binary for transcoding. You can revert to system ffmpeg later
            in Settings → Playback.
          </p>
        </div>
      )}

      {failed && state?.status === 'failed' && (
        <div className="rounded-xl bg-red-500/5 p-4 ring-1 ring-red-500/30">
          <p className="text-sm font-semibold text-red-300">Download failed</p>
          <p className="mt-1 text-xs text-red-300/70">{state.reason}</p>
          <button
            type="button"
            onClick={() => startMutation.mutate()}
            disabled={startMutation.isPending}
            className="mt-3 inline-flex items-center gap-1 rounded-md bg-red-500/15 px-3 py-1.5 text-xs font-semibold text-red-200 transition hover:bg-red-500/25"
          >
            <RefreshCw size={11} />
            Retry
          </button>
        </div>
      )}
    </div>
  );
}

// ─── Step 5: Launchpad ───────────────────────────────────────────────
//
// Replaces the static "You're all set!" trophy with a checklist of
// concrete next actions. Each row is a real link into the app (or
// Settings) so the user finishes the wizard with momentum.
function Launchpad({
  indexerCount,
  onPick,
}: {
  indexerCount: number;
  onPick: (href: string) => void;
}) {
  return (
    <div className="space-y-4">
      <div className="text-center">
        <div className="mx-auto mb-3 grid h-14 w-14 place-items-center rounded-full bg-[var(--accent)]/10">
          <Sparkles size={28} className="text-[var(--accent)]" />
        </div>
        <p className="text-lg font-semibold text-white">Setup complete</p>
        <p className="mt-1 text-sm text-[var(--text-muted)]">
          Pick a starting point — or click "Open library" to dive in.
        </p>
      </div>

      <div className="space-y-2">
        <LaunchpadRow
          href="/movies"
          label="Browse the library"
          hint="Films + shows you've added will appear here."
          onPick={onPick}
        />
        <LaunchpadRow
          href="/discover"
          label="Discover trending content"
          hint="TMDB-powered recommendations, search, and lists."
          onPick={onPick}
        />
        {indexerCount === 0 && (
          <LaunchpadRow
            href="/settings/indexers"
            label="Add an indexer"
            hint="You skipped this step — add one to start downloading."
            highlight
            onPick={onPick}
          />
        )}
        <LaunchpadRow
          href="/settings/quality"
          label="Tune your quality profile"
          hint="Defaults are 1080p preferred, 720p acceptable. Adjust here."
          onPick={onPick}
        />
        <LaunchpadRow
          href="/settings/integrations"
          label="Connect Trakt or webhooks"
          hint="Optional. Trakt syncs watched/watchlist, webhooks notify Discord/Slack."
          onPick={onPick}
        />
      </div>
    </div>
  );
}

function LaunchpadRow({
  href,
  label,
  hint,
  highlight = false,
  onPick,
}: {
  href: string;
  label: string;
  hint: string;
  highlight?: boolean;
  onPick: (href: string) => void;
}) {
  return (
    <button
      type="button"
      onClick={() => onPick(href)}
      className={cn(
        'flex w-full items-start justify-between gap-3 rounded-lg p-3 text-left ring-1 transition',
        highlight
          ? 'bg-amber-500/5 ring-amber-500/30 hover:bg-amber-500/10'
          : 'bg-white/[0.02] ring-white/5 hover:bg-white/[0.04] hover:ring-white/15'
      )}
    >
      <div className="min-w-0">
        <p className="text-sm font-medium text-white">{label}</p>
        <p className="mt-0.5 text-xs text-[var(--text-muted)]">{hint}</p>
      </div>
      <ArrowRight size={14} className="mt-1 flex-shrink-0 text-[var(--text-muted)]" />
    </button>
  );
}
