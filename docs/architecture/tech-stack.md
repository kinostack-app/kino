# kino — tech stack

## Backend (Rust)

| Category | Choice | Why |
|---|---|---|
| Web framework | **axum** | Tokio-native, Tower middleware, dominant ecosystem, built-in WebSocket |
| SQLite | **sqlx** (SQLite feature) | Compile-time verified SQL queries against the real schema. If a query references a column that doesn't exist, it fails to compile. Built-in connection pool. Async (spawn_blocking internally for SQLite — invisible to you). |
| Migrations | **sqlx** (built-in) | `sqlx migrate add`, `sqlx migrate run`. SQL files in `migrations/` directory, tracked in a `_sqlx_migrations` table. |
| OpenAPI | **utoipa** + utoipa-axum | Code-first OpenAPI 3.x generation from derive macros. Swagger UI via utoipa-swagger-ui. |
| HTTP client | **reqwest** | Tokio-native, connection pooling, JSON/cookie support. For TMDB + Torznab. |
| Serialization | **serde** + serde_json + **quick-xml** | serde_json for API/TMDB. quick-xml for Torznab (XML). serde_path_to_error for debugging. |
| Image processing | **image** + **fast-image-resize** | Pure Rust, JPEG decode/resize/encode. fast-image-resize for SIMD-accelerated hot path. |
| Async runtime | **tokio** (multi-threaded) | The only serious option. Everything in the stack is built for it. |
| CLI/config | **clap** v4 derive | Merged structopt. Env var support. |
| Templates | **minijinja** | Lightweight `{{placeholder}}` rendering for webhook bodies. By Armin Ronacher. |
| BitTorrent | **librqbit** | Rust-native, persistence, file selection, `bind_device_name`. |
| VPN | **boringtun** | Cloudflare's userspace WireGuard. Kernel WireGuard via netlink as primary when CAP_NET_ADMIN available. |
| Process management | **tokio::process::Command** | FFmpeg child processes. kill_on_drop, async stdout/stderr. |

### Key patterns

**SQLite connection pool:**
```rust
let pool = SqlitePoolOptions::new()
    .max_connections(5)
    .connect("sqlite:kino.db?mode=rwc").await?;
sqlx::migrate!().run(&pool).await?;  // run pending migrations on startup

// compile-time verified query
let movie = sqlx::query_as!(Movie, "SELECT * FROM movie WHERE id = ?", id)
    .fetch_one(&pool).await?;
```

**WebSocket fan-out:**
```rust
// tokio::sync::broadcast for pushing events to all connected clients
let (tx, _) = broadcast::channel::<Event>(256);
```

**FFmpeg lifecycle:**
```rust
let mut child = Command::new("ffmpeg")
    .args(&[...])
    .kill_on_drop(true)
    .spawn()?;
// Cancel via CancellationToken, graceful shutdown via stdin 'q'
```

## Frontend (TypeScript / React)

| Category | Choice | Why |
|---|---|---|
| Build | **Vite** + SWC | Fast builds. SWC over Babel. |
| Framework | **React** | Ecosystem for video players, Cast SDK, hey-api codegen. Largest component library support. |
| Routing | **TanStack Router** | SPA-first, typesafe route params, loader integration with TanStack Query. React Router v7 is SSR-focused now. |
| Data fetching | **TanStack Query v5** | Infinite scroll via `useInfiniteQuery`, optimistic updates, background refetch. hey-api generates TQ hooks directly. |
| Client state | **Zustand** | 1KB, no boilerplate. For non-server state: sidebar open, player state, cast session. |
| API client | **hey-api** | Auto-generated from OpenAPI spec. Type-safe. TanStack Query integration. |
| Styling | **Tailwind CSS v4** | Design tokens in config. |
| Components | **shadcn/ui** (Radix primitives) | Copy-paste components in your source tree. Full style control. Accessible (Radix underneath). |
| Video player | **Vidstack** (headless) + hls.js | Headless provider + React hooks. You build the UI, Vidstack handles HLS/media state/text tracks. |
| PWA | **vite-plugin-pwa** (Workbox) | Manifest generation, service worker, install prompt, runtime caching strategies. |
| Animations | **CSS transitions** + **View Transitions API** + **Motion** | CSS for basics, View Transitions for page morphs, Motion for complex gestures/staggered grids. |
| Icons | **Lucide** | ~1500 icons, good media coverage, shadcn/ui default. |
| Forms | **React Hook Form** + **Zod** | For settings page. Uncontrolled, fast. Zod shared with TanStack Router search params. |
| Toasts | **Sonner** | shadcn/ui integrated. "Download complete" / "new episode" notifications. |
| WebSocket | **Native WebSocket** | Single connection, events routed to TanStack Query cache invalidation via `queryClient.invalidateQueries()`. |

### Key patterns

**Data flow:**
```
OpenAPI spec → hey-api codegen → typed TanStack Query hooks → React components
```

**WebSocket → cache invalidation:**
```typescript
ws.onmessage = (event) => {
  const { topic, action } = JSON.parse(event.data);
  queryClient.invalidateQueries({ queryKey: [topic] });
};
```

**Infinite scroll:**
```typescript
const { data, fetchNextPage, hasNextPage } = useInfiniteQuery({
  queryKey: ['movies', filters],
  queryFn: ({ pageParam }) => api.getMovies({ cursor: pageParam, limit: 25 }),
  getNextPageParam: (lastPage) => lastPage.next_cursor,
});
```

## Distribution

Kino ships as a single native binary per platform. The frontend is
embedded at compile time via `rust_embed` — one file, no runtime web
server mount, no separate asset directory. FFmpeg is the only
external runtime dependency and is resolved in this order:

1. Bundled alongside the binary in platform packages (`.deb`,
   `.dmg`, `.msi`, Raspberry Pi image).
2. `ffmpeg` / `ffprobe` on PATH (Homebrew, winget, AUR, distro
   package).
3. Explicit `ffmpeg_path` in `config`.

### Runtime layout

```
{install_dir}/
  kino[.exe]              # single binary, frontend + backend
  ffmpeg[.exe]            # bundled with platform package; optional on PATH installs
  ffprobe[.exe]
{data_path}/
  kino.db[-wal][-shm]     # SQLite
  images/                 # cached TMDB posters, logos
  trickplay/              # seek thumbnails
  sessions/               # librqbit persistence
{media_library_path}/     # user-chosen
{download_path}/          # user-chosen
```

### Per-platform install path

| Platform | Install mechanism | Service registration |
|---|---|---|
| Linux | `.deb` / `.rpm` / static tarball / Homebrew on Linux / AUR | systemd user or system unit |
| macOS | Signed `.dmg` / Homebrew cask | `launchd` agent |
| Windows | Signed `.msi` / winget / Scoop | Windows Service (SCM) |
| Raspberry Pi | Custom Pi OS image (pi-gen) | systemd unit pre-enabled |
| Docker (optional channel) | `ghcr.io/kinostack-app/kino` | N/A (compose) |

On Linux the binary needs `CAP_NET_ADMIN` for WireGuard interface
creation. The systemd unit requests the capability once at install;
for ad-hoc runs, `sudo setcap cap_net_admin+ep kino` is sufficient.

Docker remains an optional distribution channel for users who already
run their media stack in containers. The image wraps the same Linux
binary — it's not a different build target.

See `docs/roadmap/21-cross-platform-deployment.md` for the full
matrix, CI pipeline, and per-channel release process.
