import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { AlertTriangle, Check, Copy, Eye, EyeOff, Loader2, RefreshCw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import {
  getHomePreferences,
  lanProbe,
  mdnsTest,
  rotateApiKey,
  updateHomePreferences,
} from '@/api/generated/sdk.gen';
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

      <NetworkingSection />

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

/**
 * mDNS / "kino.local" settings + LAN reachability test.
 *
 * Lets the user configure the broadcast hostname (defaults to `kino`),
 * disable mDNS entirely (e.g. for environments where multicast is
 * blocked at the AP), and probe end-to-end reachability from the
 * current browser to confirm LAN clients can hit kino. The probe is
 * what surfaces a firewall block: localhost works (this page is
 * loaded), but `fetch(http://<lan-ip>:<port>/api/v1/health)` from the
 * SAME browser fails → almost certainly the host firewall.
 */
function NetworkingSection() {
  const { config, updateField } = useSettingsContext();
  const mdnsEnabled = Boolean(config.mdns_enabled ?? true);
  const mdnsHostname = String(config.mdns_hostname ?? 'kino');
  const [hostnameError, setHostnameError] = useState('');
  const [testResult, setTestResult] = useState<NetworkTestResult | null>(null);
  const [testing, setTesting] = useState(false);

  const validateHostname = (v: string) => {
    // RFC-952/1123-ish: alphanumeric + hyphens, no leading/trailing
    // hyphen, 1–63 chars. Avahi rejects anything else outright.
    const trimmed = v.trim().toLowerCase();
    if (trimmed.length === 0) {
      setHostnameError('Hostname is required when mDNS is enabled');
    } else if (trimmed.length > 63) {
      setHostnameError('Hostname must be 63 characters or fewer');
    } else if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(trimmed)) {
      setHostnameError('Use letters, digits, and hyphens only — no leading/trailing hyphen');
    } else {
      setHostnameError('');
    }
    updateField('mdns_hostname', trimmed);
  };

  const runTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const probe = await lanProbe();
      const ips = probe.data?.ipv4s ?? [];
      const port = probe.data?.port ?? 80;
      // Browser-side LAN reachability probe: race fetches against
      // every bound IPv4 with a 2s timeout. If at least one returns
      // 200, LAN clients can reach us. If none do, it's almost
      // always the host firewall.
      const reachable = await Promise.any(
        ips.map(async (ip) => {
          const ctrl = new AbortController();
          const t = setTimeout(() => ctrl.abort(), 2000);
          try {
            const res = await fetch(`http://${ip}:${port}/api/v1/health`, {
              signal: ctrl.signal,
              cache: 'no-store',
            });
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            return ip;
          } finally {
            clearTimeout(t);
          }
        })
      ).catch(() => null);
      // mDNS resolution from the backend's perspective.
      const mdns = await mdnsTest({ body: { hostname: mdnsHostname || null } });
      setTestResult({
        ips,
        port,
        reachable,
        mdnsOk: mdns.data?.ok ?? false,
        mdnsMessage: mdns.data?.message ?? '',
      });
    } catch (e) {
      setTestResult({
        ips: [],
        port: 80,
        reachable: null,
        mdnsOk: false,
        mdnsMessage: `Probe failed: ${e instanceof Error ? e.message : String(e)}`,
      });
    } finally {
      setTesting(false);
    }
  };

  return (
    <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
      <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
        Networking · kino.local
      </h2>
      <FormField
        label="mDNS broadcast"
        description="Advertise kino on the local network so other devices can use http://<hostname>.local"
        help="Disable if your access point blocks multicast (some hotel / enterprise Wi-Fi). LAN clients will need to use the IP address directly when off."
      >
        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={mdnsEnabled}
            onChange={(e) => updateField('mdns_enabled', e.target.checked)}
            className="w-4 h-4 accent-[var(--accent)]"
          />
          <span className="text-sm text-[var(--text-secondary)]">
            {mdnsEnabled ? 'Enabled' : 'Disabled'}
          </span>
        </label>
      </FormField>
      <FormField
        label="Hostname"
        description='Resolves as "<hostname>.local" — e.g. "kino" → http://kino.local'
        error={hostnameError}
      >
        <TextInput
          value={mdnsHostname}
          onChange={validateHostname}
          placeholder="kino"
          error={Boolean(hostnameError)}
        />
      </FormField>

      <div className="mt-4">
        <button
          type="button"
          onClick={runTest}
          disabled={testing}
          className="h-9 px-4 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] hover:text-white transition flex items-center gap-2 disabled:opacity-50"
        >
          {testing ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
          Test LAN reachability
        </button>
        {testResult && <NetworkTestResultCard result={testResult} hostname={mdnsHostname} />}
      </div>
    </section>
  );
}

