# Native clients

> **Not yet implemented.** No Capacitor wrap, no iOS/Android app
> project, no tvOS target. The web PWA is the only client today.
> This document is design-only.

Packaging the kino React frontend as installable native apps across phones, tablets, and TVs. The goal is broad client coverage (iOS, Android, tvOS, Android TV, Fire TV, etc.) *without* maintaining a second UI codebase.

Today the only client is the PWA at `:5173` + a Chromecast sender. That covers desktop browsers and casting-from-browser, but no native app stores, no Apple TV, no smart-TV launchers, no offline downloads, no lean-back remote-control UX.

This subsystem defines the multi-target packaging strategy that fills those gaps while keeping **one React codebase** as the source of truth for ~70% of the client surface.

## Design principles

- **One React codebase, multiple packaging targets.** The Vite `dist/` is the input; each target wraps it in a platform-native shell. We do not ship a second UI framework (React Native, SwiftUI for the primary UI, etc.) except where the platform leaves no alternative.
- **Native only where web literally cannot ship.** Apple TV is the one platform with no WebView + no JS framework path; everything else can reuse the bundle.
- **Thin native glue only.** Each target adds a small native layer for platform integrations (Cast, AirPlay, background audio, file downloads, focus nav). No native UI reimplementation.
- **No custom browser.** We don't wrap Electron/Tauri on desktop — the browser is already the desktop client. Native packaging is for *mobile and TV* where there is no browser-bookmark equivalent.
- **Same server, same API.** Native apps are pure clients against the existing REST + WebSocket surface. No new backend endpoints beyond what subsystem 05 / 09 / 11 already define.

## Target matrix

| Target | Approach | UI reuse | Status |
|---|---|---|---|
| **iOS / iPadOS** | Capacitor (WKWebView) | 100% | Planned |
| **Android (phone + tablet)** | Capacitor (Chromium WebView) | 100% | Planned |
| **Android TV** | Capacitor APK + Leanback manifest + D-pad focus layer | 100% (plus focus nav) | Planned — see §Android TV |
| **Fire TV (current gen)** | Same as Android TV | 100% | Planned — sunset path (see §Fire TV) |
| **LG webOS** | Packaged SPA via `ares-*` CLI | 100% | Already planned in 11-cast.md |
| **Samsung Tizen** | Packaged SPA via Tizen Studio | 100% | Planned |
| **Apple TV (tvOS)** | Native SwiftUI + AVKit thin client | 0% (UI), ~40% (types, API client, business logic) | Planned — see §Apple TV reality check |
| **Roku** | — | — | Out of scope (BrightScript, proprietary) |
| **Desktop (Linux/macOS/Windows)** | Browser + optional tray — see 22-desktop-tray.md | 100% | Partially shipped (tray planned) |

Realistic coverage: **Capacitor covers iOS + iPadOS + Android + Android TV + Fire TV from one config**. Smart-TV webOS/Tizen reuse the same Vite bundle with different packaging. Apple TV is the one genuine outlier.

## Packaging strategy

### Capacitor (iOS, iPadOS, Android, Android TV, Fire TV)

Capacitor 8+ is the primary packaging tool. The Vite bundle runs unchanged inside:

- **iOS / iPadOS**: `WKWebView`. Native HLS via `<video>`, AirPlay route picker for free with `x-webkit-airplay="allow"` + Background Modes capability.
- **Android / Android TV / Fire TV (pre-Vega)**: Chromium `WebView`. hls.js works as-is.

Build flow:

```
npm run build                          # Vite → frontend/dist/
npx cap copy ios                       # Sync dist/ into ios/App/App/public/
npx cap open ios                       # Xcode opens — Archive → upload to TestFlight
npx cap copy android
npx cap open android                   # Android Studio opens — Build → Signed .aab
```

Single `capacitor.config.ts` at repo root; platform folders (`ios/`, `android/`) are committed, generated once via `cap add`.

Assumption: the existing frontend is already responsive and works in a WebView without desktop-only APIs. The current PWA runs in a 5173 browser tab; validating Capacitor fit = installing Capacitor, running `cap copy`, booting the iOS simulator, and confirming video playback + API calls work. Any regression is a Capacitor-specific bug to fix inline in the React code, not a reason to fork the UI.

### Packaged SPA (webOS, Tizen)

Same `frontend/dist/` bundle, different wrapper:

- **webOS**: `ares-package` CLI from the LG webOS SDK produces an `.ipk`. Needs `appinfo.json` + `icon.png`. Sideload via LG Developer Mode or submit to LG Content Store.
- **Tizen**: Tizen Studio produces a `.wgt`. Needs `config.xml` + `icon.png`. Sideload via Tizen Developer Mode or submit to Samsung Galaxy Store.

