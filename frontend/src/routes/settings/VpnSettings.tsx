import { useQuery } from '@tanstack/react-query';
import { AlertCircle, CheckCircle2, Circle, FileText, Radio } from 'lucide-react';
import { useState } from 'react';
import {
  getStatus2 as getVpnStatus,
  testConnection as testVpnConnection,
} from '@/api/generated/sdk.gen';
import {
  FormField,
  SecretInput,
  SelectInput,
  TestButton,
  TextInput,
  Toggle,
} from '@/components/settings/FormField';
import { cn } from '@/lib/utils';
import { useSettingsContext } from './SettingsLayout';

/** Parse a WireGuard .conf file into individual fields */
function parseWireGuardConfig(text: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (trimmed.startsWith('#') || trimmed.startsWith('[') || !trimmed.includes('=')) continue;
    const eqIdx = trimmed.indexOf('=');
    const key = trimmed.slice(0, eqIdx).trim();
    const val = trimmed.slice(eqIdx + 1).trim();

    switch (key.toLowerCase()) {
      case 'privatekey':
        result.vpn_private_key = val;
        break;
      case 'address':
        result.vpn_address = val;
        break;
      case 'dns':
        result.vpn_dns = val;
        break;
      case 'publickey':
        result.vpn_server_public_key = val;
        break;
      case 'endpoint':
        result.vpn_server_endpoint = val;
        break;
    }
  }
  return result;
}

// Address must be a CIDR ("10.2.0.2/32"). Loose check — we let obvious
// typos through with a warning rather than blocking save.
const CIDR_RE = /^\d{1,3}(?:\.\d{1,3}){3}\/\d{1,2}$/;
// Endpoint is host:port — hostname can be IP or DNS name.
const ENDPOINT_RE = /^[A-Za-z0-9.-]+:\d{1,5}$/;

