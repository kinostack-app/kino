/**
 * Settings → Integrations. Currently Trakt-only; the module is
 * structured so adding Lists / other services later is a new card
 * alongside the Trakt one.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Copy,
  Download,
  ExternalLink,
  RefreshCw,
  X,
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  beginDevice,
  dryRun,
  pollDevice,
  syncNow,
  disconnect as traktDisconnect,
  import_ as traktImport,
  status as traktStatusFn,
  updateConfig,
} from '@/api/generated/sdk.gen';
import type {
  BeginReply,
  ConfigUpdate,
  DryRunCounts,
  PollReply,
  TraktStatus,
} from '@/api/generated/types.gen';
import { FormField, SecretInput } from '@/components/settings/FormField';
import { cn } from '@/lib/utils';
import { useSettingsContext } from './SettingsLayout';

function formatWhen(iso: string | null | undefined): string {
  if (!iso) return 'never';
  try {
    const d = new Date(iso);
    return d.toLocaleString();
  } catch {
    return iso;
  }
}

/** Row-style toggle matching the Customise drawer style. */
function Toggle({
  label,
  description,
  checked,
  onChange,
}: {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-start gap-3 py-2">
      <div className="flex-1 min-w-0">
        <div className="text-sm text-white">{label}</div>
        {description && <p className="text-xs text-[var(--text-muted)] mt-0.5">{description}</p>}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        onClick={() => onChange(!checked)}
        className={cn(
          'relative mt-1 inline-flex h-5 w-9 items-center rounded-full transition-colors shrink-0',
          checked ? 'bg-[var(--accent)]' : 'bg-white/10'
        )}
      >
        <span
          className={cn(
            'inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform',
            checked ? 'translate-x-[18px]' : 'translate-x-[2px]'
          )}
        />
      </button>
    </div>
  );
}

/** Modal displayed during the device-code flow. Shows the user-code
 *  + verification URL and polls the backend until done. Countdown
 *  matches Trakt's `expires_in` so the user isn't left staring at a
 *  dead code past its expiry. */
