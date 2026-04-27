//! Domain enums. Used across subsystems — defined ahead of use.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Status lifecycle for movies and episodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentStatus {
    Wanted,
    Downloading,
    Available,
    Watched,
}

impl ContentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wanted => "wanted",
            Self::Downloading => "downloading",
            Self::Available => "available",
            Self::Watched => "watched",
        }
    }
}

impl std::fmt::Display for ContentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Show status from TMDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShowStatus {
    Returning,
    Ended,
    Cancelled,
    Upcoming,
}

/// Download state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Grabbing,
    Downloading,
    Paused,
    Stalled,
    Completed,
    Importing,
    Imported,
    Failed,
    Seeding,
    CleanedUp,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Grabbing => "grabbing",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Stalled => "stalled",
            Self::Completed => "completed",
            Self::Importing => "importing",
            Self::Imported => "imported",
            Self::Failed => "failed",
            Self::Seeding => "seeding",
            Self::CleanedUp => "cleaned_up",
        }
    }
}

impl std::fmt::Display for DownloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Release status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseStatus {
    Available,
    Pending,
    Grabbed,
    Rejected,
}

/// History event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Grabbed,
    DownloadCompleted,
    Imported,
    Failed,
    Deleted,
    Upgraded,
    Watched,
    FileRenamed,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grabbed => "grabbed",
            Self::DownloadCompleted => "download_completed",
            Self::Imported => "imported",
            Self::Failed => "failed",
            Self::Deleted => "deleted",
            Self::Upgraded => "upgraded",
            Self::Watched => "watched",
            Self::FileRenamed => "file_renamed",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stream type within a media file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
}

/// Video resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
pub enum Resolution {
    #[serde(rename = "480")]
    R480 = 480,
    #[serde(rename = "720")]
    R720 = 720,
    #[serde(rename = "1080")]
    R1080 = 1080,
    #[serde(rename = "2160")]
    R2160 = 2160,
}

/// Video source type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Bluray,
    Webdl,
    Webrip,
    Hdtv,
    Cam,
    Telesync,
    Telecine,
    Dvd,
    Screener,
}

/// Video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp9,
    Mpeg2,
    Vc1,
    Xvid,
}

/// Audio codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Ac3,
    Eac3,
    Dts,
    DtsHd,
    Truehd,
    Atmos,
    Flac,
    Opus,
    Lpcm,
    Mp3,
}

/// HDR format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HdrFormat {
    Sdr,
    Hdr10,
    #[serde(rename = "hdr10plus")]
    Hdr10Plus,
    DolbyVision,
    Hlg,
}

/// Monitor new items strategy for shows.
///
/// `Future`: auto-acquire new episodes as they air. This is what
/// users usually mean by "monitor this show" — the scheduler grabs
/// anything with an air date from now on.
///
/// `None`: track the show but don't auto-download anything. The
/// user grabs episodes manually via the card's "+" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MonitorNewItems {
    Future,
    None,
}

/// Why a show sits in the library. Drives the self-cleanup
/// behaviour when its last acquired episode is discarded:
/// `Adhoc` shows auto-remove when empty, `Explicit` shows stick
/// around because the user deliberately followed them.
///
/// Transitions:
/// - Follow dialog `create_show` → `Explicit`
/// - Auto-follow from Play / Get / acquire-by-tmdb → `Adhoc`
/// - Manage dialog `update_show_monitor` → flips `Adhoc` → `Explicit`
///   (submitting the dialog is commitment)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FollowIntent {
    Explicit,
    Adhoc,
}

/// VPN port forwarding provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardProvider {
    None,
    Natpmp,
    Airvpn,
    Pia,
}

/// Hardware acceleration method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HwAcceleration {
    None,
    Vaapi,
    Nvenc,
    Qsv,
}
