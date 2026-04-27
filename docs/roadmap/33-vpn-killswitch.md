# VPN killswitch

> **Status (2026-04-27): Phases A + B shipped; C + D outstanding.**
> Soft pause-all on stale-handshake + auto-resume on reconnect
> (Phase A) and the 5-minute IP-leak self-test (Phase B) are both
> live in `backend/crates/kino/src/download/vpn/{killswitch,leak_check}.rs`,
> gated on the `vpn_killswitch_enabled` flag (default on when VPN is
> enabled). Phase B emits `IpLeakDetected` on egress mismatch,
> pauses every active download immediately, and surfaces
> `protected` plus the observed/expected IPs on the `/health` VPN
> panel (`api/health.rs::VpnPanel`). Phase C (nftables firewall —
> the only layer that survives a process crash) and Phase D
> (settings UI for the toggle + a shield indicator in TopNav beyond
> the existing health-card surface) remain design-only.
>
> **Audit (2026-04-27):** code matches spec for A + B. Frontend
> doesn't yet render `protected` / `leak_*` despite the data being
> plumbed — that's the cheap half of Phase D, ready to land
> whenever the styling work happens.

Network-level guarantee that torrent traffic *cannot* leave the host on the bare interface when the VPN tunnel is down. Belt-and-braces over the existing `bind_device_name` split tunneling: even if our process crashes, the rate-limit task gets stuck, or someone misconfigures librqbit, no peer connection makes it out the wrong NIC. Optional, default-on, fail-closed.

## Scope

**In scope:**

- Linux nftables (preferred) / iptables (fallback) drop rule that bans BitTorrent peer traffic on any interface other than the VPN's `wg0` (or whatever `KINO_VPN_INTERFACE` resolves to)
- Soft pause-all on VPN-down event from the existing `vpn_health` scheduler task — pauses every active download until handshake recovers
- IP-leak self-test on first connect + every 5 minutes: TCP handshake to a known external endpoint, compare observed IP against the VPN's expected egress IP
- Status surface: a new `vpn_protected: bool` field on `/api/v1/health`'s `vpn` panel, rendered as a green shield / red shield in the TopNav and the health card
- Capability detection at startup — flag clearly in `/status` when the killswitch can't be installed (no `CAP_NET_ADMIN`, no nftables/iptables binary, etc.) so the user knows they're unprotected
- One config toggle: `vpn_killswitch_enabled` (default `true` when `vpn_enabled = true`); ignored when `vpn_enabled = false`
- Idempotent install / uninstall — re-applying on service (or container) restart doesn't double-stack rules
- Respect a configurable allow-list of LAN CIDRs (`192.168.0.0/16`, `10.0.0.0/8`, `172.16.0.0/12`) for tracker / peer LAN-discovery scenarios

**Out of scope:**

- Killswitch on macOS / Windows native — only Linux is in scope at launch (the VPN pipeline itself is Linux-only today; see `03-download.md`)
- Replacing gluetun — kino is a *consumer* of a VPN tunnel, not a VPN provider. The `bind_device_name` integration covers the gluetun-style setup; the killswitch protects against the boringtun userspace fallback in the in-process path
- Per-torrent killswitch — it's an all-or-nothing protection; partial split is what `bind_device_name` is for
- Kill-switching the API server / web UI / streaming traffic — explicitly *not* through the VPN by design
- Ban-list management for trackers (`tracker-blocked-by-firewall` is a tracker-config concern, not killswitch)
- Log every blocked packet — too noisy; a counter on the rule is enough for `nft list` diagnostics
- DNS leak protection beyond what the VPN provides at the resolver level

## Why a separate killswitch from `bind_device_name`

Today's split-tunneling sets `opts.bind_device_name = Some("wg0")` on the librqbit Session, which means each peer socket gets `SO_BINDTODEVICE`. This is *almost* a killswitch already — if `wg0` doesn't exist or is down, the bind fails and no traffic flows.

Three windows where it isn't enough:

1. **Race during reconnect.** WG goes down → kernel routing falls back to the default route → librqbit's existing socket pool may still be open and may briefly attempt re-handshakes through the wrong interface before the next `vpn_health` tick (300s interval).
2. **Misconfiguration.** A user switches the VPN provider's UDP port, the tunnel comes up but on the wrong interface name, and `bind_device_name` silently no-ops. A firewall rule keyed on "anything BitTorrent that isn't on wg0" catches this.
3. **Process bug.** A future regression that drops the bind option entirely (an `..Default::default()` getting added in the wrong place) leaks immediately. The firewall rule is independent of the process and survives across restarts.

