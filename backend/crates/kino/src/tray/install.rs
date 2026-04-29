//! Per-user tray autostart install — `kino install-tray` /
//! `kino uninstall-tray`.
//!
//! Each desktop OS has its own per-user "run this at login" path:
//!
//! - **Linux**: `~/.config/autostart/kino-tray.desktop` (XDG spec).
//!   The .deb / .rpm packages ship a system-wide entry at
//!   `/etc/xdg/autostart/kino-tray.desktop` so users on those
//!   installs don't need to run this command. `AppImage` / tarball
//!   / `cargo install` users do.
//!
//! - **`macOS`**: `~/Library/LaunchAgents/tv.kino.tray.plist` +
//!   `launchctl bootstrap gui/<uid>`. Per-user agent (NOT a daemon)
//!   so it only runs while the user is logged in. Native `.pkg`
//!   users run this command once after install; `cargo install`
//!   users run it themselves.
//!
//! - **Windows**: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\KinoTray`
//!   = `"<path-to-kino.exe>" tray`. Standard Windows per-user
//!   autostart. Native `.msi` users run this command once after
//!   install; `cargo install` users run it themselves.
//!
//! All three are idempotent: re-running `install` is safe (overwrites
//! the entry), and `uninstall` is best-effort (doesn't fail if the
//! entry was already removed manually).

