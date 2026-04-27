//! Per-Cast-session worker thread + the registry that owns them.
//!
//! Each active session runs on its own dedicated OS thread because
//! `rust_cast`'s I/O is blocking — `CastDevice::receive()` parks on
//! the TCP socket until the next protobuf message lands. We bridge
//! to tokio via two channels:
//!
//! - **Inbound** (`crossbeam::channel::Receiver<SessionCommand>`):
//!   typed control commands from axum handlers (Play, Pause, Seek,
//!   Stop). The thread polls this with a short timeout so it can
//!   interleave command handling with the blocking
//!   `CastDevice::receive()` loop.
//! - **Outbound** (`tokio::sync::mpsc::Sender<SessionEvent>`):
//!   status updates the runtime broadcasts to WS subscribers + the
//!   DB updater consumes.
//!
//! The thread terminates when the inbound channel closes (manager
//! dropped the sender) or the device drops the TLS connection past
//! the reconnect ladder's cap.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver as CbReceiver, Sender as CbSender, TryRecvError};
use rust_cast::channels::heartbeat::HeartbeatResponse;
use rust_cast::channels::media::{
    Image, LoadOptions, Media, MediaResponse, Metadata, MovieMediaMetadata, StreamType,
};
use rust_cast::channels::receiver::CastDeviceApp;
use rust_cast::{CastDevice, ChannelMessage};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

/// First reconnect delay (5 s) per the `rust_cast` / pychromecast
/// reference values. Doubles up to [`RECONNECT_MAX_SECS`].
const RECONNECT_INITIAL_SECS: u64 = 5;
const RECONNECT_MAX_SECS: u64 = 300;
// COMMAND_POLL_INTERVAL_MS retired — the worker now interleaves
// command-drain + receive() blocking on the heartbeat cadence
// (~5s), which is fine for our latency budget.

/// Commands the runtime side sends into the worker thread.
#[derive(Debug)]
pub enum SessionCommand {
    Play,
    Pause,
    Stop,
    /// Absolute seek, seconds since start of content.
    Seek(f64),
    /// Tear the session down cleanly. Worker exits its loop.
    Shutdown,
}

/// Status / lifecycle events the worker thread emits back to the
/// runtime. Consumers: WS broadcaster (turns these into `cast.*`
/// frames), DB updater (writes to `cast_session.last_status_json`).
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// `launch_app` returned successfully; app is live on the
    /// receiver. Carries the Cast-protocol handles needed to
    /// reattach across a backend restart.
    Launched {
        transport_id: String,
        session_id: String,
    },
    /// `MEDIA_STATUS` frame from the receiver. JSON pre-serialised so
    /// the runtime can persist + broadcast without re-parsing.
    Status {
        position_ms: Option<i64>,
        json: String,
    },
    /// Reconnect ladder is in flight. UI uses this to render a
    /// "Reconnecting…" badge.
    Reconnecting { attempt: u32, next_delay_secs: u64 },
    /// Receiver closed the app or the reconnect ladder gave up.
    /// Worker has already exited by the time this lands.
    Ended { reason: SessionEndReason },
    /// Non-fatal error worth surfacing to the operator (parse
    /// failure, unexpected message). The session keeps running.
    Warning(String),
}

#[derive(Debug, Clone)]
pub enum SessionEndReason {
    /// User-initiated stop, or the receiver app exited cleanly.
    Stopped,
    /// Reconnect ladder hit its cap, or the receiver kept rejecting
    /// our `launch_app` call.
    Failed(String),
}

/// Configuration passed into the worker thread when it spawns. All
/// the data the thread needs to do its job — extracted into a
/// struct so the spawn callsite is one argument and the thread
/// boundary is explicit.
#[derive(Debug)]
pub struct SessionConfig {
    pub session_id: String,
    pub device_host: String,
    pub device_port: u16,
    /// Custom Cast Receiver app id (Cast Console). Defaults to
    /// the kino-registered receiver if config carries one,
    /// otherwise the default media receiver `CC1AD845`.
    pub app_id: String,
    /// HLS / direct-play URL the receiver should LOAD.
    pub content_url: String,
    /// `application/vnd.apple.mpegurl` for HLS, `video/mp4` for
    /// direct play, etc.
    pub content_type: String,
    /// Display title for the receiver's now-playing UI.
    pub title: String,
    /// Subtitle / description for the now-playing UI.
    pub subtitle: Option<String>,
    /// Optional poster URL.
    pub poster_url: Option<String>,
    /// Resume position in seconds. `None` = start from 0.
    pub start_position_sec: Option<f64>,
}