The killswitch covers all three by making leak-prevention a kernel-level invariant rather than a per-socket option.

## Architecture

### Three layers

**Layer 1 — Soft (process-level, immediate today).** When `vpn_health` detects a dead handshake, *before* attempting reconnect, iterate every download in `downloading | grabbing | stalled` state and call `pause_download`. On successful reconnect, resume them. ~30 LOC. No new dependencies. Limitation: trusts the process to do it; doesn't protect during process crash. Default on whenever VPN is enabled.

**Layer 2 — Hard (firewall, opt-in).** On VPN connect, install nftables rules that:

- Allow all traffic on the loopback interface
- Allow LAN CIDRs (configurable)
- Allow traffic on `$VPN_INTERFACE` (typically `wg0`)
- Allow UDP traffic to the VPN's peer endpoint port (so the tunnel itself can come up)
- Block everything else *for the kino process / cgroup*

Process scoping via cgroup v2 lets us drop only kino's traffic (`tcp,udp` dport != 53 → DROP if not from VPN cgroup), so the rest of the host stays untouched. Falls back to PID-based marking via `cgroup match` when cgroup v2 path isn't accessible.

When `vpn_killswitch_enabled = true` and the install fails for any reason (no `CAP_NET_ADMIN`, no `nft` binary, kernel without nftables), we surface a *critical* warning in `/status` and refuse to start the torrent client. Better to refuse to download than to leak.

**Layer 3 — Self-test (verification, periodic).** Every 5 min the `vpn_killswitch_check` task makes a TCP connection to a known endpoint (`https://api.ipify.org` or self-hosted equivalent) using the same socket bind options as librqbit, and confirms the observed external IP matches what the VPN provider advertises. A mismatch flips `vpn_protected = false` on the health panel and emits an `IpLeakDetected` event (history + webhook delivery).

### Default behaviour by config

| `vpn_enabled` | `vpn_killswitch_enabled` | Effect |
|---|---|---|
| false | (any) | No killswitch. No VPN binding. User opted out of VPN entirely; warned in setup wizard. |
| true | false | Layer 1 only (soft pause-all on disconnect). User explicitly disabled hard killswitch — could be running gluetun externally with its own killswitch. |
| true | true (default) | All three layers. Refuses to start torrent client if firewall install fails. |

### When `bind_interface` is None but `vpn_enabled = true`

Means the user has the in-process boringtun tunnel but isn't binding librqbit to it. Probably a misconfig; the killswitch defaults to assuming `wg0` and warns in `/status`.

## Implementation phases

1. **Phase A — soft pause-all on VPN-down.** Extend `vpn_health::check_once` to: before attempting reconnect, query for downloads in active states and call `pause_download` on each. After successful reconnect, resume them. Adds a `paused_by_killswitch` column (or marker in `error_message`) so we know which to resume vs. which the user paused themselves. Gated on `vpn_killswitch_enabled = true`. ~1 day.

2. **Phase B — IP-leak self-test.** New `vpn_killswitch_check` scheduler task at 5 min interval. Calls a configurable IP-discovery URL with the same bind options the torrent client uses, surfaces result on `/api/v1/health` panel, emits `IpLeakDetected` event on mismatch. ~1 day.

3. **Phase C — nftables install.** New module `download/vpn/killswitch.rs`:
   - `install(interface, peer_endpoint, lan_cidrs) -> Result<()>` — applies the rule set
   - `uninstall() -> Result<()>` — idempotent removal
   - `verify() -> bool` — confirms rules are still loaded (cheap `nft list table inet kino` parse)

   Called from VPN connect / disconnect lifecycle. Capability check at startup; refuses to enable if `nft` missing or `CAP_NET_ADMIN` unavailable. ~3-4 days including install-permission docs (systemd unit grant, optional-Docker `cap_add`) and integration testing.

4. **Phase D — UI surface.** Health card shows shield-on / shield-off / shield-warning. TopNav status dot tints based on `vpn_protected`. Settings → VPN page exposes the toggle + explains the trade-off. Setup wizard adds a "Killswitch active — recommended" line. ~1 day.

Phases A and B are useful on their own and don't need root. Phase C is the meat of the feature and the only one that actually prevents leaks at the kernel level.

## Configuration