function humanHandshake(secs: number | null | undefined): string {
  if (secs === null || secs === undefined) return 'never';
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

function StatusCard() {
  const [publicIp, setPublicIp] = useState<string | null>(null);
  const [testMessage, setTestMessage] = useState<string | null>(null);

  const { data } = useQuery({
    queryKey: ['kino', 'vpn', 'status'],
    queryFn: async () => {
      const { data } = await getVpnStatus();
      return data;
    },
    refetchInterval: 5000,
  });

  const runTest = async (): Promise<boolean> => {
    try {
      const { data } = await testVpnConnection();
      setPublicIp(data?.public_ip ?? null);
      setTestMessage(data?.message ?? null);
      return Boolean(data?.ok);
    } catch (e) {
      setTestMessage(e instanceof Error ? e.message : 'test failed');
      return false;
    }
  };

  const status = data?.status ?? 'disconnected';
  const isConnected = status === 'connected';
  const isConnecting = status === 'connecting';
  const isError = status === 'error';

  return (
    <div className="rounded-lg border border-white/10 bg-[var(--bg-card)]/40 p-4 mb-6">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-start gap-3">
          <span
            className={cn(
              'mt-0.5 h-7 w-7 rounded-full flex items-center justify-center ring-1 flex-shrink-0',
              isConnected && 'bg-green-500/10 text-green-400 ring-green-500/20',
              isConnecting && 'bg-sky-500/10 text-sky-400 ring-sky-500/20',
              isError && 'bg-red-500/10 text-red-400 ring-red-500/20',
              !isConnected &&
                !isConnecting &&
                !isError &&
                'bg-white/5 text-[var(--text-muted)] ring-white/10'
            )}
          >
            {isConnected && <CheckCircle2 size={14} />}
            {isConnecting && <Radio size={14} className="animate-pulse" />}
            {isError && <AlertCircle size={14} />}
            {!isConnected && !isConnecting && !isError && <Circle size={14} />}
          </span>
          <div className="min-w-0">
            <p className="text-sm font-semibold text-white capitalize">{status}</p>
            <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs text-[var(--text-muted)]">
              {data?.interface && (
                <>
                  <dt>Interface</dt>
                  <dd className="font-mono text-[var(--text-secondary)]">{data.interface}</dd>
                </>
              )}
              {data?.forwarded_port != null && (
                <>
                  <dt>Forwarded port</dt>
                  <dd className="font-mono text-[var(--text-secondary)]">{data.forwarded_port}</dd>
                </>
              )}
              <dt>Last handshake</dt>
              <dd className="font-mono text-[var(--text-secondary)]">
                {humanHandshake(data?.last_handshake_ago_secs)}
              </dd>
              {publicIp && (
                <>
                  <dt>Egress IP</dt>
                  <dd className="font-mono text-[var(--text-secondary)]">{publicIp}</dd>
                </>
              )}
            </dl>
            {testMessage && <p className="mt-2 text-xs text-[var(--text-muted)]">{testMessage}</p>}
          </div>
        </div>
        <TestButton onTest={runTest} label="Test tunnel" />
      </div>
    </div>
  );
}

export function VpnSettings() {
  const { config, updateField } = useSettingsContext();
  const [pasteMode, setPasteMode] = useState(false);
  const [pasteText, setPasteText] = useState('');
  const [parseResult, setParseResult] = useState<string | null>(null);

  const address = String(config.vpn_address ?? '');
  const endpoint = String(config.vpn_server_endpoint ?? '');
  const provider = String(config.vpn_port_forward_provider ?? 'none');
  const providerNeedsKey = provider === 'airvpn' || provider === 'pia';

  const addressError = address && !CIDR_RE.test(address) ? 'Expected CIDR — e.g. 10.2.0.2/32' : '';
  const endpointError =
    endpoint && !ENDPOINT_RE.test(endpoint)
      ? 'Expected host:port — e.g. vpn.example.com:51820'
      : '';

  const handleParse = () => {
    const parsed = parseWireGuardConfig(pasteText);
    const count = Object.keys(parsed).length;
    if (count === 0) {
      setParseResult('Could not parse any fields. Check the format.');
      return;
    }
    for (const [key, val] of Object.entries(parsed)) {
      updateField(key, val);
    }
    setParseResult(`Parsed ${count} field${count !== 1 ? 's' : ''}`);
    setPasteMode(false);
    setPasteText('');
    setTimeout(() => setParseResult(null), 3000);
  };

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">VPN</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">WireGuard tunnel for torrent traffic</p>

      {Boolean(config.vpn_enabled) && <StatusCard />}

      <FormField
        label="Enabled"
        description="Route all torrent traffic through VPN"
        help="When enabled, all BitTorrent traffic is routed through a WireGuard tunnel. API, web UI, and streaming use the normal network interface."
      >
        <Toggle
          checked={Boolean(config.vpn_enabled)}
          onChange={(v) => updateField('vpn_enabled', v)}
        />
      </FormField>

      {Boolean(config.vpn_enabled) && (
        <>
          {/* Quick import */}
          <div className="border-t border-white/5 pt-4 mt-4 mb-4">
            {!pasteMode ? (
              <div className="flex items-center gap-3">
                <button
                  type="button"
                  onClick={() => setPasteMode(true)}
                  className="flex items-center gap-2 px-4 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] hover:text-white ring-1 ring-white/10 transition"
                >
                  <FileText size={16} />
                  Paste WireGuard Config
                </button>
                {parseResult && <span className="text-xs text-green-400">{parseResult}</span>}
              </div>
            ) : (
              <div className="space-y-3">
                <p className="text-xs text-[var(--text-muted)]">
                  Paste your WireGuard .conf file contents below. The [Interface] and [Peer]
                  sections will be parsed automatically.
                </p>
                <textarea
                  value={pasteText}
                  onChange={(e) => setPasteText(e.target.value)}
                  placeholder={`[Interface]\nPrivateKey = ...\nAddress = 10.2.0.2/32\nDNS = 10.2.0.1\n\n[Peer]\nPublicKey = ...\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0`}
                  rows={8}
                  className="w-full px-3 py-2 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white font-mono placeholder:text-[var(--text-muted)]/50 focus:outline-none focus:ring-1 focus:ring-[var(--accent)] resize-none"
                />
                {parseResult && <p className="text-xs text-red-400">{parseResult}</p>}
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={handleParse}
                    disabled={!pasteText.trim()}
                    className="px-4 py-1.5 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
                  >
                    Parse & Fill Fields
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setPasteMode(false);
                      setPasteText('');
                      setParseResult(null);
                    }}
                    className="px-4 py-1.5 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
          </div>

          <section className="space-y-1 border-t border-white/5 pt-4">
            <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
              WireGuard
            </h2>
            <FormField
              label="Private Key"
              description="From [Interface] section — treat as a password"
              help="From [Interface] section of your WireGuard config. This grants access to your VPN account and should be kept secret."
            >
              <SecretInput
                value={String(config.vpn_private_key ?? '')}
                onChange={(v) => updateField('vpn_private_key', v)}
              />
            </FormField>
            <FormField
              label="Address"
              description="Interface IP"
              error={addressError}
              help="The IP + mask assigned to your WireGuard interface by the VPN provider. Always a /32 for WireGuard."
            >
              <TextInput
                value={address}
                onChange={(v) => updateField('vpn_address', v)}
                placeholder="10.2.0.2/32"
                error={Boolean(addressError)}
              />
            </FormField>
            <FormField
              label="Server Public Key"
              help="From [Peer] section — the VPN server's public key. Safe to display."
            >
              <TextInput
                value={String(config.vpn_server_public_key ?? '')}
                onChange={(v) => updateField('vpn_server_public_key', v)}
              />
            </FormField>
            <FormField
              label="Server Endpoint"
              description="host:port"
              error={endpointError}
              help="From [Peer] Endpoint field. DNS name or IP, followed by the WireGuard port (usually 51820)."
            >
              <TextInput
                value={endpoint}
                onChange={(v) => updateField('vpn_server_endpoint', v)}
                placeholder="vpn.example.com:51820"
                error={Boolean(endpointError)}
              />
            </FormField>
            <FormField
              label="DNS"
              description="Optional"
              help="From [Interface] DNS field. Usually the VPN gateway IP. Leave empty to inherit the host's resolver."
            >
              <TextInput
                value={String(config.vpn_dns ?? '')}
                onChange={(v) => updateField('vpn_dns', v)}
                placeholder="10.2.0.1"
              />
            </FormField>
          </section>

          <section className="space-y-1 border-t border-white/5 pt-6 mt-4">
            <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
              Port Forwarding
            </h2>
            <FormField
              label="Provider"
              help="Required for incoming peer connections. Without port forwarding, only outgoing connections work — fewer peers, slower downloads."
            >
              <SelectInput
                value={provider}
                onChange={(v) => updateField('vpn_port_forward_provider', v)}
                options={[
                  { value: 'none', label: 'None' },
                  { value: 'natpmp', label: 'NAT-PMP (ProtonVPN)' },
                  { value: 'airvpn', label: 'AirVPN' },
                  { value: 'pia', label: 'PIA' },
                ]}
              />
            </FormField>
            {providerNeedsKey && (
              <FormField
                label="API Key"
                description="Provider-specific"
                help={
                  provider === 'airvpn'
                    ? 'Generate an API key from your AirVPN client area → API section.'
                    : 'Your PIA account password or API token, depending on your plan.'
                }
              >
                <SecretInput
                  value={String(config.vpn_port_forward_api_key ?? '')}
                  onChange={(v) => updateField('vpn_port_forward_api_key', v)}
                />
              </FormField>
            )}
          </section>
        </>
      )}
    </div>
  );
}
