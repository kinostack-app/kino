---
title: Reference
description: Configuration fields, CLI subcommands, environment variables, the HTTP API, and the diagnostic bundle.
---

The reference section is for looking things up. Each page enumerates one
surface of Kino exhaustively — every configuration field, every CLI
subcommand, every environment variable the binary reads at boot. If
you're trying to learn what Kino does or how to set it up, start in
[Getting started](../getting-started) or [Setup](../setup) first.

## Pages

- [Configuration](./configuration) — every field in the `config` row,
  grouped by area, with defaults and when to change each.
- [CLI](./cli) — `kino` subcommands and flags (`serve`, `reset`,
  service install / uninstall, tray).
- [Environment variables](./env-vars) — what Kino reads from the
  process environment at boot, when each one is set automatically by
  the platform packages, and when you'd set it yourself.
- [HTTP API](./api) — auth model, OpenAPI spec location, the
  WebSocket event stream, and where the in-app explorer lives.
- [Diagnostic bundle](./diagnostic-bundle) — what
  `GET /api/v1/diagnostics/export` collects, what it redacts, and how
  to fetch it.
