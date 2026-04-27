# Web UI subsystem

Progressive Web App — the primary interface for kino. Should feel like a streaming service (Netflix, Disney+), not an admin panel (*arr stack).

## Design philosophy

**Streaming first, management second.** The default experience is browsing, watching, and discovering content. Automation management (downloads, indexers, quality profiles) is accessible but not prominent. Opening kino should feel like opening a streaming app, not an admin console.

**Single-user simplicity.** No login screen, no permissions, no user switcher. The app loads directly into the home screen. Auth is handled transparently via the API key.

**Immersive visuals.** Full-bleed backdrops, poster art everywhere, dark theme, cinematic feel. The visual richness is what separates a streaming app from an admin panel.

## Screens

### Primary (daily use)

| Screen | Path | Description |
|---|---|---|
| **Home** | `/` | Hero spotlight banner, "Continue Watching" row, "Recently Added" row, "Trending" row, "Popular" rows, genre rows |
| **Discover** | `/discover` | Full TMDB browse — trending, popular, upcoming. Genre/year/network filters. Infinite scroll grid. |
| **Movie detail** | `/movie/:id` | Full-bleed backdrop, poster, metadata, cast, trailer, quality badge, similar/recommended. Play or Request button depending on status. |
| **Show detail** | `/show/:id` | Same as movie plus season tabs, episode list with still images and progress bars. Per-season monitoring controls. |
| **Search** | `/search` | Global search with live results. Federated: "In Your Library" (playable) and "On TMDB" (requestable) in two sections simultaneously. |
| **Player** | `/play/:mediaId` | Full-screen video player. Minimal chrome. See Player section below. |
| **Library** | `/library` | Grid of all local content. Sort by title, date added, recently watched. Filter by type, genre, status, quality. |
| **Calendar** | `/calendar` | Upcoming episodes and movie releases in weekly/monthly view. Air dates, download status. |

### Secondary (management)

| Screen | Path | Description |
|---|---|---|
| **Downloads** | `/downloads` | Active queue with real-time progress via WebSocket. Speed, ETA, seeders. History of recent imports. |
| **Wanted** | `/wanted` | Content in `wanted` state that hasn't been found. Last searched date. Manual search trigger. |
| **Settings** | `/settings` | Single scrollable page with anchored sections: Server, VPN, Indexers, Quality Profiles, Library Management, Notifications, Naming. |

11 screens total.

## Navigation

### Desktop

Minimal **top bar** — not a sidebar. Sidebars waste horizontal space on the content grid.

```
[Logo]  [Home] [Discover] [Library] [Calendar] [Downloads]    [Search...]  [⚙️]
```

- 5 navigation items max
- Search input always visible in the top bar
- Settings behind the gear icon — opens as a separate page
- The top bar is 56px tall, dark, semi-transparent over the hero banner on the home screen

### Mobile

**Bottom tab bar** with 4 items:

```
[Home] [Search] [Library] [Downloads]
```

- Search promoted to its own tab (primary action on mobile)
- Calendar and Discover accessible from Home
- Settings behind a menu icon in the top bar

## Key components

### Hero spotlight

Top of the home screen. Large billboard showing a featured item — the top continue-watching item, the most recently added movie, or a trending TMDB pick.

- Full-bleed backdrop image (TMDB `w1920` size)
- Gradient overlay fading to dark background
- Title, year, brief overview, quality badge
- "Play" and "More Info" buttons
- Auto-rotates through 3-5 items (optional)

### Poster card (TitleCard)

The core browsable unit. 2:3 aspect ratio poster image.