/// Active sessions, keyed by `session_id`. Hands out command
/// senders to handlers; routes events from worker threads to the
/// WS broadcaster + DB updater.
#[derive(Debug, Clone)]
pub struct CastSessionManager {
    inner: Arc<CastSessionManagerInner>,
}

#[derive(Debug)]
struct CastSessionManagerInner {
    sessions: Mutex<HashMap<String, CbSender<SessionCommand>>>,
}

impl Default for CastSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CastSessionManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CastSessionManagerInner {
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Spawn a worker thread for a new session. Returns the
    /// `session_id` (caller persists this on the `cast_session`
    /// row) plus the event receiver the runtime drains for
    /// status broadcasts. Errors only on the initial Cast TCP
    /// connect — once the worker is up, all subsequent failures
    /// surface as [`SessionEvent::Reconnecting`] or
    /// [`SessionEvent::Ended`] events on the channel.
    pub async fn spawn(
        &self,
        config: SessionConfig,
    ) -> anyhow::Result<(String, mpsc::Receiver<SessionEvent>)> {
        let session_id = config.session_id.clone();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<SessionCommand>();
        let (event_tx, event_rx) = mpsc::channel::<SessionEvent>(64);

        // Stash the command sender first so a quick follow-up
        // PAUSE / STOP from the user finds the session even if the
        // thread hasn't reached its first receive() yet.
        self.inner
            .sessions
            .lock()
            .await
            .insert(session_id.clone(), cmd_tx);

        std::thread::Builder::new()
            .name(format!("cast-session-{session_id}"))
            .spawn(move || worker(config, cmd_rx, event_tx))
            .map_err(|e| anyhow::anyhow!("spawn cast worker thread: {e}"))?;

        Ok((session_id, event_rx))
    }

    /// Fire-and-forget command into the worker. Returns false when
    /// the session id is unknown (already torn down).
    pub async fn send(&self, session_id: &str, cmd: SessionCommand) -> bool {
        let guard = self.inner.sessions.lock().await;
        let Some(tx) = guard.get(session_id) else {
            return false;
        };
        // crossbeam send only fails when the receiver dropped, which
        // means the worker thread already exited. Treat as
        // "session gone" too.
        tx.send(cmd).is_ok()
    }

    /// Remove a session from the registry. Caller is responsible
    /// for sending [`SessionCommand::Shutdown`] first if it wants
    /// the worker to exit cleanly; this just drops the registry
    /// entry so subsequent `send()` calls return false.
    pub async fn forget(&self, session_id: &str) {
        self.inner.sessions.lock().await.remove(session_id);
    }

    /// Generate a new opaque session id (`UUIDv4`). Persisted on
    /// the `cast_session.id` column.
    #[must_use]
    pub fn new_session_id() -> String {
        Uuid::new_v4().to_string()
    }
}

// ─── Worker thread body ──────────────────────────────────────────
//
// Pure blocking code inside this function — no `await`, no
// `tokio::*` outside the channel handles. Runs on its own OS
// thread so it can park on `CastDevice::receive()` without
// blocking the runtime.

#[allow(
    clippy::too_many_lines,
    reason = "single-thread state machine; splitting hurts more than it helps"
)]
#[allow(
    clippy::needless_pass_by_value,
    reason = "thread body — the function IS the thread, all args are moved into it"
)]
fn worker(
    config: SessionConfig,
    cmd_rx: CbReceiver<SessionCommand>,
    event_tx: mpsc::Sender<SessionEvent>,
) {
    let mut reconnect_delay_secs = RECONNECT_INITIAL_SECS;
    let mut attempt = 0u32;

    loop {
        let result = run_one_connect(&config, &cmd_rx, &event_tx);
        match result {
            Ok(end_reason) => {
                let _ = event_tx.blocking_send(SessionEvent::Ended { reason: end_reason });
                return;
            }
            Err(err) => {
                attempt += 1;
                let delay = reconnect_delay_secs;
                tracing::warn!(
                    session_id = %config.session_id,
                    error = %err,
                    attempt,
                    next_delay_secs = delay,
                    "cast worker: connection lost, retrying",
                );
                let _ = event_tx.blocking_send(SessionEvent::Reconnecting {
                    attempt,
                    next_delay_secs: delay,
                });
                // Sleep is interruptible by a Shutdown command so a
                // user-initiated cancel during a reconnect window
                // doesn't make them wait the full backoff.
                if wait_or_shutdown(&cmd_rx, Duration::from_secs(delay)) {
                    let _ = event_tx.blocking_send(SessionEvent::Ended {
                        reason: SessionEndReason::Stopped,
                    });
                    return;
                }
                reconnect_delay_secs = (reconnect_delay_secs * 2).min(RECONNECT_MAX_SECS);
            }
        }
    }
}

