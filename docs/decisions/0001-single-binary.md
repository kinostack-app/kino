# ADR-0001: Single binary, single process

**Status:** accepted
**Date:** 2026-04-25 (recorded retroactively)

## Context

Self-hosted media automation typically composes 7+ separate
processes — discovery, acquisition, indexer aggregation, download
client, library/streaming, request UI, VPN sidecar. Each speaks its
own API; users wire them together. The result works but has
well-known pain: configuration drift, inter-service auth, no shared
event bus, fragile container orchestration, eight Docker images and
twelve volumes.

## Decision

kino ships as **one Rust binary** that owns acquisition, download,
import, playback, transcode, intro detection, Cast, scrobble, and
the SPA backend in a single process with a single SQLite database.

## Alternatives considered

- **Microservices.** The status-quo design for this category, but
  the inter-service wiring + drift it produces is the whole
  problem we're trying to escape.
- **Single binary with embedded plugins.** Plugin systems add
  abstraction we don't need at our scale. A single user does not
  benefit from runtime-loaded extensions.
- **Two-binary split (server + worker).** No win without
  horizontal scaling, which is explicitly a non-goal.

## Consequences

- **Win:** zero inter-service auth/wiring/drift. Subsystems share
  an `AppState`, an event bus, and a database. New features compose
  trivially.
- **Win:** one Docker image, one config, one log stream, one DB
  backup target.
- **Win:** atomic deploys. The whole stack updates as one unit.
- **Cost:** one process is also one failure domain. A bug in the
  intro skipper can in principle crash the playback path. Mitigated
  by `tokio::spawn` + `panic = "abort"` boundaries and aggressive
  testing.
- **Cost:** no horizontal scaling path. Acceptable: target audience
  is single-user self-hosted; vertical scaling on a NAS or VPS
  comfortably handles "all my media."
- **Cost:** the codebase grows to encompass everything. Mitigated
  by domain-module organisation (see ADR-0005).

## Supersedes

None.