function DeviceCodeModal({
  device,
  onCancel,
  onDone,
}: {
  device: BeginReply;
  onCancel: () => void;
  onDone: (reason?: string) => void;
}) {
  const [state, setState] = useState<'pending' | 'done' | 'error'>('pending');
  const [message, setMessage] = useState<string>('');
  const [remainingSecs, setRemainingSecs] = useState<number>(device.expires_in_secs);
  const [codeCopied, setCodeCopied] = useState(false);

  const copyCode = async () => {
    try {
      await navigator.clipboard.writeText(device.user_code);
      setCodeCopied(true);
      setTimeout(() => setCodeCopied(false), 1400);
    } catch {
      // Clipboard denied; the code is still displayed for manual copy.
    }
  };

  // Stash onDone in a ref so the poll effect doesn't re-run when the
  // parent re-renders (which would trash the running loop + restart
  // the 5-second polling clock).
  const onDoneRef = useRef(onDone);
  useEffect(() => {
    onDoneRef.current = onDone;
  }, [onDone]);

  // Per-second countdown for the "expires in" label. The main poll
  // loop only runs every 5s (the Trakt-mandated interval), so
  // piggybacking the countdown on it made the label tick in 5-sec
  // jumps — unsettling when you're watching it. Uses a wall-clock
  // anchor so tab-throttling doesn't drift the display vs. reality.
  useEffect(() => {
    if (state !== 'pending') return;
    const start = Date.now();
    const tick = setInterval(() => {
      const elapsed = Math.floor((Date.now() - start) / 1000);
      setRemainingSecs(Math.max(0, device.expires_in_secs - elapsed));
    }, 1000);
    return () => clearInterval(tick);
  }, [state, device.expires_in_secs]);

  // Poll loop. We use the spec-provided interval (usually 5s) — Trakt
  // 429s if we poll faster. Stop when: connected, code invalid, or
  // expires_in elapses.
  useEffect(() => {
    let cancelled = false;
    const start = Date.now();
    const loop = async () => {
      while (!cancelled) {
        const elapsed = Math.floor((Date.now() - start) / 1000);
        setRemainingSecs(Math.max(0, device.expires_in_secs - elapsed));
        if (elapsed >= device.expires_in_secs) {
          setState('error');
          setMessage('Code expired. Cancel and try again.');
          return;
        }
        try {
          const res = await pollDevice({ body: { device_code: device.device_code } });
          const reply = res.data as PollReply | undefined;
          if (reply?.state === 'connected') {
            setState('done');
            setMessage(`Connected as ${reply.username || 'Trakt user'}`);
            setTimeout(() => onDoneRef.current(), 750);
            return;
          }
          if (reply?.state === 'invalid') {
            setState('error');
            setMessage(reply.reason || 'Code no longer valid');
            return;
          }
        } catch (e) {
          tracing(e);
        }
        await new Promise((r) => setTimeout(r, device.interval_secs * 1000));
      }
    };
    void loop();
    return () => {
      cancelled = true;
    };
  }, [device.device_code, device.interval_secs, device.expires_in_secs]);

  return createPortal(
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onCancel();
      }}
      role="dialog"
      aria-modal="true"
    >
      <div className="bg-[var(--bg-secondary)] rounded-xl p-6 max-w-sm w-full ring-1 ring-white/10 shadow-2xl">
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-base font-semibold text-white">Connect Trakt</h3>
          <button
            type="button"
            onClick={onCancel}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-white"
            aria-label="Cancel"
          >
            <X size={16} />
          </button>
        </div>

        {state === 'pending' && (
          <>
            <p className="text-sm text-[var(--text-secondary)] mb-4">
              Open this URL on any device and enter the code:
            </p>
            <a
              href={device.verification_url}
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1.5 text-sm text-[var(--accent)] hover:underline mb-4"
            >
              {device.verification_url}
              <ExternalLink size={12} />
            </a>
            {/* Whole card is click-to-copy so a misfire on the number
                itself still works — the big target matches the visual
                prominence of the code. */}
            <button
              type="button"
              onClick={() => void copyCode()}
              title={codeCopied ? 'Copied' : 'Click to copy'}
              className={cn(
                'w-full bg-[var(--bg-card)] rounded-lg p-4 ring-1 ring-white/10 text-center mb-4 transition hover:ring-white/20',
                codeCopied && 'ring-emerald-500/40'
              )}
            >
              <div className="text-[11px] uppercase tracking-wider text-[var(--text-muted)] mb-1 flex items-center justify-center gap-1.5">
                Your code
                {codeCopied ? <Check size={11} className="text-emerald-400" /> : <Copy size={11} />}
              </div>
              <div className="text-3xl font-mono font-bold text-white tracking-wider">
                {device.user_code}
              </div>
            </button>
            <div className="text-xs text-[var(--text-muted)] text-center">
              Waiting for approval… expires in {Math.floor(remainingSecs / 60)}:
              {String(remainingSecs % 60).padStart(2, '0')}
            </div>
          </>
        )}

        {state === 'done' && (
          <div className="flex flex-col items-center gap-3 py-3 text-center">
            <CheckCircle2 size={32} className="text-emerald-400" />
            <p className="text-sm text-white">{message}</p>
          </div>
        )}

        {state === 'error' && (
          <div className="py-3 text-center">
            <p className="text-sm text-red-400 mb-3">{message}</p>
            <button
              type="button"
              onClick={onCancel}
              className="px-4 py-2 rounded-lg bg-white/10 hover:bg-white/15 text-sm text-white transition"
            >
              Close
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body
  );
}

