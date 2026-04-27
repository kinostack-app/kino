# Cast receiver

> **Current status:** Chromecast custom receiver is shipped. webOS
> and Android TV companion apps described later in this document
> are **planned, not implemented** — the device-registration API and
> remote-control WebSocket plumbing aren't wired up. If you're on
> a kino build today, Chromecast is the only cast target.

A custom Cast receiver served by the kino binary, plus the
forward-looking design for native TV apps that would give full DV +
lossless-audio direct play on devices that support it.

## Why companion apps

The Web UI in a browser can stream video, but it's limited by what
the browser's media pipeline supports. A native app on the TV gets
full access to the hardware decoder:

| Path | DV | HDR10 | TrueHD Atmos | Transcode needed? |
|---|---|---|---|---|
| Browser on laptop | No | Depends | No | Often |
| Chromecast (shipped) | No (strips DV from MKV) | Yes | No | Audio only |
| LG webOS app (planned) | Yes (full Profile 7) | Yes | Yes (eARC passthrough) | None |
| Android TV app (planned) | Depends on device | Yes | Via passthrough | Rarely |

The eventual webOS app on an LG OLED would deliver the full quality
chain — DV Profile 7 + TrueHD Atmos — with zero transcoding. The
Chromecast path today is the single shipped non-browser target and
still gets 4K HDR10 with AC-3 audio via smart track selection.

## Chromecast receiver (shipped)

A Custom Web Receiver registered with the Google Cast Developer
Console. Loaded by the Chromecast when the user casts from the Web UI.

### Limitations

- **DV stripped** — Chromecast doesn't support DV Profile 7 in MKV;
  falls back to HDR10 base layer.
- **No TrueHD** — uses the AC-3 compatibility track (smart audio
  selection handles this server-side).
- Still gets 4K HDR10 with DD 5.1 — good, just not the full quality
  chain.

### Functionality

- Receive play command from the sender, play video via Cast
  Application Framework (CAF)
- Subtitles, progress reporting, remote control
- Idle screen

The sender (Web UI) uses the Cast SDK to launch the receiver and
send commands. The receiver communicates with kino's API for
streaming and progress.

### Tech stack

- Google Cast Application Framework (CAF) — handles media playback,
  transport controls
- Registered at Cast Developer Console with a receiver URL served by
  kino (`/cast/receiver.html`)

### Files

```
cast/
  receiver.html   — main page, loads CAF SDK
  receiver.css    — idle screen styling
  receiver.js     — progress reporting, subtitle setup
```

---

# Planned: webOS + Android TV companion apps

Everything below is forward-looking design, not shipped code. The
device-registration endpoint, WebSocket remote-control plumbing, and
the apps themselves are all TODO. Do not infer from this document
that any of this runs today.

## Architecture

All companion apps would be thin clients. They wouldn't browse,
search, or manage content — they play video and receive commands
from the Web UI.

### Remote control pattern

No Cast SDK or DIAL protocol. The phone (Web UI) and the TV app both
connect to kino's server; the phone sends a "play" command, the
server relays it to the TV app via WebSocket.

```
Phone (Web UI)                    kino server                    TV (companion app)
    |                                  |                                |
    |   POST /api/v1/playback/remote   |                                |
    |   {device: "living-room",        |                                |
    |    media_id: 42}                 |                                |
    |--------------------------------->|                                |
    |                                  |   WebSocket event:             |
    |                                  |   {type: "playback_start",     |
    |                                  |    media_id: 42}               |
    |                                  |------------------------------->|
    |                                  |                                |
    |                                  |   GET /api/v1/playback/42/direct
    |                                  |<-------------------------------|
    |                                  |   [video stream]               |
    |                                  |------------------------------->|
    |                                  |                                |
    |   Seek/Pause/Stop commands       |   WebSocket relay              |
    |--------------------------------->|------------------------------->|
    |                                  |                                |
    |                                  |   Progress reports             |
    |                                  |<-------------------------------|
```

The TV app would register itself as a device on startup:
```
POST /api/v1/devices/register
{name: "Living Room TV", type: "webos", capabilities: {dv: true, truehd: true, hevc: true, ...}}
```

This would let the playback-info endpoint return the optimal stream
selection for that specific device.

## webOS app (LG TVs) — planned

A web app packaged with the LG webOS SDK. Would run in the TV's
Chromium-based shell with access to the native media pipeline.
Installed via LG Developer Mode or the LG Content Store.

LG OLEDs have full hardware support for:
- Dolby Vision Profile 5, 7, 8
- HDR10, HDR10+, HLG
- HEVC, VP9, AV1 (2022+ models)
- TrueHD/Atmos (decoded to PCM or passed through via eARC)
- MKV containers

When a webOS app plays a file via HTML5 `<video>`, the TV's hardware
decoder handles everything. A 64 GB 4K DV remux with TrueHD Atmos
would play without any server-side processing — direct play, full
quality.

### Planned functionality

- **Playback:** receive play command via WebSocket, load video URL
  in `<video>` element
- **Subtitle display:** WebVTT loaded as `<track>` element
- **Audio track selection:** app requests optimal audio track from
  the server based on TV capabilities
- **Progress reporting:** position updates every 10 seconds
- **Remote controlled:** seek, pause, stop, subtitle/audio switching
  via WebSocket from the phone
- **Idle screen:** kino branding, recently added artwork cycling

### Tech stack

- HTML/CSS/JS — same as the Cast receiver, minimal
- webOS SDK for packaging + TV APIs (remote control events, media
  playback hooks)
- Installable via LG Developer Mode (sideload) or submitted to LG
  Content Store

### Files

```
webos/
  index.html      — main app page
  app.js          — WebSocket handler, playback logic, remote control
  style.css       — idle screen, metadata overlay, controls
  appinfo.json    — webOS app manifest (ID, version, icon)
  icon.png        — app icon for launcher
```

## Android TV app — planned

A WebView wrapper around kino's Web UI, packaged as an Android TV
app. Would give access to the Android media pipeline (DV on
supported devices, TrueHD passthrough). Same remote-control pattern
via WebSocket. Mostly relevant for non-LG TVs running Google TV /
Android TV.

## Device capabilities API — planned

Companion apps would register their capabilities on startup. The
playback subsystem would use these to make smart stream/track
selection decisions.

```
POST /api/v1/devices/register
{
  "name": "Living Room TV",
  "type": "webos",
  "capabilities": {
    "video_codecs": ["hevc", "h264", "av1", "vp9"],
    "audio_codecs": ["truehd", "eac3", "ac3", "aac"],
    "hdr_formats": ["dolby_vision", "hdr10", "hdr10plus", "hlg"],
    "max_resolution": 2160,
    "supports_mkv": true
  }
}
```

```
GET /api/v1/devices
```

Would return registered devices. The Web UI would show a "Play
on..." picker when multiple devices are available.

When the user says "play on Living Room TV", the playback-info
endpoint would use that device's capabilities to determine: direct
play the MKV with DV + TrueHD (webOS can handle it), or select the
AC-3 track and strip DV (Chromecast can't).
