# Subsystem reference (shipped only)

Each file here describes the **actual shipping behaviour** of one
domain. If code disagrees with a doc, treat the doc as needing
update — not the code as wrong.

Planned-only subsystems live in [`../roadmap/`](../roadmap/).
Numbered file prefixes (`05-playback.md`, `14-indexer-engine.md`)
are stable across the move so cross-references stay legible.

## What's here

| # | Doc | Notes |
|---|---|---|
| 00 | [Release parser](./00-release-parser.md) | |
| 01 | [Metadata](./01-metadata.md) | |
| 02 | [Search](./02-search.md) | |
| 03 | [Download](./03-download.md) | |
| 04 | [Import](./04-import.md) | |
| 05 | [Playback](./05-playback.md) | |
| 06 | [Cleanup](./06-cleanup.md) | |
| 07 | [Scheduler](./07-scheduler.md) | |
| 08 | [Notification](./08-notification.md) | |
| 09 | [API](./09-api.md) | |
| 10 | [Web UI](./10-web-ui.md) | |
| 11 | [Cast](./11-cast.md) | Chromecast receiver shipped; companion-TV-app design at the bottom of the doc is forward-looking |
| 12 | [Trickplay](./12-trickplay.md) | |
| 13 | [Startup](./13-startup.md) | |
| 14 | [Indexer engine](./14-indexer-engine.md) | |
| 15 | [Intro skipper](./15-intro-skipper.md) | |
| 16 | [Trakt](./16-trakt.md) | |
| 17 | [Lists](./17-lists.md) | |
| 18 | [UI customisation](./18-ui-customisation.md) | Core shipped; small follow-ups noted at top of doc |
| 19 | [Backup & restore](./19-backup-restore.md) | Phase 1 shipped; restore prompts for restart (Phase 2 will exit-and-rely-on-supervisor) |
| 20 | [Health dashboard](./20-health-dashboard.md) | Page shipped; WS delta-patch is a follow-up (poll-based today) |
| 25 | [mDNS discovery](./25-mdns-discovery.md) | Responder shipped; Avahi-socket coexistence path not yet implemented |
| 29 | [Show & movie logos](./29-show-logos.md) | |
| 31 | [Backend integration testing](./31-integration-testing.md) | TestApp builder + mock TMDB/Trakt/Torznab + ~70 flow-test files |
| 32 | [Server-side Cast sender](./32-cast-sender.md) | Backend + frontend wired end-to-end; works on Firefox (server speaks Cast directly) |

## Promotion path

When a feature ships:

1. `git mv` its planning doc from `roadmap/` to here.
2. Rewrite the doc to describe what was actually built (not what
   was planned).
3. Update any references to the old path.
