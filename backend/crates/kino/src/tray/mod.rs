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
//! - `install` / `uninstall` are still stubs — the per-user
//!   autostart entry (registry / `LaunchAgent` / `~/.config/autostart`)
//!   lands in a follow-up task. Native installers handle this for
//!   Tier 1 users; the subcommand is the tarball-fallback path

mod lock;
mod run;
mod stub;

pub use run::run;
pub use stub::{install, uninstall};
