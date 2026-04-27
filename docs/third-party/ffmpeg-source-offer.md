# FFmpeg source offer

kino does **not** redistribute FFmpeg. We facilitate a download of
the upstream `jellyfin-ffmpeg` binary on demand, when the user
clicks **Settings → Playback → Download jellyfin-ffmpeg**.

Implementation: [`backend/crates/kino/src/playback/ffmpeg_bundle.rs`](../../backend/crates/kino/src/playback/ffmpeg_bundle.rs).

## What kino downloads

The pinned version constant in `ffmpeg_bundle.rs` selects an
upstream tag from
[`github.com/jellyfin/jellyfin-ffmpeg`](https://github.com/jellyfin/jellyfin-ffmpeg/releases).
For each (platform, version) pair, kino has a pinned SHA256 of the
GPL-build portable archive published by Jellyfin:

| Platform | Asset filename pattern |
|---|---|
| Linux x86_64 | `jellyfin-ffmpeg_{VERSION}_portable_linux64-gpl.tar.xz` |
| Linux aarch64 | `jellyfin-ffmpeg_{VERSION}_portable_linuxarm64-gpl.tar.xz` |
| macOS x86_64 | `jellyfin-ffmpeg_{VERSION}_portable_mac64-gpl.tar.xz` |
| macOS aarch64 | `jellyfin-ffmpeg_{VERSION}_portable_macarm64-gpl.tar.xz` |
| Windows x86_64 | `jellyfin-ffmpeg_{VERSION}_portable_win64-clang-gpl.zip` |
| Windows aarch64 | `jellyfin-ffmpeg_{VERSION}_portable_winarm64-clang-gpl.zip` |

These are the GPL builds (with x264 / x265 / etc.). The version
tracked by the `JELLYFIN_FFMPEG_VERSION` constant is the source of
truth — bumping it is a deliberate one-file change with auditable
hashes.

## Where to obtain the source

Because kino doesn't host or mirror the FFmpeg / jellyfin-ffmpeg
binaries — every download is a direct `https://github.com/jellyfin/jellyfin-ffmpeg/releases/download/v{VERSION}/...`
fetch — Jellyfin is the actual distributor and is responsible for
the GPL §6 source-offer obligation alongside their own binaries.

For the kino user's convenience, the corresponding source is
available at:

- **jellyfin-ffmpeg patches & Jellyfin's build scripts:**
  [`github.com/jellyfin/jellyfin-ffmpeg`](https://github.com/jellyfin/jellyfin-ffmpeg)
  — clone the tag matching the `JELLYFIN_FFMPEG_VERSION` constant
  in `backend/crates/kino/src/playback/ffmpeg_bundle.rs` (e.g. `v7.1.3-5`).
- **FFmpeg upstream source:** the patch series on top of
  [`github.com/FFmpeg/FFmpeg`](https://github.com/FFmpeg/FFmpeg) —
  the exact upstream tag is recorded in the jellyfin-ffmpeg release
  notes for each version.
- **System-package alternative:** users who install FFmpeg via apt /
  brew / pacman / chocolatey instead of the kino in-app download
  receive the source-offer from their distribution maintainer, not
  from kino.

## When kino's posture changes

This document holds as long as kino:

1. Does not mirror, cache, or proxy jellyfin-ffmpeg binaries on a
   kino-controlled host
2. Does not bundle FFmpeg into kino release archives produced by
   `cargo-dist`
3. Does not statically link against any FFmpeg library (we shell out
   via `std::process::Command` — see
   [`docs/roadmap/24-attributions.md`](../roadmap/24-attributions.md) §2)

If any of these three change — e.g. we mirror jellyfin-ffmpeg on a
CDN for download speed, or we add an `--enable-bundled-ffmpeg` build
flag — the source-offer obligation moves to us, and this document
must be updated to point at a kino-hosted source archive at the
matching tag.