interface NetworkTestResult {
  ips: string[];
  port: number;
  reachable: string | null;
  mdnsOk: boolean;
  mdnsMessage: string;
}

function NetworkTestResultCard({
  result,
  hostname,
}: {
  result: NetworkTestResult;
  hostname: string;
}) {
  const { ips, port, reachable, mdnsOk, mdnsMessage } = result;
  const lanOk = reachable !== null;
  const allGood = lanOk && mdnsOk;
  const portSuffix = port === 80 ? '' : `:${port}`;
  return (
    <div
      className={`mt-3 rounded-lg ring-1 p-3 ${
        allGood ? 'bg-emerald-500/5 ring-emerald-500/20' : 'bg-amber-500/5 ring-amber-500/25'
      }`}
    >
      <div className="flex items-start gap-2">
        {allGood ? (
          <Check size={16} className="text-emerald-400 mt-0.5 flex-shrink-0" />
        ) : (
          <AlertTriangle size={16} className="text-amber-400 mt-0.5 flex-shrink-0" />
        )}
        <div className="flex-1 min-w-0 text-xs space-y-1.5">
          {ips.length === 0 ? (
            <p className="text-amber-300">
              No LAN interfaces found — kino is bound only to localhost. Check that the host has an
              active network connection.
            </p>
          ) : (
            <p className={lanOk ? 'text-emerald-300' : 'text-amber-300'}>
              <span className="font-medium">LAN: </span>
              {lanOk
                ? `reachable at http://${reachable}${portSuffix}/`
                : `bound to ${ips.join(', ')} but no LAN client could reach kino at port ${port}.`}
            </p>
          )}
          <p className={mdnsOk ? 'text-emerald-300' : 'text-amber-300'}>
            <span className="font-medium">mDNS: </span>
            {mdnsMessage || (mdnsOk ? 'OK' : 'unavailable')}
          </p>
          {!lanOk && ips.length > 0 && <FirewallRemediation />}
          {lanOk && mdnsOk && (
            <p className="text-emerald-200/80">
              Open{' '}
              <code className="font-mono">
                http://{hostname}.local{portSuffix}
              </code>{' '}
              from any device on this network.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Surfaces the exact `kino allow-firewall` invocation when the LAN
 * probe fails. We don't try to detect the user's distro from the
 * browser; the CLI subcommand auto-detects UFW vs firewalld and
 * triggers a graphical password prompt (Polkit on Linux, UAC on
 * Windows, osascript on macOS).
 */
function FirewallRemediation() {
  const [copied, setCopied] = useState(false);
  const cmd = 'sudo kino allow-firewall';
  return (
    <div className="mt-2 rounded-md bg-black/20 ring-1 ring-white/5 p-2 space-y-1.5">
      <p className="text-[var(--text-secondary)] leading-relaxed">
        Looks like a firewall is blocking inbound traffic. Run this on the kino host:
      </p>
      <div className="flex items-center gap-2">
        <code className="flex-1 font-mono text-[11px] text-white bg-black/30 px-2 py-1.5 rounded">
          {cmd}
        </code>
        <button
          type="button"
          onClick={() => {
            navigator.clipboard.writeText(cmd);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
          }}
          className="h-7 px-2 rounded bg-white/5 hover:bg-white/10 text-[var(--text-secondary)] hover:text-white transition flex items-center gap-1 text-[11px]"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <p className="text-[10px] text-[var(--text-muted)] leading-relaxed">
        Triggers a graphical password prompt. Auto-detects UFW / firewalld; falls back to printing
        the raw nftables command if neither is active.
      </p>
    </div>
  );
}
