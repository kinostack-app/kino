import { client } from '@/api/generated/client.gen';

/**
 * Configure the SDK client.
 *
 * Cookie mode (the default — backend serves the SPA from the same
 * origin): `credentials: 'include'` makes the browser auto-attach
 * the `kino-session` HttpOnly cookie on every request. We set no
 * `Authorization` header at boot — handlers that don't have a
 * cookie yet (the paste-key screen, the QR-redeem screen) are
 * `is_public_path` on the backend and accept unauthenticated
 * requests by design.
 *
 * Cross-origin / Bearer mode (someone hosts the SPA on CF Pages
 * and points at a remote kino backend): the SPA exchanges the
 * master key for a Bearer token via `POST /sessions/cli` once at
 * setup, stores it in the auth store, and `setBearerToken` below
 * threads it onto every request. Cookies don't help here because
 * the backend's Set-Cookie won't be sent cross-origin without
 * SameSite=None — which we deliberately don't do.
 */
client.setConfig({
  baseUrl: '',
  credentials: 'include',
});

/** Attach a Bearer token to every subsequent request. Call with
 *  `null` to strip the header (used by logout). */
export function setBearerToken(token: string | null) {
  client.setConfig({
    baseUrl: '',
    credentials: 'include',
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
}

/** Build a TMDB image URL */
export function tmdbImage(path: string | null | undefined, size = 'w500'): string | undefined {
  if (!path) return undefined;
  return `https://image.tmdb.org/t/p/${size}${path}`;
}
