# ADR 0010 — Per-OS networking primitives

**Status:** Accepted (2026-04-26) — codifies what the existing VPN
+ mDNS code already does

## Context

Kino's networking surface touches four per-OS APIs:

- **TUN device creation** — for the WireGuard VPN (subsystem 13's
  killswitch, subsystem 21 §2)
- **Default-route mutation** — to push all torrent traffic through
  the VPN tunnel
- **DNS override** — optional; some VPN providers require it
- **mDNS responder** — for `kino.local` discovery on the LAN
  (subsystem 25)

Each has different APIs / privilege models / deployment quirks per
OS. The cross-platform audit (Phase 3) confirmed the existing
choices and crystallises the rationale.

## Decision

Use the following crates / approach across all platforms; document
the per-OS quirks where they bite.

| Concern | Crate / approach | Cross-platform? |
|---|---|---|
| WireGuard userspace | `boringtun` | Yes (pure Rust) |
| TUN device | `tun` crate (0.8) | Yes — abstracts `/dev/net/tun` (Linux), `utun` (macOS), `wintun.dll` (Windows) |
| Default-route mutation | per-OS shell-out (`ip` / `route` / `route.exe`) | No — explicit per-OS code paths in `download/vpn/` |
| DNS override | per-OS shell-out (`/etc/resolv.conf` / `scutil` / `netsh`) | No — optional; v1 leaves to user |
| mDNS responder | `mdns-sd` crate | Yes |
| Torrent socket binding | librqbit's `bind_device_name` | Linux only today; macOS/Windows upstream PR or fork (audit Phase 1 known unknown) |
| HTTP server bind | axum on `tokio::net::TcpListener` | Yes |

## Why these picks

**boringtun + `tun`** are the cleanest cross-platform path. Mullvad
uses both in production for their VPN client. The `tun` crate's
0.8.x line abstracts the device-creation surface; OS-specific
behaviour (privilege requirements, interface naming) is documented
in subsystem 21 §2.

**mdns-sd** is pure-Rust, no platform-native daemon required (Avahi
on Linux, Bonjour on macOS — both work alongside our advertiser).
Subsystem 25 documents the responder + the Avahi-coexistence path
(currently not implemented; Avahi-socket integration is a known
unknown).

**Per-OS shell-out for routing + DNS** beats trying to find a
cross-platform abstraction. The `ip`/`route`/`netsh` commands are
stable, well-documented, and ship with the OS. A pure-Rust
alternative (`netlink-packet-route` for Linux, raw `ioctl` for
macOS, IP Helper API for Windows) would multiply our maintenance
without removing the per-OS code paths anyway.

## Per-OS quirks captured

### TUN device creation

| OS | Privilege | Quirk |
|---|---|---|
| Linux | `CAP_NET_ADMIN` (or root) | systemd unit grants via `AmbientCapabilities=` — already correct in `debian/service` |
| macOS | root | `utun` device numbering is sequential (`utun0`, `utun1`...); we don't pin a specific index |
| Windows | Admin | `wintun.dll` ships in the release archive; auto-installs the driver on first adapter creation. No separate driver download |

### Default-route mutation

| OS | Command | Notes |
|---|---|---|
| Linux | `ip route replace default dev wg0 table N` | iproute2 ships with every modern distro |
| macOS | `route add default -interface utunN` | `route` ships with macOS |
| Windows | `route.exe add 0.0.0.0 mask 0.0.0.0 ... IF iface` | OR via IP Helper API for programmatic use; we shell out for now |

Existing code in `backend/crates/kino/src/download/vpn/` has a
single `#[cfg(target_os = "linux")]` gate today. macOS + Windows
implementations land with the cross-platform shipping work
(subsystem 21).

### DNS override (v1: optional, deferred)

Most VPN providers don't require DNS override; Mullvad is the
notable exception. Default behaviour: don't touch DNS. Settings
toggle to opt in (lands when the per-OS DNS code does).

### Sleep / wake (macOS specifically)

`utun` interfaces survive sleep inconsistently. Existing reconnect
logic in `download/vpn/` handles this; macOS may need additional
wake-event listening (deferred to subsystem 21's "known
limitations").

### librqbit `bind_device_name` on macOS / Windows

Linux uses `SO_BINDTODEVICE`. On other OSes the librqbit hook may
silently no-op or return an error. **Day-one verification task**
for the first cross-platform release. Mitigation if it doesn't
work:

- Bind sockets at our layer (setsockopt `IP_BOUND_IF` on macOS,
  `IP_UNICAST_IF` on Windows) before handing to librqbit
- OR upstream a librqbit PR adding the per-OS variants

This is the largest known unknown in the cross-platform networking
story. Tracked under subsystem 21 "known unknowns" (item 1).

### mDNS responder coexistence

| OS | Existing mDNS daemon | Our responder coexists? |
|---|---|---|
| Linux | Avahi (most distros) | **Conflict on port 5353.** Mitigation: detect Avahi via D-Bus probe, defer to it for advertising, fall back to our responder when Avahi isn't running. **Not yet implemented** (subsystem 25 known limitation) |
| macOS | mDNSResponder (system) | Our responder coexists; macOS allows multiple responders to multicast |
| Windows | None by default | Our responder is the only one |

## Consequences

- **The VPN code has explicit per-OS branches.** Acceptable; the
  abstractions don't help. Each branch is small (under 100 LOC per
  OS once written)
- **DNS override is opt-in v1.** Default: don't touch system DNS.
  Document for users who need it (Mullvad-style providers)
- **Linux Avahi conflict needs fixing before we ship Linux native
  packages broadly.** Either: respond on a different port (breaks
  discovery), use Avahi's `org.freedesktop.Avahi` D-Bus interface
  to register our service through it, or ship a `kino-avahi.service`
  alongside `kino.service`. Decided in subsystem 25 follow-up
- **librqbit cross-platform binding is a real blocker** for VPN-
  protected torrents on macOS/Windows. Until verified or fixed, we
  may have to disable the VPN feature (or ship with a warning) on
  those platforms. Surfaces in the release-readiness checklist for
  subsystem 21

## Alternatives considered

- **Use `tokio-tun` instead of `tun`.** Rejected — `tun` 0.8.x has
  better cross-platform coverage (Windows `wintun`)
- **Pure-Rust DNS override** (`netlink-packet-route` etc.) instead
  of shelling out to `ip`. Rejected — adds dependency surface
  without removing per-OS conditional code
- **Different mDNS crate** (`zeroconf`, `astro-dnssd`). Rejected —
  `mdns-sd` is pure-Rust, no system daemon required, fewer system
  dependencies
- **Roll our own WireGuard.** No — `boringtun` is production-grade
  and Mullvad maintains it

## Related ADRs

- ADR 0007 — No paid signing (separate concern; mentioned because
  signed binaries reduce per-OS install friction for the same
  networking code)
- subsystem 13 — Startup, killswitch
- subsystem 21 — Cross-platform deployment §2 (per-OS specifics)
- subsystem 25 — mDNS discovery
