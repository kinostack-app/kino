# Testing strategy

How kino is verified across platforms, deployment modes, and
release phases. Pulled together by the cross-platform audit
(2026-04-26) — until that audit, testing was implicit ("Linux dev
container has a 1500-test suite, that's it").

## Layers

Five layers, ordered from cheap-and-fast to expensive-and-slow:

| Layer | What it checks | Where it runs | When |
|---|---|---|---|
| **Unit + module tests** | Pure logic, parsing, helpers, schema-derived types | Linux dev container (`just test`) | Every save while developing; pre-merge in CI |
| **Integration tests** | Multi-module flows: import → DB → events; HTTP handlers via axum's tower stack; sqlite migrations | Linux dev container | Pre-merge in CI; `just test` |
| **Cross-OS compile gate** | Code compiles + clippy-clean on Linux, macOS, Windows under both `--features tray` and `--no-default-features` | GitHub Actions matrix | Tag-push (release.yml plan stage) |
| **Cross-OS smoke** | Binary launches, `/api/v1/status` returns ok, service registers, port binds | GitHub Actions matrix runners | Pre-release (currently manual; automated in v1.1) |
| **End-to-end install** | Native package installs, service starts, web UI loads, library scan succeeds | VM-based job (currently manual; not automated yet) | Pre-release sign-off |

## What's automated today (2026-04-26)

| Layer | Status |
|---|---|
| Unit + module | ✓ Full suite via `just test` (cargo-nextest); 1500+ tests |
| Integration | ✓ Same suite — flow tests live in `backend/crates/kino/src/flow_tests/` |
| Cross-OS compile | Partial — `.github/workflows/ci.yml` builds Linux only. release.yml has the per-OS build matrix but only fires on tag push |
| Cross-OS smoke | ✗ Not automated. Manual verification on macOS/Windows hosts |
| End-to-end install | ✗ Not automated |

## What we add and when

### Phase A — before first real release tag

**Add Linux clippy-on-headless to ci.yml.** Already present.
Checking the `--no-default-features` build path catches
"accidentally relied on the tray feature" regressions.

**Add a manual-trigger smoke workflow** — `workflow_dispatch` job
in `.github/workflows/release.yml` (or a separate `smoke.yml`)
that takes a build artefact and launches it on each Tier 1
runner, hits `/api/v1/status`, and exits. ~30 minutes wall-clock
per OS; cheap when triggered manually.

### Phase B — after first real user reports bugs we'd have caught

**Add cross-OS compile to ci.yml.** Today only Linux runs on PRs;
macOS + Windows builds are deferred to release.yml. If we hit a
cross-OS compile regression that release.yml catches but PR-CI
doesn't, that's signal to add macOS + Windows to PR-CI. Cost:
runner minutes (10× Linux for macOS, 2× for Windows). Rate-limit
by only running per-OS on PRs that change cross-platform-relevant
files (`src/tray/**`, `src/service_install/**`, `src/paths.rs`,
`src/download/vpn/**`, `Cargo.toml`).

**Add a per-OS smoke test to channels.yml.** After the release
artefacts publish, fire one job per OS that downloads the just-
published binary, installs it, hits `/api/v1/status`, uninstalls,
exits. Catches "we shipped a broken binary" within minutes of
publishing.

### Phase C — when the cost of a regression on a niche platform is
       high enough

**VM-based end-to-end install tests.** Spin up Debian / Fedora /
Arch / macOS / Windows VMs, install the package, run a scripted
flow (add a movie, fake a torrent, verify import lands the file).
Cost: ~30 minutes per VM, GitHub-runner-hours add up. Defer until
we have user traffic that justifies it.

Reference: bottom's CI runs across many BSD VMs via dedicated
runners. Overkill for v0; useful once we ship to the same
audience.

## Per-OS test matrix

| OS / arch | Compile | Unit tests | Smoke (binary launches + /status) | Service install | Tray | E2E install |
|---|---|---|---|---|---|---|
| Linux x86_64 (Ubuntu 24.04 in CI) | ✓ | ✓ | Manual today; automate in Phase A | Manual | Manual | Manual |
| Linux ARM64 (Ubuntu 24.04 native runner) | release.yml only | ✗ — Linux x86_64 covers logic | Phase A | Pi image build (channels.yml) covers boot | n/a (Pi image is headless) | Pi image build verifies boot |
| macOS ARM64 | release.yml only | ✗ — same | Phase A | Phase B | Phase B (when tray icon work lands) | Phase C |
| macOS x86_64 | release.yml only | ✗ | Phase A | Phase B | Phase B | Phase C |
| Windows x86_64 | release.yml only | ✗ | Phase A | Phase B (depends on Windows SCM dispatcher — task #524) | Phase B | Phase C |
| Pi appliance image | n/a (built on Linux ARM64) | n/a | channels.yml (pi-gen-action verifies boot) | n/a | n/a | Manual |

## What gets manual-only forever

Some things will never have automated tests because the cost
exceeds the value:

- **Real Chromecast device casting** — physical hardware needed.
  Manual test on the user's actual Chromecast Ultra
- **Real VPN provider tunnels** — Mullvad / ProtonVPN credentials
  in CI = key management headache. Test on the user's actual
  account
- **Real torrent swarms** — public trackers are flaky and depend
  on actual seeders. Tests use librqbit's test harness with mock
  peers
- **Notarisation / signing flows** — only fire on real release
  builds. Don't ship a "test signing" path
- **macOS Sleep/Wake VPN reconnect** — needs real hardware that
  actually sleeps; CI VMs don't simulate this

## Fakes vs real services

Documented elsewhere ([architecture/fakes-vs-real.md](./fakes-vs-real.md))
but worth noting in the cross-platform audit: our test suite uses
fakes for TMDB / Trakt / Prowlarr-style indexers (HTTP-mock via
wiremock) so unit + integration tests don't need network. Adopted
across the suite; new tests that hit real services should be
exceedingly rare.

## Release-readiness checklist

Pre-tag, the maintainer should verify:

| Item | How |
|---|---|
| `just fix-ci` clean (default + `--no-default-features`) | `cd backend && just fix-ci && cargo check -p kino --no-default-features` |
| `just test` passing | `cd backend && just test` |
| Frontend `npm run lint && npm run typecheck && npm run test` clean | Standard frontend checks |
| Cross-OS compile via the release.yml plan stage | Push branch, trigger `workflow_dispatch` with `tag=dry-run` |
| Per-OS smoke (manual until Phase A) | Download dry-run artefacts, launch on macOS + Windows, hit /api/v1/status |
| `cargo audit` for advisories on new deps | `cargo audit` (consider adding to ci.yml as a Phase A item) |
| Backup-restore round-trip on the new schema | Manual: backup at vN-1, install vN, restore, verify |
| RELEASING.md runbook followed | See `RELEASING.md` (task #526) |

## Scope explicitly excluded

- **Mutation testing.** Not now; cost/value unclear at our scale
- **Property-based testing.** Used selectively where it's already
  paid off (release-parser); not a global mandate
- **Fuzzing.** Worth doing for the release-parser (it eats user-
  supplied filenames) and the indexer-response parser. Not for the
  whole binary
- **Performance benchmarks.** No SLAs to defend yet. Add when we
  have a regression complaint that bench numbers would have
  caught
- **Coverage targets.** Coverage is reported (`cargo llvm-cov`) but
  no minimum threshold gate. Per-PR diff coverage is a Phase B
  consideration

## Related docs

- [Cross-platform deployment (subsystem 21)](../roadmap/21-cross-platform-deployment.md) §8 release engineering
- [Cross-platform paths (architecture)](./cross-platform-paths.md)
- [Service install (architecture)](./service-install.md)
- [Fakes vs real services (architecture)](./fakes-vs-real.md) — if it
  exists; otherwise this is a new doc to write