/// One full connect → launch → loop cycle. `Ok(reason)` = clean
/// terminal state (user stop, receiver app exit). `Err(_)` = the
/// caller should retry through the reconnect ladder.
#[allow(
    clippy::too_many_lines,
    reason = "single-shot connect+launch+message loop; splitting hurts readability of the linear protocol flow"
)]
fn run_one_connect(
    config: &SessionConfig,
    cmd_rx: &CbReceiver<SessionCommand>,
    event_tx: &mpsc::Sender<SessionEvent>,
) -> Result<SessionEndReason, anyhow::Error> {
    // Cast devices present a Google-signed cert; standard host
    // verification works. If a network's MITM proxy intercepts
    // the connection (rare on a home LAN, common on corporate
    // ones) the connect_without_host_verification fallback would
    // help — leaving that as a Phase 2 consideration.
    let cast_device = CastDevice::connect(config.device_host.clone(), config.device_port)
        .map_err(|e| anyhow::anyhow!("cast connect: {e:?}"))?;

    cast_device
        .connection
        .connect("receiver-0")
        .map_err(|e| anyhow::anyhow!("cast connection.connect: {e:?}"))?;

    let app = cast_device
        .receiver
        .launch_app(&CastDeviceApp::Custom(config.app_id.clone()))
        .map_err(|e| anyhow::anyhow!("cast launch_app: {e:?}"))?;

    let transport_id = app.transport_id.clone();
    let session_id = app.session_id.clone();

    // Open a virtual connection to the launched app session — the
    // media channel won't route to it without this step.
    cast_device
        .connection
        .connect(transport_id.clone())
        .map_err(|e| anyhow::anyhow!("cast connection.connect(transport): {e:?}"))?;

    let _ = event_tx.blocking_send(SessionEvent::Launched {
        transport_id: transport_id.clone(),
        session_id: session_id.clone(),
    });

    // Build the Media payload from the session config and load it.
    // Use load_with_opts when the caller provided a resume position
    // so the receiver seeks before playback starts (avoids the
    // visible 0:00 → user-position jump).
    let media = build_media(config);
    let load_opts = LoadOptions {
        current_time: config.start_position_sec.unwrap_or(0.0),
        autoplay: true,
    };
    cast_device
        .media
        .load_with_opts(transport_id.clone(), session_id.clone(), &media, load_opts)
        .map_err(|e| anyhow::anyhow!("cast media.load_with_opts: {e:?}"))?;

    // Main message loop. We can't poll the Cast socket non-
    // blockingly with rust_cast 0.21 — `receive()` parks until a
    // message arrives. To stay responsive to commands we do a
    // hybrid: drain pending commands first, then block on
    // receive() with a short timeout via a thread-local trick.
    //
    // rust_cast doesn't expose receive_timeout, so we use the
    // pragmatic alternative: shorter loop ticks driven by the
    // heartbeat the receiver sends every ~5 s, plus an explicit
    // command-drain step at the top of every iteration.
    loop {
        // Drain any pending commands without blocking.
        loop {
            match cmd_rx.try_recv() {
                Ok(SessionCommand::Play) => {
                    if let Err(e) = cast_device.media.play(transport_id.clone(), 0) {
                        tracing::warn!(error = ?e, "cast play failed");
                    }
                }
                Ok(SessionCommand::Pause) => {
                    if let Err(e) = cast_device.media.pause(transport_id.clone(), 0) {
                        tracing::warn!(error = ?e, "cast pause failed");
                    }
                }
                Ok(SessionCommand::Seek(secs)) => {
                    // rust_cast's seek takes f32 — narrowing from f64
                    // is fine because the value came from
                    // `position_ms / 1000.0` and any normal runtime
                    // (≤ tens of hours) lives well inside f32's
                    // precision window.
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "f64→f32 for Cast API; runtime under 24h fits cleanly"
                    )]
                    let secs_f32 = secs as f32;
                    if let Err(e) =
                        cast_device
                            .media
                            .seek(transport_id.clone(), 0, Some(secs_f32), None)
                    {
                        tracing::warn!(error = ?e, "cast seek failed");
                    }
                }
                Ok(SessionCommand::Stop | SessionCommand::Shutdown) => {
                    let _ = cast_device.receiver.stop_app(session_id.clone());
                    return Ok(SessionEndReason::Stopped);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Manager forgot us — treat as a clean stop so
                    // the receiver app doesn't linger.
                    let _ = cast_device.receiver.stop_app(session_id.clone());
                    return Ok(SessionEndReason::Stopped);
                }
            }
        }

        // Block on the next protocol message. The heartbeat keeps
        // this from blocking longer than ~5 s, which is also our
        // command-latency ceiling.
        match cast_device.receive() {
            Ok(ChannelMessage::Heartbeat(HeartbeatResponse::Ping)) => {
                let _ = cast_device.heartbeat.pong();
            }
            Ok(ChannelMessage::Media(MediaResponse::Status(status))) => {
                // rust_cast 0.21's Status doesn't impl Serialize, so
                // we hand-build a small JSON payload with the fields
                // the frontend actually consumes. Anything richer
                // can graduate to a typed shape later.
                let entry = status.entries.first();
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "f32→i64 ms cast; tens-of-hours runtime well inside i64"
                )]
                let position_ms = entry
                    .and_then(|e| e.current_time)
                    .map(|t| (f64::from(t) * 1000.0) as i64);
                let player_state = entry.map(|e| format!("{:?}", e.player_state));
                let json = serde_json::json!({
                    "player_state": player_state,
                    "current_time_sec": entry.and_then(|e| e.current_time),
                });
                let _ = event_tx.blocking_send(SessionEvent::Status {
                    position_ms,
                    json: json.to_string(),
                });
            }
            Ok(ChannelMessage::Receiver(_status)) => {
                // RECEIVER_STATUS without our app means the user
                // closed it via the TV's own UI or another sender
                // hijacked. Either way we're done.
                if !receiver_status_has_our_app(&cast_device, &config.app_id) {
                    return Ok(SessionEndReason::Stopped);
                }
            }
            Ok(_) => {} // Connection / unhandled — ignore.
            Err(e) => {
                // TLS dropped, socket EOF, malformed protobuf —
                // bubble up so the reconnect ladder kicks in.
                return Err(anyhow::anyhow!("cast receive: {e:?}"));
            }
        }
    }
}

