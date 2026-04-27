# Health dashboard

> **Current status:** The `/health` page is shipped — storage,
> network, indexers, downloads, transcoder, scheduler, integrations,
> recent errors all render. Real-time updates are **poll-based
> today** (10s `refetchInterval` on the query); WebSocket delta
> patching is not yet wired, so values can lag by up to a tick
> after a state change. Good enough for launch; the WS path is a
> follow-up.

A unified `/health` page consolidating every real-time status signal Kino exposes. Replaces the current pattern where health info is scattered across seven settings pages. Pure observability — no actionable buttons here (users go to the relevant settings page to act).

## Scope

**In scope:**
- One dashboard page showing storage, network, indexers, downloads, transcoder, scheduler, integrations, recent errors
- Top-line status summary ("All systems operational" / "2 issues detected")
- Real-time updates via the existing WebSocket event stream
- Colour-coded status per panel (green / amber / red)
- A small status dot in TopNav that links here

**Out of scope:**
- Action buttons (restart VPN, reconnect indexer, etc.) — those live in settings
- Log viewer — separate concern, existing logs infrastructure
- Performance / per-request tracing — dev tool territory
- Historical charts / time-series graphs — adds significant implementation surface; v1 is instantaneous state only
- Alerting (notify when X goes bad) — covered by the notification subsystem already

## Panels

| Panel | Source | Degraded when | Red when |
|---|---|---|---|
| **Storage** | `df` on `media_path`, `download_path`, `data_path` | any path < 10% free | any path < 2% free |
| **VPN** *(when enabled)* | `03-download.md` VPN monitor | reconnect in progress | tunnel down > 2 min |
| **Indexers** | Per-indexer `indexer_status` table | any indexer at escalation level ≥ 2 | any indexer disabled |
| **Downloads** | librqbit session stats | >1 stalled torrent | >1 dead torrent in last hour |
| **Transcoder** | Playback subsystem session registry | queue depth > 2 | any failed session in last hour |
| **Scheduler** | `SchedulerState` table | any task failing its last run | any task failing N consecutive runs |
| **Metadata (TMDB)** | TMDB client rate-limit + circuit state | circuit half-open | circuit open |

Future panels:

- **Port forwarding** (VPN PMP / NAT-PMP handshake state) — covered by the VPN panel's aggregated status today; break out when/if we surface per-refresh port cadence.
- **OpenSubtitles** — waits on the integration shipping beyond the config toggle.
- **Trakt** — token-expiry already surfaces via the standalone Integrations page toast; fold in here when we need side-by-side scrobble queue depth.
- **Recent errors** — needs a structured-error tail source; `/logs` covers the operator view today.

Integrations panels auto-hide when the integration isn't configured (same principle as empty Home rows).

## Overall status synthesis

Top-line banner at the top of the dashboard computes a single summary from per-panel states:

- Any panel **red** → banner says **"N issues detected"** (red), lists them with links to anchors on the page
- Any panel **amber** but no red → **"Minor issues"** (amber)
- All panels **green** → **"All systems operational"** (green)

The TopNav status dot mirrors this:

