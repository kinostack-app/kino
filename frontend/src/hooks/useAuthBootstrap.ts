/**
 * App-mount auth bootstrap.
 *
 * Calls `GET /api/v1/bootstrap` to learn whether (a) the backend
 * has been set up at all and (b) this browser already has a valid
 * session cookie. Picks cookie vs bearer mode based on whether the
 * SPA origin matches the backend's. Result is stashed in the auth
 * store so the rest of the app can branch on it.
 *
 * Used by `<AuthGate>` to decide what to render at the top of the
 * tree: setup wizard, paste-key screen, or the actual app.
 */

import { useEffect, useState } from 'react';
import { bootstrap } from '@/api/generated/sdk.gen';
import { type AuthMode, useAuthStore } from '@/state/auth';

export function useAuthBootstrap() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);
  const setBootstrap = useAuthStore((s) => s.setBootstrap);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const res = await bootstrap();
        if (cancelled) return;
        const data = res.data;
        if (!data) throw new Error('empty bootstrap response');
        // Same-origin → cookies work. Different origin → bearer mode.
        // We approximate "same-origin" by checking whether the
        // bootstrap fetch went out as a same-origin request — when
        // the SDK has `baseUrl: ''`, the request lands on the SPA's
        // own origin, which is the cookie-mode case.
        const mode: AuthMode = sameOriginDeploy() ? 'cookie' : 'bearer';
        setBootstrap({
          mode,
          sessionActive: data.session_active,
          setupComplete: data.setup_complete,
        });
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof Error ? e : new Error(String(e)));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [setBootstrap]);

  return { loading, error };
}

/** Heuristic: when the SDK's baseUrl is empty, requests go to the
 *  SPA's own origin — that's the cookie-mode case. A non-empty
 *  baseUrl (set at build time for cross-origin deploys) tips us
 *  into bearer mode. */
function sameOriginDeploy(): boolean {
  // Vite's `import.meta.env.VITE_KINO_API_BASE` is the build-time
  // override for cross-origin deploys. Unset → cookie mode.
  return !import.meta.env.VITE_KINO_API_BASE;
}
