# Notification subsystem

Event-driven notification delivery. Listens for state changes across the system, logs them to History, pushes real-time updates to the UI via WebSocket, and fires templated webhooks to user-configured HTTP endpoints.

## Responsibilities

- Listen for events from all subsystems
- Write History entities for every significant event
- Push real-time events to connected UI clients via WebSocket
- Fire webhooks to configured targets with templated payloads
- Handle delivery failures with retry and backoff

## Events

| Event | Source | Example |
|---|---|---|
| `grabbed` | Search | "Breaking Bad S05E14 — Bluray-1080p grabbed from indexer" |
| `download_started` | Download | "Breaking Bad S05E14 downloading — 4.2 GB" |
| `download_complete` | Download | "Breaking Bad S05E14 download finished" |
| `download_failed` | Download | "Breaking Bad S05E14 stalled — retrying with alternative release" |
| `imported` | Import | "Breaking Bad S05E14 — Bluray-1080p added to library" |
| `upgraded` | Import | "The Matrix upgraded from WEB-1080p to Bluray-1080p" |
| `watched` | Playback | "The Matrix marked as watched" |
| `new_episode` | Metadata | "Breaking Bad S05E15 detected — airing 2026-04-02" |
| `health_warning` | Various | "VPN disconnected", "Disk space low (8 GB remaining)", "Indexer X disabled after repeated failures" |

## Delivery

### 1. History

Every event is written to the History table with:
- `event_type`, `date`, `movie_id`/`episode_id`
- `source_title`, `quality` (where applicable)
- `data` — JSON bag with event-specific context (the full `AppEvent` blob)

This is unconditional — all events are logged regardless of notification settings. The UI notification feed is a filtered query on this table.

**ID extraction.** The `movie_id` / `episode_id` top-level columns power per-item history queries (`WHERE movie_id = ?` / `WHERE episode_id = ?`). Every variant that carries a resolvable content id is extracted — not just the obvious `MovieAdded` / `Imported` / `Watched`. Download-lifecycle variants (`ReleaseGrabbed`, `DownloadStarted`, `DownloadComplete`, `DownloadFailed`, `DownloadCancelled`, `DownloadPaused`, `DownloadResumed`, `DownloadMetadataReady`) only carry `download_id` on the wire, so the history listener joins `download_content` to find the linked movie or episode. Season-pack downloads map to the first linked episode; the full detail remains in the `data` blob.

### 2. WebSocket push

Connected UI clients receive events in real-time via the WebSocket connection at `ws://host/api/v1/ws`. Every frame is a serialized `AppEvent` — the same enum the backend uses for history + webhooks — so the frontend's `meta.invalidatedBy` layer narrows by `event.event` with no hand-rolled shadow types.

**Payload:**
```json
{
  "event": "imported",
  "movie_id": 42,
  "title": "The Matrix",
  "quality": "Bluray-1080p"
}
```

All events are pushed — the UI filters client-side based on what's relevant to the current view. No per-event configuration for WebSocket.

**Lag frames.** When a slow client can't keep up, `tokio::broadcast` returns `RecvError::Lagged(n)`. The WS handler converts that to an `AppEvent::Lagged { skipped: n }` frame — same on-the-wire shape as every other event, so the frontend handles it via the shared dispatcher rather than a bespoke string check.

### 3. Webhooks

For each event, iterate over enabled WebhookTarget entities where the matching event flag is true (e.g., `on_import = true`).

**Template rendering:**

The `body_template` field on WebhookTarget contains a JSON string with `{{placeholders}}` that get replaced with event data:

```json
{
  "content": "{{event_emoji}} **{{title}}** — {{quality}}\n{{event_description}}"
}
```

**Available placeholders:**

| Placeholder | Description | Example |
|---|---|---|
| `{{event}}` | Event type | `imported` |
| `{{event_description}}` | Human-readable description | "Added to library" |
| `{{title}}` | Movie or episode title | "The Matrix" |
| `{{show}}` | Show name (TV only) | "Breaking Bad" |
| `{{season}}` | Season number (TV only) | "5" |
| `{{episode}}` | Episode number (TV only) | "14" |
| `{{quality}}` | Quality string | "Bluray-1080p" |
| `{{year}}` | Release year | "1999" |
| `{{size}}` | File size | "4.2 GB" |
| `{{indexer}}` | Indexer name | "My Indexer" |
| `{{message}}` | Health/error message | "VPN disconnected" |
| `{{url}}` | Kino web UI URL for the content | "http://kino:8080/movie/42" |

