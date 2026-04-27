---
title: Install on Linux
description: Install kino on Debian / Ubuntu, Fedora / RHEL, Arch, and any glibc distro via AppImage.
---

kino ships as a native package on every major distro. Pick the line
that matches your system.

## Debian / Ubuntu

```sh
# Download .deb from the latest GitHub release
sudo apt install ./kino_<version>_amd64.deb
```

This installs the binary, registers a `kino` systemd service, creates
a `kino` service user, and starts the server. Open
`http://localhost:8080` and run through the setup wizard.

Service control:

```sh
sudo systemctl status kino
sudo systemctl restart kino
sudo journalctl -u kino -f
```

## Fedora / RHEL

```sh
sudo dnf install ./kino-<version>.x86_64.rpm
```

Same systemd integration as the .deb package.

## Arch Linux

```sh
yay -S kino-bin
# or paru -S kino-bin
```

Pulls the [`kino-bin`](https://aur.archlinux.org/packages/kino-bin)
package from the AUR (binary release, not source build). Maintained
by the kino release pipeline; refreshed automatically on each
release.

## AppImage (any glibc distro)

If your distro isn't covered above:

```sh
curl -fsSL https://github.com/kinostack-app/kino/releases/latest/download/kino-x86_64.AppImage -o kino.AppImage
chmod +x kino.AppImage
./kino.AppImage serve
```

The AppImage is self-contained except for **FFmpeg**, which kino
expects on `PATH`:

```sh
sudo apt install ffmpeg     # Debian / Ubuntu
sudo dnf install ffmpeg     # Fedora
```

To run as a system service:

```sh
sudo ./kino.AppImage install-service
```

To remove:

```sh
sudo ./kino.AppImage uninstall-service
```

## Firewall

kino listens on port `8080` by default. Adjust your distro's
firewall:

```sh
# ufw (Debian/Ubuntu)
sudo ufw allow 8080/tcp

# firewalld (Fedora/RHEL)
sudo firewall-cmd --add-port=8080/tcp --permanent
sudo firewall-cmd --reload
```

## Tray + autostart

The `.deb` and `.rpm` packages ship the desktop tray (system tray
icon + menu). On GNOME 40+, install the
[AppIndicator and KStatusNotifierItem extension](https://extensions.gnome.org/extension/615/appindicator-support/)
so the icon shows up.

To enable per-user autostart of the tray:

```sh
kino install-tray
```

## Configuring the data directory

By default kino stores its database + cached metadata in a per-OS
location:

| Mode | Path |
|---|---|
| systemd service | `/var/lib/kino/` |
| User mode (`--user` install) | `$XDG_DATA_HOME/kino/` (typically `~/.local/share/kino/`) |
| AppImage (no service) | `$XDG_DATA_HOME/kino/` |

Override via `--data-path /path/to/dir` or `KINO_DATA_PATH=/path/to/dir`.
