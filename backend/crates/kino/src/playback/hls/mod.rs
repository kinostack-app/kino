//! HLS — protocol-specific handlers + helpers. Three endpoints
//! (`master.m3u8`, `variant.m3u8`, `segments/{slug}`) and the helpers
//! that build their content. The transcode lifecycle (ffmpeg
//! spawn / respawn / cleanup) lives in `playback/transcode.rs` —
//! these handlers just serve the bytes ffmpeg writes and
//! orchestrate session state.

pub mod master;
pub mod segment;
pub mod variant;
