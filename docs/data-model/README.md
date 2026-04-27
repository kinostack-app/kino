# Data model

The SQLite schema, table relationships, and derived state.

## Migration approach

- **Pre-launch:** rewrite the initial schema migration rather than
  accumulate migrations. Any prior install resets via `just reset`.
- **Post-launch:** forward-only migrations under
  `backend/migrations/`. Numbered + dated. Never edit a shipped
  migration; add a new one.

## Files

- [`01-schema.md`](./01-schema.md) — full SQL schema, table-by-table
  reference, derived state notes.

## Cross-references

- The canonical schema is `backend/migrations/20260328000001_initial_schema.sql`.
- Per-domain models live in their domain modules (see
  [`../architecture/crate-layout.md`](../architecture/crate-layout.md)).
- Derived state (the user-visible macro state per content entity)
  is documented in [`../architecture/state-machines.md`](../architecture/state-machines.md)
  and computed in `content/derived_state.rs`.
