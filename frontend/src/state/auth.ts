/**
 * Auth state — bootstrap result + auth-mode-aware media URL helper.
 *
 * Two modes the SPA can be in:
 *   - `cookie`: same-origin deploy (the default). The browser auto-
 *     attaches the `kino-session` HttpOnly cookie on every fetch /
 *     `<img>` / `<video>` / WebSocket request. URL helpers return
 *     bare paths.
 *   - `bearer`: cross-origin deploy (CF Pages → remote backend).
 *     Cookies don't cross origins; XHR sends a `Authorization:
 *     Bearer …` header instead, and media elements get short-lived
 *     HMAC-signed URLs via `POST /sessions/sign-url`.
 *
 * Mode is chosen by `useAuthBootstrap` from the `/bootstrap`
 * response: when the SPA's origin matches the backend's, cookie
 * mode wins. Cross-origin always falls back to bearer.
 */

import { create } from 'zustand';

export type AuthMode = 'cookie' | 'bearer';

interface AuthState {
  /** Resolved on app mount; null until the first `/bootstrap` lands. */
  mode: AuthMode | null;
  /** True when the backend reports a valid session for this request. */
  sessionActive: boolean;
  /** True once a master `api_key` is in `config` — setup wizard hides. */
  setupComplete: boolean;
  /** Bearer token in cross-origin mode; null in cookie mode. */
  bearerToken: string | null;
  setBootstrap(p: { mode: AuthMode; sessionActive: boolean; setupComplete: boolean }): void;
  setSessionActive(v: boolean): void;
  setBearerToken(token: string | null): void;
}

export const useAuthStore = create<AuthState>((set) => ({
  mode: null,
  sessionActive: false,
  setupComplete: false,
  bearerToken: null,
  setBootstrap: (p) =>
    set({
      mode: p.mode,
      sessionActive: p.sessionActive,
      setupComplete: p.setupComplete,
    }),
  setSessionActive: (v) => set({ sessionActive: v }),
  setBearerToken: (token) => set({ bearerToken: token }),
}));

/** Synchronous accessor for non-React callers (websocket setup,
 *  imperative URL builders). Returns `cookie` until the bootstrap
 *  response lands so first-render media URLs are bare paths. */
export function currentAuthMode(): AuthMode {
  return useAuthStore.getState().mode ?? 'cookie';
}

/**
 * Resolve a media path into a renderable URL.
 *
 * In cookie mode, returns the bare path — the browser auto-sends
 * the session cookie. In bearer mode, the caller already has a
 * pre-signed URL (or should fetch one) and should use that
 * instead. This helper exists so call sites don't have to know
 * which mode the SPA is in.
 *
 * The optional `sigUrl` argument is the response from a recent
 * `/sessions/sign-url` call; when present in bearer mode, it's
 * used verbatim.
 */
export function mediaUrl(path: string, sigUrl?: string): string {
  if (currentAuthMode() === 'bearer' && sigUrl) {
    return sigUrl;
  }
  return path;
}
