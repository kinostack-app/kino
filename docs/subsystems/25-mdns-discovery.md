# mDNS discovery

> **Shipped (2026-04-26).** `mdns-sd` responder wires up in
> `mdns::start` after the HTTP listener binds. Publishes A records
> for every non-loopback IPv4 plus a `_http._tcp.local.` service
> record carrying `path=/` and `version={kino_version}` TXT entries.
> Toggle + hostname + service-name configurable via
> `mdns_enabled` / `mdns_hostname` / `mdns_service_name` config
> fields. The Avahi-socket co-existence path described below is not
> yet implemented — kino's responder runs alongside any host
> avahi-daemon, which works but is wasteful on Linux hosts that
> already have one.

Advertise the kino service on the local network via mDNS so users can reach it at `http://kino.local:{port}` from any device on the LAN without needing to know its IP. Zero-config: works the moment the service starts, no router DNS entries required.

## Scope

**In scope:**
- Publish an A record for `kino.local` pointing at the host's LAN IP
- Publish an `_http._tcp.local.` service record on the configured port so Bonjour browsers (macOS Finder, `avahi-browse`) list kino
- Configurable hostname (default `kino`) for users running multiple instances on one LAN (`kino-4k.local`, etc.)
- Opt-out via config / env var for users who don't want the advertisement

**Out of scope:**
- HTTPS certificates for `.local` — browsers treat mDNS names as insecure origins; HTTPS is a separate concern (reverse proxy + real DNS)
- Discovery of *other* services from within kino (we advertise, we don't browse)
- Android support — Chrome on Android historically doesn't resolve `.local` names; documented as a known limitation
- mDNS inside the devcontainer bridge network — Docker bridges don't forward link-local multicast (224.0.0.251). Dev testing uses the host-avahi socket passthrough documented below. Native installs advertise directly with no workaround; users on the optional Docker channel need `network_mode: host` or macvlan

## Architecture

### Advertiser, not responder library

Kino publishes its own name + service record using a pure-Rust mDNS responder (`mdns-sd`). No dependency on system Avahi or Bonjour at runtime — the binary stays self-contained across Linux, macOS, and Windows.

One caveat on Linux hosts that *also* run `avahi-daemon`: both responders will answer queries for `kino.local`, which works but is wasteful. When we detect a running `avahi-daemon` via its socket, we skip our own responder and publish via Avahi's D-Bus/socket API instead. Falls back to our built-in responder everywhere else.

### Networking requirements

mDNS is link-local multicast on `224.0.0.251:5353`. The kino process must have a network interface reachable to the target LAN:

| Deployment | Works? |
|---|---|
| Bare-metal / systemd service | Yes |
| Docker with `network_mode: host` | Yes |
| Docker with bridge networking | No (bridge blocks multicast); use Avahi-socket bind-mount for dev, host networking for the optional Docker channel |
| Docker with macvlan | Yes |

Documented in the install guide per platform.

## 1. Hostname and records

Config keys:

| Key | Default | Notes |
|---|---|---|
| `mdns.enabled` | `true` | Disable to skip all advertisement |
| `mdns.hostname` | `kino` | Becomes `{hostname}.local` |
| `mdns.service_name` | `Kino` | Human-readable label in Bonjour browsers |

Records published:

- **A record**: `{hostname}.local` → host's LAN IPv4
- **AAAA record**: `{hostname}.local` → host's LAN IPv6 if present
- **PTR + SRV + TXT** under `_http._tcp.local.` advertising `{service_name}._http._tcp.local.` on the configured port, with TXT entries `path=/` and `version={kino_version}`

Interface selection: bind to all non-loopback, non-link-local interfaces by default. Users with multi-NIC hosts (VPN tunnels, Tailscale, etc.) can restrict via `mdns.interfaces = ["eth0"]`.

## 2. Lifecycle

- **Startup**: after the HTTP server binds successfully, register records. Log the final URL (`kino available at http://kino.local:8080`).
- **Shutdown**: send goodbye packets (TTL 0) so neighbours drop the name promptly rather than waiting for the TTL to expire. Best-effort — process crashes won't send goodbyes; record expires naturally.
- **IP change** (DHCP lease renew, interface up/down): re-publish with the new address. Handled by the library's interface-watcher.
- **Name conflict** (another host already claims `kino.local`): mDNS probe detects it; log a warning and append a numeric suffix (`kino-2.local`). User can set a distinct hostname to avoid the collision.

## 3. Dev environment

The devcontainer's `backend` service inherits `network_mode: "service:dev"`, and `dev` uses bridge networking — so the built-in responder can't reach the LAN. For LAN testing in dev, bind-mount the host's Avahi socket:

```yaml
# docker-compose.override.yml (dev only)
services:
  dev:
    volumes:
      - /var/run/avahi-daemon/socket:/var/run/avahi-daemon/socket
```

Kino detects the socket and publishes through the host's avahi-daemon, which handles multicast on the host LAN. No host D-Bus mount required (known instability — avoid). Linux-only; on macOS/Windows, LAN mDNS inside Docker Desktop isn't practical — test against a native binary build instead.

## Entities touched

- **Reads:** service port + bind address from kino config, host network interfaces via OS APIs
- **Writes:** none in the database — pure runtime state
- **External:** multicasts on `224.0.0.251:5353`; optionally talks to `/var/run/avahi-daemon/socket`

## Dependencies

- `mdns-sd` — pure-Rust mDNS responder, actively maintained
- `if-addrs` — enumerate local network interfaces for record publication

No new system binaries in the common path. Avahi integration on Linux hosts is opportunistic — if Avahi isn't running we use our own responder.

## Error states

- **No network interfaces found** → log warning, skip advertisement. Service still runs; just not discoverable.
- **Multicast send fails** (firewall, restrictive network) → log once per interface, continue. Service still works by IP.
- **Name conflict unresolvable** (suffix collisions exhaust retries) → log error, disable advertisement for this session.
- **Avahi socket present but unresponsive** → fall back to built-in responder.
- **Port 5353 already bound by another responder** → built-in responder fails; detect and fall back to Avahi socket where available, otherwise log and skip.

## Known limitations

- **Android doesn't resolve `.local` in Chrome** — users on Android must use the IP or a real DNS entry. Documented in the help pages.
- **Corporate / guest networks often block mDNS** — AP isolation silently drops multicast between clients. Nothing we can do from the server side.
- **IPv6 support depends on the network** — some home routers don't forward IPv6 link-local mDNS; A record (IPv4) is the reliable path.
- **HTTPS on `.local` requires self-signed certs and browser-specific trust workflows** — out of scope for v1; users needing HTTPS put kino behind a reverse proxy with a real domain.
- **Multiple kino instances on one LAN** need distinct hostnames set explicitly — auto-suffix works but the suffix isn't stable across restarts.