Both TV OSes run Chromium-based runtimes and accept a standard SPA, so the Vite bundle works directly. The remote-control layer (D-pad focus + back button) is shared with the Android TV path — a single focus-nav library in the React codebase serves all four TV targets.

Subsystem 11 already specifies the webOS app's functional role (thin client, WebSocket-driven). This subsystem defines *how it's packaged* and confirms the same bundle covers Tizen.

### Native (Apple TV only)

Apple TV must be a real native app — a small SwiftUI + AVKit project that:

- Consumes kino's existing REST API (library browse, search, metadata)
- Plays HLS via `AVPlayer` (native hardware decode, hardware HDR/DV, TrueHD passthrough via eARC)
- Participates in the WebSocket remote-control pattern from subsystem 11
- Does *not* reimplement the full web UI — scope is library browse + playback, not settings / downloads / calendar

Code reuse from the React codebase: the OpenAPI-generated TypeScript client → regenerated as Swift via `swift-openapi-generator` against the same `openapi.json`. That covers types + API surface. Business logic (quality selection, progress heuristics) is re-implemented in Swift — a few hundred lines, not thousands.

See §Apple TV reality check for why no web-wrapper path exists.

## Apple TV reality check

The "one codebase via Capacitor" dream dies at tvOS. Validated as of tvOS 26.4 (March 2026):

