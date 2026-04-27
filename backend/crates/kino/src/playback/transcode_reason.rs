//! `TranscodeReason` — why a playback request wasn't eligible for
//! direct play.
//!
//! Every decision the playback layer makes (direct-play vs. remux
//! vs. full transcode) produces a set of reasons when anything is
//! less than the fully direct path. The decision engine walks
//! container → video codec → video profile → video level →
//! pixel format → bit depth → HDR metadata → audio codec → audio
//! channels → audio sample rate → subtitle codec, and each
//! mismatch adds a flag.
//!
//! The set is surfaced three ways:
//! 1. **URL query** (`?tr=container_not_supported,audio_codec_not_supported`)
//!    — the transcode session carries its own reasons so logs +
//!    diagnostics can link a ffmpeg spawn back to its cause.
//! 2. **`PlayPrepareReply.transcode_reasons`** — the frontend
//!    renders "transcoding because: audio codec not supported"
//!    so the user doesn't have to guess.
//! 3. **Structured logs** — every session-start log line carries
//!    the `Display` form.
//!
//! Snake-case over numeric flag indices: the wire format is
//! greppable in logs and self-documenting. A future compact
//! encoding (bitmask int) can replace it without schema churn
//! because the public type is a set of typed values, not a
//! serialized number.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A single reason the playback layer had to diverge from direct
/// play. Flags combine into `TranscodeReasons` — see that type for
/// the set-level API.
///
/// Variant order is stable: new variants append. The `as_str` /
/// `from_str` mapping is the wire contract — changing a name is
/// a breaking change.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TranscodeReason {
    // ── Container ────────────────────────────────────────────────
    /// The source container is not in the client's accept list.
    /// Often remuxable — this flag alone is sufficient for remux
    /// rather than full transcode.
    ContainerNotSupported,

    // ── Video ────────────────────────────────────────────────────
    /// Source video codec (e.g., HEVC to an old browser) is not
    /// in the client's accept list.
    VideoCodecNotSupported,
    /// Codec is supported but the profile (e.g., HEVC Main 10 to
    /// a Main-only decoder) isn't.
    VideoProfileNotSupported,
    /// Codec + profile are supported but the level exceeds client
    /// capability (H.264 L5.1 to a L4.1 decoder).
    VideoLevelNotSupported,
    /// Pixel bit depth (10-bit H.264 to Safari) isn't supported.
    VideoBitDepthNotSupported,
    /// HDR format (HDR10+, DV Profile 5) not supported by the
    /// client — either tone-map or strip dynamic metadata.
    VideoRangeTypeNotSupported,
    /// Resolution exceeds the client's maximum.
    VideoResolutionNotSupported,
    /// Frame rate is above the client's maximum (or an unsupported
    /// variable-framerate marker).
    VideoFramerateNotSupported,
    /// Bitrate exceeds the user's configured streaming cap.
    VideoBitrateNotSupported,

    // ── Audio ────────────────────────────────────────────────────
    /// Primary (and any fallback) audio codec not accepted —
    /// forces an audio-track transcode.
    AudioCodecNotSupported,
    /// Channel layout (e.g., 7.1 to a stereo-only device) forces
    /// a downmix.
    AudioChannelsNotSupported,
    /// Sample rate (e.g., 192 kHz source) above client support.
    AudioSampleRateNotSupported,
    /// Audio bit depth (e.g., 24-bit PCM) above client support.
    AudioBitDepthNotSupported,
    /// Audio bitrate exceeds the user's configured streaming cap.
    AudioBitrateNotSupported,

    // ── Subtitles ────────────────────────────────────────────────
    /// An image-based subtitle (PGS, VOBSUB, DVB) is selected and
    /// the client can't render it — forces burn-in, which forces
    /// a video transcode.
    SubtitleCodecNotSupported,
}

impl TranscodeReason {
    /// Canonical snake-case wire name. Must match the serde
    /// representation exactly — changing one without the other is
    /// a breaking change.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContainerNotSupported => "container_not_supported",
            Self::VideoCodecNotSupported => "video_codec_not_supported",
            Self::VideoProfileNotSupported => "video_profile_not_supported",
            Self::VideoLevelNotSupported => "video_level_not_supported",
            Self::VideoBitDepthNotSupported => "video_bit_depth_not_supported",
            Self::VideoRangeTypeNotSupported => "video_range_type_not_supported",
            Self::VideoResolutionNotSupported => "video_resolution_not_supported",
            Self::VideoFramerateNotSupported => "video_framerate_not_supported",
            Self::VideoBitrateNotSupported => "video_bitrate_not_supported",
            Self::AudioCodecNotSupported => "audio_codec_not_supported",
            Self::AudioChannelsNotSupported => "audio_channels_not_supported",
            Self::AudioSampleRateNotSupported => "audio_sample_rate_not_supported",
            Self::AudioBitDepthNotSupported => "audio_bit_depth_not_supported",
            Self::AudioBitrateNotSupported => "audio_bitrate_not_supported",
            Self::SubtitleCodecNotSupported => "subtitle_codec_not_supported",
        }
    }
}