- Hover: scale up slightly, show overlay with title, year, brief description
- Status badge overlay: "Wanted", "Downloading 45%", "1080p WEB"
- Progress bar at bottom for continue-watching items (like Netflix's red bar)
- Single-click: navigate to detail page
- Blurhash placeholder while image loads

### Horizontal scroll row (MediaSlider)

Rows of poster cards with horizontal scrolling.

- Snap-to-item scrolling
- Peek next item at the edge to signal scrollability
- Chevron navigation buttons on hover (desktop)
- Hide scrollbars
- Title link above each row ("Continue Watching", "Trending Movies", etc.)

### Detail page

Full-bleed backdrop with gradient overlay. Poster floats left, metadata right.

- Title, year, runtime, certification, genres
- Quality badge (if in library): "Bluray-1080p HDR10"
- Cast list with profile images (horizontal scroll)
- Trailer (embedded YouTube)
- **CTA button hierarchy:**
  - Available → large "Play" button (dominant), small "More" dropdown
  - Wanted/Downloading → status indicator with progress
  - Not requested → "Add to Library" button (single click, immediate search)
- Recommendations row at the bottom

### Episode list

On show detail pages, within season tabs:

- Episode still image (TMDB), episode number, title
- Runtime, air date
- Progress bar if partially watched
- Quality badge if file exists
- "Wanted" indicator if monitored but no file

### Video player

Custom HTML5 video player. The most complex component.

- **Direct play** via native `<video>` with range requests
- **HLS playback** via hls.js (for transcode path)
- **Subtitle track picker** — dropdown with available text/embedded subs
- **Audio track picker** — dropdown with available audio tracks
- **Progress bar** with trickplay thumbnail preview on hover (generated during import)
- **Resume prompt** — "Resume from 45:32" or "Play from beginning"
- **Auto-play next episode** — 10-second countdown overlay when current episode nears end (90% threshold)
- **Skip intro button** — appears during intro segment if chapter markers indicate it
- **Chromecast button** — Cast SDK integration, send to receiver
- **Keyboard shortcuts** — space (pause), left/right (seek 10s), f (fullscreen), m (mute), up/down (volume)
- **Minimal chrome** — controls auto-hide after 3 seconds of no interaction

### Download card

Real-time download status, updated via WebSocket.

- Title, quality, size
- Progress bar with percentage
- Download speed, upload speed, ETA
- Seeders / leechers
- Pause / cancel actions

## Request flow

Requesting content should feel like "adding to your list" — one click, no modal, no forms.

1. User finds content via Search or Discover
2. Detail page shows "Add to Library" button
3. Click → immediate: create entity, trigger search, show toast "Searching for The Matrix..."
4. If found immediately → toast updates "The Matrix — Bluray-1080p downloading"
5. Card updates in real-time via WebSocket

No modal, no quality profile picker (uses default). If the user has multiple quality profiles, a small dropdown can appear next to the button, but the default action is always single-click.

## Visual design

### Theme

- **Dark by default** — dark grey backgrounds (#0a0a0a to #1a1a1a), not pure black
- High contrast text (white on dark)
- Accent color for interactive elements (buttons, links, progress bars)
- Image-forward: posters and backdrops do the visual heavy lifting

### Polish details

- **Skeleton states** — card-shaped placeholders while loading (never empty white space)
- **View Transitions API** — poster morphs from grid position to detail page position on navigation. Falls back to fade for unsupported browsers.
- **Backdrop cross-fade** — when navigating between items, backdrop images cross-fade
- **Smooth scroll** — all horizontal scroll rows use smooth snap scrolling
- **Responsive images** — request appropriate size from kino's image API based on viewport (`?w=200` for mobile cards, `?w=500` for desktop)

## Responsive design

| Breakpoint | Layout |
|---|---|
| < 640px (mobile) | Single column, bottom tab bar, full-width cards, 2-column poster grid |
| 640–1024px (tablet) | Top bar, 3-4 column poster grid, side-by-side detail layout |
| > 1024px (desktop) | Top bar, 5-7 column poster grid, full detail layout with backdrop |

The app should be usable on a phone first — this is how most people browse streaming apps.

## PWA

- **Web App Manifest** — `display: standalone`, dark theme color, app icons, splash screens
- **Service worker** — cache images aggressively (immutable, CacheFirst), API responses with StaleWhileRevalidate for browse data, NetworkFirst for real-time data
- **Install prompt** — subtle banner on first visit suggesting install
- **Offline indicator** — when disconnected, show cached content as available, grey out actions that need the network
- **Background sync** — queue playback progress reports when connectivity is intermittent

## Technology

- **Framework:** to be decided during implementation (React/Next.js or SolidJS/SolidStart — both viable)
- **Styling:** Tailwind CSS with a defined design token system. CSS modules for complex components (player, settings forms).
- **API client:** auto-generated from OpenAPI spec via hey-api. Type-safe, stays in sync with the backend.
- **Video:** hls.js for HLS, native `<video>` for direct play. Custom player shell — no third-party player library.
- **State:** TanStack Query (or equivalent) for data fetching, caching, infinite scroll. Native WebSocket for real-time updates.
- **Real-time:** single WebSocket connection, events routed to reactive stores.
- **Animations:** View Transitions API for page transitions, CSS transitions for component animations.
