# kino documentation

The canonical reference for how kino works today, why it works
that way, and what's coming next.

## What lives where

### Reference — "how kino works today"

- **[architecture/](./architecture/)** — cross-cutting patterns
  (state machines, operations, invariants, consistency model,
  crate layout, auth, events, logging, tech stack). Read this
  when you're touching any code.
- **[data-model/](./data-model/)** — SQL schema, table relationships,
  derived state.
- **[subsystems/](./subsystems/)** — per-domain reference.
  **Shipped behaviour only.** A subsystem doc here describes what
  the code actually does today.

### Decisions — "why kino works this way"

- **[decisions/](./decisions/)** — Architecture Decision Records
  (ADRs). Numbered, immutable. Each captures one decision with
  context, alternatives, and consequences. Written once, never
  edited (only superseded by a later ADR).

### Forward-looking — "what we're building next"

- **[roadmap/](./roadmap/)** — features that don't exist yet. A
  doc here describes intent, not reality. Promoted to
  `subsystems/` when the feature ships.

### Third-party + supplemental

- **[third-party/](./third-party/)** — source-offer + redistribution
  notes for bundled or downloaded upstream software (e.g. FFmpeg).

### Operations — "how to do things to a running kino"

- **[runbooks/](./runbooks/)** — short, action-oriented operator
  how-tos.

## Conventions

- **Numbered files in `subsystems/` and `roadmap/` mirror their
  tracker IDs** (`05-playback.md`, `14-indexer-engine.md`,
  `33-vpn-killswitch.md`). Numbers stay stable when a doc moves
  between `subsystems/` and `roadmap/`.
- **ADR filenames are zero-padded four digits**:
  `0001-single-binary.md`, `0002-sqlite-not-postgres.md`.
- **Architecture docs are flat**, no sub-folders.
- **Runbooks open with a one-line "when to use this"** then jump
  straight to commands.

## How to add a new doc

| Adding... | Goes in... |
|-----------|-----------|
| A new pattern that crosses subsystems | `architecture/` |
| A new shipped subsystem | `subsystems/` |
| A planned feature | `roadmap/` |
| A non-trivial choice that closes alternatives | `decisions/` (next ADR number) |
| A "how do I X" answer for operators | `runbooks/` |
