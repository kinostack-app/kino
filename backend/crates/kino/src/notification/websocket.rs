//! WebSocket event push to connected clients.

use tokio::sync::broadcast;

/// Broadcast an event to all connected WebSocket clients.
pub fn broadcast_event(tx: &broadcast::Sender<String>, event: &super::Event) {
    if let Ok(json) = serde_json::to_string(event) {
        // Ignore errors — no receivers is fine
        let _ = tx.send(json);
    }
}