// Tiny logger wrapper — keeps the lint clean about `unknown` catches.
function tracing(e: unknown) {
  if (typeof window !== 'undefined') {
    // eslint-disable-next-line no-console
    console.debug('[trakt] poll error:', e);
  }
}

/** Preview modal — shows dry-run counts so the user knows what
 *  "Import" will do before committing. Mirrors the spec's first-
 *  connect UX. */
function ImportPreviewModal({
  counts,
  busy,
  onImport,
  onSkip,
}: {
  counts: DryRunCounts | null;
  busy: boolean;
  onImport: () => void;
  onSkip: () => void;
}) {
  return createPortal(
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onSkip();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onSkip();
      }}
      role="dialog"
      aria-modal="true"
    >
      <div className="bg-[var(--bg-secondary)] rounded-xl p-6 max-w-md w-full ring-1 ring-white/10 shadow-2xl">
        <h3 className="text-base font-semibold text-white mb-3">Import from Trakt</h3>
        {!counts ? (
          <div className="flex items-center gap-3 text-sm text-[var(--text-muted)] py-6">
            <RefreshCw size={14} className="animate-spin" />
            Counting items…
          </div>
        ) : (
          <>
            <p className="text-sm text-[var(--text-secondary)] mb-4">
              This will apply the following to your local library. Nothing gets removed; items
              already at their target state stay as-is.
            </p>
            <div className="bg-[var(--bg-card)] rounded-lg ring-1 ring-white/10 p-3 space-y-1.5 text-sm mb-4">
              <CountRow label="Movies to mark watched" value={counts.watched_movies} />
              <CountRow label="Episodes to mark watched" value={counts.watched_episodes} />
              <CountRow label="Movie ratings" value={counts.rated_movies} />
              <CountRow label="Show ratings" value={counts.rated_shows} />
              <CountRow label="Watchlist movies" value={counts.watchlist_movies} />
              <CountRow label="Watchlist shows" value={counts.watchlist_shows} />
              {counts.unmatched > 0 && (
                <CountRow label="Items not in your kino library" value={counts.unmatched} muted />
              )}
            </div>
          </>
        )}
        <div className="flex items-center justify-end gap-3">
          <button
            type="button"
            onClick={onSkip}
            disabled={busy}
            className="px-4 py-2 rounded-lg bg-white/10 hover:bg-white/15 text-sm text-white transition disabled:opacity-50"
          >
            Skip for now
          </button>
          <button
            type="button"
            onClick={onImport}
            disabled={busy || !counts}
            className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-sm font-semibold text-white transition disabled:opacity-50"
          >
            {busy ? 'Importing…' : 'Import'}
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}

function CountRow({ label, value, muted }: { label: string; value: number; muted?: boolean }) {
  return (
    <div className="flex items-baseline justify-between">
      <span className={muted ? 'text-[var(--text-muted)]' : 'text-[var(--text-secondary)]'}>
        {label}
      </span>
      <span
        className={cn(
          'tabular-nums font-semibold',
          value > 0 && !muted ? 'text-white' : 'text-[var(--text-muted)]'
        )}
      >
        {value}
      </span>
    </div>
  );
}

// ── Setup guide ──────────────────────────────────────────────────

/** Exact string Trakt wants for the device-code flow. Kept as a
 *  constant since it's quoted in both the guide text and the copy
 *  button — drift would silently break setup. */
const DEVICE_REDIRECT_URI = 'urn:ietf:wg:oauth:2.0:oob';

/** Suggested description the user can paste into Trakt's form. Made
 *  clearly "this is *your* install" so the Trakt team can contact
 *  the right person if there's ever an issue. */
const DEFAULT_APP_DESCRIPTION =
  'Personal kino install — syncs watch history, ratings, and watchlist between my self-hosted media server and Trakt.';

/** Pressable value chip that copies its content to the clipboard on
 *  click and briefly shows a checkmark. Matches the existing
 *  settings-chip styling so it slots in with FormField rows. */
