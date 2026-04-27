# Agents

Guidance for LLM coding agents (Claude, Codex, Cursor, Aider,
Continue, etc.) working on kino. The canonical orientation lives
in [`CLAUDE.md`](./CLAUDE.md) — this file is its cross-vendor
sibling so non-Claude agents land the same instructions.

If you're a human contributor: read [`CONTRIBUTING.md`](./CONTRIBUTING.md)
instead. This doc is for the machines.

## Read first

[`CLAUDE.md`](./CLAUDE.md) covers:

- Project tech stack (Rust + axum + sqlx; React + TypeScript + Vite)
- Devcontainer layout (4 sibling containers sharing a network namespace)
- Quality gate scripts (`just fix-ci` is the **authoritative**
  Rust check; `npm run lint:fix` for frontend)
- Critical rules: always use the script wrappers, never raw
  `cargo`/`vitest`/`biome`
- The OpenAPI contract — backend defines, frontend consumes via
  generated types

Treat that file as load-bearing. Re-read on every new session.

## Project structure

```
backend/        Rust workspace; main binary at backend/crates/kino
frontend/       React SPA — kino's in-app UI
web/site/       Astro marketing site → kinostack.app
web/docs/       Astro + Starlight docs site → docs.kinostack.app
web/shared/     Master brand assets (not deployed; manually synced)
docs/           Maintainer-facing spec — start at docs/README.md
ref/            Cloned reference projects (gitignored except for the
                REFERENCES.md index). Read-only study material; DO NOT
                import code from here. Either copy a small snippet with
                attribution in a comment, or write your own.
.github/        Workflows, dependabot, issue templates
packaging/      Per-channel manifests (AUR, pi-gen, AppImage). Top-level
                packaging files (.deb / .rpm / WiX MSI metadata) live
                in backend/crates/kino/Cargo.toml + workspace metadata
.devcontainer/  VS Code dev container (Docker socket mounted; matches
                the docker-compose.yml dev environment)
```

## Subsystems

`docs/subsystems/` — what's shipping. `docs/roadmap/` — designed
but not yet built. `docs/architecture/` — cross-cutting patterns.
`docs/decisions/` — ADRs.

Read the relevant subsystem doc before touching code in it.
Subsystem doc trumps implementation; if they disagree, treat the
doc as canonical and update the code (or update the doc with a
clear "implementation diverged because X" rationale).

## Quality gates — run before declaring done

```sh
# Backend
cd backend && just fix-ci && just test

# Frontend
cd frontend && npm run lint:fix && npm run typecheck && npm run test

# After backend schema changes
cd backend && just codegen
```

`just fix-ci` is rustc + clippy with `-D warnings`. If it passes,
the code compiles. Don't second-guess it.

## Things to avoid

- Don't mock the database in tests. We use a real SQLite test pool
  via `crate::test_support`. Mocked tests pass; real-DB integration
  reveals migration drift
- Don't add telemetry, analytics, crash uploads, or anything that
  transmits user state. See [ADR 0008](./docs/decisions/0008-privacy-posture.md).
  Update-version polling is the explicit carve-out (no user data
  sent)
- Don't store secrets outside SQLite without checking [ADR 0006](./docs/decisions/0006-credential-storage.md).
  Keyring isn't viable in our service-mode deployment
- Don't add OS-keystore deps (`keyring` crate) unless the deployment
  posture changes — the design doc explains the trade-offs
- Don't write platform-specific code without a `cfg(target_os)`
  gate. The tray feature is the only feature flag in widespread use
- Don't bypass `just fix-ci` with `--no-verify` or similar. If a
  hook fails, fix the underlying issue
- Don't change git config or the user's global tool installs

## Things we want

- Subsystem-aligned, focused PRs
- AppEvent emitted on every state-changing handler (frontend cache
  invalidation depends on it — see [`docs/architecture/events.md`](./docs/architecture/events.md))
- ToSchema derives on every public DTO (frontend type generation
  depends on it — see [`docs/architecture/conventions.md`](./docs/architecture/conventions.md))
- Tests for new code; the test suite is fast (~7s for ~1000 tests)
- Doc updates alongside code changes when the subsystem spec needs
  to reflect new behaviour

## When in doubt

Read the spec. Then read the failing test. Then ask the human in
the PR description.
