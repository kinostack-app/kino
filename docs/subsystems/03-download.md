# Download subsystem

Built-in BitTorrent client over a WireGuard VPN tunnel. Manages the full torrent lifecycle from grab to seeding completion.

Uses librqbit (Rust BitTorrent library) for the torrent engine and a userspace WireGuard tunnel via **boringtun** for the VPN. Only torrent traffic goes through the VPN — the web UI, API, and streaming traffic use the normal network.

Why boringtun-only: the kernel WireGuard module gives a small throughput win on Linux but adds platform branching, a capability-probe, and a whole second code path. Torrent traffic rarely saturates a CPU that can't handle userspace crypto at the ~10 % overhead boringtun adds, and boringtun runs unchanged on Linux, macOS, and Windows — which matters once the native-binary rollout (subsystem 21) lands.

The VPN implementation targets Linux only today. macOS / Windows are unblocked by boringtun at the protocol layer but still need platform-specific default-route and TUN-device plumbing (subsystem 21).

## Responsibilities

- Manage VPN tunnel lifecycle (connect, maintain, reconnect)
- ProtonVPN NAT-PMP port forwarding (request, refresh, handle rotation)
- Add torrents from magnet links or .torrent URLs
- Select specific files from season packs
- Monitor download progress
- Detect stalled/dead torrents and trigger retry
- Manage concurrent download queue
- Seed to configured ratio/time limits
- Persist state and resume after restart
- Enforce bandwidth limits

## VPN tunnel

### Split tunneling via bind_device_name

librqbit supports `bind_device_name` in its session options, which uses `SO_BINDTODEVICE` on Linux to force all BitTorrent sockets (peer connections, DHT, tracker communication) onto a specific network interface.

Setup:

1. Create userspace WireGuard tunnel (`wg0`) via boringtun
2. Configure with provider credentials: private key, server endpoint, peer public key, allowed IPs = `0.0.0.0/0`
3. Assign VPN IP address to the interface
4. Start librqbit session with `bind_device_name: Some("wg0")`

Result: all torrent traffic routes through VPN. API server, web UI, and video streaming bind to the normal interface untouched.

### VPN lifecycle

- **Startup:** VPN connects before librqbit session starts. No torrents can run without VPN.
- **Health check:** Periodic ping to VPN gateway (every 30s). If unreachable, pause all active torrents.
- **Reconnect:** If tunnel drops, attempt reconnect with exponential backoff. Resume torrents once reconnected.
- **Disabled:** If `vpn_enabled = false` in Config, librqbit runs without VPN binding. User's choice — not recommended but supported.

## Port forwarding

Port forwarding is essential for torrent performance. Without it, only outgoing peer connections work — incoming connections are blocked by the VPN's NAT. This means fewer peers, slower downloads, and potentially no connectivity at all for less popular content.

### Provider support

The WireGuard tunnel is generic — any provider that gives you a WireGuard config works. Port forwarding is provider-specific, implemented as a pluggable interface (`PortForwarder` trait):

| Provider | Protocol | Implementation |
|---|---|---|
| ProtonVPN / Mullvad-style | NAT-PMP (RFC 6886) | `NatPmpForwarder` |
| AirVPN | Static port (user pastes pre-allocated port from AirVPN dashboard) | `AirVpnForwarder` |
| No forwarding | — | user sets provider to `none`; tunnel still protects the IP, no inbound peer connections |

User selects their provider in Config. Each implementation is small (~100-200 lines) and independent.

**PIA is not shipped.** Their port-forward API needs a TLS-skip-hostname workaround plus a 15-minute keep-alive loop plus ~60-day full re-exchange, which adds meaningful surface area. Users on PIA should run gluetun in front of kino or skip port forwarding.

### Lifecycle (NAT-PMP example, others follow the same pattern)

```
VPN connected
    ↓
Request port mapping from gateway
    → receive assigned external port
    ↓
Configure librqbit: announce_port = assigned_port
    ↓
Refresh loop (every 45s):
    → renew mapping
    → if port unchanged: continue
    → if port changed: update announce_port, force re-announce to all trackers
    → if failed: retry with backoff, pause torrents if gateway unreachable
```

### Port change handling

Port changes are rare but possible if a refresh lapses. When detected:

1. Update librqbit's `announce_port` to the new port
2. Force tracker re-announce on all active torrents so peers can find us at the new port
3. Log the event
4. Fire notification (health event)

librqbit performs periodic tracker announces anyway, so a brief gap during port change is acceptable.

## Torrent lifecycle

### Adding a torrent

When Search grabs a release:

1. Check concurrent download limit — if at limit, queue the torrent
2. Verify VPN is connected and port is forwarded
3. Add to librqbit session:
   ```
   session.add_torrent(
       AddTorrent::from_url(magnet_or_torrent_url),
       AddTorrentOptions {
           only_files: file_indices,    // for season packs: only wanted episodes
           output_folder: download_path,
           paused: is_queued,           // true if over concurrent limit
       }
   )
   ```
