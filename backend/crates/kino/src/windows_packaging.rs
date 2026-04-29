//! Detection of the MSIX (Microsoft Store) install context on Windows.
//!
//! When kino is installed via the Microsoft Store (or any other MSIX
//! distribution), the binary lives under `C:\Program Files\WindowsApps\`
//! and runs inside an MSIX container. In that mode:
//!
//! - The Windows service is registered by the MSIX manifest's
//!   `<windows.service>` extension, NOT by `kino install-service`.
//! - The tray autostart is registered by the manifest's
//!   `<windows.startupTask>` extension, NOT by an HKCU\…\Run entry.
//! - Self-update is disabled in favour of Store-managed updates
//!   (see `docs/roadmap/27-auto-update.md`, when that subsystem lands).
//!
//! So the `kino install-service` and `kino install-tray` CLI commands
//! must be no-ops under MSIX — the manifest already did the registration
//! at install time, and trying again can fail with `ACCESS_DENIED` (the
//! `WindowsApps` tree is sealed) or worse, leave a redundant SCM entry.
//!
//! This module is the single shared detection helper used by
//! `service_install/windows.rs`, `tray/install.rs`, and (eventually)
//! the auto-update subsystem. Module is Windows-only — the callers
//! all sit inside `#[cfg(target_os = "windows")]` already.

/// Returns true when the running binary is installed via MSIX
/// (Microsoft Store or sideloaded `.msix`).
///
/// Detection is by `current_exe()` path: every MSIX-installed app
/// runs from a per-package directory under `C:\Program Files\WindowsApps\`.
/// Non-MSIX installs (cargo-dist `.msi`, winget, `cargo install`, raw
/// archive) live elsewhere and return false.
pub fn is_msix_installed() -> bool {
    std::env::current_exe()
        .map(|p| {
            // NTFS is case-insensitive and some user setups have
            // mixed-case path components; lowercase before matching.
            p.to_string_lossy()
                .to_ascii_lowercase()
                .contains(r"\windowsapps\")
        })
        .unwrap_or(false)
}
