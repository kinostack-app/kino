# Security policy

## Supported versions

Kino is pre-release; security fixes land on `main`. Once we cut
`v1.0.0`, this section will document supported branches.

## Reporting a vulnerability

**Don't open a public issue for security bugs.** Instead:

1. **Open a private security advisory through GitHub** —
   <https://github.com/kinostack-app/kino/security/advisories/new>.
   Or email <security@kinostack.app>.
2. Include:
   - A description of the issue and the affected components
   - Steps to reproduce, or a proof-of-concept
   - Your assessment of impact (data exposure, RCE, denial-of-service, etc.)
   - Whether you'd like attribution in the public advisory and how

We'll acknowledge your report within **5 working days** and aim to
publish a fix within **30 days** for high-severity issues. Lower-
severity issues batch into the next release.

## What's in scope

Vulnerabilities in:

- The kino server binary and its HTTP / WebSocket API
- The release-installation flow (deb / rpm / msi / dmg / Pi image / Docker)
- Authentication, authorisation, and credential handling
- Built-in integrations: VPN, mDNS, Trakt, TMDB, Cast sender, indexers
- The desktop tray binary
- Documentation that misleads users into insecure configurations

## What's not in scope

- Vulnerabilities in third-party services kino integrates with
  (TMDB, Trakt, indexer providers, Chromecast firmware) — report
  to those projects directly. Tell us if our integration code
  exposes you to one
- Issues that require physical access to an unlocked, signed-in
  machine
- Self-DoS via configuration choices (e.g. setting an absurd
  retention policy that fills disk)
- Bugs in dependencies we can verifiably pass through (e.g. a
  reqwest CVE we don't trigger). Report to the upstream first
- Issues in the marketing site (`kinostack.app`) that don't affect
  the binary

## Trust model

See [`docs/decisions/0008-privacy-posture.md`](./docs/decisions/0008-privacy-posture.md)
and [`docs/decisions/0006-credential-storage.md`](./docs/decisions/0006-credential-storage.md)
for the explicit posture on:

- Telemetry (none, ever; update-version polling is a documented
  exception that transmits no user data)
- Secret storage (SQLite at rest, protected by filesystem
  permissions; not OS keystore)
- Code signing (no paid certs at launch; package channels carry
  the trust)

If your finding lands in the gap between what we declare and what
we actually do, please tell us — that's a class of bug we want to
hear about.

## Recognition

We list reporters in published advisories with their consent.
We're a pre-revenue OSS project and can't pay bounties; we'll
credit publicly and link to your handle / site of choice.
