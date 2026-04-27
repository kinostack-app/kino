import { useQuery } from '@tanstack/react-query';
import {
  AlertTriangle,
  Bell,
  ChevronDown,
  ChevronRight,
  Loader2,
  MessageSquare,
  Plus,
  Trash2,
} from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import {
  createWebhook,
  deleteWebhook,
  listWebhooks,
  testWebhook,
  updateWebhook,
} from '@/api/generated/sdk.gen';
import type {
  CreateWebhook,
  UpdateWebhook,
  WebhookTarget as Webhook,
} from '@/api/generated/types.gen';
import {
  FormField,
  SelectInput,
  TestButton,
  TextInput,
  Toggle,
} from '@/components/settings/FormField';
import {
  type BrowserNotifEventKey,
  type BrowserNotifPrefs,
  DEFAULT_PREFS,
  fireTestNotification,
  getPermission,
  isSupported,
  readPrefs,
  requestPermission,
  writePrefs,
} from '@/lib/browser-notifications';
import { WEBHOOKS_KEY } from '@/state/library-cache';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

const EVENT_TOGGLES: { key: keyof Webhook; label: string }[] = [
  { key: 'on_grab', label: 'Grab' },
  { key: 'on_download_complete', label: 'Download' },
  { key: 'on_import', label: 'Import' },
  { key: 'on_upgrade', label: 'Upgrade' },
  { key: 'on_failure', label: 'Failure' },
  { key: 'on_watched', label: 'Watched' },
  { key: 'on_health_issue', label: 'Health' },
];

const METHOD_OPTIONS = [
  { value: 'POST', label: 'POST' },
  { value: 'PUT', label: 'PUT' },
  { value: 'PATCH', label: 'PATCH' },
  { value: 'GET', label: 'GET' },
];

/**
 * Presets that prefill body_template + headers for the three most
 * common webhook targets. Each one assumes the user will paste the
 * correct per-service URL (Discord webhook, Slack incoming webhook,
 * or their ntfy topic URL). Preset names become the default target
 * name too.
 */
const PRESETS: {
  id: string;
  label: string;
  apply: () => Partial<Webhook>;
}[] = [
  {
    id: 'discord',
    label: 'Discord',
    apply: () => ({
      name: 'Discord',
      method: 'POST',
      body_template: '{"content": "**{{event}}** · {{title}} — {{quality}}"}',
      headers: null,
    }),
  },
  {
    id: 'slack',
    label: 'Slack',
    apply: () => ({
      name: 'Slack',
      method: 'POST',
      body_template: '{"text": "*{{event}}* {{title}} — {{quality}}"}',
      headers: null,
    }),
  },
  {
    id: 'ntfy',
    label: 'ntfy',
    apply: () => ({
      name: 'ntfy',
      method: 'POST',
      body_template: '{{event}}: {{title}} — {{quality}}',
      headers: JSON.stringify({ Title: 'kino', Priority: 'default' }),
    }),
  },
];

function blankWebhook(): Partial<Webhook> {
  return {
    name: '',
    url: '',
    method: 'POST',
    headers: null,
    body_template: null,
    enabled: true,
    on_grab: true,
    on_import: true,
    on_download_complete: true,
    on_upgrade: true,
    on_failure: true,
    on_watched: false,
    on_health_issue: true,
  };
}

/**
 * Format an RFC3339 timestamp as a short "Nm ago" / "Nh ago" string.
 * Used on failure-state tooltips so the user sees recency without
 * the UI carrying a date-fns dependency.
 */
