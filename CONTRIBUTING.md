# Contributing to Kino

Thanks for the interest. Quick orientation, then specifics.

## Getting set up

The repo ships a devcontainer that brings up the backend, frontend,
and the kinostack.app static site as four sibling Docker containers
sharing a network namespace.

```sh
# In VS Code: "Reopen in Container". Or, manually:
docker compose up -d

# All four services + a dev shell come up. Backend rebuilds on .rs
# changes via watchexec; frontend has Vite HMR; web (Astro) has its
# own watcher.
```

`CLAUDE.md` at the repo root is the canonical orientation — read it
first. It covers the dev workflow, the quality-gate scripts, and
project layout.

## Quality gates

```sh
# Backend
cd backend
just fix-ci          # auto-fix + verify (preferred)
just test            # full test suite

# Frontend
cd frontend
npm run lint:fix
npm run typecheck
npm run test

# Web site
cd web
npm run build
```

`just fix-ci` is the **authoritative** Rust check. If it passes, the
code compiles. We treat clippy `-D warnings` as our baseline.

### Optional — pre-commit / pre-push hooks

Two ways to wire local hooks that run lint + format checks before
commit / push. Both are opt-in; CI catches everything either way.

**Path A — `prek` (Rust, recommended)**:

```sh
cargo install prek
prek install
```

Reads `.pre-commit-config.yaml`. Single Rust binary, no Python.

**Path B — Python `pre-commit`** (the original framework):

```sh
pip install pre-commit
pre-commit install
```

Same config file; same hooks. Use this if you don't want to install
prek.

**Path C — plain shell hooks** (zero-dep):

```sh
./.githooks/setup
```

Sets `git core.hooksPath` to `.githooks/`. Simpler than the
framework path; runs `cargo fmt --check` + biome on commit, full
quality gate on push.

Bypass any of them with `git commit --no-verify` or
`git push --no-verify` when needed.

## Subsystem layout

The `docs/` directory has the full system specification:

- [`docs/subsystems/`](./docs/subsystems/) — what's shipping
- [`docs/roadmap/`](./docs/roadmap/) — design-only, not yet built
- [`docs/architecture/`](./docs/architecture/) — cross-cutting patterns
- [`docs/decisions/`](./docs/decisions/) — ADRs

Read the relevant subsystem before implementing in it.

## Branching model

**Trunk-based.** All work lands on `main` via PR. Feature branches
are short-lived (hours to days, not weeks). No long-lived
`develop`, no parallel release branches.

- `main` is always green: CI passes, tests pass, deployable
- Feature work happens on a topic branch named `<author>/<topic>`
  or `<topic>` — convention, not enforced
- PRs squash-merge into `main`. The PR title becomes the commit
  message — make it descriptive
- We don't backport to release branches. Pre-1.0, the answer to
  "fix this on the previous version" is "upgrade." When we hit
  v1.x and have an LTS commitment, this section gets rewritten

## Pull request flow

1. Fork the repo + branch from `main`
2. Make your change. Keep PRs focused — one subsystem, one concern
3. Run the quality gates above
4. Open the PR. The template will guide you on what to include
5. CI runs lint, type-check, tests, and (for cross-OS-relevant
   files) macOS + Windows clippy
6. A maintainer reviews; iterate from there
7. Squash merge — your PR title is the commit message on `main`

## Conventional commits + PR titles

We **recommend** [Conventional Commits](https://www.conventionalcommits.org/)
for PR titles. We don't enforce them in CI — squash merges mean the
PR title becomes the canonical message on `main`, and a clear PR
title is a separate concern from a strict commit-format gate.

PR titles should be short and direct:

- `feat(import): handle multi-disc episodes`
- `fix(vpn): retry handshake on transient EAGAIN`
- `docs(subsystem-19): correct restore-flow diagram`
- `chore(deps): bump axum 0.8 → 0.9`
- `refactor(scheduler): pull tick body into a helper`
- `test(playback): cover 4K HDR transcode fallback`

The `(scope)` should match the subsystem when relevant.

Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`,
`build`, `ci`, `perf`, `style`. Add `!` after the type for breaking
changes (`feat!: drop the v1 cast endpoint`).

## Versioning

[SemVer](https://semver.org/). We're pre-1.0, so we follow the
0.x carve-out: a **minor bump signals a breaking change** (e.g.
`0.1.x → 0.2.0`); patch bumps are bug fixes only. Once we cut
`v1.0.0`:

- **Major** (`1.x → 2.0`): breaking API change
- **Minor** (`1.0 → 1.1`): new feature, backwards-compatible
- **Patch** (`1.0.0 → 1.0.1`): bug fix only

[release-please](https://github.com/googleapis/release-please)
manages CHANGELOG.md + version bumps automatically based on
conventional-commit types in PR titles. The `release-please`
workflow keeps an open "Release PR" on `main` showing what the
next version + CHANGELOG entry would be; merging it cuts the tag.

You don't write CHANGELOG.md entries by hand — the tool generates
them from commit history. Make your PR titles informative.

## Feature flags

Two distinct things go by this name:

- **Cargo features** (build-time): we use these. `tray` is the
  primary one — desktop builds get the tray; headless (Pi /
  Docker) builds drop it. New deps that should be optional add a
  feature gate; see how `tray-icon`, `tao`, `webbrowser` are
  scoped in `backend/crates/kino/Cargo.toml`
- **Runtime feature flags** (LaunchDarkly-style): we don't use
  these. The product surface is small enough that on/off toggles
  live in the SQLite `config` table as plain settings, exposed in
  Settings → System. If we ever ship A/B-style runtime gating,
  it's an ADR moment

## What we welcome

- Bug fixes with regression tests
- Documentation improvements + spec corrections
- New indexer definitions (Cardigann YAML format)
- Performance improvements with benchmarks
- Cross-platform improvements (macOS / Windows / Pi)

## What needs discussion first

Open an issue or Discussion before starting work on:

- New subsystems or major architectural changes
- New top-level dependencies
- Breaking API changes
- Anything that touches the privacy posture (see [ADR 0008](./docs/decisions/0008-privacy-posture.md))

A 5-minute issue beats a 5-hour PR that gets closed because it
doesn't fit the project's direction.

## Security

Don't open public issues for security bugs. See
[`SECURITY.md`](./SECURITY.md).

## License

By contributing you agree your work is licensed under the project's
GPL-3.0-or-later license — see [`LICENSE`](./LICENSE).

We use the [Developer Certificate of Origin](https://developercertificate.org/)
(DCO) rather than a CLA. Sign each commit with `git commit -s` to add
a `Signed-off-by:` trailer attesting that you have the right to
contribute the change under the project's license.

Copyleft expectations: any derivative work distributed publicly must
remain under GPL-3.0-or-later. If you fork kino, your changes must
also be GPL. This protects every kino user's freedom to inspect,
modify, and self-host the software they run.
