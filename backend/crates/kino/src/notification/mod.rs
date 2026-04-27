//! Notification — durable history log + WebSocket push + outbound
//! webhooks. Every state-changing event in the app passes through
//! `events::AppEvent` first (the typed variant set), then the events
//! listener fans it out to:
//!   * `history::log_event` — durable row for the history view
//!   * `websocket` — broadcast to live tabs
//!   * `webhook::deliver` — outbound HTTP with retry ladder
//!
//! ## Public API
//!
//! - `Event` (this file) — the un-typed shape webhook templates
//!   render against. Built from `AppEvent` in
//!   `events/listeners.rs::build_notification_event`.
//! - `history::{History, log_event, list_history}` — model + writer
//!   + handler
//! - `webhook::{WebhookTarget, deliver, send_once}` — model + main
//!   delivery primitive + test-button bypass
//! - `webhook_retry::retry_sweep` — scheduler-driven sweep that
//!   re-tries quarantined webhooks
//! - `websocket` — broadcaster machinery
//! - `ws_handlers` — `/ws` HTTP upgrade endpoint
//!
//! Internal: nothing — this module is one of the smallest seams
//! between `AppEvent` and the outside world.

pub mod history;
pub mod webhook;
pub mod webhook_retry;
pub mod websocket;
pub mod ws_handlers;

use serde::Serialize;

/// An event that can be logged, pushed via WebSocket, and sent to webhooks.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub event_type: String,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub title: Option<String>,
    pub show: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub quality: Option<String>,
    pub year: Option<i64>,
    pub size: Option<String>,
    pub indexer: Option<String>,
    pub message: Option<String>,
}

impl Event {
    pub fn simple(event_type: &str, title: &str) -> Self {
        Self {
            event_type: event_type.to_owned(),
            movie_id: None,
            episode_id: None,
            title: Some(title.to_owned()),
            show: None,
            season: None,
            episode: None,
            quality: None,
            year: None,
            size: None,
            indexer: None,
            message: None,
        }
    }
}