impl fmt::Display for TranscodeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TranscodeReason {
    type Err = ParseTranscodeReasonError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "container_not_supported" => Ok(Self::ContainerNotSupported),
            "video_codec_not_supported" => Ok(Self::VideoCodecNotSupported),
            "video_profile_not_supported" => Ok(Self::VideoProfileNotSupported),
            "video_level_not_supported" => Ok(Self::VideoLevelNotSupported),
            "video_bit_depth_not_supported" => Ok(Self::VideoBitDepthNotSupported),
            "video_range_type_not_supported" => Ok(Self::VideoRangeTypeNotSupported),
            "video_resolution_not_supported" => Ok(Self::VideoResolutionNotSupported),
            "video_framerate_not_supported" => Ok(Self::VideoFramerateNotSupported),
            "video_bitrate_not_supported" => Ok(Self::VideoBitrateNotSupported),
            "audio_codec_not_supported" => Ok(Self::AudioCodecNotSupported),
            "audio_channels_not_supported" => Ok(Self::AudioChannelsNotSupported),
            "audio_sample_rate_not_supported" => Ok(Self::AudioSampleRateNotSupported),
            "audio_bit_depth_not_supported" => Ok(Self::AudioBitDepthNotSupported),
            "audio_bitrate_not_supported" => Ok(Self::AudioBitrateNotSupported),
            "subtitle_codec_not_supported" => Ok(Self::SubtitleCodecNotSupported),
            other => Err(ParseTranscodeReasonError(other.to_owned())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown transcode reason: {0}")]
pub struct ParseTranscodeReasonError(pub String);

/// Set of reasons. Ordered (`BTreeSet`) so serialization is
/// deterministic and logs diff cleanly.
///
/// Wire format is `Vec<TranscodeReason>` — serde flattens via
/// `#[serde(transparent)]`, and utoipa generates a
/// `Array<TranscodeReason>` schema so the frontend sees a plain
/// `TranscodeReason[]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct TranscodeReasons(BTreeSet<TranscodeReason>);

impl TranscodeReasons {
    /// Empty set — direct-play decisions return this.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// Add a reason. No-op if already present.
    pub fn add(&mut self, reason: TranscodeReason) {
        self.0.insert(reason);
    }

    /// Merge another set into this one.
    pub fn extend(&mut self, other: &Self) {
        self.0.extend(other.0.iter().copied());
    }

    #[must_use]
    pub fn contains(&self, reason: TranscodeReason) -> bool {
        self.0.contains(&reason)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = TranscodeReason> + '_ {
        self.0.iter().copied()
    }

    /// Emit as a comma-separated snake-case list, suitable for a
    /// URL query parameter. Empty set → empty string so callers
    /// can gate `?tr=` emission on `!is_empty()`.
    #[must_use]
    pub fn to_query_value(&self) -> String {
        let mut out = String::new();
        let mut first = true;
        for r in self.iter() {
            if !first {
                out.push(',');
            }
            out.push_str(r.as_str());
            first = false;
        }
        out
    }

    /// Parse a comma-separated query value. Unknown tokens are
    /// collected into the error rather than silently dropped —
    /// the caller can warn and continue with the parsed subset,
    /// or reject outright.
    pub fn from_query_value(s: &str) -> Result<Self, ParseTranscodeReasonsError> {
        let mut reasons = Self::new();
        let mut unknown = Vec::new();
        for token in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            match TranscodeReason::from_str(token) {
                Ok(r) => reasons.add(r),
                Err(ParseTranscodeReasonError(t)) => unknown.push(t),
            }
        }
        if unknown.is_empty() {
            Ok(reasons)
        } else {
            Err(ParseTranscodeReasonsError {
                parsed: reasons,
                unknown,
            })
        }
    }
}

impl FromIterator<TranscodeReason> for TranscodeReasons {
    /// Build a set from any iterator of reasons.
    /// `TranscodeReasons::from_iter([...])` / `.collect()` both work.
    fn from_iter<I: IntoIterator<Item = TranscodeReason>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl fmt::Display for TranscodeReasons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("direct_play");
        }
        let mut first = true;
        for r in self.iter() {
            if !first {
                f.write_str(" + ")?;
            }
            f.write_str(r.as_str())?;
            first = false;
        }
        Ok(())
    }
}

