# ADR 0008 — Privacy posture: no telemetry, update-version polling allowed

**Status:** Accepted (2026-04-26)

## Context

Self-hosted software users strongly resent binaries that transmit
their behaviour or data to external services. The user has stated
this as an absolute for kino — a values-level rule, not a trade-off
to be revisited.

Separately, an auto-update feature (subsystem 27) is desirable so
users learn about new versions without having to manually check
GitHub. This requires the binary to make an outbound HTTP request,
which superficially looks like telemetry but isn't — it sends no
user data.

## Decision

**Telemetry: never.** No analytics, usage counters, error reports,
crash dumps uploaded automatically, anonymised metrics, or any
mechanism that transmits information about the user, their config,
their library, or their usage to any external service.

**Update-version polling: allowed.** The binary may issue an
outbound HTTPS GET to a public release-metadata endpoint (e.g. the
GitHub Releases API) to check for a newer version. The poll request
must:

- Send no user-identifying state beyond what any HTTP request
  unavoidably reveals (source IP, generic User-Agent like
  `kino-update-check`, current version in the User-Agent or query
  string)
- Be opt-out via a Settings → System toggle (default-on)
- Surface only as a "version X.Y.Z available — view release notes"
  notification; never auto-download, never auto-install
- Have a low frequency (once per day; backed off on errors)

## Why the carve-out

A "is there a new version?" poll is one-way receive: the binary asks
"what is your latest tag?" and the answer is the same for every
caller. No user-specific state is transmitted. The asymmetry of
benefit (user learns about security updates, bug fixes) vs cost
(server logs an IP + version + timestamp) makes this an obvious
win — the same trade-off `apt update`, `brew update`, `npm outdated`
all make.

The principled distinction:

| Pattern | Allowed? | Reason |
|---|---|---|
| Sending `{"event":"playback","movie_id":42}` | **No** | User behaviour leaving the box |
| Sending `{"version":"0.1.0","os":"linux"}` | **No** | Even minimal usage stats leak fingerprintable info |
| Receiving `{"latest":"0.2.0","url":"..."}` after a content-less GET | **Yes** | Pure read; no state transmitted; same answer for every caller |
| Auto-uploading panic backtraces | **No** | Crash dumps contain user state (paths, config, etc.) |
| Opt-in "send this debug bundle" via UI button | **Yes** | Explicit user action; never automatic |

The server-side observation (kino's update-check endpoint logs an IP
and a version) is a separate trust question for whoever runs that
endpoint. We mitigate by keeping the request as content-less as
possible and using the existing GitHub Releases API (Microsoft's
log-retention policy applies, not ours).

## Consequences

- **No analytics dashboard.** We don't know how many users we have,
  what features they use, what crashes they hit. We rely on:
  - GitHub stars + Discord activity for "is the project alive"
  - User-submitted bug reports for "what's broken"
  - Opt-in diagnostic-bundle export (task #528) for "help me debug
    my install"
- **Update-check endpoint TBD.** Subsystem 27 (auto-update, currently
  roadmap-only) will pick the endpoint — almost certainly the
  GitHub Releases API at `api.github.com/repos/.../releases/latest`.
  Re-using GitHub's infra means no new server to operate
- **User-Agent design matters.** A poll request with
  `User-Agent: kino/0.1.2 linux x86_64 i7-12700K kino-instance-id-XYZ`
  would transmit fingerprintable info. The agreed-upon shape:
  `User-Agent: kino-update-check/{semver}` — version is the only
  variable, fingerprintable to about as many bits as a `Last-Modified`
  header
- **Settings toggle to disable.** Honour-system: a user who wants
  zero outbound polls turns it off in Settings → System →
  "Check for updates"
- **Document this prominently.** Privacy-conscious users (the kind
  who deploy self-hosted software) check before installing. The
  marketing site + install docs need a `/privacy` page that says
  exactly this

## What this enables and disables

| Feature / consideration | Allowed under this policy? |
|---|---|
| Update-version polling (subsystem 27) | Yes |
| Auto-download of new binary | No (user clicks; we don't push) |
| Auto-install of new binary | No (user runs the installer; we may launch it for them on Windows after they confirm) |
| Crash auto-upload | No |
| Opt-in "submit diagnostic bundle to GitHub Issue" via UI button | Yes |
| Local-only error logging (SQLite log_entry table) | Yes — never leaves the box |
| Anonymous usage counters | No |
| Bandwidth / storage telemetry to a server | No |
| GitHub Releases API for tag list | Yes |
| TMDB / Trakt / indexer API calls | Yes — but those are user-configured integrations, not telemetry |
| mDNS responder advertising kino.local on the LAN | Yes — local-network discovery, doesn't leave the LAN |
| Health-check endpoint a load balancer or monitoring tool can hit | Yes — server-side, not initiated by the binary |

## Implementation notes for subsystem 27

When the auto-update feature lands, this ADR is the design constraint:

- Single endpoint hit per check
- No headers identifying the install (no machine ID, no install date,
  no config hash, no anything beyond the User-Agent's version
  string)
- Settings toggle defaults to on; honour the toggle on every check
- The "update available" notification is the only side effect; user
  decides whether to act
- Log every check at INFO so operators auditing outbound network
  activity can see exactly what was sent

## Alternatives considered

- **No update polling at all** (the original wording in `feedback_no_telemetry.md`
  before this ADR was authored). Rejected because users don't reliably
  hear about security updates without the prompt; security-by-not-
  knowing is the wrong default
- **Opt-in update polling** (default-off). Rejected because users
  who don't go looking for the toggle stay on outdated versions;
  the security-update propagation rate would be poor
- **Self-hosted update endpoint at kinostack.app instead of GitHub.**
  Rejected for now because it adds infra we don't operate. May
  revisit if GitHub becomes unreliable or we want richer
  release-channel logic (stable / beta / nightly)

## Related ADRs

- ADR 0006 — Credential storage (related: what data lives where)
- subsystem 27 — Auto-update (the consumer of this policy; not yet
  implemented)
