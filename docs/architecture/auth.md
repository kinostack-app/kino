# Authentication

Single-user app. The master credential is `config.api_key`.
Everything else (browser cookies, named CLI tokens, QR-paired
devices) is a derived row in the `session` table that's
independently revocable.

This system shipped 2026-04-25. This doc captures the model;
implementation lives in `auth_session/` and `auth.rs`.

## Credentials accepted

The auth middleware accepts any of:

| Credential | When | Notes |
|---|---|---|
| `Cookie: kino-session=<id>` | Browser, same-origin | The everyday path. Auto-attached by the browser on every fetch / `<img>` / `<video>` / WebSocket upgrade. |
| `Authorization: Bearer <api_key>` | CLI, scripts, integrations | The master credential or a named CLI token (which is also a session id). |
| `X-Api-Key: <api_key>` | Same as Bearer | Legacy header form; kept for compatibility with curl examples. |
| `?api_key=<api_key>` | URL fallback for elements that can't set headers | Discouraged; use cookies or signed URLs. |
| `?sig=<hmac>&exp=<epoch>` | Cross-origin `<video>` / `<img>` | HMAC-signed URL covering method+path+expiry. Issued by `/sessions/sign-url`. |
| `?cast_token=<jwt>` on `/play/{kind}/{id}/...` | Chromecast receiver | Receiver-scoped. |

Equality checks on credentials use **constant-time comparison**
(`subtle::ConstantTimeEq`) so a timing side-channel can't recover
the master key one byte at a time.

## Session model

`session` table:

```
id            TEXT PRIMARY KEY  — 32 random bytes URL-safe-base64; doubles as cookie value / bearer token
label         TEXT              — "Firefox on PopOS" / "homelab-cron-script" / etc
user_agent    TEXT              — captured at create
ip            TEXT              — captured at create
source        TEXT              — browser | cli | qr-bootstrap | bootstrap-pending | auto-localhost
created_at    TEXT
last_seen_at  TEXT              — touched on each authed request
expires_at    TEXT
consumed_at   TEXT              — set on bootstrap-pending after redemption (prevents replay)
```

Lifetimes:

- Browser sessions: **30 days** sliding-refresh-on-use
- QR-pairing tokens: **5 minutes**, single-use
- Auto-localhost sessions: **30 days** (functionally same as browser)
- CLI tokens: caller-specified, capped at **365 days**

## Auto-localhost cookies

When `/bootstrap` fires from a loopback IP (`127.0.0.0/8` or `::1`)
**AND** the `X-Forwarded-For` chain is loopback-only, the backend
issues an `auto-localhost` session automatically. No paste-key
screen. The reasoning: a same-machine browser already has
filesystem access to `config.api_key`, so requiring re-paste adds
zero security and friction-tests every dev iteration.

The X-Forwarded-For check prevents a reverse proxy that happens to
bind on localhost from gifting public-internet requests a session.

## QR-pairing flow

1. Already-signed-in device hits `POST /sessions/bootstrap-token` →
   gets a one-time token + 5-min expiry.
2. UI renders the token as a QR code containing
   `https://kino-host/?pair=<token>`.
3. Receiving device opens the URL.
4. `<AuthGate>` detects `?pair=...`, calls `POST /sessions/redeem`,
   the server marks the token consumed and issues a fresh `qr-bootstrap`
   session.
5. Frontend strips the param and reloads.

## Signed media URLs

For cross-origin deploys where cookies don't cross origins:

```
POST /sessions/sign-url { path: "/api/v1/play/movie/42/direct", ttl_secs: 900 }
→ { url: "/api/v1/play/movie/42/direct?sig=...&exp=...", expires_at: ... }
```

The signature commits to **method + path + expiry**, so a sig
issued for `GET /direct/123` cannot be replayed against `DELETE
/movies/123`. Verified constant-time.

Restricted to the routes that make sense for `<video>`/`<img>`:
`/play/*`, `/images/*`, `/stream/*`. Anything else returns 400
to prevent state-changing endpoints from being signable.

## Cookie security

- `HttpOnly` always — JavaScript can't read it (XSS-resistant).
- `SameSite=Lax` — CSRF protection without breaking cross-tab nav.
- `Secure` only when `X-Forwarded-Proto: https` (or request is HTTPS).
  Pure-`Secure` on plain HTTP would be silently dropped by browsers.

## Master credential rotation

`POST /config/rotate-api-key` rotates `config.api_key` AND wipes
every session row. A stolen cookie can't outlive the credential it
was derived from.

## Per-device visibility + revocation

Settings → Devices lists every active session. Each row shows
label, source, IP, last seen. The user can:

- Revoke individual sessions
- "Sign out everything else" (revokes all but current)
- Generate named CLI tokens
- Mint a QR-pairing token for a new device

The current session is marked with a "this device" badge and
self-revoke is disabled to prevent the user from accidentally
locking themselves out.

## What's NOT in this model

- No password auth. The API key IS the credential.
- No OAuth provider for kino itself. (Trakt OAuth is separate; that's
  Trakt authenticating us, not us authenticating users.)
- No multi-user support. Single-user by design.
- No 2FA. The threat model is "single-user self-hosted"; 2FA on top
  of an already-secret API key has minimal added benefit.

## Anti-patterns this prevents

- **Hardcoded UUIDs in the SPA.** Removed entirely. `lib/api.ts`
  configures cookie/bearer mode; no static credential anywhere.
- **API key in URL leaking to logs / browser history / referrers.**
  Same-origin uses cookies; cross-origin uses signed URLs that
  expire fast.
- **Single revocation forces relogin everywhere.** Per-device
  sessions = surgical revocation.
