# ADR 0006 — Credential storage stays in SQLite

**Status:** Accepted (2026-04-26)

## Context

Kino stores several secrets that an OS-native keystore (Keychain on
macOS, Credential Manager on Windows, Secret Service / kwallet /
GNOME-keyring on Linux) is *designed* to hold:

- `KINO_API_KEY` — bearer token for the HTTP API
- Trakt OAuth refresh token
- TMDB API key
- Per-indexer API tokens
- WireGuard private + preshared keys (VPN config)

Today these all live in the SQLite `config` and `vpn_config` tables.
The cross-platform audit (Phase 3) examined whether to migrate to
the `keyring` crate and the per-OS native stores beneath it.

## Decision

**Keep secrets in SQLite. Don't adopt `keyring`.** Re-evaluate when
either of the following becomes true:

1. Primary deployment shifts away from the systemd / launchd /
   Windows Service service-mode toward user-mode (e.g. tray-only
   becomes the dominant install path)
2. A specific incident or concrete user complaint motivates the
   security-vs-complexity trade-off

## Why

**Service-mode is incompatible with most keystores.** Our primary
deployment (`.deb` / `.rpm` / AUR / Pi appliance / Docker / Windows
Service / macOS LaunchDaemon) runs the binary as a non-interactive
system user with no GUI session:

| Platform | Keystore | Works in service-mode? |
|---|---|---|
| Linux systemd | Secret Service via D-Bus | **No** — D-Bus session bus isn't available to system services |
| macOS LaunchDaemon | System Keychain | **Partial** — accessible via privileged helper, unreliable in practice |
| Windows Service (LocalSystem) | Credential Manager | **Partial** — credentials stored under SYSTEM account, isolated from any logged-in user's store |

Adopting keyring forces a fallback path *anyway* — we'd ship
`secrets/{keyring,sqlite}.rs` with detection logic, and the most
common deployment (the system service) would always end up on the
SQLite branch. We'd carry the abstraction cost without the security
benefit.

**The threat model is "trust the OS at rest".** Kino runs on
self-hosted servers (Pi, NAS, dedicated box, laptop). Secrets in
`kino.db` are protected by:
- `0600 kino:kino` filesystem permissions (set by `.deb`/`.rpm`
  postinst)
- The user's full-disk encryption (their responsibility)
- Backup encryption (subsystem 19 has a passphrase option)

If an attacker can read `/var/lib/kino/kino.db` they almost certainly
already have SSH keys, GPG keys, browser cookies, and other plaintext
secrets on the same machine. Keyring is defence-in-depth at this
trust layer, not a blocker.

**Backup-restore stays simple.** Subsystem 19 archives `kino.db`. A
user restoring on a new machine sees their Trakt OAuth still
connected, their indexer keys present, their VPN config intact. If
secrets lived in keyring, restore would require manual re-auth on
every new machine — UX regression we'd rather not ship.

Self-hosted media tools have converged on the SQLite-as-secret-store
pattern for the reasons above — keys + OAuth tokens stored in the
app's own data file rather than an OS keyring.

## Consequences

- **`kino.db` contains plaintext secrets.** Document this explicitly
  in subsystem 19 (backup-restore) so users know to enable the
  backup passphrase if they email backups around or push to cloud
  storage
- **Filesystem-permission discipline matters.** `.deb`/`.rpm`
  postinst (`debian/postinst`, `rpm/postinstall.sh`) must set
  `kino:kino` ownership + `0600` mode on `kino.db`. Verify on every
  release
- **Log redaction stays the only secret-leakage defence at runtime.**
  `observability/redact.rs` already scrubs API keys, tokens,
  passwords from logs. Keep the redaction list in sync as new
  secret-bearing fields are added
- **No `keyring` dep.** Saves ~50 KB binary + a dep that has known
  brittleness on minimal Linux containers (no D-Bus)

## Migration path (if/when we revisit)

The Phase 3 audit drafted the migration code. Stored in our notes for
when the trigger condition arrives:

- Hybrid `SecretBackend` trait (`secrets/{mod,sqlite,keyring,migration}.rs`)
- First-boot probe: try keyring; on failure stay on SQLite
- Backup posture: keep secrets in `kino.db` (Option B in audit) so
  restore stays seamless; keyring is the "live" store, DB is the
  "snapshot"
- Total work: ~1500-2500 LOC across the trait, per-OS shims,
  migration, callsite changes

## Alternatives considered

- **Adopt `keyring` everywhere.** Rejected because service-mode is
  the primary deployment and Secret Service / Credential Manager
  don't work there
- **Encrypt the SQLite DB at rest** (e.g. SQLCipher). Rejected for
  v0: the encryption key would have to live somewhere; we're back to
  the keyring problem. Worth revisiting if/when we want to ship
  encrypted backups by default
- **Hybrid: keyring for tray, SQLite for service.** Rejected because
  the tray and service share the same DB; it'd be confusing to have
  some secrets in one place and some in another