| Field | UI | Default | Phase | Notes |
|---|---|---|---|---|
| `vpn_killswitch_enabled` | Settings → VPN | `true` (when `vpn_enabled = true`) | A (shipped) | Master toggle. Off ⇒ Layer 1 still tries on disconnect (cheap), Layers 2+3 dormant. |
| `vpn_killswitch_check_url` | Settings → VPN | `https://api.ipify.org/` | B (shipped) | IP-discovery endpoint for self-test. Self-hostable. |
| `vpn_killswitch_check_interval_secs` | *not configurable* | 300 | B (shipped, hardcoded) | Matches `vpn_health` cadence. |
| `vpn_killswitch_expected_egress` | *derived* | from VPN provider config | B (shipped, derived) | The "should look like" IP for the self-test comparison. |
| `vpn_killswitch_lan_cidrs` | Settings → VPN | `["192.168.0.0/16", "10.0.0.0/8", "172.16.0.0/12"]` | C (not shipped) | Allow-list for LAN-side traffic. Lands when Phase C (nftables) does. |

## Entities touched

- **Reads:** `download` table (for pause-all on disconnect), `config` for killswitch knobs
- **Writes:** `download.state` flips during pause-all; `download.error_message` carries `paused_by_killswitch` marker
- **Creates:** nftables `inet kino` table with two chains (`forward`, `output`); cgroup v2 entry for kino's process
- **Emits:** new `AppEvent::IpLeakDetected { observed_ip, expected_ip }`, `AppEvent::VpnKillswitchInstalled`, `AppEvent::VpnKillswitchUninstalled` — flow through the existing event listener / WS / webhook pipeline

## Dependencies

- `nftnl` Rust crate for nftables interaction (alternative: shell out to `nft` binary; simpler, less elegant)
- `cgroups-rs` for cgroup v2 process placement (alternative: write to `/sys/fs/cgroup` directly)
- Linux capabilities: existing `CAP_NET_ADMIN` is sufficient; no new caps. Native installs grant it via the systemd unit; Docker-channel users keep `--cap-add=NET_ADMIN` (same as today).
- Runtime requirements:
  - `nft` binary present (`nftables` package on every mainstream distro)
  - Kernel with `nf_tables` module (kernel ≥ 4.9, default in every modern distro)
  - cgroup v2 mounted (default on systemd-based hosts ≥ 2020)

Capability detection at startup logs a single concise warning if any prerequisite is missing and forces `vpn_killswitch_enabled = false` for the session.

## Error states

- **`nft` binary missing on first install** — log error, refuse to start torrent client (when `vpn_killswitch_enabled = true`), surface critical warning in `/status` linking to install instructions for common distros.
- **`CAP_NET_ADMIN` missing** — same treatment as above. The VPN tunnel itself needs this cap so this is also blocking for VPN startup.
- **nftables ruleset already populated by another tenant** — install adds our table without flushing. Uninstall only removes our table. Two kino instances on one host would have separate tables (`inet kino_<pid>`) — pick a stable suffix like `inet kino_<port>` so it survives restarts.
- **Self-test endpoint unreachable from inside VPN** — log warning every 5 min; don't trip the leak detector (we can't tell if it's a leak or a network blip). After 3 consecutive failures, mark `vpn_protected = unknown` and surface in health.
- **IP mismatch detected** — emit `IpLeakDetected`, flip `vpn_protected = false`, *immediately* pause all downloads (don't wait for the next health tick), keep the firewall rules in place (they should have prevented this; investigate why they didn't).
- **Reconnect succeeds but firewall rule wasn't reinstalled** — `verify()` runs alongside health check; missing rules trigger reinstall before resuming.

## Known limitations

- **Linux only.** macOS uses `pf`, Windows uses WFP — both possible but not in scope at launch, since the VPN tunnel itself is Linux-only today.
- **Soft Layer 1 doesn't survive process crash.** A `kill -9` on the kino process leaves whatever librqbit was doing in flight. Layer 2 (firewall) is what protects against this; Layer 1 alone is best-effort.
- **The 5-minute self-test cadence is a leak window.** If the VPN drops and reconnects to a different egress within 5 min, the self-test won't catch the brief mismatch. Acceptable trade-off; tighter polling burns battery and CPU on idle hosts. The firewall layer doesn't have this gap.
- **DNS leaks are out of scope.** If the user's resolver isn't routed through the VPN, queries leak. We document the gap and recommend setting `vpn_dns` in config; we don't enforce it.
- **WebRTC peer connections aren't a concern** because librqbit doesn't use WebRTC. Just noting it in case someone reads BitTorrent killswitch advice and assumes WebRTC mitigations apply.
- **No host-side leak indicator.** The shield in the kino UI tells you kino isn't leaking; if other apps on the same host are leaking that's not kino's job. The user is on their own for whole-host VPN posture.
- **Single VPN endpoint assumed.** No multi-server failover; if the user's VPN provider does endpoint rotation, the killswitch's expected-egress check needs rotating too. Out of scope for v1; covered by the periodic self-test catching the drift.
