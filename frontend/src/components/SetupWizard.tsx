import {
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle,
  Database,
  Edit2,
  ExternalLink,
  Globe,
  Loader2,
  Lock,
  Search,
  Shield,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import {
  listIndexers,
  listQualityProfiles,
  trendingMovies,
  updateConfig,
  updateQualityProfile,
} from '@/api/generated/sdk.gen';
import { FormField, SecretInput, TextInput } from '@/components/settings/FormField';
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
  { title: 'Ready', description: "You're all set" },
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

  // TMDB test
  const testTmdbKey = useCallback(async (key: string) => {
    if (!key.trim()) return;
    setTmdbTest('testing');
    try {
      await updateConfig({ body: { tmdb_api_key: key } });
      const { data } = await trendingMovies();
      setTmdbTest(data?.results?.length ? 'pass' : 'fail');
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
        return config.tmdb_api_key.trim() !== '';
      case 2:
        return languages.length > 0;
      case 3:
        return indexerCount > 0;
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
    <div className="fixed inset-0 z-50 bg-[var(--bg-primary)] flex items-center justify-center p-4">
      <div className="w-full max-w-lg">
        {/* Logo */}
        <div className="flex items-center gap-2.5 mb-8">
          <img src="/kino-mark.svg" alt="kino" className="h-8 w-auto" />
          <span className="text-2xl font-bold tracking-tight">kino</span>
        </div>

        {/* Progress */}
        <div className="flex items-center gap-2 mb-8">
          {STEPS.map((s, i) => (
            <div key={s.title} className="flex items-center gap-2">
              <div
                className={cn(
                  'w-7 h-7 rounded-full grid place-items-center text-xs font-semibold transition-colors',
                  i < step
                    ? 'bg-[var(--accent)] text-white'
                    : i === step
                      ? 'bg-white/10 text-white ring-2 ring-[var(--accent)]'
                      : 'bg-white/5 text-[var(--text-muted)]'
                )}
              >
                {i < step ? <Check size={14} /> : i + 1}
              </div>
              {i < STEPS.length - 1 && (
                <div
                  className={cn(
                    'w-8 h-0.5 rounded',
                    i < step ? 'bg-[var(--accent)]' : 'bg-white/10'
                  )}
                />
              )}
            </div>
          ))}
        </div>

        {/* Step content */}
        <div className="mb-8">
          <h1 className="text-xl font-bold mb-1">{STEPS[step].title}</h1>
          <p className="text-sm text-[var(--text-muted)] mb-6">{STEPS[step].description}</p>

          {/* Step 0: Storage */}
          {step === 0 && (
            <div className="space-y-1">
              <FormField label="Media Library" description="Where organized files are stored">
                <TextInput
                  value={config.media_library_path}
                  onChange={(v) => update('media_library_path', v)}
                  placeholder="/media/library"
                />
              </FormField>
              <FormField label="Download Path" description="Where torrents download to">
                <TextInput
                  value={config.download_path}
                  onChange={(v) => update('download_path', v)}
                  placeholder="/media/downloads"
                />
              </FormField>
            </div>
          )}

          {/* Step 1: TMDB */}
          {step === 1 && (
            <div className="space-y-1">
              <FormField label="API Read Access Token" description="Required for content metadata">
                <div className="flex gap-2">
                  <div className="flex-1">
                    <SecretInput
                      value={config.tmdb_api_key}
                      onChange={(v) => update('tmdb_api_key', v)}
                      placeholder="eyJ..."
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
              <a
                href="https://www.themoviedb.org/settings/api"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 text-xs text-[var(--accent)] hover:underline"
              >
                <ExternalLink size={12} />
                Get a free API key from TMDB
              </a>
            </div>
          )}

          {/* Step 2: Languages */}
          {step === 2 && (
            <div className="min-h-[200px]">
              <p className="text-sm text-[var(--text-muted)] mb-4">
                Kino only grabs releases in the languages you pick. Pick as many as you want — the
                first one is your preferred language. Untagged releases are treated as your first
                pick (scene convention). You can change this any time in Settings → Quality.
              </p>
              <div className="flex flex-wrap gap-1.5">
                {WIZARD_LANGUAGES.map((l) => {
                  const on = languages.includes(l.code);
                  return (
                    <button
                      key={l.code}
                      type="button"
                      onClick={() => {
                        if (on) {
                          setLanguages(languages.filter((c) => c !== l.code));
                        } else {
                          setLanguages([...languages, l.code]);
                        }
                      }}
                      className={cn(
                        'px-3 py-1.5 rounded-full text-sm font-medium transition-colors',
                        on
                          ? 'bg-[var(--accent)] text-white'
                          : 'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white ring-1 ring-white/10'
                      )}
                    >
                      {l.label}
                    </button>
                  );
                })}
              </div>
              {languages.length === 0 && (
                <p className="text-xs text-amber-400 mt-3">
                  Pick at least one language to continue.
                </p>
              )}
            </div>
          )}

          {/* Step 3: Indexers */}
          {step === 3 && (
            <div className="min-h-[200px]">
              {/* Choose type */}
              {indexerStep === 'choose' && (
                <div className="space-y-3">
                  {/* Added indexers list */}
                  {addedIndexers.length > 0 && (
                    <div className="space-y-1.5">
                      {addedIndexers.map((idx) => {
                        // Look up the definition to get the real type (public/private)
                        const defType = idx.definition_id
                          ? (definitions.find((d) => d.id === idx.definition_id)?.indexer_type ??
                            'public')
                          : idx.indexer_type;
                        const displayType = idx.indexer_type === 'torznab' ? 'Torznab' : defType;

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
                                    setAddedIndexers((prev) => prev.filter((i) => i.id !== idx.id));
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
                        <p className="text-sm font-semibold text-white">Browse Indexers</p>
                        <p className="text-xs text-[var(--text-muted)]">
                          Choose from 500+ supported sites
                        </p>
                      </div>
                    </div>
                  </button>
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
                      const alreadyAdded = addedIndexers.some((i) => i.definition_id === def.id);
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
                                <CheckCircle size={12} className="text-green-400 flex-shrink-0" />
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

                  {indexerSaveError && <p className="text-xs text-red-400">{indexerSaveError}</p>}
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
                  {indexerSaveError && <p className="text-xs text-red-400">{indexerSaveError}</p>}
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

          {/* Step 4: Ready */}
          {step === 4 && (
            <div className="text-center py-8">
              <div className="w-16 h-16 rounded-full bg-green-500/10 grid place-items-center mx-auto mb-4">
                <Check size={32} className="text-green-500" />
              </div>
              <p className="text-lg font-semibold">You're all set!</p>
              <p className="text-sm text-[var(--text-muted)] mt-1">
                Start browsing and adding content to your library.
              </p>
            </div>
          )}

          {error && <p className="mt-3 text-sm text-red-400">{error}</p>}
        </div>

        {/* Actions */}
        <div className="flex items-center justify-end">
          <button
            type="button"
            onClick={next}
            disabled={saving || !canAdvance()}
            className="flex items-center gap-2 px-6 py-2.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white font-semibold text-sm disabled:opacity-50 transition"
          >
            {saving ? (
              <Loader2 size={16} className="animate-spin" />
            ) : step === STEPS.length - 1 ? (
              'Get Started'
            ) : (
              <>
                Next
                <ArrowRight size={16} />
              </>
            )}
          </button>
        </div>
      </div>
    </div>
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