function timeAgo(iso: string | null | undefined): string {
  if (!iso) return '';
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return '';
  const secs = Math.max(0, Math.round((Date.now() - then) / 1000));
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.round(hrs / 24)}d ago`;
}

function webhookStatus(wh: Webhook): {
  tone: 'ok' | 'backoff' | 'off';
  label: string;
  tooltip?: string;
} {
  if (!wh.enabled) return { tone: 'off', label: 'Disabled' };
  if (wh.disabled_until && new Date(wh.disabled_until).getTime() > Date.now()) {
    const retryIn = Math.max(
      0,
      Math.round((new Date(wh.disabled_until).getTime() - Date.now()) / 60000)
    );
    return {
      tone: 'backoff',
      label: `Retrying in ${retryIn}m`,
      tooltip: `Last failure ${timeAgo(wh.most_recent_failure_time)} — auto-retrying after backoff`,
    };
  }
  if (wh.most_recent_failure_time) {
    return {
      tone: 'ok',
      label: 'Healthy',
      tooltip: `Last failure ${timeAgo(wh.most_recent_failure_time)} (recovered)`,
    };
  }
  return { tone: 'ok', label: 'Healthy' };
}

// ───────────────────────── Browser notifications ─────────────────────────

function useBrowserNotifPrefs() {
  const [prefs, setPrefs] = useState<BrowserNotifPrefs>(DEFAULT_PREFS);
  const [permission, setPermission] = useState<NotificationPermission>('default');

  // Read prefs + current permission on mount. Both are local/window
  // state, no race with the permission prompt is possible.
  useEffect(() => {
    setPrefs(readPrefs());
    setPermission(getPermission());
  }, []);

  const save = useCallback((next: BrowserNotifPrefs) => {
    setPrefs(next);
    writePrefs(next);
  }, []);

  const setEnabled = useCallback(
    async (value: boolean) => {
      if (!value) {
        save({ ...readPrefs(), enabled: false });
        return;
      }
      const result = await requestPermission();
      setPermission(result);
      save({ ...readPrefs(), enabled: result === 'granted' });
    },
    [save]
  );

  const setEvent = useCallback(
    (key: BrowserNotifEventKey, value: boolean) => {
      const current = readPrefs();
      save({ ...current, events: { ...current.events, [key]: value } });
    },
    [save]
  );

  return { prefs, permission, setEnabled, setEvent };
}

function BrowserNotifSection() {
  const supported = isSupported();
  const { prefs, permission, setEnabled, setEvent } = useBrowserNotifPrefs();
  const active = prefs.enabled && permission === 'granted';

  return (
    <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
          Browser Notifications
        </h2>
        {active && (
          <button
            type="button"
            onClick={fireTestNotification}
            className="inline-flex items-center gap-1.5 h-8 px-3 rounded-lg bg-white/5 hover:bg-white/10 text-xs text-[var(--text-secondary)] hover:text-white ring-1 ring-white/10 transition"
          >
            Send test
          </button>
        )}
      </div>

      {!supported && (
        <div className="mb-3 rounded-lg bg-amber-500/5 ring-1 ring-amber-500/20 p-3 text-xs text-amber-200/90">
          This browser doesn&apos;t support the Web Notifications API.
        </div>
      )}

      {supported && permission === 'denied' && (
        <div className="mb-3 rounded-lg bg-amber-500/5 ring-1 ring-amber-500/20 p-3 text-xs text-amber-200/90">
          Notifications are blocked for this site. Re-enable them in your browser&apos;s site
          settings, then reload.
        </div>
      )}

      <FormField
        label="Enabled"
        description="System notifications from kino when the tab is backgrounded"
        help="In-tab events still show as toasts. Browser notifications only fire when this tab isn't focused, so you're not double-notified."
      >
        <Toggle
          checked={active}
          onChange={(v) => {
            void setEnabled(v);
          }}
          disabled={!supported || permission === 'denied'}
        />
      </FormField>

      {active && (
        <FormField
          label="Events"
          description="System notifications fire only when this tab isn\u2019t focused. Pipeline milestones (grab, download start/complete, watched) are intentionally not listed — the `imported` event covers the moment content becomes playable, and firing on every stage would be noisy."
        >
          <div className="grid grid-cols-2 gap-2">
            {(
              [
                { key: 'import', label: 'Ready to play' },
                { key: 'upgrade', label: 'Upgraded' },
                { key: 'failure', label: 'Download failed' },
                { key: 'health', label: 'Health issue' },
              ] as const
            ).map(({ key, label }) => (
              <label
                key={key}
                className="flex items-center gap-2 text-sm text-[var(--text-secondary)]"
              >
                <input
                  type="checkbox"
                  checked={prefs.events[key]}
                  onChange={(e) => setEvent(key, e.target.checked)}
                  className="rounded border-white/20 bg-[var(--bg-card)]"
                />
                {label}
              </label>
            ))}
          </div>
        </FormField>
      )}
    </section>
  );
}

// ───────────────────────── Webhooks ─────────────────────────

export function NotificationSettings() {
  const { data, isLoading } = useQuery({
    queryKey: [...WEBHOOKS_KEY],
    queryFn: async () => {
      const { data } = await listWebhooks();
      return (data ?? []) as Webhook[];
    },
    meta: { invalidatedBy: ['webhook_changed'] },
  });

  const [editing, setEditing] = useState<Partial<Webhook> | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [headerError, setHeaderError] = useState<string | null>(null);
  const isNew = editing != null && !editing.id;

  const saveMutation = useMutationWithToast({
    verb: 'save webhook',
    mutationFn: async (wh: Partial<Webhook>) => {
      if (wh.id) {
        // `UpdateWebhook` is a PATCH body: every field optional. The
        // draft `wh` is a `Partial<Webhook>` plus an `id` we strip out
        // via destructuring — no `as` needed.
        const { id, ...body } = wh;
        await updateWebhook({ path: { id }, body: body as UpdateWebhook });
      } else {
        // `CreateWebhook` requires `name` + `url`; fill them with
        // defensive empty-string defaults since the form guards
        // submission on both being non-empty.
        const body: CreateWebhook = {
          name: wh.name ?? '',
          url: wh.url ?? '',
          method: wh.method ?? null,
          headers: wh.headers ?? null,
          body_template: wh.body_template ?? null,
          on_grab: wh.on_grab ?? null,
          on_import: wh.on_import ?? null,
          on_download_complete: wh.on_download_complete ?? null,
          on_upgrade: wh.on_upgrade ?? null,
          on_failure: wh.on_failure ?? null,
          on_watched: wh.on_watched ?? null,
          on_health_issue: wh.on_health_issue ?? null,
        };
        await createWebhook({ body });
      }
    },
    onSuccess: () => {
      // Backend emits `WebhookChanged` → meta dispatcher invalidates
      // WEBHOOKS_KEY across every tab.
      setEditing(null);
      setShowAdvanced(false);
    },
  });

  const deleteMutation = useMutationWithToast({
    verb: 'delete webhook',
    mutationFn: async (id: number) => {
      await deleteWebhook({ path: { id } });
    },
    onSuccess: () => {
      setEditing(null);
      setShowAdvanced(false);
    },
  });

  // Tracks the most recent test result so the user sees "delivered
  // HTTP 200 in 123 ms" or the error message straight inside the
  // modal without leaving it.
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);

  const runTest = async (): Promise<boolean> => {
    if (!editing?.id) return false;
    setTestResult(null);
    try {
      const { data: result } = await testWebhook({ path: { id: editing.id } });
      if (!result) return false;
      setTestResult({ ok: Boolean(result.ok), message: result.message });
      return Boolean(result.ok);
    } catch (e) {
      setTestResult({
        ok: false,
        message: e instanceof Error ? e.message : 'request failed',
      });
      return false;
    }
  };

  const applyPreset = (id: string) => {
    const preset = PRESETS.find((p) => p.id === id);
    if (!preset || !editing) return;
    setEditing({ ...editing, ...preset.apply() });
    setShowAdvanced(true);
  };

  const webhooks = data ?? [];

  const validateHeaders = (value: string) => {
    if (!value.trim()) {
      setHeaderError(null);
      return;
    }
    try {
      const parsed = JSON.parse(value);
      if (typeof parsed !== 'object' || Array.isArray(parsed) || parsed === null) {
        setHeaderError('Must be a JSON object of string values.');
      } else {
        setHeaderError(null);
      }
    } catch {
      setHeaderError('Not valid JSON.');
    }
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-bold">Notifications</h1>
          <p className="text-sm text-[var(--text-muted)]">
            Browser and webhook targets for event notifications
          </p>
        </div>
      </div>

      <BrowserNotifSection />

      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
          Webhooks
        </h2>
        <button
          type="button"
          onClick={() => {
            setTestResult(null);
            setShowAdvanced(false);
            setEditing(blankWebhook());
          }}
          className="flex items-center gap-1.5 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold transition"
        >
          <Plus size={16} />
          Add Webhook
        </button>
      </div>

      {isLoading && <div className="h-20 skeleton rounded-lg" />}

      {!isLoading && webhooks.length === 0 && (
        <div className="text-center py-12 text-[var(--text-muted)]">
          <Bell size={32} className="mx-auto mb-3 opacity-30" />
          <p>No webhooks configured</p>
          <p className="text-xs mt-1">Add Discord, Slack, or ntfy — presets available</p>
        </div>
      )}

      <div className="space-y-2">
        {webhooks.map((wh) => {
          const status = webhookStatus(wh);
          const dotClass =
            status.tone === 'backoff'
              ? 'bg-amber-400'
              : status.tone === 'off'
                ? 'bg-white/20'
                : 'bg-green-500';
          return (
            <button
              key={wh.id}
              type="button"
              onClick={() => {
                setTestResult(null);
                setShowAdvanced(Boolean(wh.headers || wh.body_template));
                setEditing({ ...wh });
              }}
              className="w-full text-left p-4 rounded-lg bg-[var(--bg-card)] ring-1 ring-white/5 hover:ring-white/10 transition-colors"
            >
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-sm font-medium">{wh.name}</p>
                  <p className="text-xs text-[var(--text-muted)] mt-0.5 truncate">{wh.url}</p>
                </div>
                <div
                  className="flex items-center gap-2 flex-shrink-0"
                  title={status.tooltip ?? status.label}
                >
                  {status.tone === 'backoff' && (
                    <AlertTriangle size={12} className="text-amber-400" />
                  )}
                  <span className="text-[11px] text-[var(--text-muted)]">{status.label}</span>
                  <span className={`w-2 h-2 rounded-full ${dotClass}`} />
                </div>
              </div>
            </button>
          );
        })}
      </div>

      {/* Edit/Add Modal */}
      {editing && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
          <button
            type="button"
            className="absolute inset-0 bg-black/70 backdrop-blur-sm"
            onClick={() => {
              setEditing(null);
              setShowAdvanced(false);
            }}
            aria-label="Close"
          />
          <div className="relative w-full max-w-md bg-[var(--bg-secondary)] rounded-xl ring-1 ring-white/10 shadow-2xl overflow-hidden">
            <div className="px-5 py-4 border-b border-white/5">
              <h2 className="text-lg font-semibold">{isNew ? 'Add Webhook' : 'Edit Webhook'}</h2>
            </div>
            <div className="px-5 py-4 space-y-1 max-h-[60vh] overflow-y-auto">
              {isNew && (
                <div className="mb-3 pb-3 border-b border-white/5">
                  <p className="text-xs font-semibold text-[var(--text-muted)] uppercase tracking-wider mb-2">
                    Quick presets
                  </p>
                  <div className="flex gap-2 flex-wrap">
                    {PRESETS.map((preset) => (
                      <button
                        key={preset.id}
                        type="button"
                        onClick={() => applyPreset(preset.id)}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-xs text-[var(--text-secondary)] hover:text-white ring-1 ring-white/10 transition"
                      >
                        <MessageSquare size={12} />
                        {preset.label}
                      </button>
                    ))}
                  </div>
                  <p className="text-[10px] text-[var(--text-muted)] mt-2">
                    Prefills body template and headers. You still need to paste your webhook URL.
                  </p>
                </div>
              )}

              <FormField label="Name">
                <TextInput
                  value={editing.name ?? ''}
                  onChange={(v) => setEditing((p) => p && { ...p, name: v })}
                  placeholder="Discord"
                />
              </FormField>
              <FormField label="URL">
                <TextInput
                  value={editing.url ?? ''}
                  onChange={(v) => setEditing((p) => p && { ...p, url: v })}
                  placeholder="https://..."
                  type="url"
                />
              </FormField>
              <FormField label="Enabled">
                <Toggle
                  checked={editing.enabled ?? true}
                  onChange={(v) => setEditing((p) => p && { ...p, enabled: v })}
                />
              </FormField>

              <div className="border-t border-white/5 pt-3 mt-3">
                <p className="text-xs font-semibold text-[var(--text-muted)] uppercase tracking-wider mb-2">
                  Events
                </p>
                <div className="grid grid-cols-2 gap-2">
                  {EVENT_TOGGLES.map(({ key, label }) => (
                    <label
                      key={key as string}
                      className="flex items-center gap-2 text-sm text-[var(--text-secondary)]"
                    >
                      <input
                        type="checkbox"
                        checked={Boolean((editing as Record<string, unknown>)[key as string])}
                        onChange={(e) =>
                          setEditing((p) => p && { ...p, [key as string]: e.target.checked })
                        }
                        className="rounded border-white/20 bg-[var(--bg-card)]"
                      />
                      {label}
                    </label>
                  ))}
                </div>
              </div>

              <button
                type="button"
                onClick={() => setShowAdvanced((s) => !s)}
                className="mt-4 flex items-center gap-1 text-xs text-[var(--text-secondary)] hover:text-white"
              >
                {showAdvanced ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                Advanced
              </button>

              {showAdvanced && (
                <div className="mt-3 space-y-1 border-t border-white/5 pt-3">
                  <FormField
                    label="Method"
                    description="HTTP verb used when delivering — POST for nearly all targets"
                  >
                    <SelectInput
                      value={editing.method ?? 'POST'}
                      onChange={(v) => setEditing((p) => p && { ...p, method: v })}
                      options={METHOD_OPTIONS}
                    />
                  </FormField>
                  <FormField
                    label="Headers"
                    description="JSON object — e.g. auth headers for self-hosted targets"
                    help='For ntfy use {"Title": "kino", "Priority": "default"}. For auth use {"Authorization": "Bearer abc123"}.'
                  >
                    <textarea
                      value={editing.headers ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        setEditing((p) => p && { ...p, headers: v || null });
                        validateHeaders(v);
                      }}
                      placeholder='{"Authorization": "Bearer abc123"}'
                      rows={3}
                      className="w-full px-3 py-2 rounded-lg bg-[var(--bg-card)] ring-1 ring-white/10 focus:ring-white/25 font-mono text-xs text-[var(--text-primary)] transition"
                    />
                    {headerError && <p className="text-[11px] text-red-400 mt-1">{headerError}</p>}
                  </FormField>
                  <FormField
                    label="Body template"
                    description="Optional — leave empty to send event JSON"
                    help="Placeholders: {{event}} {{title}} {{show}} {{quality}} {{year}} {{season}} {{episode}} {{size}} {{indexer}} {{message}}"
                  >
                    <textarea
                      value={editing.body_template ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        setEditing((p) => p && { ...p, body_template: v || null });
                      }}
                      placeholder='{"content": "{{event}}: {{title}}"}'
                      rows={4}
                      className="w-full px-3 py-2 rounded-lg bg-[var(--bg-card)] ring-1 ring-white/10 focus:ring-white/25 font-mono text-xs text-[var(--text-primary)] transition"
                    />
                  </FormField>
                </div>
              )}

              {testResult && (
                <div
                  className={`mt-3 rounded-lg p-2 ring-1 text-xs ${
                    testResult.ok
                      ? 'bg-green-500/5 ring-green-500/20 text-green-200'
                      : 'bg-red-500/10 ring-red-500/20 text-red-300'
                  }`}
                >
                  {testResult.message}
                </div>
              )}
            </div>
            <div className="flex items-center justify-between px-5 py-4 border-t border-white/5 gap-2">
              <div className="flex items-center gap-2">
                {!isNew && editing.id && (
                  <button
                    type="button"
                    onClick={() => editing.id && deleteMutation.mutate(editing.id)}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm text-red-400 hover:bg-red-600/10 transition"
                  >
                    <Trash2 size={14} />
                    Delete
                  </button>
                )}
                {!isNew && editing.id && <TestButton onTest={runTest} label="Test" />}
              </div>
              <div className="flex gap-2">
                <button
                  type="button"
                  onClick={() => {
                    setEditing(null);
                    setShowAdvanced(false);
                  }}
                  className="px-4 py-1.5 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => saveMutation.mutate(editing)}
                  disabled={
                    saveMutation.isPending || !editing.name || !editing.url || headerError !== null
                  }
                  className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
                >
                  {saveMutation.isPending && <Loader2 size={14} className="animate-spin" />}
                  Save
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
