# Server-side Cast sender

> **Status (2026-04-27): shipped.** Backend module at
> `backend/crates/kino/src/cast_sender/{discovery,device,session,handlers,mod}.rs`
> (~1275 LOC); routes registered in `main.rs` —
> `/api/v1/cast/devices`, `/api/v1/cast/sessions`,
> `/api/v1/cast/sessions/{id}/{play|pause|seek}`. Frontend wires
> `frontend/src/components/cast/{CastButton,CastPopover,CastMiniBar}.tsx`,
> `state/cast-store.ts`, the player chrome `CastButton`,
> `useCastHandoff.ts`. Mounted in `TopNav.tsx` + the player
> `ControlBar.tsx`. Auth path covered by
> `flow_tests/cast_token.rs`.

A Rust Cast-protocol sender that runs inside the kino backend, so any browser — Firefox included — can initiate and control a Chromecast session. The client UI only hits kino's REST/WebSocket API; kino speaks Cast directly to the device.

## What problem it solves

Google's Cast sender SDK is only shipped to Chromium (Chrome, Edge, and derivatives). Firefox and Safari have no Cast API. That limitation is browser-side: the Cast wire protocol is well-documented TLS + Protocol Buffers and has multiple mature open-source implementations (`pychromecast` powers Home Assistant, `node-castv2` powers dozens of Node tools, `rust-cast` is the Rust port).

Putting the sender on the server sidesteps every browser limitation and collapses every client to one path: the web app, the Capacitor apps, and eventually a CLI all talk to `/api/v1/cast/*`, not `chrome.cast.*`. The receiver app is unaffected — same custom Web Receiver spec'd in `11-cast.md`.

This also gives a single place to persist Cast session state so a backend restart can re-join an already-playing session on the TV.

## Scope

**In scope:**
- Device discovery via mDNS + manual add-by-IP fallback
- Launch / stop the kino custom receiver
- `LOAD`, `PLAY`, `PAUSE`, `STOP`, `SEEK`, `SET_VOLUME`, `GET_STATUS`
- Full queue ops (`QUEUE_LOAD`, `QUEUE_INSERT`, `QUEUE_UPDATE`, `QUEUE_REMOVE`, `QUEUE_NEXT`, `QUEUE_PREV`)
- Authenticated streaming via `custom_data` on `LOAD`
- Custom `urn:x-cast:dev.kino.*` message channel between sender and receiver
- Heartbeat + reconnect + session resume after network blips
- REST API + WebSocket status relay to the web/mobile UI

