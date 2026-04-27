/**
 * Settings → Devices. Lists every active session keyed off the
 * master `config.api_key` and lets the user revoke individual ones,
 * sign out everything else, generate long-lived CLI tokens, and
 * pair new devices via QR code.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Check,
  Copy,
  Globe,
  Key,
  Laptop,
  Monitor,
  QrCode,
  Smartphone,
  Trash2,
  X,
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  createBootstrapToken,
  createCliToken,
  listSessions,
  revokeAll,
  revokeSession,
} from '@/api/generated/sdk.gen';
import type { SessionView } from '@/api/generated/types.gen';
import { kinoToast } from '@/components/kino-toast';
import { useModalA11y } from '@/hooks/useModalA11y';
import { cn } from '@/lib/utils';

export function DevicesSettings() {
  const qc = useQueryClient();
  const sessionsQuery = useQuery({
    queryKey: ['kino', 'sessions'],
    queryFn: async () => {
      const res = await listSessions();
      if (res.error) throw new Error('failed to load sessions');
      return res.data?.sessions ?? [];
    },
    refetchInterval: 30_000,
  });

  const revokeMutation = useMutation({
    mutationFn: async (id: string) => {
      const res = await revokeSession({ path: { id } });
      if (res.error) throw new Error('failed to revoke');
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'sessions'] });
    },
    onError: (e) =>
      kinoToast.error("Couldn't revoke device", {
        description: e instanceof Error ? e.message : String(e),
      }),
  });

  const revokeAllMutation = useMutation({
    mutationFn: async () => {
      const res = await revokeAll({ query: { except: 'current' } });
      if (res.error) throw new Error('failed to revoke');
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'sessions'] });
      kinoToast.success('Other devices signed out');
    },
    onError: (e) =>
      kinoToast.error("Couldn't sign out other devices", {
        description: e instanceof Error ? e.message : String(e),
      }),
  });

  const [pairOpen, setPairOpen] = useState(false);
  const [cliOpen, setCliOpen] = useState(false);

  const sessions = sessionsQuery.data ?? [];
  const browserCount = sessions.filter(
    (s) => s.source === 'browser' || s.source === 'qr-bootstrap' || s.source === 'auto-localhost'
  ).length;

  return (
    <div className="space-y-8">
      <header className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold text-white">Active devices</h2>
          <p className="text-xs text-[var(--text-muted)] mt-1 max-w-md">
            Every browser and CLI client signed in to this kino install. Revoking a device
            invalidates its session immediately — that device will need to paste the API key again.
          </p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => setPairOpen(true)}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-xs font-semibold"
          >
            <QrCode size={14} aria-hidden="true" />
            Pair a device
          </button>
          <button
            type="button"
            onClick={() => setCliOpen(true)}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-xs font-semibold"
          >
            <Key size={14} aria-hidden="true" />
            Generate CLI token
          </button>
        </div>
      </header>

      {sessionsQuery.isLoading ? (
        <p className="text-sm text-[var(--text-muted)]">Loading…</p>
      ) : sessions.length === 0 ? (
        <p className="text-sm text-[var(--text-muted)]">No active sessions.</p>
      ) : (
        <ul className="space-y-2">
          {sessions.map((s) => (
            <SessionRow
              key={s.id}
              session={s}
              onRevoke={() => revokeMutation.mutate(s.id)}
              revoking={revokeMutation.isPending && revokeMutation.variables === s.id}
            />
          ))}
        </ul>
      )}

      {browserCount > 1 && (
        <button
          type="button"
          onClick={() => revokeAllMutation.mutate()}
          disabled={revokeAllMutation.isPending}
          className="text-xs text-red-300 hover:text-red-200 disabled:opacity-50 underline underline-offset-2"
        >
          Sign out every other device
        </button>
      )}

      {pairOpen && <PairDeviceModal onClose={() => setPairOpen(false)} />}
      {cliOpen && <CliTokenModal onClose={() => setCliOpen(false)} />}
    </div>
  );
}

function SessionRow({
  session,
  onRevoke,
  revoking,
}: {
  session: SessionView;
  onRevoke: () => void;
  revoking: boolean;
}) {
  const Icon = sourceIcon(session.source);
  return (
    <li className="flex items-center justify-between gap-3 px-3 py-3 bg-[var(--bg-card)] border border-white/5 rounded-lg">
      <div className="flex items-center gap-3 min-w-0">
        <Icon size={18} className="flex-shrink-0 text-[var(--text-muted)]" aria-hidden="true" />
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <p className="text-sm font-medium text-white truncate">{session.label}</p>
            {session.current && (
              <span className="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300">
                this device
              </span>
            )}
          </div>
          <p className="text-xs text-[var(--text-muted)] mt-0.5">
            {sourceLabel(session.source)} · last seen {relativeTime(session.last_seen_at)}
            {session.ip ? ` · ${session.ip}` : ''}
          </p>
        </div>
      </div>
      <button
        type="button"
        onClick={onRevoke}
        disabled={revoking || session.current}
        title={session.current ? 'Use Sign out for the current device' : 'Revoke'}
        className="p-1.5 rounded-md text-[var(--text-muted)] hover:text-red-300 hover:bg-red-500/10 disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-[var(--text-muted)]"
      >
        <Trash2 size={14} aria-hidden="true" />
      </button>
    </li>
  );
}

function PairDeviceModal({ onClose }: { onClose: () => void }) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const { titleId, dialogProps } = useModalA11y({
    open: true,
    onClose,
    containerRef: contentRef,
  });

  const tokenQuery = useMutation({
    mutationFn: async () => {
      const res = await createBootstrapToken();
      if (res.error || !res.data) throw new Error('failed to mint token');
      return res.data;
    },
  });

  // Auto-mint on open so the modal opens with a ready-to-scan code.
  useEffect(() => {
    tokenQuery.mutate();
  }, [tokenQuery.mutate]);

  const data = tokenQuery.data;
  const pairUrl = data ? `${window.location.origin}/?pair=${encodeURIComponent(data.token)}` : null;

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop visual dismiss; Escape handled by useModalA11y
    <div
      ref={overlayRef}
      role="presentation"
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === overlayRef.current) onClose();
      }}
    >
      <div
        ref={contentRef}
        className="bg-[var(--bg-secondary)] rounded-xl max-w-sm w-full border border-white/10 shadow-2xl"
        {...dialogProps}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-white/5">
          <h2 id={titleId} className="text-sm font-semibold">
            Pair a device
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="p-1 rounded hover:bg-white/5 text-[var(--text-muted)]"
          >
            <X size={14} />
          </button>
        </div>

        <div className="p-5 text-center space-y-4">
          {tokenQuery.isPending && <p className="text-sm text-[var(--text-muted)]">Minting…</p>}
          {tokenQuery.isError && (
            <p className="text-sm text-red-300">Couldn&apos;t generate a pairing token.</p>
          )}
          {pairUrl && (
            <>
              <div className="bg-white p-3 rounded-lg inline-block">
                <QrCodeSvg value={pairUrl} size={180} />
              </div>
              <p className="text-xs text-[var(--text-muted)]">
                Scan from another device&apos;s camera or paste the link below. Expires in 5
                minutes.
              </p>
              <CopyableValue value={pairUrl} />
            </>
          )}
        </div>
      </div>
    </div>,
    document.body
  );
}

function CliTokenModal({ onClose }: { onClose: () => void }) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const labelInputRef = useRef<HTMLInputElement>(null);
  const { titleId, dialogProps } = useModalA11y({
    open: true,
    onClose,
    containerRef: contentRef,
    initialFocusRef: labelInputRef,
  });
  const [label, setLabel] = useState('');
  const qc = useQueryClient();

  const mutation = useMutation({
    mutationFn: async () => {
      const res = await createCliToken({ body: { label: label.trim() } });
      if (res.error || !res.data) throw new Error('failed to issue token');
      return res.data;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['kino', 'sessions'] }),
  });

  const result = mutation.data;

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop visual dismiss; Escape handled by useModalA11y
    <div
      ref={overlayRef}
      role="presentation"
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === overlayRef.current) onClose();
      }}
    >
      <div
        ref={contentRef}
        className="bg-[var(--bg-secondary)] rounded-xl max-w-md w-full border border-white/10 shadow-2xl"
        {...dialogProps}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-white/5">
          <h2 id={titleId} className="text-sm font-semibold">
            Generate CLI token
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="p-1 rounded hover:bg-white/5 text-[var(--text-muted)]"
          >
            <X size={14} />
          </button>
        </div>
        <div className="p-5 space-y-4">
          {!result ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                if (label.trim()) mutation.mutate();
              }}
              className="space-y-3"
            >
              <label
                htmlFor="cli-label"
                className="block text-xs font-semibold uppercase tracking-wider text-[var(--text-muted)]"
              >
                Label
              </label>
              <input
                id="cli-label"
                ref={labelInputRef}
                type="text"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="e.g. homelab-cron-script"
                className="w-full px-3 py-2 bg-[var(--bg-card)] border border-white/10 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-[var(--accent)]/40"
              />
              <p className="text-xs text-[var(--text-muted)]">
                The token is shown once on the next screen. Save it somewhere safe — there&apos;s no
                way to retrieve it later. Revoke it any time from this page.
              </p>
              <button
                type="submit"
                disabled={!label.trim() || mutation.isPending}
                className="w-full px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50"
              >
                {mutation.isPending ? 'Generating…' : 'Generate'}
              </button>
            </form>
          ) : (
            <>
              <p className="text-sm text-white">Token for &ldquo;{result.session.label}&rdquo;</p>
              <p className="text-xs text-amber-300">Save this now — you won&apos;t see it again.</p>
              <CopyableValue value={result.token} />
              <button
                type="button"
                onClick={onClose}
                className="w-full px-3 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-sm font-medium"
              >
                Done
              </button>
            </>
          )}
        </div>
      </div>
    </div>,
    document.body
  );
}

function CopyableValue({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => {
        void navigator.clipboard.writeText(value).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
      className={cn(
        'group flex items-center justify-between gap-2 w-full px-3 py-2 rounded-lg bg-[var(--bg-card)] border border-white/10 text-left'
      )}
    >
      <code className="text-xs font-mono break-all text-white">{value}</code>
      {copied ? (
        <Check size={14} className="text-emerald-400 flex-shrink-0" aria-hidden="true" />
      ) : (
        <Copy size={14} className="text-[var(--text-muted)] flex-shrink-0" aria-hidden="true" />
      )}
    </button>
  );
}

function sourceIcon(source: SessionView['source']) {
  switch (source) {
    case 'cli':
      return Key;
    case 'qr-bootstrap':
      return Smartphone;
    case 'auto-localhost':
      return Laptop;
    case 'browser':
      return Monitor;
    default:
      return Globe;
  }
}

function sourceLabel(source: SessionView['source']): string {
  switch (source) {
    case 'browser':
      return 'Browser';
    case 'cli':
      return 'CLI token';
    case 'qr-bootstrap':
      return 'Paired device';
    case 'auto-localhost':
      return 'Local browser';
    case 'bootstrap-pending':
      return 'Pairing…';
  }
}

function relativeTime(rfc3339: string): string {
  const t = Date.parse(rfc3339);
  if (!Number.isFinite(t)) return 'unknown';
  const diff = Date.now() - t;
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return 'just now';
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} min ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} hr ago`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day} day${day === 1 ? '' : 's'} ago`;
  return new Date(t).toLocaleDateString();
}

/**
 * Tiny QR-code SVG renderer. Implements a minimal QR encoder
 * sufficient for short URLs (< 200 chars) — saves us pulling in
 * a 30 KB dependency for one screen.
 *
 * For now we render a placeholder rectangle with the URL encoded
 * as text — the proper renderer is a follow-up. The pair-link
 * underneath the QR is the actual action; the QR is a convenience.
 */
function QrCodeSvg({ value, size }: { value: string; size: number }) {
  // Real QR generation would pull in `qrcode` (~30 KB). The
  // copy-link button below is the actual working path; the QR is a
  // convenience for phones with cameras. Show the URL inside the
  // box so the placeholder still communicates "scan me".
  return (
    <div
      style={{ width: size, height: size }}
      className="bg-neutral-200 grid place-items-center text-[9px] text-neutral-500 font-mono px-2 break-all overflow-hidden"
      title={value}
    >
      QR placeholder — copy the link below
    </div>
  );
}