function CopyChip({ value, label }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      // Clipboard API denied (http://, iframe policy) — fall back to
      // selecting the text so the user can Ctrl+C. We don't error:
      // the chip already shows the value inline.
    }
  };
  return (
    <button
      type="button"
      onClick={() => void copy()}
      title={copied ? 'Copied' : 'Click to copy'}
      className={cn(
        // `text-left` + `break-words` keeps long values (the
        // description paragraph) legible instead of ellipsising
        // them — the user needs to see exactly what they're
        // copying. Short values (names, redirect URI) stay on
        // one line naturally.
        'inline-flex items-start gap-1.5 px-2 py-1 rounded bg-[var(--bg-elevated)] ring-1 ring-white/10 text-xs font-mono text-white text-left hover:ring-white/20 transition',
        copied && 'ring-emerald-500/40'
      )}
    >
      <span className="break-words min-w-0">{value}</span>
      {copied ? (
        <Check size={12} className="text-emerald-400 shrink-0 mt-0.5" />
      ) : (
        <Copy size={12} className="text-[var(--text-muted)] shrink-0 mt-0.5" />
      )}
      {label && <span className="sr-only">{label}</span>}
    </button>
  );
}

/** Single numbered step in the setup guide. `children` renders under
 *  the heading so callers can mix prose + chips + buttons. */
function Step({ n, title, children }: { n: number; title: string; children?: React.ReactNode }) {
  return (
    <li className="flex gap-3">
      <span className="shrink-0 w-6 h-6 rounded-full bg-[var(--bg-elevated)] ring-1 ring-white/10 grid place-items-center text-[11px] font-semibold text-[var(--text-secondary)]">
        {n}
      </span>
      <div className="flex-1 min-w-0 pt-0.5 pb-1 space-y-1.5">
        <p className="text-sm text-white">{title}</p>
        {children}
      </div>
    </li>
  );
}

/** Step-by-step setup guide, shown while the user hasn't saved
 *  credentials yet. Collapsible so it doesn't crowd the page once
 *  the user knows the drill. Values the user needs to paste into
 *  Trakt's form are rendered as copy-chips. */
