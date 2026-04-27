# Changelog

Notable changes per release. Format follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows [SemVer](https://semver.org/) once we hit v1.0.0.

## Unreleased

Pre-1.0 development happens on `main`; entries below queue up for
the first tagged release. Once we ship a real release the
unreleased section moves to a versioned heading and the next
unreleased section opens beneath this one.

### Added

- Subsystem 19: backup & restore (Phase 1 MVP)
- Subsystem 22: desktop tray (icon, menu, 5s health poll, single-instance lock)
- Subsystem 21: cross-platform deployment scaffold — cargo-dist 0.31.0, .github/workflows/{ci,release,channels}.yml, .deb / .rpm / .msi / .pkg / Homebrew tap / shell installer / Docker / Pi image / AUR PKGBUILD
- `etcetera` per-OS path resolution (`paths::default_data_dir()`); `--data-path` falls back to platform-appropriate XDG / native default
- Five ADRs covering credential storage, code-signing posture, privacy, filesystem invariants, networking primitives
- mDNS responder for `kino.local` discovery (subsystem 25)
- Server-side Cast sender (subsystem 32)
- VPN killswitch (subsystem 33, Phases A + B)

### Changed

- `setup_tracing` detects `JOURNAL_STREAM` and drops timestamp + ANSI under journald to keep `journalctl -u kino` clean
- `Cli::data_path` is now optional; explicit flag → env var → per-OS default (was hardcoded `./data`)

### Security

- GitHub Attestations enabled for SLSA build provenance
- Pre-release: no signed binaries yet (see [ADR 0007](./docs/decisions/0007-no-paid-signing-at-launch.md))

[Pre-1.0]: https://github.com/kinostack-app/kino
