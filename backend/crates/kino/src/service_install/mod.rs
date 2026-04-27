//! Tarball-fallback service install (`kino install-service` /
//! `kino uninstall-service`).
//!
//! Native packages (`.deb`, `.rpm`, `.msi`, `.dmg`) handle service
//! registration during install, so most users never invoke these.
//! They exist for power users who download the raw archive or run
//! `cargo install kino`. See `docs/architecture/service-install.md`
//! for the full design + per-OS matrix.
//!
//! Implementation status (2026-04-26):
//! - **Linux** (`linux.rs`) — implemented: writes systemd unit (system
//!   or user mode), reloads daemon, enables + starts. Idempotent
//! - **macOS** (`macos.rs`) — stub. Design captured in
//!   `docs/architecture/service-install.md`; needs a macOS host to
//!   exercise + verify
//! - **Windows** (`windows.rs`) — stub. Needs `windows-service` crate
//!   integration + UAC manifest + Windows toolchain. Design captured
//!   in the same doc

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

/// Install Kino as a platform-native service (systemd unit /
/// `LaunchDaemon` plist / Windows SCM registration), enabling it to
/// start on boot and starting it now. `user_mode` is honoured on
/// Linux (writes a per-user systemd unit instead of system-wide);
/// ignored on macOS and Windows where per-user services use the
/// separate tray autostart path.
///
/// Requires elevated privileges in system mode. On Linux, expect to
/// be re-run with `sudo` if not root. Future: macOS adds an
/// `osascript` admin prompt; Windows adds a UAC re-launch — see the
/// design doc.
pub fn install(user_mode: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::install(user_mode);

    #[cfg(target_os = "macos")]
    {
        let _ = user_mode;
        return macos::install();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = user_mode;
        return windows::install();
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = user_mode;
        anyhow::bail!("service install isn't supported on this OS");
    }
}

/// Stop the platform service and remove its descriptor. Does NOT
/// delete user data (config, DB, library) — that's a separate
/// explicit `kino reset` step.
pub fn uninstall() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::uninstall();

    #[cfg(target_os = "macos")]
    return macos::uninstall();

    #[cfg(target_os = "windows")]
    return windows::uninstall();

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    anyhow::bail!("service uninstall isn't supported on this OS");
}