/// Block on the command channel for up to `dur`. Returns true if a
/// Shutdown / Stop arrived (or the manager forgot us — channel
/// disconnected), false on timeout or any other command (which we
/// drop on the floor — PLAY / PAUSE / SEEK during a reconnect window
/// have nowhere to go).
fn wait_or_shutdown(cmd_rx: &CbReceiver<SessionCommand>, dur: Duration) -> bool {
    match cmd_rx.recv_timeout(dur) {
        Ok(SessionCommand::Shutdown | SessionCommand::Stop)
        | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => true,
        Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => false,
    }
}

/// Re-inspect the receiver's app list to see if our launched
/// session is still alive. Cheap when the Status frame is fresh.
fn receiver_status_has_our_app(device: &CastDevice<'_>, app_id: &str) -> bool {
    device
        .receiver
        .get_status()
        .ok()
        .is_some_and(|s| s.applications.iter().any(|a| a.app_id == app_id))
}

fn build_media(config: &SessionConfig) -> Media {
    let images = config
        .poster_url
        .as_ref()
        .map(|u| {
            vec![Image {
                url: u.clone(),
                dimensions: None,
            }]
        })
        .unwrap_or_default();

    let metadata = Metadata::Movie(MovieMediaMetadata {
        title: Some(config.title.clone()),
        subtitle: config.subtitle.clone(),
        studio: None,
        release_date: None,
        images,
    });

    Media {
        content_id: config.content_url.clone(),
        content_type: config.content_type.clone(),
        stream_type: StreamType::Buffered,
        metadata: Some(metadata),
        duration: None,
    }
}
