/**
 * "Paste your kino API key" screen. Shown when bootstrap reports
 * setup-complete but no active session — i.e. fresh browser, new
 * device, expired cookie, cross-origin first visit.
 *
 * Posts the key to `/api/v1/sessions`, which (on success) sets the
 * `kino-session` cookie and returns the new session row. The app
 * then re-bootstraps via a hard reload — simpler than threading
 * "session just landed" state through every consumer.
 */

import { useMutation } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { createSession, redeem } from '@/api/generated/sdk.gen';
import { kinoToast } from '@/components/kino-toast';
import { setBearerToken } from '@/lib/api';
import { useAuthStore } from '@/state/auth';

export function PasteKeyScreen() {
  const [key, setKey] = useState('');
  const [tokenInput, setTokenInput] = useState('');
  const [tab, setTab] = useState<'key' | 'token'>(initialTab());
  const keyInputRef = useRef<HTMLInputElement>(null);
  const tokenInputRef = useRef<HTMLInputElement>(null);

  // Focus the active tab's input on mount + on tab switch. Avoids
  // `autoFocus` which biome flags (it's accessible-when-deliberate
  // but the lint can't tell).
  useEffect(() => {
    if (tab === 'key') keyInputRef.current?.focus();
    else tokenInputRef.current?.focus();
  }, [tab]);
  const mode = useAuthStore((s) => s.mode);
  const setSessionActive = useAuthStore((s) => s.setSessionActive);
  const setBearer = useAuthStore((s) => s.setBearerToken);

  const pasteMutation = useMutation({
    mutationFn: async (apiKey: string) => {
      const res = await createSession({
        body: { api_key: apiKey, label: navigator.userAgent || 'Browser' },
      });
      if (res.error) throw new Error(extractError(res.error));
      if (!res.data) throw new Error('empty response');
      return res.data;
    },
    onSuccess: (data) => {
      // In bearer mode the cookie isn't going to help us — we need
      // a Bearer token instead. Stash the session id (which doubles
      // as the bearer) and feed it through the SDK header.
      if (mode === 'bearer') {
        setBearerToken(data.session.id);
        setBearer(data.session.id);
      }
      setSessionActive(true);
      // Hard reload so every cached query re-runs against the new
      // auth state. Cheaper than threading "session just landed"
      // through every subscriber.
      window.location.reload();
    },
    onError: (e) => {
      kinoToast.error("Couldn't sign in", {
        description: e instanceof Error ? e.message : String(e),
      });
    },
  });

  const tokenMutation = useMutation({
    mutationFn: async (token: string) => {
      const res = await redeem({
        body: { token, label: navigator.userAgent || 'Paired device' },
      });
      if (res.error) throw new Error(extractError(res.error));
      if (!res.data) throw new Error('empty response');
      return res.data;
    },
    onSuccess: () => {
      setSessionActive(true);
      window.location.reload();
    },
    onError: (e) => {
      kinoToast.error("Couldn't pair device", {
        description: e instanceof Error ? e.message : String(e),
      });
    },
  });

  return (
    <div className="min-h-screen flex items-center justify-center bg-[var(--bg-primary)] text-[var(--text-primary)] px-6">
      <div className="max-w-sm w-full">
        <h1 className="text-2xl font-semibold mb-1">Sign in to kino</h1>
        <p className="text-sm text-[var(--text-secondary)] mb-5">
          Paste your API key from the kino install&apos;s Settings → General page, or use a
          device-pairing token from another signed-in browser.
        </p>

        <div className="flex gap-1 mb-3">
          <TabButton active={tab === 'key'} onClick={() => setTab('key')}>
            API key
          </TabButton>
          <TabButton active={tab === 'token'} onClick={() => setTab('token')}>
            Pairing token
          </TabButton>
        </div>

        {tab === 'key' ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (key.trim()) pasteMutation.mutate(key.trim());
            }}
            className="space-y-3"
          >
            <input
              ref={keyInputRef}
              type="password"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder="Paste API key"
              autoComplete="off"
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-white/10 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-[var(--accent)]/40"
            />
            <button
              type="submit"
              disabled={!key.trim() || pasteMutation.isPending}
              className="w-full px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50"
            >
              {pasteMutation.isPending ? 'Signing in…' : 'Sign in'}
            </button>
          </form>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (tokenInput.trim()) tokenMutation.mutate(tokenInput.trim());
            }}
            className="space-y-3"
          >
            <input
              ref={tokenInputRef}
              type="text"
              value={tokenInput}
              onChange={(e) => setTokenInput(e.target.value)}
              placeholder="Paste pairing token"
              autoComplete="off"
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-white/10 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-[var(--accent)]/40"
            />
            <button
              type="submit"
              disabled={!tokenInput.trim() || tokenMutation.isPending}
              className="w-full px-3 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold disabled:opacity-50"
            >
              {tokenMutation.isPending ? 'Pairing…' : 'Pair device'}
            </button>
            <p className="text-xs text-[var(--text-muted)]">
              Pairing tokens are generated from Settings → Devices on a device that&apos;s already
              signed in. They expire after 5 minutes.
            </p>
          </form>
        )}
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={
        active
          ? 'px-3 py-1.5 rounded-md text-xs font-semibold bg-white/10 text-white'
          : 'px-3 py-1.5 rounded-md text-xs font-medium text-[var(--text-muted)] hover:text-white'
      }
    >
      {children}
    </button>
  );
}

function initialTab(): 'key' | 'token' {
  // Default to "API key" — that's the path most users hit. The
  // pairing-token tab is a power-user / phone-setup affordance.
  return 'key';
}

function extractError(e: unknown): string {
  if (e && typeof e === 'object') {
    const top = e as {
      error?: { message?: string };
      message?: string;
    };
    if (top.error?.message) return top.error.message;
    if (top.message) return top.message;
  }
  return 'Sign-in failed.';
}
