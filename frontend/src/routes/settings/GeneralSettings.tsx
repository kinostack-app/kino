import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { AlertTriangle, Check, Copy, Eye, EyeOff, Loader2, RefreshCw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { getHomePreferences, rotateApiKey, updateHomePreferences } from '@/api/generated/sdk.gen';
import type { HomePreferences } from '@/api/generated/types.gen';
import { kinoToast } from '@/components/kino-toast';
import { FormField, NumberInput, TestButton, TextInput } from '@/components/settings/FormField';
import { useSettingsContext } from './SettingsLayout';

export function GeneralSettings() {
  const { config, updateField } = useSettingsContext();
  const apiKey = String(config.api_key ?? '***');
  const port = Number(config.listen_port ?? 8080);
  const baseUrl = String(config.base_url ?? '');
  const [portError, setPortError] = useState('');
  const [baseUrlError, setBaseUrlError] = useState('');
  const [copied, setCopied] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const [rotating, setRotating] = useState(false);
  const [confirmRotate, setConfirmRotate] = useState(false);

  const validatePort = (v: number) => {
    if (v < 1 || v > 65535) {
      setPortError('Port must be between 1 and 65535');
    } else {
      setPortError('');
    }
    updateField('listen_port', v);
  };

  const commitBaseUrl = (raw: string) => {
    // Normalize: strip trailing slashes; auto-prefix with `/` if non-empty.
    let v = raw.replace(/\/+$/, '');
    if (v && !v.startsWith('/')) v = `/${v}`;
    setBaseUrlError(v && !/^\/[\w\-./]*$/.test(v) ? 'Invalid base URL path' : '');
    updateField('base_url', v);
  };

  const copyKey = () => {
    navigator.clipboard.writeText(apiKey);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleRotate = async () => {
    setRotating(true);
    try {
      await rotateApiKey();
      // The backend emits ConfigChanged; the layout refetches the config.
      // Tell the user to copy the new key from the field.
      setConfirmRotate(false);
    } finally {
      setRotating(false);
    }
  };

  // Use the browser's current hostname for an accurate preview.
  const appOrigin = `${window.location.protocol}//${window.location.hostname}:${port}`;
  const appUrl = `${appOrigin}${baseUrl}`;

  const testBaseUrl = async () => {
    try {
      const res = await fetch(`${baseUrl || ''}/api/v1/status`);
      return res.ok;
    } catch {
      return false;
    }
  };

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">General</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">Server configuration and API access</p>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Personal
        </h2>
        <GreetingNameField />
      </section>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3 flex items-center gap-2">
          Server
          <span
            className="text-[10px] font-medium normal-case tracking-normal text-amber-400/90 bg-amber-500/10 ring-1 ring-amber-500/20 rounded px-1.5 py-0.5"
            title="Changes to these fields take effect after the next backend restart"
          >
            restart required
          </span>
        </h2>
        <FormField
          label="Listen Address"
          description="IP address to bind to"
          help="Use 0.0.0.0 to listen on all interfaces, or 127.0.0.1 for localhost only."
        >
          <TextInput
            value={String(config.listen_address ?? '')}
            onChange={(v) => updateField('listen_address', v)}
            placeholder="0.0.0.0"
          />
        </FormField>
        <FormField label="Port" description="HTTP port" error={portError}>
          <NumberInput
            value={port}
            onChange={validatePort}
            min={1}
            max={65535}
            error={Boolean(portError)}
          />
        </FormField>
        <FormField
          label="Base URL"
          description="For reverse proxy"
          help="Leave empty unless you're running kino behind a reverse proxy at a sub-path. Leading slash is added automatically."
          error={baseUrlError}
          preview={baseUrl ? `Your app at: ${appUrl}` : undefined}
        >
          <div className="flex gap-2">
            <div className="flex-1">
              <TextInput
                value={baseUrl}
                onChange={commitBaseUrl}
                placeholder="/kino"
                error={Boolean(baseUrlError)}
              />
            </div>
            {baseUrl && <TestButton onTest={testBaseUrl} label="Verify" />}
          </div>
        </FormField>
      </section>

      <section>
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          API Key
        </h2>
        <FormField
          label="API Key"
          description="Used to authenticate API calls"
          help="Used by external tools (and the web UI) to authenticate with kino's API. Rotate if you think the key has leaked — existing API consumers will need the new value."
        >
          <div className="space-y-2">
            <div className="flex gap-2">
              <div className="relative flex-1 min-w-0">
                <code
                  className="block w-full h-9 pl-3 pr-10 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-[var(--text-secondary)] leading-9 font-mono truncate select-all tracking-[0.15em]"
                  title={revealed ? 'API key visible' : 'API key hidden — click the eye to reveal'}
                >
                  {revealed ? apiKey : '•'.repeat(Math.min(apiKey.length, 36))}
                </code>
                <button
                  type="button"
                  onClick={() => setRevealed((v) => !v)}
                  className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-[var(--text-muted)] hover:text-white transition"
                  aria-label={revealed ? 'Hide API key' : 'Reveal API key'}
                  title={revealed ? 'Hide' : 'Reveal'}
                >
                  {revealed ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>
              <button
                type="button"
                onClick={copyKey}
                className="h-9 px-3 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white transition flex items-center gap-1.5 text-sm flex-shrink-0"
              >
                {copied ? <Check size={14} /> : <Copy size={14} />}
                {copied ? 'Copied' : 'Copy'}
              </button>
              <button
                type="button"
                onClick={() => setConfirmRotate(true)}
                disabled={confirmRotate}
                className="h-9 px-3 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white transition flex items-center gap-1.5 text-sm flex-shrink-0 disabled:opacity-50 disabled:cursor-not-allowed"
                title="Generate a new API key"
              >
                <RefreshCw size={14} />
                Rotate
              </button>
            </div>

            {confirmRotate && (
              <div className="flex items-start gap-3 rounded-lg bg-red-500/10 ring-1 ring-red-500/20 p-3">
                <AlertTriangle size={16} className="text-red-400 flex-shrink-0 mt-0.5" />
                <div className="flex-1 min-w-0">
                  <p className="text-sm text-red-300 font-medium">Rotate API key?</p>
                  <p className="text-xs text-[var(--text-secondary)] mt-0.5 leading-relaxed">
                    The current key stops working immediately. Any external clients (and this
                    browser session) will need the new value.
                  </p>
                </div>
                <div className="flex items-center gap-2 flex-shrink-0">
                  <button
                    type="button"
                    onClick={() => setConfirmRotate(false)}
                    disabled={rotating}
                    className="h-8 px-3 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white transition text-xs"
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    onClick={handleRotate}
                    disabled={rotating}
                    className="h-8 px-3 rounded-lg bg-red-500 hover:bg-red-500/90 text-white transition flex items-center gap-1.5 text-xs font-medium"
                  >
                    {rotating && <Loader2 size={14} className="animate-spin" />}
                    Rotate key
                  </button>
                </div>
              </div>
            )}
          </div>
        </FormField>
      </section>
    </div>
  );
}

/**
 * Greeting-name input — writes to the `user_preferences.greeting_name`
 * column read by Home's "Good evening, {name}" header. Lives in
 * General settings rather than the customise drawer because it's a
 * personal-detail setting, not a layout choice. Auto-saves 600ms
 * after the user stops typing — no save button, matches the spec's
 * "no nagging" UX from §18.
 */
function GreetingNameField() {
  const qc = useQueryClient();
  const { data: prefs } = useQuery<HomePreferences | null>({
    queryKey: ['kino', 'preferences', 'home'],
    queryFn: async () => {
      const res = await getHomePreferences();
      return (res.data as HomePreferences | undefined) ?? null;
    },
  });
  // Local input state seeded from server. Tracks edits; debounce
  // commits via the mutation below.
  const [name, setName] = useState<string>(prefs?.greeting_name ?? '');
  const [hydrated, setHydrated] = useState(false);
  useEffect(() => {
    if (!hydrated && prefs) {
      setName(prefs.greeting_name ?? '');
      setHydrated(true);
    }
  }, [prefs, hydrated]);

  const save = useMutation({
    mutationFn: async (v: string) => {
      const trimmed = v.trim();
      // Explicit clear when the input is empty — backend's COALESCE
      // would otherwise preserve the previously-set name. For a
      // non-empty value the clear flag must be false so `greeting_name`
      // is applied.
      await updateHomePreferences({
        body: trimmed ? { greeting_name: trimmed } : { clear_greeting_name: true },
      });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'preferences', 'home'] });
    },
    // Autosave is silent on success but a dropped write should
    // surface — otherwise the user types their name and has no
    // signal it didn't land.
    onError: (err) => {
      kinoToast.error("Couldn't save greeting name", {
        description: err instanceof Error ? err.message : String(err),
      });
    },
  });

  // Debounce so we don't fire a request per keystroke while the user
  // types their name. 600ms is the same window the rest of the app
  // uses for free-text fields (search, etc.). Hold the latest
  // mutation in a ref so the effect doesn't re-bind every render.
  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    if (!hydrated) return;
    if ((prefs?.greeting_name ?? '') === name) return;
    const t = setTimeout(() => saveRef.current.mutate(name), 600);
    return () => clearTimeout(t);
  }, [name, hydrated, prefs?.greeting_name]);

  return (
    <FormField
      label="Your name"
      description={'Used in the Home greeting (e.g. "Good evening, Robert")'}
      help="Leave blank to drop the name and just show the time-of-day greeting."
    >
      <TextInput value={name} onChange={setName} placeholder="e.g. Robert" />
    </FormField>
  );
}
