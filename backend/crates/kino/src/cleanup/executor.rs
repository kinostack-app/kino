//! Production [`RemovalExecutor`] — dispatches to the live torrent
//! session and the filesystem.
//!
//! The tracker is decoupled from these concrete dependencies so unit
//! tests can swap a mock executor in. This module provides the wiring
//! that the scheduler's `cleanup_retry` task uses.
//!
//! Resource semantics:
//! - `Torrent` — calls `client.remove(hash, delete_files = true)`.
//!   "Already gone" (no torrent with that hash in librqbit) counts
//!   as success, since the goal — the torrent isn't in the session
//!   anymore — has been met.
//! - `File` — `tokio::fs::remove_file`. ENOENT counts as success.
//! - `Directory` — `tokio::fs::remove_dir_all` (recursive). ENOENT
//!   counts as success.

use std::sync::Arc;

use super::tracker::{RemovalExecutor, ResourceKind};
use crate::download::TorrentSession;

/// Executor that knows how to remove every [`ResourceKind`].
#[derive(Debug, Clone)]
pub struct AppRemovalExecutor {
    /// `None` when the torrent client wasn't started (e.g. VPN
    /// failed to come up). Torrent removals while None still treat
    /// "no client" as a transient failure so the queue retries
    /// after the client is up.
    torrent: Option<Arc<dyn TorrentSession>>,
}

impl AppRemovalExecutor {
    #[must_use]
    pub fn new(torrent: Option<Arc<dyn TorrentSession>>) -> Self {
        Self { torrent }
    }
}

impl RemovalExecutor for AppRemovalExecutor {
    async fn execute(&self, kind: ResourceKind, target: &str) -> Result<(), String> {
        match kind {
            ResourceKind::Torrent => {
                let Some(client) = self.torrent.as_ref() else {
                    return Err("torrent client not running".to_owned());
                };
                match client.remove(target, true).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        // Already-gone error vocabularies vary across
                        // librqbit versions; the substring check is
                        // intentionally broad. Worst case: a real
                        // failure looks like "already gone" and we
                        // delete a queue row that should have retried;
                        // the original orphan would then be invisible,
                        // but we'd also have logged the error already.
                        let msg = e.to_string().to_lowercase();
                        if msg.contains("not found")
                            || msg.contains("no such")
                            || msg.contains("unknown torrent")
                        {
                            Ok(())
                        } else {
                            Err(e.to_string())
                        }
                    }
                }
            }
            ResourceKind::File => match tokio::fs::remove_file(target).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            ResourceKind::Directory => match tokio::fs::remove_dir_all(target).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
        }
    }
}
