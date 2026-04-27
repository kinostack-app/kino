/**
 * Top-of-tree auth gate. Decides what to render before the router
 * gets a chance to mount any data-fetching component:
 *
 *   1. Bootstrap pending      → minimal full-screen spinner
 *   2. Setup not complete     → setup wizard handles its own auth
 *      (the wizard endpoints are public so the user can configure
 *      `api_key` from a fresh install).
 *   3. Setup done, no session → paste-key screen
 *   4. Session active         → render the app
 *
 * Without this gate, the router would mount and every route's
 * `useQuery` would fire 401s before the user has any way to
 * authenticate.
 */

import { type ReactNode, useEffect, useState } from 'react';
import { redeem } from '@/api/generated/sdk.gen';
import { useAuthBootstrap } from '@/hooks/useAuthBootstrap';
import { useAuthStore } from '@/state/auth';
import { PasteKeyScreen } from './PasteKeyScreen';

/**
 * Strip and auto-redeem a `?pair=<token>` query param. Used by the
 * QR-code device-pairing flow: the originating device renders a
 * URL like `https://kino.example/?pair=…`, the receiving device
 * scans + opens it, and we redeem the token before falling into the
 * normal bootstrap path.
 *
 * Returns true while a redemption is in flight so the gate can show
 * its loading spinner instead of the paste-key screen.
 */
function usePairTokenRedeem(): { pending: boolean } {
  const [pending, setPending] = useState(() => {
    if (typeof window === 'undefined') return false;
    return new URLSearchParams(window.location.search).has('pair');
  });

  useEffect(() => {
    if (!pending) return;
    const params = new URLSearchParams(window.location.search);
    const token = params.get('pair');
    if (!token) {
      setPending(false);
      return;
    }
    void (async () => {
      try {
        await redeem({ body: { token, label: navigator.userAgent || 'Paired device' } });
      } catch {
        // Fall through — paste-key screen will show with the
        // existing error toast pattern instead.
      } finally {
        params.delete('pair');
        const qs = params.toString();
        const newUrl = window.location.pathname + (qs ? `?${qs}` : '') + window.location.hash;
        window.history.replaceState({}, '', newUrl);
        // Hard reload so cookie + bootstrap re-resolve cleanly.
        window.location.reload();
      }
    })();
  }, [pending]);

  return { pending };
}

export function AuthGate({ children }: { children: ReactNode }) {
  const pair = usePairTokenRedeem();
  const { loading, error } = useAuthBootstrap();
  const sessionActive = useAuthStore((s) => s.sessionActive);
  const setupComplete = useAuthStore((s) => s.setupComplete);

  if (loading || pair.pending) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-[var(--bg-primary)]">
        {/* biome-ignore lint/a11y/useSemanticElements: role="status" on a div is the documented a11y pattern for live-region announcements; <output> would change focus/layout semantics and isn't a one-to-one swap */}
        <div
          role="status"
          aria-label="Loading"
          className="w-6 h-6 rounded-full border-2 border-white/20 border-t-white motion-safe:animate-spin"
        />
      </div>
    );
  }

  if (error) {
    return (
      <div className="min-h-screen flex flex-col items-center justify-center gap-3 px-6 text-center bg-[var(--bg-primary)] text-[var(--text-primary)]">
        <h1 className="text-xl font-semibold">Couldn&apos;t reach kino</h1>
        <p className="text-sm text-[var(--text-secondary)] max-w-sm">
          {error.message}. Check that the backend is running and reachable from this device.
        </p>
        <button
          type="button"
          onClick={() => window.location.reload()}
          className="mt-2 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold"
        >
          Retry
        </button>
      </div>
    );
  }

  // Setup wizard takes over when there's no `api_key` configured
  // yet. It's reached as a top-level route already; we just render
  // the children (router) and the wizard renders itself based on
  // `/status`.
  if (!setupComplete) {
    return <>{children}</>;
  }

  if (!sessionActive) {
    return <PasteKeyScreen />;
  }

  return <>{children}</>;
}