use anyhow::Context as _;

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{Context as _, anyhow};
    use std::path::PathBuf;

    fn autostart_path() -> anyhow::Result<PathBuf> {
        // XDG_CONFIG_HOME first; fall back to $HOME/.config per the
        // XDG Base Directory spec. We avoid pulling in `dirs` because
        // the workspace standardised on `etcetera` for this; for the
        // narrow autostart-path question, the env-var dance is a
        // one-liner that doesn't need a dep.
        let base = match std::env::var("XDG_CONFIG_HOME") {
            Ok(xdg) if !xdg.trim().is_empty() => PathBuf::from(xdg),
            _ => {
                let home = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
                PathBuf::from(home).join(".config")
            }
        };
        Ok(base.join("autostart").join("kino-tray.desktop"))
    }

    pub fn install() -> anyhow::Result<()> {
        let path = autostart_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let exe = std::env::current_exe().context("locating current binary path")?;
        let exe_str = exe.to_string_lossy();
        let body = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Kino tray\n\
             GenericName=Kino server status indicator\n\
             Comment=Status indicator + quick-open for the Kino media server\n\
             Exec={exe_str} tray\n\
             Icon=kino\n\
             Categories=AudioVideo;Video;Network;\n\
             Terminal=false\n\
             StartupNotify=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("✓ wrote {}", path.display());
        eprintln!("  Tray will start at next login. Start now:  kino tray &");
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        let path = autostart_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                eprintln!("✓ removed {}", path.display());
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("kino-tray autostart entry isn't installed");
                Ok(())
            }
            Err(e) => Err(anyhow::Error::from(e).context(format!("removing {}", path.display()))),
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{Context as _, anyhow};
    use std::path::PathBuf;
    use std::process::Command;

    const LABEL: &str = "tv.kino.tray";

    fn agents_dir() -> anyhow::Result<PathBuf> {
        let home = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
        Ok(PathBuf::from(home).join("Library").join("LaunchAgents"))
    }

    fn plist_path() -> anyhow::Result<PathBuf> {
        Ok(agents_dir()?.join(format!("{LABEL}.plist")))
    }

    fn render_plist(exe: &std::path::Path) -> anyhow::Result<String> {
        let exe_str = exe
            .to_str()
            .ok_or_else(|| anyhow!("current binary path isn't valid UTF-8"))?;
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe_str}</string>
        <string>tray</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
        ))
    }

    fn current_uid() -> anyhow::Result<String> {
        let out = Command::new("id")
            .arg("-u")
            .output()
            .context("running `id -u`")?;
        if !out.status.success() {
            anyhow::bail!("id -u failed");
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    pub fn install() -> anyhow::Result<()> {
        let dir = agents_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let exe = std::env::current_exe().context("locating current binary path")?;
        let plist = render_plist(&exe)?;
        let path = plist_path()?;
        std::fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("✓ wrote {}", path.display());

        // Best-effort load now so the user doesn't have to re-login.
        // launchctl unload first (idempotent) then load. Both can fail
        // silently if launchd hasn't seen the agent before — that's
        // fine; the next login picks it up.
        if let Ok(uid) = current_uid() {
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/{LABEL}")])
                .status();
            let _ = Command::new("launchctl")
                .args(["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()])
                .status();
        }
        eprintln!("  Tray will start at next login (and now, if launchctl bootstrapped cleanly).");
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        let path = plist_path()?;
        if let Ok(uid) = current_uid() {
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/{LABEL}")])
                .status();
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                eprintln!("✓ removed {}", path.display());
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("kino-tray LaunchAgent isn't installed");
                Ok(())
            }
            Err(e) => Err(anyhow::Error::from(e).context(format!("removing {}", path.display()))),
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use anyhow::{Context as _, anyhow};
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

    const RUN_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "KinoTray";

    pub fn install() -> anyhow::Result<()> {
        // Under MSIX, writing to HKCU\…\Run from inside the container
        // is allowed but the entry is *redirected* to a per-package
        // virtual registry — Settings → Apps → Startup won't show it,
        // so the user can't toggle it off through the standard UI. It's
        // also redundant with whatever the manifest declares (today:
        // nothing — tray autostart for MSIX is deferred to a future
        // release that ships a small launcher binary; see
        // `backend/crates/kino/msix/README.md`).
        if crate::windows_packaging::is_msix_installed() {
            eprintln!("kino is running under MSIX; tray autostart isn't wired yet.");
            eprintln!("  Launch the tray manually:  kino tray");
            return Ok(());
        }

        let exe = std::env::current_exe().context("locating current binary path")?;
        let exe_str = exe
            .to_str()
            .ok_or_else(|| anyhow!("current binary path isn't valid UTF-8"))?;
        // Quote the exe path so spaces survive (`Program Files`).
        let cmd = format!("\"{exe_str}\" tray");
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(RUN_PATH)
            .with_context(|| format!("opening HKCU\\{RUN_PATH}"))?;
        key.set_value(VALUE_NAME, &cmd)
            .with_context(|| format!("writing HKCU\\{RUN_PATH}\\{VALUE_NAME}"))?;
        eprintln!("✓ registered HKCU\\{RUN_PATH}\\{VALUE_NAME} = {cmd}");
        eprintln!("  Tray will start at next login. Start now:  kino tray");
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        if crate::windows_packaging::is_msix_installed() {
            eprintln!(
                "kino is running under MSIX; uninstall the Store package to remove autostart."
            );
            return Ok(());
        }

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = match hkcu.open_subkey_with_flags(RUN_PATH, KEY_READ | KEY_WRITE) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("kino-tray autostart entry isn't installed");
                return Ok(());
            }
            Err(e) => {
                return Err(anyhow::Error::from(e).context(format!("opening HKCU\\{RUN_PATH}")));
            }
        };
        match key.delete_value(VALUE_NAME) {
            Ok(()) => {
                eprintln!("✓ removed HKCU\\{RUN_PATH}\\{VALUE_NAME}");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("kino-tray autostart entry isn't installed");
                Ok(())
            }
            Err(e) => {
                Err(anyhow::Error::from(e)
                    .context(format!("deleting HKCU\\{RUN_PATH}\\{VALUE_NAME}")))
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    pub fn install() -> anyhow::Result<()> {
        anyhow::bail!("tray autostart isn't implemented for this platform");
    }
    pub fn uninstall() -> anyhow::Result<()> {
        anyhow::bail!("tray autostart isn't implemented for this platform");
    }
}

/// `kino install-tray` — write the per-user autostart entry and
/// (best-effort) start the tray now.
pub fn install() -> anyhow::Result<()> {
    imp::install().context("installing tray autostart")
}

/// `kino uninstall-tray` — remove the per-user autostart entry.
/// Idempotent: returns Ok if the entry was already removed.
pub fn uninstall() -> anyhow::Result<()> {
    imp::uninstall().context("removing tray autostart")
}
