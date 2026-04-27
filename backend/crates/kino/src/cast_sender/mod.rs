//! Server-side Chromecast sender (subsystem 32).
//!
//! Replaces the browser-native `chrome.cast.*` SDK so users on
//! Firefox / Safari (which have no Cast API) can still cast to a
//! Chromecast. The browser only ever talks to kino's REST + WS
//! surface; kino speaks the Cast V2 protobuf protocol directly to
//! the device via [`rust_cast`].
//!
//! ## Public API
//!
//! - [`discovery::start`] — long-running mDNS browser registered on
//!   startup; populates the `cast_device` table as Chromecasts come
//!   and go on the LAN. Owned by [`AppState`]; lives for the
//!   lifetime of the process.
//! - [`session::CastSessionManager`] — registry of active per-device
//!   sessions, each running on its own dedicated `std::thread` to
//!   bridge `rust_cast`'s blocking I/O to tokio. Held on `AppState`.
//! - [`handlers`] — HTTP routes for the `/api/v1/cast/*` family,
//!   registered via `main.rs`.
//! - [`device::CastDevice`] / [`device::CastSession`] — DB row +
//!   API response shapes; consumed by handlers.
//!
//! ## Concurrency model
//!
//! `rust_cast` is purely blocking — `CastDevice::receive()` blocks
//! on the TCP socket until the next protobuf message arrives.
//! Bridging:
//!
//! ```text
//!                   ┌──────── tokio runtime ────────┐
//!  axum handler ──► CastSessionManager ──cmd──► std::thread
//!                                       ◄─event─       │
//!                                                      │
//!                                                      ▼
//!                                              CastDevice (TLS)
//!                                                      │
//!                                                      ▼
//!                                                   Chromecast
//! ```
//!
//! - Per-session **command channel** (`crossbeam::channel`) carries
//!   typed [`session::SessionCommand`] from handlers into the
//!   blocking thread.
//! - Per-session **event channel** (`tokio::sync::mpsc`) carries
//!   [`session::SessionEvent`] back out for the WS broadcaster +
//!   DB updater to consume.
//!
//! Total thread count: 1 (mDNS daemon) + N (active sessions). N is
//! bounded by the number of Chromecasts a single user is casting to
//! simultaneously — typically 0–2.

pub mod device;
pub mod discovery;
pub mod handlers;
pub mod session;

pub use device::{CastDevice, CastSession, CastSessionStatus};
pub use session::CastSessionManager;