- Green dot → all clear
- Amber → degraded (clickable → /health)
- Red → critical (clickable → /health, pulses gently)
- Hidden when dashboard feature is disabled (never — there's no reason to)

## Data model

### `health_snapshot` endpoint

A single aggregated endpoint composes all panels server-side:

```
GET /api/v1/health
```

Response shape:

```json
{
  "overall": "operational",  // "operational" | "degraded" | "critical"
  "checked_at": "2026-04-18T14:22:01Z",
  "panels": {
    "storage": {
      "status": "operational",
      "paths": [
        { "label": "Media library", "path": "/media", "free_bytes": 5.4e12, "total_bytes": 1.8e13 },
        { "label": "Downloads",     "path": "/downloads", "free_bytes": 2.1e11, "total_bytes": 2.0e12 },
        { "label": "Kino data",     "path": "/data", "free_bytes": 4.5e10, "total_bytes": 1.0e11 }
      ]
    },
    "vpn": {
      "status": "operational",
      "connected": true,
      "provider": "ProtonVPN",
      "server": "nl-free-42",
      "external_ip": "89.xx.xx.xx",
      "uptime_seconds": 86400
    },
    "port_forwarding": {
      "status": "operational",
      "port": 48291,
      "last_refresh": "2026-04-18T14:21:30Z",
      "next_refresh": "2026-04-18T14:22:15Z"
    },
    "indexers": {
      "status": "operational",
      "items": [
        { "name": "1337x", "status": "healthy", "last_success": "2026-04-18T14:20:12Z", "escalation_level": 0 },
        ...
      ]
    },
    "downloads": { ... },
    "transcoder": { ... },
    "scheduler": { ... },
    "metadata_tmdb": { ... },
    "opensubtitles": { ... },
    "trakt": { ... },
    "recent_errors": [
      { "subsystem": "search", "message": "...", "at": "..." },
      ...
    ]
  }
}
```

Each panel has a `status` field using the same three values. Frontend renders directly from this structure.

### Live updates

The existing WebSocket event stream (`state/websocket.ts`) already publishes events like `vpn.disconnected`, `download.stalled`, `indexer.disabled`, etc. The Health page subscribes to relevant event types and patches the affected panel without a full refetch. Full refetch every 30s as a fallback in case events are missed.

On initial page load: single `GET /api/v1/health` fetch populates everything.

### No new persistent schema

All data in the dashboard is computed from existing subsystem state or filesystem queries. **No new tables.** Health dashboard is purely a composition + presentation layer.

## Implementation

### Backend composition

`/api/v1/health` handler fetches each panel's data in parallel (tokio::join!) and assembles the response. If a single panel's check fails or times out (>500ms), it's returned with `status: "unknown"` rather than failing the whole endpoint. The page gracefully degrades to partial info.

Existing primitives to compose:

- `04-import.md` disk-space check (exists in Config / Cleanup)
- `03-download.md` VPN monitor, port forwarder, librqbit stats
- `14-indexer-engine.md` `indexer_status` table
- `05-playback.md` session registry (needs exposure if not already)
- `07-scheduler.md` `SchedulerState` table
- Recent errors: tail of a structured error log (implementation detail)

### Frontend

New route `frontend/src/routes/Health.tsx`. Single TanStack Query hook pulls `/api/v1/health`, WebSocket subscriber patches slices. Layout: responsive grid of panel cards (3-col desktop, 1-col mobile).

Each card reuses the same `HealthCard` component:

```tsx
<HealthCard
  title="VPN"
  status="operational"
  summary="Connected to nl-free-42"
  details={...}
  actionLink="/settings/vpn"
/>
```

`actionLink` is the "Manage" link (top-right of card) that takes user to the relevant settings page for actionable changes. Dashboard itself never mutates state.

### TopNav status dot

Small round indicator to the right of the Cast button in `components/layout/TopNav.tsx`. Green/amber/red based on `overall`. Accessible — `aria-label` announces the current state ("Status: all systems operational"). Clicking routes to `/health`.

Pulse animation only on red, and respects `prefers-reduced-motion`.

## UX

### Empty states per panel

- Storage panel when no library mounted yet (fresh install): shows `/data` only. Not an error.
- VPN panel when VPN disabled in config: hides the panel entirely. If someone runs sans VPN, they chose that — no "VPN: off" scolding.
- Integration panels when disconnected: hide entirely.
- Indexers when zero configured: shows "No indexers configured" with a link to setup.

### First-failure visibility

A panel going from green → amber/red triggers (via WebSocket event) a brief highlight animation on the card even if the user is on another route. Actually — scratch that; cross-route toasts belong to the notification subsystem, not here. The dashboard just reflects current state; notifications surface transitions.

The TopNav dot turning red is the only cross-route signal.

### Link-out pattern

Every panel card has a "Manage →" link top-right pointing to the relevant settings page:

| Panel | Link target |
|---|---|
| Storage | Settings → Library |
| VPN / Port forwarding | Settings → VPN |
| Indexers | Settings → Indexers |
| Downloads | Settings → Downloads (or /library/downloading for active) |
| Transcoder | Settings → Playback |
| Scheduler | Settings → Tasks |
| TMDB / OpenSubtitles | Settings → Metadata |
| Trakt | Settings → Integrations → Trakt |
| Recent errors | Settings → Logs |

Deep-links so the user goes straight to the actionable page without hunting.

## Entities touched

- **Reads:** Every subsystem's live state + selected tables (`indexer_status`, `SchedulerState`, `trakt_sync_state`, `trakt_scrobble_queue`, recent errors)
- **Writes:** None — pure observability

## Dependencies

- All subsystems listed in the panel table above (reads-only)
- Existing WebSocket event stream (for live updates)
- Existing filesystem `df` implementation (from `06-cleanup.md`)

No new system binaries, no new external services.

## Error states

- **Single panel data-collection fails** → panel shows `status: unknown` with a "Couldn't check (retry)" subtitle. Doesn't break the page.
- **Whole `/health` endpoint times out** → frontend keeps showing last-known values with a "Stale (x minutes ago)" banner.
- **WebSocket disconnected** → fall back to 30-second polling; reconnect attempts continue via existing reconnect logic.
- **No data at all on first load** (cold start before any collectors run) → skeleton cards with "Gathering..." labels.

## Known limitations

- **No historical view.** Dashboard shows current state only — "VPN was down 2 hours ago" requires checking logs, not this page. Adding time-series is a bigger feature (future consideration).
- **No alerting from here.** Notifications about state transitions come from the notification subsystem as they always have. Dashboard reflects, doesn't alert.
- **Panel set is hardcoded.** Users can't hide panels they don't care about (e.g. hiding Transcoder if they never transcode). Matches the "don't over-customise" philosophy — we picked the right panels, not every panel.
- **TopNav dot is always shown.** No toggle to hide it. Intentional — health should be glanceable; giving users a way to turn off the warning is a footgun.
- **Recent errors is last-10 only.** Full error history lives in logs. If 10 feels too few in practice, adjust — no schema change needed.
