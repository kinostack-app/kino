//! Playback domain — decision engine, transcode pipeline, HLS / direct
//! serving, trickplay, cast tokens, intro skipper.
//!
//! The flow on every play request:
//!   1. `source::resolve_byte_source` picks library file vs in-flight torrent
//!   2. `decision::plan_playback` decides direct / remux / transcode
//!   3. The matching handler (direct, hls/master, etc.) serves bytes
//!
//! ## Public API
//!
//! Cross-domain types (re-exported below):
//! - `PlayKind` — URL discriminator used by acquisition's
//!   grab-and-watch reply, `watch_now`'s orchestration, and the cast
//!   token endpoint
//! - `PlaybackPlan` + `PlaybackMethod` + `SourceInfo` — decision
//!   engine output, consumed by the play prepare endpoint and by
//!   client capabilities resolution
//! - `ClientCapabilities` + `DetectedClient` — UA-driven client
//!   inference shared between prepare + master endpoints
//! - `HwCapabilities` + per-backend types — exposed for the settings
//!   page's hw-acceleration selector
//! - `Profile`, `ProfileChain`, `ProfileKind` — fallback chain when
//!   HW transcode fails mid-session
//! - `AudioTrack` / `SubtitleTrack` / `LoadedStreams` — stream
//!   metadata served to the player and cast endpoint
//! - `TranscodeProgress`, `TranscodeReason(s)`, `TranscodeSessionState`
//!   — surfaced by the prepare reply + admin diagnostics
//!
//! Submodule responsibilities (everything below is internal to
//! playback unless re-exported above):
//! - `source` — byte-source resolution; sole entry point for
//!   "what's serving this entity right now"
//! - `decision` — playback plan algorithm
//! - `transcode` — `TranscodeManager` + ffmpeg lifecycle
//! - `hls/{master,variant,segment}` — HLS protocol handlers
//! - `cast` + `cast_token` — Chromecast token mint + receiver URL
//! - `trickplay` family — sprite generation, streaming-mode probe,
//!   trickplay vtt
//! - `subtitle` — subtitle stream extraction + serving
//! - `progress` — playback progress write-through
//! - `intro_skipper` — chromaprint-based intro detection
//! - `hw_probe` + `hw_probe_cache` — startup HW backend detection
//! - `handlers` — HTTP routes for the prepare / direct / subtitle /
//!   trickplay / progress endpoints. Held together by the helpers
//!   the HLS handlers also depend on (`play_session_id`, etc.)
//! - `watch_state`, `probe_handlers` — watched-state HTTP + admin
//!   probe HTTP

pub mod cast;
pub mod cast_token;
pub mod chapter_model;
pub mod decision;
pub mod downmix;
pub mod ffmpeg_bundle;
pub mod file_pick;
pub mod handlers;
pub mod hls;
pub mod hw_probe;
pub mod hw_probe_cache;
pub mod hwa_error;
pub mod intro_skipper;
pub mod probe_handlers;
pub mod profile;
pub mod progress;
pub mod source;
pub mod stream;
pub mod stream_model;
pub mod stream_probe;
pub mod subtitle;
pub mod transcode;
pub mod transcode_reason;
pub mod transcode_state;
pub mod trickplay;
pub mod trickplay_gen;
pub mod trickplay_stream;
pub mod watch_state;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Entity kind for the unified play URL. Mirrors the `{kind}` path
/// segment in `/api/v1/play/{kind}/{entity_id}/...`. Lives here at the
/// domain root because it's a domain concept (every playback module
/// needs it: source resolution, HLS / direct handlers, the `watch_now`
/// orchestrator, the cast-token endpoint). Handlers used to own it,
/// which inverted the dependency direction.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlayKind {
    Movie,
    Episode,
}

impl PlayKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Episode => "episode",
        }
    }
}

pub use decision::{
    BrowserFamily, ClientCapabilities, ClientOs, DetectedClient, PlaybackMethod, PlaybackOptions,
    PlaybackPlan, SourceInfo, plan_playback,
};
pub use hw_probe::{BackendState, BackendStatus, HwBackend, HwCapabilities, HwaFailureKind};
pub use profile::{Profile, ProfileChain, ProfileKind};
pub use stream::{
    AudioTrack, LoadedStreams, SubtitleTrack, VideoTrackInfo, load_streams, load_streams_from_probe,
};
pub use transcode::TranscodeProgress;
pub use transcode_reason::{TranscodeReason, TranscodeReasons};
pub use transcode_state::TranscodeSessionState;