**Out of scope:**
- The Cast *receiver* itself — lives in `11-cast.md`
- DIAL, AirPlay (see `30-native-clients.md` for AirPlay)
- Multi-zone / speaker-group specific behaviours (they work as generic devices, we don't expose group-only APIs)
- Casting from a browser using `chrome.cast.*` — deliberately replaced by this subsystem
- Runtime Cast SDK updates from Google (the protocol is stable; we don't poll for breaking changes)

## Architecture

```
 Browser / Capacitor ──POST /cast/sessions──►  kino backend
                   ◄──WS: mediastatus/…────         │
                                                    │  TLS + protobuf
                                                    ▼  (port 8009)
                                              Chromecast
                                                    │
                                                    │  HTTPS
                                                    ▼
                                        receiver.html (same origin as kino)
                                                    │
                                                    │  HLS + kino auth token
                                                    ▼
                                              kino /api/v1/playback
```

All Cast protocol chatter lives on the backend. The UI only ever sees JSON over HTTP/WebSocket.

### Library choices

- **`rust_cast`** (v0.22+) — speaks the Cast V2 protobuf protocol over `rustls`. Gives us typed `connection`, `heartbeat`, `receiver`, and `media` channels, plus a public `MessageManager::send()` for raw JSON messages we need for the queue ops the crate doesn't wrap. Pure Rust, no external binaries.
- **`mdns-sd`** — pure-Rust Multicast DNS-SD browser. Same crate already used by the `25-mdns-discovery.md` *responder*, reused here as a *browser*. No Avahi dependency.

Both crates are blocking (`std::net::TcpStream`, `std::thread`). Bridged to tokio via dedicated OS threads per active session — see §Concurrency model.

## 1. Discovery

### mDNS browse

Chromecast devices announce `_googlecast._tcp.local.` with:

- **SRV**: host + port `8009`
- **A / AAAA**: LAN IP(s)
- **TXT**: `fn=<friendly name>`, `md=<model>`, `id=<uuid>`, `ic=<icon path>`, `ve=<version>`

The discovery service runs a single `ServiceDaemon` for the lifetime of the backend:

```rust
let mdns = ServiceDaemon::new()?;
let rx  = mdns.browse("_googlecast._tcp.local.")?;
while let Ok(event) = rx.recv_async().await {
    match event {
        ServiceEvent::ServiceResolved(info) => cache.upsert(info.into()),
        ServiceEvent::ServiceRemoved(_, fullname) => cache.mark_missing(&fullname),
        _ => {}
    }
}
```

Cache entries carry `last_seen`. Anything not re-announced for 60s drops from `GET /api/v1/cast/devices`. The mDNS daemon itself handles the goodbye/TTL protocol — we just react to its events.

### Docker caveat

mDNS is link-local multicast (`224.0.0.251:5353`). Docker's default bridge network drops multicast host→container, so inside kino's bridged devcontainer — and any Docker channel install running without `network_mode: host` — the browse loop returns nothing. The native binary on the host doesn't hit this.

Behaviour per deployment:

| Deployment | Behaviour |
|---|---|
| Native binary (Linux / macOS / Windows) | Works |
| systemd service | Works |
| Docker `network_mode: host` | Works |
| Docker bridge | No discovery; users rely on manual add |
| Docker macvlan | Works |

Install docs recommend `network_mode: host` for Docker users for the same reason `25-mdns-discovery.md` does. The manual-add path (below) exists regardless.

### Manual add-by-IP

```
POST /api/v1/cast/devices
{"ip": "10.0.1.42"}
```

Backend opens a Cast connection to `10.0.1.42:8009`, calls `receiver.get_status()`, and reads the friendly name out of the returned `ReceiverStatus`. On success the device is persisted to the `cast_device` table and returned in subsequent `GET /api/v1/cast/devices` listings, flagged `source = manual`.

Covers: mDNS-suppressing routers, VLAN boundaries, bridged Docker installs, devices on a different subnet reachable via static route.

## 2. Session lifecycle

### Connect → launch → load

```
POST /api/v1/cast/sessions
{"device_id": "abcd", "media_id": 42}
```

Per request, a worker thread:

1. `CastDevice::connect(host, 8009)` — TLS handshake against native roots. Chromecast presents a Google-signed cert; host verification passes. Falls back to `connect_without_host_verification` only on cert-mismatch errors.
2. `connection.connect(DEFAULT_RECEIVER_ID)` + kick off a heartbeat loop.
3. `receiver.get_status()` — is the kino receiver already running?
   - **Yes** → skip launch; cache its `transport_id` + `session_id` from the status. This is the "join" path.
   - **No** → `receiver.launch(KINO_APP_ID)`. The returned `Application` has `transport_id` and `session_id`.
4. `connection.connect(transport_id)` — open a virtual connection to the app session (without this, the media channel doesn't route).
5. `media.load_with_opts(transport_id, session_id, &media)` with:
   - `contentId = https://{kino-host}/api/v1/playback/{media_id}/hls/master.m3u8`
   - `contentType = "application/vnd.apple.mpegurl"`
   - `metadata = Metadata::TvShow { series_title, season, episode, images, … }` or `Metadata::Movie`
   - `custom_data = { "kino_auth": "<short-lived JWT>", "start_position_ms": 0 }`

`KINO_APP_ID` is the $5-registered receiver ID configured in the Cast Developer Console. Same value drives the manifest in `11-cast.md`.

### Control

```
POST /api/v1/cast/sessions/{id}/play
POST /api/v1/cast/sessions/{id}/pause
POST /api/v1/cast/sessions/{id}/seek  {"position_ms": 123456}
POST /api/v1/cast/sessions/{id}/volume {"level": 0.4}
POST /api/v1/cast/sessions/{id}/stop
```

Each endpoint resolves the backend's session-state handle, forwards the call to the worker thread, and returns once the Cast reply round-trip completes (usually <100ms LAN).

### Status relay

The worker's `CastDevice::receive()` loop emits `ChannelMessage::Media(MediaStatus)` events. Each event is:

1. Converted to a JSON `CastStatus { position_ms, player_state, current_item_id, volume, … }`.
2. Broadcast on the existing kino WebSocket hub, filtered by session id.
3. Persisted into `cast_session.last_status_json` so reconnecting clients get last-known state immediately.

The UI listens on `/ws` for `cast.status` events, same pattern as playback/download events today.

### Session teardown

`DELETE /api/v1/cast/sessions/{id}` → `receiver.stop(session_id)`, close the TLS connection, mark session `ended` in DB, stop the worker thread. If the device went away or TCP dropped, the reconnect loop eventually times out and transitions to `ended` automatically (see §Reconnection).

## 3. Reconnection

Cast connections die on network hiccups all the time — Wi-Fi roaming, device sleep, router restarts, iptables flaps on Docker hosts. The sender is responsible for recovering transparently.

Values are copied from pychromecast's production-hardened `socket_client.py`:

| Constant | Value | Purpose |
|---|---|---|
| `SELECT_TIMEOUT` | 5s | Blocking socket read timeout |
| `HEARTBEAT_TIMEOUT` | 30s | No PONG within this → drop + reconnect |
| `PING_INTERVAL` | 5s | Send PING every 5s regardless of traffic |
| `RETRY_INITIAL` | 5s | First reconnect delay |
| `RETRY_MAX` | 300s | Backoff cap (5 min) |

Backoff: `delay = min(delay * 2, RETRY_MAX)` until a successful handshake resets it to `RETRY_INITIAL`.

### Join vs re-launch

On every successful reconnect the worker does `receiver.get_status()` and inspects the returned applications:

- **kino app still running** with our `app_id` → grab the current `transport_id` + `session_id`, `connection.connect(transport_id)` on the media namespace, resume status relay. The receiver's playback is uninterrupted.
- **app not running** → the receiver exited (user closed it, or another sender hijacked the device). Emit `cast.session_ended` on the WebSocket and transition the DB row to `ended`. Do *not* auto-relaunch — that's a UX decision left to the sender.

This is Google's recommended "join" pattern, equivalent to Chromium's `requestSessionById`.

### Full-backend restart

`cast_session` rows persist `{device_id, transport_id, session_id, kino_media_id, last_position_ms}`. On startup, the cast service iterates `status = active` rows and attempts a `get_status`-then-join against each device. Any that fail after the backoff loop are closed out and marked `ended`.

## 4. Queue operations

### Built-ins

`rust_cast` already wraps `QUEUE_LOAD` via `MediaChannel::load_queue` / `load_with_queue`. That's enough to start a queue at session create time:

```rust
media.load_with_queue(
    transport_id,
    session_id,
    &first_media,
    &next_media_list,
)?;
```

### Raw helpers for the rest

Every queue message uses the same namespace as `LOAD` (`urn:x-cast:com.google.cast.media`) and the same envelope `{type, requestId, mediaSessionId, …}`. `rust_cast`'s `MessageManager::send` accepts arbitrary namespace + payload, so the missing ops are thin JSON helpers rather than a library fork:

```rust
fn queue_insert(&self, media_session_id: i64, items: Vec<QueueItem>, insert_before: Option<i64>) -> Result<()>;
fn queue_update(&self, media_session_id: i64, items: Vec<QueueItemUpdate>) -> Result<()>;
fn queue_remove(&self, media_session_id: i64, item_ids: Vec<i64>) -> Result<()>;
fn queue_next(&self, media_session_id: i64) -> Result<()>;
fn queue_prev(&self, media_session_id: i64) -> Result<()>;
```

Replies arrive as `ChannelMessage::Raw(CastMessage)` because the crate's typed parser doesn't know these types; we match on `type` in the JSON string and emit typed events to the WebSocket relay.

kino uses this for:
- **Up Next binge**: `QUEUE_LOAD` on session start with the next 3 episodes of the current season.
- **User "add to queue"**: `QUEUE_INSERT` from the UI's context menu.
- **Re-order**: `QUEUE_UPDATE` with a new `orderId`.

Playback across queue items is receiver-side: the custom Web Receiver auto-advances without kino doing anything, then reports a new `MEDIA_STATUS` with the next `currentItemId`.

## 5. Custom message channel

A dedicated namespace `urn:x-cast:dev.kino` carries sender↔receiver messages that don't fit the standard media model. Examples:

| Direction | Type | Purpose |
|---|---|---|
| → receiver | `kino.show_skip_button` | Render Skip Intro button on the TV (fallback when sender auto-skip is off) |
| → receiver | `kino.refresh_metadata` | Pull updated metadata for the current item (e.g. after Trakt re-fetch) |
| ← receiver | `kino.position_tick` | High-frequency position updates for the trickplay scrubber |
| ← receiver | `kino.error_diagnostic` | Player-side diagnostic payload when a stream fails, so kino can log the real browser-side reason |

On the sender side these are `MessageManager::send(CastMessage { namespace: "urn:x-cast:dev.kino", … })`. On the receiver side the CAF `CastReceiverContext.addCustomMessageListener` hook delivers them to the receiver JS.

The media namespace stays reserved for Google-defined messages only; everything kino-specific lives here.

## 6. Concurrency model

`rust_cast` is blocking. The kino backend is tokio. Bridging:

- One **dedicated `std::thread` per active Cast session**, owning its `CastDevice` and its `MessageManager`.
- Thread's main loop:
  1. `select!`-style poll on: inbound `CastDevice::receive()` events, outbound command channel (`crossbeam::channel::Receiver`), heartbeat interval, reconnect timer.
  2. Events are pushed into a `tokio::sync::mpsc::Sender` that feeds the main runtime.
  3. Commands come from axum handlers via a `tokio::sync::oneshot` reply channel per request.
- Discovery daemon runs similarly — one long-lived thread wrapping `mdns_sd::ServiceDaemon`.

Total thread count: `1 (mdns) + N (active sessions)`. N is bounded — a home install rarely has more than 2 simultaneous casts — and spawning is cheap. Alternatives (tokio-based Cast client, async rust-cast fork) were considered but the blocking model is well-contained and lifts the library choice constraint.

## 7. API

```
GET    /api/v1/cast/devices                 # mDNS + manual-added devices
POST   /api/v1/cast/devices                 # manual add by IP
DELETE /api/v1/cast/devices/{id}            # forget a manual device

POST   /api/v1/cast/sessions                # start a session (launch+load)
GET    /api/v1/cast/sessions/{id}           # current status
DELETE /api/v1/cast/sessions/{id}           # stop + close

POST   /api/v1/cast/sessions/{id}/play
POST   /api/v1/cast/sessions/{id}/pause
POST   /api/v1/cast/sessions/{id}/stop
POST   /api/v1/cast/sessions/{id}/seek      {"position_ms": 123456}
POST   /api/v1/cast/sessions/{id}/volume    {"level": 0.6, "muted": false}

POST   /api/v1/cast/sessions/{id}/queue/load    {"items": [...]}
POST   /api/v1/cast/sessions/{id}/queue/insert  {"items": [...], "before": 12}
PATCH  /api/v1/cast/sessions/{id}/queue         {"reorder": [...], "shuffle": true}
DELETE /api/v1/cast/sessions/{id}/queue/{item_id}
POST   /api/v1/cast/sessions/{id}/queue/next
POST   /api/v1/cast/sessions/{id}/queue/prev
```

WebSocket frames on `/ws`:

```json
{"type": "cast.status",         "session_id": "…", "data": { … MediaStatus … }}
{"type": "cast.session_ended",  "session_id": "…", "reason": "receiver_exit"}
{"type": "cast.device_added",   "device": { … }}
{"type": "cast.device_removed", "device_id": "…"}
```

All existing REST/WS shapes (auth via API key / JWT, error envelope) are reused unchanged.

## 8. Schema

Two new tables.

```sql
CREATE TABLE cast_device (
    id            TEXT PRIMARY KEY,         -- mDNS id, or "manual:<uuid>"
    name          TEXT NOT NULL,
    ip            TEXT NOT NULL,
    port          INTEGER NOT NULL DEFAULT 8009,
    model         TEXT,
    source        TEXT NOT NULL,            -- 'mdns' | 'manual'
    last_seen     TEXT,                     -- ISO 8601
    created_at    TEXT NOT NULL
);

CREATE TABLE cast_session (
    id                  TEXT PRIMARY KEY,
    device_id           TEXT NOT NULL REFERENCES cast_device(id) ON DELETE CASCADE,
    transport_id        TEXT,
    session_id          TEXT,
    media_id            INTEGER REFERENCES media(id) ON DELETE SET NULL,
    current_item_id     INTEGER,            -- queue item
    status              TEXT NOT NULL,      -- 'starting' | 'active' | 'ended' | 'errored'
    last_status_json    TEXT,               -- most recent MEDIA_STATUS for fast UI rehydrate
    last_position_ms    INTEGER,
    started_at          TEXT NOT NULL,
    ended_at            TEXT
);

CREATE INDEX idx_cast_session_status ON cast_session(status);
CREATE INDEX idx_cast_session_device ON cast_session(device_id);
```

`cast_device` with `source='mdns'` is ephemeral — rows are re-populated from mDNS on startup. `source='manual'` rows are authoritative user config and persist across restarts.

`cast_session` is authoritative. Session resume after backend restart reads `status='active'` rows and attempts to reattach (see §Reconnection).

## 9. Receiver coordination

This subsystem expects the receiver app spec'd in `11-cast.md` to:

- Read `message.customData.kino_auth` on every `LOAD` and attach it as `Authorization: Bearer <token>` to all HLS segment + manifest requests.
- Respect `customData.start_position_ms` when presented on `LOAD` (resume playback).
- Subscribe to `urn:x-cast:dev.kino` for the custom messages in §5.
- Report position back via the same channel at ≥2 Hz so the sender UI can render an accurate scrubber without Cast's own polling.
- Auto-advance across queue items and emit `MEDIA_STATUS` with the new item.

No protocol extensions beyond stock CAF — every hook above is a first-class CAF API.

## 10. Error states

| Situation | Behaviour |
|---|---|
| mDNS browse returns nothing on bridged Docker | No error. Users add devices manually. Health dashboard warning if `source='mdns'` devices = 0 *and* host networking is not detected |
| Device IP wrong on manual add | Connect fails fast (TCP refused / timeout). Return 400 with the socket error |
| TLS handshake fails | Retry once with `connect_without_host_verification`; log; on second failure, surface "device unreachable" |
| Launch returns `LAUNCH_ERROR` | Most common cause: receiver app ID not registered / developer account in a bad state. Surface a user-actionable message; Cast Console link in the error response |
| `LOAD_FAILED` from receiver | Usually an auth or codec issue. Log the receiver's `detailedErrorCode`; receiver-side diagnostic pushed via `kino.error_diagnostic` is attached to the history entry |
| Heartbeat timeout | Enter reconnect loop. UI sees `cast.session_ended` only after full backoff ladder exhausts (max 5 min) |
| Kino backend restart mid-session | Reattach on boot via persisted `{transport_id, session_id}`; if receiver app already exited, mark ended |
| Another sender hijacks the device | Receiver's `RECEIVER_STATUS` drops our app → worker detects, emits `session_ended`, closes socket |
| Receiver deployment updated mid-session | Transparent: receiver-side reload is a browser-level concern, protocol session continues |

All errors land in the `history` table via the existing event pipeline, same shape as download/import errors.

## 11. Testing

- **Unit**: JSON shape fixtures for each queue message, parsed back to confirm receiver-side compatibility.
- **Protocol replay**: canned PCAPs from a real Chromecast handshake + LOAD, replayed against the sender to verify parse. PCAPs live in `backend/tests/fixtures/cast/`.
- **Integration**: the devcontainer can't reach LAN Chromecasts by default (bridge network). Dev uses `KINO_CAST_DEV_DEVICE_IP` env var to point at a real device on the host LAN; the manual-add path exercises the full flow end-to-end.
- **CI**: no hardware available. Protocol-level tests only. The blocking `rust_cast` calls are mocked behind a trait for handler-level tests.

## Entities touched

- **Creates:** `cast_device`, `cast_session` rows
- **Reads:** `media`, `episode`, `movie` (metadata payload for `LOAD`), `config` (Cast app ID, mDNS enable flag)
- **Updates:** `cast_session.status`, `cast_session.last_status_json`, `cast_session.last_position_ms`
- **Writes to WebSocket hub:** `cast.status`, `cast.session_ended`, `cast.device_added`, `cast.device_removed`

## Dependencies

- `rust_cast` 0.22+ (Cast protocol over rustls + protobuf)
- `mdns-sd` (same dep as 25-mdns-discovery.md)
- `rustls`, `tokio`, `serde_json` (already in the workspace)
- Existing JWT issuance for the `kino_auth` token on `LOAD`

No new system binaries. No external services. No Google SDK.

Fixed cost for the end user: **$5 one-time** Cast Console registration for the receiver app ID. That cost is shared with the receiver subsystem (`11-cast.md`); this subsystem adds nothing recurring.

## Known limitations

- **Firefox-on-PopOS and other Chromium-gap browsers** still need kino to be reachable (obviously). The point is they don't need to run the Cast SDK themselves.
- **Docker bridge installs lose mDNS discovery.** Manual add works. The install docs recommend host networking.
- **Blocking library** — one OS thread per active session. Fine at home-install scale; would need revisiting at thousands of concurrent sessions, which is not kino's target.
- **Receiver app changes still require a manual redeploy** to the HTTPS URL registered with Cast Console. Not specific to the sender; noted so readers understand the end-to-end update path.
- **Heartbeat timeout is 30s** — a real network blip can freeze the UI scrubber briefly before the reconnect + re-status arrives. Acceptable tradeoff vs aggressive timeouts that reconnect on every Wi-Fi frame loss.
- **Cast has no native resume-from-DRM-state** — reloading a DRM session after reconnect means a fresh licence fetch. kino's receiver does not use DRM, so this is theoretical.

## Deliberately out of scope

- **Browser-side `chrome.cast.*` support.** Explicitly replaced. The server-side sender is the only path.
- **Cast v1 / DIAL.** Legacy, unnecessary — every Chromecast supports v2.
- **Non-Cast protocols (DLNA, UPnP).** Different subsystem if ever wanted.
- **AirPlay sender.** See `30-native-clients.md`; it's an iOS-only concern solved by `AVRoutePickerView` on the native client, not by kino's backend.
- **Cast Connect / Android app JOIN.** Android Cast apps can hand off to an Android TV kino app directly; that's a client-side concern covered in `30-native-clients.md`.

## Cross-references

- **11-cast.md** — the *receiver* spec. This subsystem is the sender half that talks to that receiver.
- **25-mdns-discovery.md** — shares `mdns-sd`; this subsystem *browses*, that one *responds*. Same Docker-networking caveat applies.
- **05-playback.md** — the HLS URLs the Cast receiver plays are the same ones the browser player uses.
- **09-api.md** — the REST/WebSocket surface this subsystem registers endpoints on.
- **30-native-clients.md** — the native Capacitor apps consume this subsystem's `/cast/*` API instead of bundling the native Cast SDKs. That's a meaningful simplification of the Capacitor custom-plugin work tracked there.
