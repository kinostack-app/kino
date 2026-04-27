//! Placeholder bodies for the tray autostart subcommands. Replaced
//! when the `auto-launch` integration lands. Returning a clear
//! `anyhow` error rather than `unimplemented!()` so a user who runs
//! the subcommand sees a friendly message instead of a panic
//! backtrace.

const PENDING: &str = "Tray autostart subcommand is scaffolded but not yet implemented. \
                       Native installers (.msi / .dmg / .deb) handle autostart today. \
                       See docs/roadmap/22-desktop-tray.md.";

/// `kino install-tray` — writes the per-user autostart entry and
/// starts the tray now.
///
/// Future shape: detect OS → `auto-launch` registers the platform-
/// appropriate autostart entry → spawn the tray as a detached child.
pub fn install() -> anyhow::Result<()> {
    anyhow::bail!(PENDING);
}

/// `kino uninstall-tray` — removes the per-user autostart entry and
/// kills any running tray for this user.
pub fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!(PENDING);
}
