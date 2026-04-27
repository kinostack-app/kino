# Indexer-definition test fixtures

Two synthetic Cardigann YAMLs that exercise the parser surfaces
real-world definitions hit. Used by
`parse_synthetic_tracker_definition` and
`parse_synthetic_magnet_tracker_definition` in
[`../definition.rs`](../definition.rs) as regression checks against
the two parser shapes we care about:

- `synthetic-tracker.yml` — wide `caps` + `categorymappings`,
  ordered `search.fields` (insertion order is load-bearing —
  Cardigann fields reference earlier ones via `{{ .Result.x }}`
  templates), multi-step `download.selectors`, settings with
  text/select/info widgets, `keywordsfilters` chain
- `synthetic-magnet-tracker.yml` — uses the `download.infohash`
  block (vs the more common `download.selectors` extraction)

## Why synthetic

Real-world Cardigann definitions evolve upstream as sites change
selectors, add anti-bot challenges, or move between hosts. A
parser regression test pinned against an upstream YAML would
start failing for reasons unrelated to our parser, every time
the upstream evolves. Synthetic fixtures don't drift — they
exercise the parser shape and only change when the parser
changes.

The production loader (`indexers/loader.rs`) fetches the full
upstream definition set at runtime; that's how we use real
definitions in production. The test fixtures here exist purely
for parser regression coverage.

## Updating

If our parser changes and we want broader coverage, add another
`synthetic-*.yml` and a matching test in `definition.rs`. Keep
each synthetic fixture focused on a specific parser surface —
don't recreate the whole Cardigann spec; write a fixture that
makes a single regression test fail loudly when the parser
breaks.