- **No `WKWebView` on tvOS.** The `__TVOS_PROHIBITED` marker has been on `WKWebView` since tvOS 9 (2015). Radar #22738023 filed in 2015, never resolved, archived January 2025. Private-API workarounds (`tvOSBrowser`) are explicit App Store rejections.
- **Capacitor has no tvOS target.** Ionic confirmed this on the forums and there is no community fork, because there is nothing on the platform to build against.
- **TVMLKit (Apple's JS-for-tvOS framework) was deprecated at WWDC24.** Apple's migration guide points explicitly at SwiftUI.
- **`react-native-tvos` exists and is maintained**, but it is React *Native*, not React DOM. kino's shadcn/ui + Tailwind v4 + hls.js + custom `<video>` player do not port. Realistic code reuse from a pure web app is 30–50%, and it doubles the toolchain burden.

The three options for tvOS coverage, in priority order:

1. **AirPlay from the iOS Capacitor app** — sidesteps tvOS entirely for casual playback. Fails the "lean-back with just the Siri Remote" use case (iPhone must stay awake, kino open).
2. **Small native SwiftUI + AVKit app** — recommended if tvOS is a real requirement. Few thousand lines of Swift, library browse + playback only, REST API against kino backend.
3. **`react-native-tvos`** — only if kino's mobile story also moves to RN. Not worth it for tvOS alone.

Deliberately rejected: waiting for Capacitor tvOS (won't happen), TVMLKit (deprecated), browser-on-tvOS (there is no Safari on tvOS).

## Custom plugin work

Capacitor's plugin ecosystem covers most integrations. Kino-specific gaps that require custom native code:

| Capability | Plugin | Effort | Notes |
|---|---|---|---|
| **Chromecast sender** | Custom (no maintained community plugin) | Multi-day native | Existing community plugins (`gameleap`, `caprockapps`) are effectively abandoned (≤4 commits, no releases). Fork or write our own embedding `GoogleCast.framework` (iOS) + `androidx.mediarouter` (Android). The Cast Web SDK can't discover devices from WKWebView (no mDNS) and can't bind to Play Services from Android WebView, so a native plugin is not optional. |
| **AirPlay button (iOS)** | Custom, small | ~1 day | `x-webkit-airplay="allow"` + Background Modes gets the native route picker for free. A first-class in-app button wraps `AVRoutePickerView`. |
| **Background audio (iOS)** | Custom, small | ~50 lines Swift | `AVAudioSession.setCategory(.playback)` + Background Modes capability. Without this, playback pauses when the app backgrounds. Media-session art and lock-screen controls via `@jofr/capacitor-media-session` or Capawesome's equivalent. |
| **Offline downloads (foreground)** | `@capacitor/file-transfer` + `@capacitor/filesystem` | Integration only | Works out of the box on both platforms. iOS surfaces in Files app via `UIFileSharingEnabled`. |
| **Offline downloads (background)** | Custom | Multi-day | Foreground-only downloads are acceptable for v1. Background-continued (app killed, download finishes) needs `URLSession` (iOS) + `WorkManager` (Android) wrappers. Defer. |
| **Secure storage (API key)** | `@capawesome/capacitor-secure-preferences` or `aparajita/capacitor-secure-storage` | Config only | Keychain (iOS) + Keystore (Android) — off-the-shelf. |
| **Wake lock** | `@capacitor-community/keep-awake` | Config only | Off-the-shelf. |
| **PiP** | Native `<video>` PiP on iOS 14+ works in WKWebView | None | Free. |
| **Focus nav (TV targets)** | In-house React hook + CSS | ~1 week | Shared across Android TV, Fire TV, webOS, Tizen. D-pad up/down/left/right + back button + OK. |

### hls.js + WKWebView gotchas

- WKWebView doesn't do MSE reliably on iOS, so hls.js falls back to native Safari HLS (`<video src=".m3u8">`). We lose hls.js-driven ABR and subtitle switching on iOS — AVFoundation's own ABR + subtitle rendering kicks in instead. Acceptable; it's what Safari users already get.
- **Do not enable `@capacitor/http`'s fetch-patching mode.** It breaks hls.js relative URL resolution ([hls.js#6755](https://github.com/video-dev/hls.js/issues/6755)).

## Build toolchain

### iOS (Capacitor + native tvOS)

- macOS required (or `macos-latest` GitHub Actions runner)
- Xcode 26 for Capacitor 8
- Apple Developer Program: **$99/year**
- Distribution certificate + provisioning profile
- TestFlight for internal/external beta
- App Store Connect submission for public release
- Output: `.ipa` via Xcode Organizer or `xcrun altool`

One Developer Program account covers both the iOS Capacitor app and the tvOS native app.

### Android (Capacitor + Android TV + Fire TV)

- Android Studio (Hedgehog+)
- JDK 17+
- SDK 34/35
- Google Play Console: **$25 one-time**
- Keystore for signing
- Output: `.aab` for Play Store, `.apk` for sideload (Fire TV still accepts APKs)

One keystore + Play account covers Android phone, Android tablet, and Android TV. Fire TV distribution is via Amazon Appstore (free developer account) using the same `.apk`.

### webOS (LG)

- Node-based `ares-cli` from the LG webOS SDK — cross-platform, no Mac/Windows lock-in
- LG Developer account (free) for Content Store submission
- Developer Mode on the TV for sideload testing
- Output: `.ipk`

### Tizen (Samsung)

- Tizen Studio (cross-platform, Eclipse-based)
- Samsung Developer account (free) for Galaxy Store submission
- Developer Mode on the TV for sideload testing
- Output: `.wgt`

### CI matrix

Extends the `release.yml` defined in subsystem 21. Per tag push:

| Job | Runner | Artefact |
|---|---|---|
| `ios` | `macos-latest` | `.ipa` + TestFlight upload |
| `tvos` | `macos-latest` | `.ipa` (Apple TV) + TestFlight upload |
| `android` | `ubuntu-latest` | `.aab` (Play) + `.apk` (Fire TV / sideload) |
| `webos` | `ubuntu-latest` | `.ipk` |
| `tizen` | `ubuntu-latest` | `.wgt` |

All consume the same `frontend/dist/` bundle produced by an upstream `web-build` job (Capacitor + webOS + Tizen — TV/phone share identical bundle). tvOS builds independently from the Swift project.

## Sunset / platform-risk notes

- **Fire TV → Vega OS.** Amazon is migrating Fire TV to a Linux-based Vega OS starting with the 2026 Fire TV Stick HD. Existing Capacitor APKs won't install on new hardware. Vega OS SDK details are unannounced as of 2026-04. Treat Fire TV as a sunset target: current-gen users get the APK, new-gen coverage depends on whatever SDK Vega ships.
- **Capacitor 8 → 9+ cadence.** Capacitor has major releases roughly yearly. Plan for an annual upgrade sweep that also bumps Xcode/Android Studio minimums.
- **Apple Developer renewal.** $99/year, non-negotiable. Lapse = both iOS and tvOS apps get pulled from the store.
- **LG / Samsung store policies.** Smart-TV store submission processes are slower and more opinionated than phone stores. Budget weeks, not hours.

## Deliberately out of scope

- **Roku.** BrightScript is a proprietary language with a proprietary UI framework (SceneGraph). Zero code reuse from React, small user base relative to the other TV platforms, no path via Capacitor or any web-wrapper. Skip unless demand materialises.
- **Tauri mobile.** As of 2026-04 Tauri's mobile support is still immature — `Tauri 2.x` supports iOS/Android but the plugin ecosystem and WebView integration lag Capacitor significantly. Not a serious contender today; revisit in a year.
- **Full React Native port.** Doubles the UI codebase. Only justifiable if Capacitor proves unsuitable, and the evidence so far is that it is suitable.
- **Electron / Tauri desktop wrapper.** The browser is the desktop client. See 22-desktop-tray.md for the tray-only approach.
- **Apple TV via `react-native-tvos`.** Rejected above — too much toolchain for too little reuse when the only consumer is tvOS.
- **Web-based Roku / Samsung orsay / legacy TV platforms.** Each is its own bespoke runtime with its own framework. Not worth the per-platform effort.

## Cross-references

- **11-cast.md** — defines the remote-control pattern (phone → server → TV app via WebSocket) that all TV targets in this subsystem implement. The Chromecast receiver, webOS app, and Android TV app functional specs live there; this subsystem defines how they're packaged.
- **10-web-ui.md** — defines the React UI that every Capacitor / packaged-SPA target wraps.
- **05-playback.md** — defines the HTTP + HLS surface every client consumes.
- **09-api.md** — defines the REST + WebSocket API every client consumes.
- **21-cross-platform-deployment.md** — defines the *server* cross-platform story; this subsystem is the client-side complement.
- **22-desktop-tray.md** — the desktop-client story (browser + tray, not a wrapped UI).

## Entities touched

- **Reads:** all existing client-facing endpoints. No new server-side state.
- **Creates / Updates:** nothing server-side. Device capability registration (subsystem 11) is extended to cover every native-client type.

## Dependencies

New client-side only:

- `@capacitor/core`, `@capacitor/cli`, `@capacitor/ios`, `@capacitor/android`
- `@capacitor/filesystem`, `@capacitor/file-transfer`
- `@capacitor-community/keep-awake`
- `@jofr/capacitor-media-session` (or Capawesome equivalent)
- `@capawesome/capacitor-secure-preferences` (or `aparajita/capacitor-secure-storage`)
- Custom plugin: `kino-capacitor-cast` (Chromecast sender — to be written)
- Custom plugin: `kino-capacitor-airplay` (AirPlay button wrapper — to be written)
- Custom plugin: `kino-capacitor-background-audio` (iOS AVAudioSession — to be written)

For Apple TV (separate Swift project):

- Xcode, SwiftUI, AVKit, AVFoundation
- `swift-openapi-generator` for API client code-gen

For webOS: `ares-cli` (build-time only).

For Tizen: Tizen Studio (build-time only).

## Known unknowns

1. **Custom Chromecast plugin maintenance burden.** Writing + maintaining our own Cast sender plugin is non-trivial. Alternative: pay for an Ionic-maintained plugin if one emerges, or cast via the native iOS/Android Cast SDKs exclusively (drop web-sender from the Capacitor webview entirely). Decide after a spike.
2. **HLS quirks in Android WebView vs Chromium.** Android WebView Chromium is consistently a few versions behind desktop Chromium and has historically had HLS regressions. Needs real-device testing, not just emulator.
3. **iOS Background Modes approval.** Apple occasionally rejects background-audio apps that don't "genuinely need" backgrounding. kino's use case (video + background audio when screen off) is standard for media apps but the first review submission should anticipate questions.
4. **webOS / Tizen review cycles.** Both stores have opaque review processes. Sideload-first is the practical distribution path until a release has proven stable.
5. **Vega OS SDK for Fire TV.** Unannounced. Could be a web-app runtime (best case: reuse Tizen/webOS packaging), could be something proprietary (worst case: Fire TV becomes out-of-scope).
6. **Focus-nav library choice.** Write in-house, adopt `norigin-spatial-navigation`, or something else? Decide after Android TV spike.

## Known limitations

- **Apple TV is not a one-codebase target.** A separate Swift project exists specifically for tvOS. Accepted cost of entering the Apple TV ecosystem.
- **No Roku app.** Ever, most likely.
- **Fire TV next-gen coverage depends on Vega OS.** Current-gen Fire TV (Android-based) is supported; new-gen hardware coverage is TBD.
- **iOS loses hls.js ABR control.** Native Safari HLS runs instead. Acceptable; matches baseline Safari behaviour.
- **Background downloads are foreground-only in v1.** App must stay active for downloads to complete.
- **No auto-update for sideloaded TV apps.** LG/Samsung store builds get store-managed updates; sideloaded builds require manual re-install.
- **Apple Developer + Play Console are recurring costs.** $99/year + $25 one-time. Non-negotiable for store distribution.
