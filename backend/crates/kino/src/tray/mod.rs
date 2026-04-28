//! Desktop tray subsystem (subsystem 22).
//!
//! Behind the `tray` Cargo feature. Exposes the `kino tray`,
//! `kino install-tray`, and `kino uninstall-tray` subcommands. The
//! tray runs in the user's GUI session and talks to the local Kino
//! server over the existing HTTP API — no new IPC.
//!
//! - `run` (this module's `run.rs`) is the live implementation: 5s
//!   poll of `/api/v1/status`, status-coloured tray icon, two-item
//!   menu (Open in browser / Quit), single-instance lock
//! - `install` / `uninstall` write the per-user autostart entry
//!   (XDG `.desktop` on Linux, `LaunchAgent` plist on macOS, `HKCU`
//!   `Run` key on Windows). Native installers (.deb / .rpm) ship
//!   a system-wide entry instead; this command is the `AppImage` /
//!   Homebrew / `cargo install` fallback path.

mod install;
mod lock;
mod run;

pub use install::{install, uninstall};
pub use run::run;