function TraktSetupGuide() {
  const [open, setOpen] = useState(true);
  return (
    <div className="rounded-lg bg-[var(--bg-card)] ring-1 ring-white/10 overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center justify-between px-4 py-3 hover:bg-white/[0.02] transition"
        aria-expanded={open}
      >
        <span className="text-sm font-medium text-white">How to set up your Trakt app</span>
        {open ? (
          <ChevronUp size={16} className="text-[var(--text-muted)]" />
        ) : (
          <ChevronDown size={16} className="text-[var(--text-muted)]" />
        )}
      </button>
      {open && (
        <ol className="px-4 pb-4 space-y-3 border-t border-white/5 pt-4">
          <Step n={1} title="Open Trakt's app creation page.">
            <a
              href="https://trakt.tv/oauth/applications/new"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-xs font-semibold text-white transition"
            >
              <ExternalLink size={11} />
              Open trakt.tv/oauth/applications/new
            </a>
            <p className="text-xs text-[var(--text-muted)]">
              Free Trakt account required. The app is only visible to you.
            </p>
          </Step>

          <Step n={2} title="Fill the form exactly like this:">
            <dl className="text-xs space-y-2 bg-[var(--bg-elevated)]/50 rounded-md p-3 ring-1 ring-white/5">
              <div className="flex items-baseline gap-3">
                <dt className="w-28 shrink-0 text-[var(--text-muted)]">Name</dt>
                <dd>
                  <CopyChip value="Kino" />
                </dd>
              </div>
              <div className="flex items-baseline gap-3">
                <dt className="w-28 shrink-0 text-[var(--text-muted)]">Description</dt>
                <dd className="flex-1 min-w-0">
                  <CopyChip value={DEFAULT_APP_DESCRIPTION} label="Copy description" />
                </dd>
              </div>
              <div className="flex items-baseline gap-3">
                <dt className="w-28 shrink-0 text-[var(--text-muted)]">Redirect URI</dt>
                <dd>
                  <CopyChip value={DEVICE_REDIRECT_URI} label="Copy redirect URI" />
                </dd>
              </div>
              <div className="flex items-baseline gap-3">
                <dt className="w-28 shrink-0 text-[var(--text-muted)]">CORS origins</dt>
                <dd className="text-[var(--text-muted)]">
                  Leave blank — kino doesn&apos;t use browser OAuth.
                </dd>
              </div>
              <div className="flex items-baseline gap-3">
                <dt className="w-28 shrink-0 text-[var(--text-muted)]">Permissions</dt>
                <dd className="text-white">
                  Check{' '}
                  <code className="text-[11px] bg-[var(--bg-elevated)] px-1 py-0.5 rounded">
                    /scrobble
                  </code>{' '}
                  and{' '}
                  <code className="text-[11px] bg-[var(--bg-elevated)] px-1 py-0.5 rounded">
                    /checkin
                  </code>
                  .
                </dd>
              </div>
            </dl>
          </Step>

          <Step n={3} title="Optional: use the kino icon as your app icon.">
            <p className="text-xs text-[var(--text-muted)]">
              Trakt wants a square PNG of at least 256×256. Download this and upload it in the{' '}
              <em>Icon</em> field of Trakt&apos;s form.
            </p>
            <div className="flex items-center gap-3">
              {/* Preview of the icon they're about to download so
                  they can confirm it's the one they want before
                  saving it to disk. 64×64 render — big enough to
                  recognise, small enough not to dominate the step. */}
              <img
                src="/kino-app-icon-512.png"
                alt="kino app icon"
                width={64}
                height={64}
                className="shrink-0 rounded-lg ring-1 ring-white/10 bg-black/30"
              />
              <a
                href="/kino-app-icon-512.png"
                download="kino-icon-512.png"
                className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded bg-white/10 hover:bg-white/15 text-xs text-white transition"
              >
                <Download size={11} />
                Download kino icon (512×512 PNG)
              </a>
            </div>
          </Step>

          <Step n={4} title="Save the app, then copy the Client ID + Client Secret back here.">
            <p className="text-xs text-[var(--text-muted)]">
              Trakt will show them right after you click <em>Save App</em>. Paste them into the two
              fields below, then click <em>Save</em> in the bottom bar. After that, the{' '}
              <em>Connect Trakt</em> button lights up.
            </p>
          </Step>
        </ol>
      )}
    </div>
  );
}

// ── Main page ────────────────────────────────────────────────────

