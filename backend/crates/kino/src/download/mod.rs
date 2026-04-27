//! Download — torrent lifecycle: queue → grab → download → seed →
//! cancel/cleanup. `manager` owns the librqbit handle and the
//! phase-state machine; `monitor` is the scheduler-tick that polls
//! per-row progress and triggers import on completion. `vpn`
//! sits here because the torrent client's bind-to-interface story
//! depends on the tunnel's lifecycle.
//!
//! ## Public API
//!
//! - `DownloadManager` — the main handle other domains use to start
//!   / pause / resume torrents. State is in librqbit + the DB; this
//!   manager is the seam.
//! - `DownloadPhase` — the typed state enum. Read-side queries match
//!   on it; import / cleanup decide eligibility from it.
//! - `TorrentSession` trait + `TorrentFileStream` — the polymorphic
//!   surface so playback / import can stream bytes from a live
//!   torrent without knowing about librqbit specifically. (Test
//!   harness implements them.)
//! - `model::Download` — DB row, consumed by handlers + acquisition's
//!   active-download lookup.
//!
//! `handlers`, `monitor`, `torrent_client` (the librqbit impl), and
//! `vpn::*` are domain-internal — `vpn::handlers` is the one HTTP
//! exception, registered through main.rs.

pub mod handlers;
pub mod manager;
pub mod model;
pub mod monitor;
pub mod phase;
pub mod session;
pub mod torrent_client;
pub mod vpn;

pub use manager::DownloadManager;
pub use phase::DownloadPhase;
pub use session::{TorrentFileStream, TorrentSession};