If no `body_template` is set, a default JSON payload is sent containing all available fields for the event. This works as-is with generic webhook receivers and ntfy.

**Escaping:** Placeholder values are JSON-escaped when inserted into a JSON template (quotes, backslashes, newlines escaped). For plain text templates, values are inserted as-is. This prevents movie titles containing special characters from breaking the JSON structure.

**Delivery:**

```
POST {webhook_target.url}
Headers: {webhook_target.headers}  (JSON object, e.g. {"Authorization": "Bearer ..."})
Body: rendered template
```

**Examples for common services:**

Discord:
```json
{"content": "🎬 **The Matrix** — Bluray-1080p added to library"}
```

Telegram Bot API (url includes the bot token and method):
```
URL: https://api.telegram.org/bot{token}/sendMessage
Body: {"chat_id": "123456", "text": "The Matrix — Bluray-1080p added to library", "parse_mode": "Markdown"}
```

ntfy (url includes the topic):
```
URL: https://ntfy.sh/my-kino-topic
Body: The Matrix — Bluray-1080p added to library
```

For ntfy, the `body_template` would just be `{{title}} — {{quality}} {{event_description}}` (plain text, not JSON). The `Content-Type` header can be set to `text/plain` via the `headers` field.

## Failure handling

If a webhook POST fails (timeout, 4xx, 5xx), the target advances one rung up the retry ladder and we persist `initial_failure_time` (only if not already set), `most_recent_failure_time`, `escalation_level`, and `disabled_until = now + backoff`.

| Level | Next attempt after | Notes |
|---|---|---|
| 0 → 1 | 30 seconds | First failure — assume transient (DNS blip, short upstream outage). |
| 1 → 2 | 15 minutes | |
| 2 → 3 | 1 hour | |
| 3 → 4 | 4 hours | |
| 4 → 5 | 24 hours | Give-up rung. Fires one `HealthWarning` on this transition so the operator sees "Webhook 'X' has failed 5 times in a row" in the UI and via other healthy targets (`on_health_issue = 1`). Level stays pinned at 5; further failures stay silent. |

Events that occur while a target is disabled are not queued — they're lost for that target. History and WebSocket still have them. When the target is re-enabled by `webhook_retry` (or by a successful delivery), it resumes receiving new events only.

**Recovery is automatic.** Any successful delivery clears `initial_failure_time`, `most_recent_failure_time`, resets `escalation_level = 0`, and nulls `disabled_until`. The next failure starts from the bottom of the ladder — a target that recovers after a brief outage shouldn't stay on a 24-hour backoff because it once flapped a month ago.

**Scheduler HealthWarning dedup.** Any scheduled task failure (indexer fetch, TMDB refresh, Trakt sync) also emits a `HealthWarning` via the scheduler. To avoid 60+ identical warnings per hour for the same flapping task, emission is gated on transitions: `HealthWarning` fires once on OK → Err, `HealthRecovered` fires once on Err → OK. Same shape as the disk-space sweep in `cleanup::disk_space_sweep`.

## Entities touched

- **Creates:** History (every event)
- **Reads:** WebhookTarget (which targets to fire, templates, event filters), Movie/Episode/Show (for template placeholders), Config (base_url for `{{url}}` placeholder)
- **Updates:** WebhookTarget (failure tracking: `initial_failure_time`, `most_recent_failure_time`, `escalation_level`, `disabled_until`)

## Dependencies

- All other subsystems (emit events)
- WebSocket connections (managed by API subsystem)
- HTTP client (for webhook delivery)
- History table (event persistence)

## Error states

- **No webhook targets configured** → events still logged to History and pushed via WebSocket
- **All targets disabled** → health warning pushed to WebSocket, visible in UI notification feed
- **Template rendering fails** (missing placeholder) → send default payload, log warning
- **WebSocket has no connected clients** → events discarded (no buffering), History is the durable store