4. Create/update Download entity with `state: downloading` (or `state: queued`)
5. Create DownloadContent links to the target movies/episodes
6. Set target movies/episodes to `status: downloading`

### File selection for season packs

When a season pack is grabbed for specific episodes:

1. Wait for torrent metadata (magnet links need to resolve first)
2. Match torrent file list against wanted episodes using filename parsing
3. Set `only_files` to the indices of matching files
4. Unwanted files are never downloaded — saves bandwidth and disk space

### Progress monitoring

Poll librqbit stats every 10 seconds for each active torrent. Update Download entity:
- `downloaded`, `uploaded`, `download_speed`, `upload_speed`
- `seeders`, `leechers`
- `eta` (computed from speed + remaining)
- `state` transitions

Push updates to connected web UI clients via WebSocket.

### Completion

When a torrent finishes downloading:

1. Update Download `state: completed`, set `completed_at`
2. Hand off to Import subsystem (next in pipeline)
3. Continue seeding

### Seeding

After import, the torrent seeds until limits are reached:

- **Ratio limit:** `seed_ratio_limit` from Config (default 1.0). Stop when upload/download ratio exceeds this.
- **Time limit:** `seed_time_limit` from Config (default 0 = unlimited). Stop after N minutes of seeding.
- Whichever limit is reached first triggers stop.

When seeding completes:
1. Remove torrent from librqbit session
2. Delete source files in download directory (the library copy exists via hardlink/copy from Import)
3. Update Download `state: imported` (or `state: cleaned_up` after file deletion)

## Stall detection

Kino implements its own stall detection on top of librqbit, since librqbit doesn't have built-in stall handling.

Poll stats every 30 seconds. Classify torrent health using configurable thresholds from Config (`stall_timeout` default 30 min, `dead_timeout` default 60 min):

| Condition | Classification | Action |
|-----------|---------------|--------|
| `download_speed > 0` | **Healthy** | Continue |
| `download_speed == 0` for < `stall_timeout / 2` | **Slow** | Wait |
| `download_speed == 0` for > `stall_timeout / 2` AND `peers > 0` | **Stalled** | Force re-announce to trackers, request more peers from DHT |
| `download_speed == 0` for > `stall_timeout` OR `peers == 0` for > `dead_timeout` | **Dead** | Fail the download |

Thresholds are deliberately generous by default. Niche content with intermittent seeders needs patience. Users can tighten them in Config.

When a download is marked **dead**:

1. Update Download `state: failed`, set error message
2. Blocklist the release (torrent hash + title)
3. Log History event (failed)
4. Fire notification (download failed)
5. Trigger Search to find the next best release for the same content
6. If Search finds an alternative → new Download starts automatically

## Queue management

librqbit doesn't have built-in queueing. Kino manages this:

- `max_concurrent_downloads` from Config (default 3)
- New grabs beyond the limit are added to librqbit as **paused**
- Queue is priority-ordered: user-requested content first, then upgrades, then by request time
- When an active torrent completes or fails, the next queued torrent is unpaused
- Queue state is persisted via the Download entity (`state: queued`)

## Resume after restart

librqbit handles torrent persistence natively:

- Session configured with `SessionPersistenceConfig::Json`
- On startup, all torrents are re-added from persistence with `fastresume: true` (skip full integrity check, use saved bitfield)
- Kino's download manager reconciles its Download entities with librqbit's restored session
- VPN must reconnect and port forwarding must re-establish before resuming active torrents

## Bandwidth limits

librqbit supports token-bucket rate limiting:

- `download_speed_limit` and `upload_speed_limit` from Config
- Applied at the session level (across all torrents)
- 0 = unlimited

## Entities touched

- **Reads:** Config (VPN settings, download limits, seeding rules), Release (torrent URL/magnet for grabbed releases)
- **Writes:** Download (create, update state/progress/speed), DownloadContent (create links to movies/episodes), Movie/Episode (update status to `downloading`)
- **Triggers:** Import subsystem (on torrent completion), Search subsystem (on stall/failure for retry), Notification (state changes)

## Dependencies

- librqbit (BitTorrent engine)
- boringtun (userspace WireGuard tunnel)
- Config table (VPN credentials, limits, paths)
- Search subsystem (triggered on failure for retry)
- Import subsystem (triggered on completion)
- Notification subsystem (progress events, failures)

## Error states

- **VPN won't connect** → all downloads paused, health notification fired, retry with backoff
- **Port forwarding fails** → downloads continue but may be slow (no incoming connections). Retry port mapping. Notification fired.
- **Torrent has no peers** → stall detection handles this → fail → retry with different release
- **Disk full** → pause all downloads, fire notification, wait for cleanup or user action
- **librqbit crash/error** → log error, attempt session restart. Persistence ensures state is recoverable.