/// `from_query_value` error — carries both the successfully-parsed
/// subset and the list of unknown tokens so the caller can decide
/// whether to log + continue or reject.
#[derive(Debug, thiserror::Error)]
#[error("unknown transcode reasons: {unknown:?}")]
pub struct ParseTranscodeReasonsError {
    pub parsed: TranscodeReasons,
    pub unknown: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_and_from_str_round_trip() {
        // Every variant the enum exposes must parse back to itself.
        // A new variant that forgets a `from_str` arm regresses
        // this — `match` is exhaustive over the literal list so
        // extending the enum without extending from_str is a
        // compile error too.
        let all = [
            TranscodeReason::ContainerNotSupported,
            TranscodeReason::VideoCodecNotSupported,
            TranscodeReason::VideoProfileNotSupported,
            TranscodeReason::VideoLevelNotSupported,
            TranscodeReason::VideoBitDepthNotSupported,
            TranscodeReason::VideoRangeTypeNotSupported,
            TranscodeReason::VideoResolutionNotSupported,
            TranscodeReason::VideoFramerateNotSupported,
            TranscodeReason::VideoBitrateNotSupported,
            TranscodeReason::AudioCodecNotSupported,
            TranscodeReason::AudioChannelsNotSupported,
            TranscodeReason::AudioSampleRateNotSupported,
            TranscodeReason::AudioBitDepthNotSupported,
            TranscodeReason::AudioBitrateNotSupported,
            TranscodeReason::SubtitleCodecNotSupported,
        ];
        for r in all {
            let s = r.as_str();
            let back = TranscodeReason::from_str(s).unwrap();
            assert_eq!(back, r, "round-trip failed for {s}");
        }
    }

    #[test]
    fn serde_matches_as_str() {
        // `#[serde(rename_all = "snake_case")]` must produce the
        // same string `as_str` returns — drift between the two
        // would be invisible in Rust but break the frontend.
        let r = TranscodeReason::VideoCodecNotSupported;
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#""video_codec_not_supported""#);
        assert_eq!(json.trim_matches('"'), r.as_str());
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(TranscodeReason::from_str("not_a_real_reason").is_err());
    }

    #[test]
    fn reasons_set_add_is_idempotent() {
        let mut r = TranscodeReasons::new();
        r.add(TranscodeReason::ContainerNotSupported);
        r.add(TranscodeReason::ContainerNotSupported);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn reasons_set_sorts_deterministically() {
        // BTreeSet orders by the derived Ord, which follows
        // declaration order — serialization must produce the same
        // token sequence regardless of insertion order so query
        // values + log lines diff cleanly.
        let mut a = TranscodeReasons::new();
        a.add(TranscodeReason::AudioCodecNotSupported);
        a.add(TranscodeReason::ContainerNotSupported);

        let mut b = TranscodeReasons::new();
        b.add(TranscodeReason::ContainerNotSupported);
        b.add(TranscodeReason::AudioCodecNotSupported);

        assert_eq!(a.to_query_value(), b.to_query_value());
        assert_eq!(
            a.to_query_value(),
            "container_not_supported,audio_codec_not_supported"
        );
    }

    #[test]
    fn reasons_query_round_trip() {
        let mut original = TranscodeReasons::new();
        original.add(TranscodeReason::ContainerNotSupported);
        original.add(TranscodeReason::AudioCodecNotSupported);
        original.add(TranscodeReason::VideoRangeTypeNotSupported);

        let q = original.to_query_value();
        let parsed = TranscodeReasons::from_query_value(&q).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn reasons_query_empty_is_empty_string() {
        let empty = TranscodeReasons::new();
        assert_eq!(empty.to_query_value(), "");
        // And empty string parses back to empty set, not an error.
        let parsed = TranscodeReasons::from_query_value("").unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn reasons_query_tolerates_whitespace() {
        let parsed = TranscodeReasons::from_query_value(
            " container_not_supported , audio_codec_not_supported ",
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn reasons_query_surfaces_unknown_tokens() {
        let err = TranscodeReasons::from_query_value(
            "container_not_supported,not_a_reason,audio_codec_not_supported,also_bogus",
        )
        .unwrap_err();
        // Valid tokens are preserved in `parsed` so a tolerant
        // caller can log + continue with the subset.
        assert_eq!(err.parsed.len(), 2);
        assert_eq!(err.unknown, vec!["not_a_reason", "also_bogus"]);
    }

    #[test]
    fn display_direct_play_for_empty() {
        let r = TranscodeReasons::new();
        assert_eq!(format!("{r}"), "direct_play");
    }

    #[test]
    fn display_joins_with_plus() {
        let mut r = TranscodeReasons::new();
        r.add(TranscodeReason::ContainerNotSupported);
        r.add(TranscodeReason::AudioCodecNotSupported);
        // Log-friendly separator — easier to grep than a comma
        // which shows up inside other fields.
        assert_eq!(
            format!("{r}"),
            "container_not_supported + audio_codec_not_supported"
        );
    }

    #[test]
    fn extend_merges_sets() {
        let mut a = TranscodeReasons::new();
        a.add(TranscodeReason::ContainerNotSupported);
        let mut b = TranscodeReasons::new();
        b.add(TranscodeReason::AudioCodecNotSupported);
        b.add(TranscodeReason::ContainerNotSupported); // duplicate
        a.extend(&b);
        assert_eq!(a.len(), 2);
        assert!(a.contains(TranscodeReason::ContainerNotSupported));
        assert!(a.contains(TranscodeReason::AudioCodecNotSupported));
    }
}
