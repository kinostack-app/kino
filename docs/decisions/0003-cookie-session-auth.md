# ADR-0003: Cookie sessions + signed media URLs (no passwords)

**Status:** accepted
**Date:** 2026-04-25

## Context

Pre-2026-04-25, kino's frontend hardcoded the dev API key UUID
into six source files. A production install whose backend
generated its own UUID on first boot would 401 on every request.
This was on the pre-release tracker as a BLOCKER.

The user explicitly asked: should we have logins, or just an API
key entered during setup? Single-user app — no real value to
passwords; the API key already exists and is the master credential.

## Decision

**The `config.api_key` is the canonical credential.** Everything
else is derived:

- **Cookie sessions** for browsers. Issued from `POST /sessions`
  by exchanging the master key once; cookie remembers for 30 days
  with sliding refresh. Auto-issued without paste-key on
  same-machine localhost requests (frictionless devcontainer +
  laptop story).
- **Bearer tokens** for CLI / scripts / cross-origin SPAs. Same
  underlying session row, returned as a long-lived token via
  `POST /sessions/cli`.
- **HMAC-signed URLs** for `<video>` / `<img>` / `<track>`
  elements that can't carry headers. Short-lived (15 min default).
  Issued via `POST /sessions/sign-url`.
- **QR-pairing tokens** for new devices. Already-signed-in device
  mints a one-time token; receiving device redeems via
  `?pair=<token>` URL.

Per-device session rows are individually revocable from
Settings → Devices.

## Alternatives considered

- **Templated `index.html` with `window.__KINO_API_KEY__`.** Lighter
  touch, no cookie infrastructure. But keeps API-key-in-URL for
  media elements (codex would still complain). Skipped because the
  cookie approach also closes the URL-leak concern.
- **Password authentication with login form.** Standard web app
  pattern. But: kino is single-user, the API key already exists
  and is high-entropy, password storage adds bcrypt + reset flows
  + 2FA debates. Friction and surface for zero added safety on a
  single-user self-hosted app.
- **Embedded OAuth provider for kino itself.** Vastly overkill.
- **JWT tokens, no session table.** Stateless wins concurrency;
  loses revocation. Per-device revocation is a feature we want.

## Consequences

- **Win:** zero credentials in URLs in the same-origin path.
  Cookies auto-attach; no `?api_key=` leakage to logs / browser
  history / referrers.
- **Win:** per-device visibility. User sees what's signed in,
  revokes individual devices.
- **Win:** key rotation cascades — `POST /config/rotate-api-key`
  wipes every session, so a stolen cookie can't outlive its
  parent credential.
- **Win:** zero-friction same-machine UX. Localhost requests get
  auto-cookied; devcontainer dev loop never sees a paste screen.
- **Cost:** cookie infrastructure (Set-Cookie, SameSite, Secure
  flag, X-Forwarded-Proto handling). One-time complexity.
- **Cost:** cross-origin deploys need bearer tokens + signed URLs
  rather than just cookies. Documented; the SPA detects via
  `VITE_KINO_API_BASE`.

## Supersedes

The implicit "just put the key in the URL" model that shipped
pre-2026-04-25.
