---
title: Install on Raspberry Pi
description: Pre-built appliance image for Raspberry Pi 4/5, or install on existing Raspberry Pi OS via the .deb package.
---

## Appliance image (recommended for fresh installs)

The appliance image is Raspberry Pi OS Lite (64-bit) with kino
pre-installed and configured to start on boot. Flash it to an SD
card with [Raspberry Pi Imager](https://www.raspberrypi.com/software/);
it appears under **Other specific-purpose OS** → **Media servers**
→ **kino**.

Or download `kino-rpi-arm64.img.xz` from the latest
[GitHub release](https://github.com/kinostack-app/kino/releases/latest)
and flash with `dd` / Etcher / Raspberry Pi Imager.

After first boot:

1. Find the Pi's IP address (your router's admin page, or
   `ping raspberrypi.local`)
2. Open `http://<pi-ip>:8080` in a browser on another device
3. Run through the setup wizard

Default SSH credentials are `pi` / `raspberry` — change them
immediately via `passwd` or by adding an `userconf.txt` to
`/boot/firmware/` per the Raspberry Pi docs.

## Existing Raspberry Pi OS install

If you already run Raspberry Pi OS:

```sh
curl -fsSL https://github.com/kinostack-app/kino/releases/latest/download/kino_<version>_arm64.deb -o kino.deb
sudo apt install ./kino.deb
```

Same systemd service registration as the standard
[Linux .deb install](./install-linux).

## Hardware recommendations

- **Pi 5 (4 GB or 8 GB)** — smooth for 1080p library, software
  transcode for 1-2 concurrent streams. The recommended target
- **Pi 4 (4 GB or 8 GB)** — works, but software transcode is borderline
  for 1080p; direct-play (no transcode) is fine
- **Pi Zero 2 W / Pi 3** — not supported. kino's dependency footprint
  (sqlx, librqbit, FFmpeg) doesn't fit comfortably on these

## Storage

Don't run kino's library off the SD card. Mount an external SSD or
HDD via USB 3 and point kino's media path at it during the setup
wizard. SD-card writes wear out fast under continuous database +
download activity.

## Hardware transcode

Pi 4 / 5 expose a V4L2 stateless H.264 / HEVC encoder. kino picks
this automatically when `ffmpeg` is built with V4L2 support — the
appliance image's bundled FFmpeg is. Direct-play (matching codec
in the playback device) is always preferred over transcode.