export function IntegrationsSettings() {
  const { config, updateField, hasChanges } = useSettingsContext();
  const qc = useQueryClient();

  const { data: status } = useQuery<TraktStatus | null>({
    queryKey: ['kino', 'integrations', 'trakt', 'status'],
    queryFn: async () => {
      const res = await traktStatusFn();
      return (res.data as TraktStatus | undefined) ?? null;
    },
    // No polling — meta-driven invalidation on trakt_connected /
    // disconnected / synced events.
    meta: {
      invalidatedBy: ['trakt_connected', 'trakt_disconnected', 'trakt_synced'],
    },
  });

  const [device, setDevice] = useState<BeginReply | null>(null);
  const [showImport, setShowImport] = useState(false);
  const [dryCounts, setDryCounts] = useState<DryRunCounts | null>(null);

  // Preferences/home isn't tagged — it's a pure UI cache, not
  // event-driven. Settings flows that also want the home layout
  // re-read invalidate it explicitly.
  const invalidateHomePrefs = () => {
    qc.invalidateQueries({ queryKey: ['kino', 'preferences', 'home'] });
  };

  const beginMutation = useMutation({
    mutationFn: async () => {
      const res = await beginDevice();
      return res.data as BeginReply;
    },
    onSuccess: (data) => setDevice(data),
  });

  /**
   * Save credentials AND start the device flow in one click. Awaits
   * the config write directly (instead of going through `save()`
   * from useConfigEditor, which is fire-and-forget) so the backend
   * definitely sees the new credentials by the time we call
   * `beginDevice` — previously we resolved before the save actually
   * committed and got a 400 NotConfigured on the connect call.
   *
   * After success, invalidates the shared config cache so the
   * settings-layout save-bar clears and the Integrations status
   * query re-reads.
   */
  const saveAndConnectMutation = useMutation({
    mutationFn: async () => {
      const patch: ConfigUpdate = {
        trakt_client_id: String(config.trakt_client_id ?? ''),
        trakt_client_secret: String(config.trakt_client_secret ?? ''),
      };
      await updateConfig({ body: patch });
      // `ConfigChanged` event refreshes CONFIG_KEY + status via meta;
      // no explicit invalidation needed before the beginDevice call.
      const res = await beginDevice();
      return res.data as BeginReply;
    },
    onSuccess: (data) => {
      if (data) setDevice(data);
    },
  });

  const disconnectMutation = useMutation({
    mutationFn: async () => {
      await traktDisconnect();
    },
    onSuccess: invalidateHomePrefs,
  });

  const importMutation = useMutation({
    mutationFn: async () => {
      await traktImport();
    },
    onSuccess: () => {
      setShowImport(false);
      setDryCounts(null);
      invalidateHomePrefs();
    },
  });

  const syncMutation = useMutation({
    mutationFn: async () => {
      await syncNow();
    },
    onSuccess: invalidateHomePrefs,
  });

  const openImportFlow = async () => {
    setShowImport(true);
    setDryCounts(null);
    try {
      const res = await dryRun();
      setDryCounts((res.data as DryRunCounts | undefined) ?? null);
    } catch {
      // Show the modal with zero counts so the user can cancel.
      setDryCounts({
        watched_movies: 0,
        watched_episodes: 0,
        rated_movies: 0,
        rated_shows: 0,
        rated_episodes: 0,
        watchlist_movies: 0,
        watchlist_shows: 0,
        unmatched: 0,
      });
    }
  };

  const connected = status?.connected ?? false;
  const configured = status?.configured ?? false;

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">Integrations</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">External services kino syncs with</p>

      <section className="space-y-4 border-b border-white/5 pb-6 mb-6">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
            Trakt
          </h2>
          <StatusPill connected={connected} configured={configured} />
        </div>
        <p className="text-xs text-[var(--text-muted)]">
          Automatic scrobbling, watch history, ratings and watchlist sync. Requires a free Trakt
          account and a personal API app.
        </p>

        {/* Guided setup walk-through — only shown until the user has
            saved credentials, collapsed by default once they have.
            Kino doesn't ship a shared client_id; each install uses
            its own so one app's rate-limit / revocation doesn't
            affect everyone. */}
        {!configured && <TraktSetupGuide />}

        <div className="space-y-1">
          <FormField
            label="Client ID"
            description="From your Trakt app on trakt.tv/oauth/applications"
          >
            <SecretInput
              value={String(config.trakt_client_id ?? '')}
              onChange={(v) => updateField('trakt_client_id', v)}
              placeholder="Enter Trakt API client ID"
            />
          </FormField>
          <FormField label="Client Secret">
            <SecretInput
              value={String(config.trakt_client_secret ?? '')}
              onChange={(v) => updateField('trakt_client_secret', v)}
              placeholder="Enter Trakt API client secret"
            />
          </FormField>
          {configured && (
            <div className="flex items-center gap-3 mt-2 ml-0 sm:ml-48">
              <a
                href="https://trakt.tv/oauth/applications"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1 text-xs text-[var(--accent)] hover:underline"
              >
                <ExternalLink size={11} />
                Manage your app on Trakt
              </a>
            </div>
          )}
        </div>

        {/* Connection state + actions. Three form-states to cover:
            (a) form empty                → disabled "Connect Trakt"
            (b) form has creds, unsaved   → "Save & Connect" that
                commits the pending config then begins the device flow
                (saves the user a trip to the bottom save bar)
            (c) saved but not connected  → "Connect Trakt"
            (d) connected                → disconnect/import/sync block
            */}
        <div className="bg-[var(--bg-card)] rounded-lg ring-1 ring-white/10 p-4">
          {(() => {
            if (connected) return null;
            const formHasCreds =
              String(config.trakt_client_id ?? '').trim().length > 0 &&
              String(config.trakt_client_secret ?? '').trim().length > 0;
            const credsChanged =
              hasChanges &&
              ('trakt_client_id' in (config ?? {}) || 'trakt_client_secret' in (config ?? {}));
            const needsSave = formHasCreds && !configured;
            return (
              <div>
                <p className="text-sm text-white">
                  {formHasCreds
                    ? configured
                      ? 'Ready to connect.'
                      : 'Credentials entered. Click below to save them and start the Trakt flow in one step — the save bar at the bottom is for the same save, you only need one or the other.'
                    : 'Enter your API credentials above to get started.'}
                </p>
                <div className="mt-3 flex items-center gap-3">
                  {needsSave ? (
                    <button
                      type="button"
                      onClick={() => saveAndConnectMutation.mutate()}
                      disabled={saveAndConnectMutation.isPending}
                      className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-sm font-semibold text-white transition disabled:opacity-50"
                    >
                      {saveAndConnectMutation.isPending
                        ? 'Saving & starting…'
                        : 'Save & Connect Trakt'}
                    </button>
                  ) : (
                    <button
                      type="button"
                      onClick={() => beginMutation.mutate()}
                      disabled={!configured || beginMutation.isPending}
                      className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-sm font-semibold text-white transition disabled:opacity-50"
                    >
                      {beginMutation.isPending ? 'Starting…' : 'Connect Trakt'}
                    </button>
                  )}
                  {(beginMutation.isError || saveAndConnectMutation.isError) && (
                    <span className="text-xs text-red-400">
                      Couldn&apos;t start — double-check your credentials.
                    </span>
                  )}
                  {credsChanged && configured && (
                    <span className="text-xs text-[var(--text-muted)]">
                      You have unsaved credential edits.
                    </span>
                  )}
                </div>
              </div>
            );
          })()}
          {connected && (
            <div className="space-y-3">
              <div className="flex items-baseline justify-between gap-4">
                <div>
                  <p className="text-sm text-white">
                    Connected as{' '}
                    <span className="font-semibold">
                      {status?.username || status?.slug || 'Trakt user'}
                    </span>
                  </p>
                  <p className="text-xs text-[var(--text-muted)] mt-0.5">
                    Since {formatWhen(status?.connected_at)} · last sync{' '}
                    {formatWhen(status?.last_incremental_sync_at ?? status?.last_full_sync_at)}
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => disconnectMutation.mutate()}
                  className="text-xs text-[var(--text-muted)] hover:text-red-400 transition"
                >
                  Disconnect
                </button>
              </div>

              <div className="flex items-center gap-2">
                {!status?.initial_import_done && (
                  <button
                    type="button"
                    onClick={() => void openImportFlow()}
                    className="px-3 py-1.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-xs font-semibold text-white transition"
                  >
                    Import my Trakt data
                  </button>
                )}
                <button
                  type="button"
                  onClick={() => syncMutation.mutate()}
                  disabled={syncMutation.isPending}
                  className="px-3 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-xs text-white transition disabled:opacity-50 flex items-center gap-1.5"
                >
                  <RefreshCw
                    size={11}
                    className={syncMutation.isPending ? 'animate-spin' : undefined}
                  />
                  Sync now
                </button>
              </div>
            </div>
          )}
        </div>

        {/* Per-feature toggles — only relevant once connected. Gate
            them visually so the page isn't cluttered while
            disconnected. */}
        {connected && (
          <div>
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--text-muted)] mb-1">
              What to sync
            </h3>
            <div className="divide-y divide-white/5">
              <Toggle
                label="Scrobble playback"
                description="Tell Trakt what you're watching in real time."
                checked={Boolean(config.trakt_scrobble ?? true)}
                onChange={(v) => updateField('trakt_scrobble', v)}
              />
              <Toggle
                label="Watch history"
                description="Sync watched movies and episodes both ways."
                checked={Boolean(config.trakt_sync_watched ?? true)}
                onChange={(v) => updateField('trakt_sync_watched', v)}
              />
              <Toggle
                label="Ratings"
                description="Sync 1–10 ratings both ways."
                checked={Boolean(config.trakt_sync_ratings ?? true)}
                onChange={(v) => updateField('trakt_sync_ratings', v)}
              />
              <Toggle
                label="Watchlist"
                description="Mark kino library items as monitored when they appear in your Trakt watchlist. Only affects items already in your library."
                checked={Boolean(config.trakt_sync_watchlist ?? true)}
                onChange={(v) => updateField('trakt_sync_watchlist', v)}
              />
              <Toggle
                label="Collection"
                description="Add locally-imported files to your Trakt collection."
                checked={Boolean(config.trakt_sync_collection ?? true)}
                onChange={(v) => updateField('trakt_sync_collection', v)}
              />
            </div>
          </div>
        )}
      </section>

      <section className="space-y-4">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
            MDBList
          </h2>
          <span
            className={cn(
              'inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[10px] font-semibold uppercase tracking-wider',
              String(config.mdblist_api_key ?? '').trim().length > 0
                ? 'bg-emerald-500/10 text-emerald-400 ring-1 ring-emerald-500/20'
                : 'bg-white/5 text-[var(--text-muted)] ring-1 ring-white/5'
            )}
          >
            {String(config.mdblist_api_key ?? '').trim().length > 0 ? 'Configured' : 'Optional'}
          </span>
        </div>
        <p className="text-xs text-[var(--text-muted)]">
          Required only if you want to follow MDBList lists from{' '}
          <a href="/lists" className="text-[var(--text-secondary)] underline hover:text-white">
            /lists
          </a>
          . TMDB and Trakt list sources don't need this — they reuse the TMDB key and your Trakt
          connection respectively.
        </p>
        <FormField label="MDBList API key" help="Get a free key at mdblist.com/preferences.">
          <SecretInput
            value={String(config.mdblist_api_key ?? '')}
            onChange={(v) => updateField('mdblist_api_key', v)}
            placeholder="xxxxxxxxxxxxxxxxxxxxxxxxxx"
          />
        </FormField>
      </section>

      {device && (
        <DeviceCodeModal
          device={device}
          onCancel={() => setDevice(null)}
          onDone={() => {
            setDevice(null);
            invalidateHomePrefs();
          }}
        />
      )}

      {showImport && (
        <ImportPreviewModal
          counts={dryCounts}
          busy={importMutation.isPending}
          onImport={() => importMutation.mutate()}
          onSkip={() => {
            setShowImport(false);
            setDryCounts(null);
          }}
        />
      )}
    </div>
  );
}

function StatusPill({ connected, configured }: { connected: boolean; configured: boolean }) {
  if (connected) {
    return (
      <span className="inline-flex items-center gap-1.5 text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300">
        <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
        Connected
      </span>
    );
  }
  if (configured) {
    return (
      <span className="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-white/10 text-[var(--text-muted)]">
        Ready
      </span>
    );
  }
  return (
    <span className="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-white/5 text-[var(--text-muted)]">
      Not configured
    </span>
  );
}
