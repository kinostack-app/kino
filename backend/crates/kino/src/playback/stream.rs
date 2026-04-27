//! Typed per-track shapes for the playback layer.
//!
//! The raw DB `models::stream::Stream` is the superset of fields
//! across video / audio / subtitle streams — useful for the
//! `/api/v1/media/{id}/streams` debug endpoint, awkward everywhere
//! else. These types narrow to the fields each consumer actually
//! reads:
//!
//! * `AudioTrack` powers the audio picker on the player + the
//!   multi-audio compat selector in the decision engine.
//! * `SubtitleTrack` powers the subtitle picker, the
//!   forced-subtitles-on-audio-change rule, and the
//!   direct-play-or-burn-in branch in the decision engine.
//! * `VideoTrackInfo` carries HDR / level / profile / bit-depth
//!   for the HDR branch landing next.
//!
//! Labels + `vtt_url` are **computed**, not stored — one source
//! of truth: row → typed. No hand-rolled mirrors on the frontend.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::playback::stream_model::Stream;

// ─── Audio ──────────────────────────────────────────────────────

/// A selectable audio track. Mirrors the DB `stream` row shape
/// but narrowed to audio-relevant fields with a computed display
/// label and a derived `is_commentary` flag for UI filtering.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AudioTrack {
    /// ffprobe stream index — feeds the HLS session's
    /// `?audio_stream=N` selector and any `-map 0:a:N` arg.
    pub stream_index: i64,
    /// Lowercase codec name (`aac`, `ac3`, `eac3`, `truehd`, ...).
    pub codec: String,
    /// BCP-47-ish language tag; ffprobe typically emits 3-letter
    /// ("eng", "jpn"). None when untagged.
    pub language: Option<String>,
    /// Channel count. `None` means ffprobe didn't surface it.
    pub channels: Option<i64>,
    /// Human layout string ("stereo", "5.1", "7.1"). May be None
    /// when ffprobe only reported channel count.
    pub channel_layout: Option<String>,
    /// Raw title from metadata; `None` when absent.
    pub title: Option<String>,
    /// Per-track bitrate in bits/second. `None` when ffprobe
    /// didn't surface one (common for stream-copy tracks in MKV
    /// containers, which only carry container-level bitrate).
    pub bitrate: Option<i64>,
    /// Sample rate in Hz (44100, 48000, 96000). `None` when
    /// absent from the probe.
    pub sample_rate: Option<i64>,
    /// Bit depth per audio sample. Mostly relevant for lossless
    /// formats — AAC / DDP don't carry per-sample bit depth.
    pub bit_depth: Option<i64>,
    /// True when the container flagged this as the default track.
    pub is_default: bool,
    /// Derived: title contains "commentary" (case-insensitive).
    /// Not a standard disposition flag — a heuristic so the
    /// picker can deprioritise director-commentary tracks when
    /// the user hasn't explicitly asked for them.
    pub is_commentary: bool,
    /// True when the track carries Dolby Atmos — detected
    /// during import from the ffprobe `profile` string
    /// ("Dolby Digital Plus + Dolby Atmos", "`TrueHD` Atmos").
    /// Drives the "Atmos" label + the future passthrough
    /// routing for Atmos-capable clients. Unrelated to
    /// whether the current client can decode it; the base
    /// EAC-3 / `TrueHD` bitstream is what the decoder sees.
    pub is_atmos: bool,
    /// Raw ffprobe `profile` string — `"DTS-HD MA"`, `"LC"`,
    /// `"Main 10"`, etc. Carried end-to-end so the HLS
    /// `CODECS` emitter can distinguish DTS-HD MA from plain
    /// DTS Core (promote `dtsc` → `dtsh`) without re-probing
    /// on every playback request.
    pub profile: Option<String>,
    /// Track-purpose tags — `"main"`, `"commentary"`, `"dub"`,
    /// `"description"`. Derived today from the `is_commentary`
    /// heuristic + `is_default` flag; will graduate to the
    /// ffprobe disposition bitmap once the #05 branch lands.
    /// Stored as a list so a single track can be e.g.
    /// `["main", "dub"]` for a localised master track.
    pub roles: Vec<String>,
    /// Display-ready label: "English · 5.1 · AC-3". Built once
    /// here so every surface (picker, Cast label, logs) sees the
    /// same string.
    pub label: String,
}

impl AudioTrack {
    /// Narrow to the decision-engine's `AudioCandidate` shape.
    /// Omits the display-level fields (label, `is_default`,
    /// `is_commentary`) the engine doesn't consult.
    #[must_use]
    pub fn to_candidate(&self) -> crate::playback::decision::AudioCandidate {
        crate::playback::decision::AudioCandidate {
            stream_index: self.stream_index,
            codec: self.codec.clone(),
            channels: self.channels,
            channel_layout: self.channel_layout.clone(),
            profile: self.profile.clone(),
        }
    }

    /// Build from a DB `Stream` row. Caller is responsible for
    /// filtering to `stream_type = 'audio'`.
    fn from_row(row: &Stream) -> Self {
        let codec = row.codec.clone().unwrap_or_default().to_ascii_lowercase();
        let is_commentary = row
            .title
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().contains("commentary"));
        let label = build_audio_label(
            row.language.as_deref(),
            row.channel_layout.as_deref(),
            row.channels,
            &codec,
            row.title.as_deref(),
            row.is_atmos,
        );
        let roles = derive_audio_roles(row.title.as_deref(), is_commentary, row.is_default);
        Self {
            stream_index: row.stream_index,
            codec,
            language: row.language.clone(),
            channels: row.channels,
            channel_layout: row.channel_layout.clone(),
            title: row.title.clone(),
            bitrate: row.bitrate,
            sample_rate: row.sample_rate,
            bit_depth: row.bit_depth,
            is_default: row.is_default,
            is_commentary,
            is_atmos: row.is_atmos,
            roles,
            label,
            profile: row.profile.clone(),
        }
    }
}

/// Heuristic track-purpose tagging. Graduates to ffprobe
/// disposition bits in the #05 branch.
fn derive_audio_roles(title: Option<&str>, is_commentary: bool, is_default: bool) -> Vec<String> {
    let mut roles: Vec<String> = Vec::new();
    let title_lc = title.map(str::to_ascii_lowercase).unwrap_or_default();
    if is_commentary {
        roles.push("commentary".into());
    } else if title_lc.contains("descriptive") || title_lc.contains("description") {
        roles.push("description".into());
    } else if title_lc.contains("dub") || title_lc.contains("dubbed") {
        roles.push("dub".into());
    } else if is_default || roles.is_empty() {
        roles.push("main".into());
    }
    roles
}

fn build_audio_label(
    language: Option<&str>,
    channel_layout: Option<&str>,
    channels: Option<i64>,
    codec: &str,
    title: Option<&str>,
    is_atmos: bool,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = language.filter(|s| !s.is_empty()) {
        parts.push(l.to_owned());
    }
    // Prefer the semantic layout ("5.1") over raw channel count
    // ("6 ch"); fall back to the count when layout is missing.
    match (channel_layout, channels) {
        (Some(layout), _) if !layout.is_empty() => parts.push(layout.to_owned()),
        (_, Some(n)) if n > 0 => parts.push(format!("{n} ch")),
        _ => {}
    }
    // Atmos is a property of the bitstream, not the codec family —
    // render it alongside the codec rather than inside it so
    // "English · 5.1 · EAC-3 · Atmos" reads cleanly.
    if !codec.is_empty() {
        parts.push(codec.to_ascii_uppercase());
    }
    if is_atmos {
        parts.push("Atmos".into());
    }
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        parts.push(format!("“{t}”"));
    }
    if parts.is_empty() {
        "Audio track".into()
    } else {
        parts.join(" · ")
    }
}

// ─── Subtitles ──────────────────────────────────────────────────

/// A selectable subtitle track. `vtt_url` is `Some` for text
/// subtitles the server can serve as `WebVTT`; `None` for
/// image-based subs (PGS, VOBSUB, DVB) that need burn-in and
/// therefore don't have a sidecar URL the player can load.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SubtitleTrack {
    pub stream_index: i64,
    /// Lowercase codec — `subrip`, `ass`, `mov_text`, `webvtt`
    /// for text; `hdmv_pgs_subtitle`, `dvd_subtitle`, etc. for
    /// image.
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    /// Derived from title.
    pub is_commentary: bool,
    /// True when loaded from an external sidecar file rather than
    /// embedded in the container.
    pub is_external: bool,
    /// Track-purpose tags — `"subtitle"`, `"caption"` (closed
    /// captions / SDH), `"commentary"`. Derived today from the
    /// existing disposition flags; graduates to the ffprobe
    /// disposition bitmap once the #05 branch lands.
    pub roles: Vec<String>,
    /// Display-ready label: "English (forced)", "English SDH".
    pub label: String,
    /// Frontend `<track>` source. `None` for image subs — the
    /// client must burn-in via the decision engine's subtitle
    /// branch (landing in the PGS burn-in commit).
    pub vtt_url: Option<String>,
}

impl SubtitleTrack {
    /// Build from a DB `Stream` row + play-route identity so
    /// `vtt_url` points at the right endpoint.
    fn from_row(row: &Stream, kind: &str, entity_id: i64) -> Self {
        let codec = row.codec.clone().unwrap_or_default().to_ascii_lowercase();
        let is_commentary = row
            .title
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().contains("commentary"));
        let is_text = is_text_subtitle_codec(&codec);
        let vtt_url = is_text.then(|| {
            format!(
                "/api/v1/play/{kind}/{entity_id}/subtitles/{}",
                row.stream_index
            )
        });
        let label = build_subtitle_label(
            row.language.as_deref(),
            row.title.as_deref(),
            row.is_forced,
            row.is_hearing_impaired,
            is_commentary,
        );
        let mut roles: Vec<String> = Vec::new();
        if is_commentary {
            roles.push("commentary".into());
        } else if row.is_hearing_impaired {
            roles.push("caption".into());
        } else {
            roles.push("subtitle".into());
        }
        Self {
            stream_index: row.stream_index,
            codec,
            language: row.language.clone(),
            title: row.title.clone(),
            is_default: row.is_default,
            is_forced: row.is_forced,
            is_hearing_impaired: row.is_hearing_impaired,
            is_commentary,
            is_external: row.is_external,
            roles,
            label,
            vtt_url,
        }
    }
}

/// Codecs the subtitle-serving endpoint can convert to `WebVTT`
/// today. Anything outside this set is image-based and must be
/// burned in.
#[must_use]
pub fn is_text_subtitle_codec(codec: &str) -> bool {
    matches!(
        codec,
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text"
    )
}

/// Image-based subtitle codecs that require server-side burn-in
/// (can't be rendered as a `<track>` sidecar). Listed
/// explicitly rather than "not text" so a novel codec name
/// doesn't silently get routed through the burn-in filter and
/// fail inside ffmpeg.
#[must_use]
pub fn is_image_subtitle_codec(codec: &str) -> bool {
    matches!(
        codec,
        "hdmv_pgs_subtitle" | "pgssub" | "dvd_subtitle" | "dvdsub" | "dvb_subtitle"
    )
}

fn build_subtitle_label(
    language: Option<&str>,
    title: Option<&str>,
    is_forced: bool,
    is_hearing_impaired: bool,
    is_commentary: bool,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = language.filter(|s| !s.is_empty()) {
        parts.push(l.to_owned());
    } else if let Some(t) = title.filter(|t| !t.is_empty()) {
        // Fall back to the raw title when ffprobe didn't surface
        // a language — e.g., a user-dropped `.srt` with no tag.
        parts.push(t.to_owned());
    }
    let mut flags: Vec<&'static str> = Vec::new();
    if is_forced {
        flags.push("forced");
    }
    if is_hearing_impaired {
        flags.push("SDH");
    }
    if is_commentary {
        flags.push("commentary");
    }
    if !flags.is_empty() {
        let joined = flags.join(", ");
        if let Some(first) = parts.first_mut() {
            *first = format!("{first} ({joined})");
        } else {
            parts.push(format!("Subtitle ({joined})"));
        }
    }
    if parts.is_empty() {
        "Subtitle".into()
    } else {
        parts.join(" · ")
    }
}

// ─── Video (stub for HDR branch) ─────────────────────────────────

/// Narrowed video-stream shape for the HDR / profile / level
/// branch. Populated for `PlayPrepareReply`'s future extension but
/// not yet consumed by the decision engine — that wiring lands
/// with the HDR branch commit.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VideoTrackInfo {
    pub stream_index: i64,
    pub codec: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub framerate: Option<f64>,
    pub pixel_format: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub hdr_format: Option<String>,
    /// Per-stream bitrate in bits/second. Often `None` for MKV
    /// stream-copy — the container tracks bitrate at the file
    /// level, not per stream.
    pub bitrate: Option<i64>,
    /// Bits per sample if ffprobe surfaced it (HEVC 10-bit /
    /// AV1 10-bit etc.). Different axis from `pixel_format` —
    /// some callers prefer the numeric field for formatting.
    pub bit_depth: Option<i64>,
}

impl VideoTrackInfo {
    fn from_row(row: &Stream) -> Self {
        Self {
            stream_index: row.stream_index,
            codec: row.codec.clone().unwrap_or_default().to_ascii_lowercase(),
            width: row.width,
            height: row.height,
            framerate: row.framerate,
            pixel_format: row.pixel_format.clone(),
            color_space: row.color_space.clone(),
            color_transfer: row.color_transfer.clone(),
            color_primaries: row.color_primaries.clone(),
            hdr_format: row.hdr_format.clone(),
            bitrate: row.bitrate,
            bit_depth: row.bit_depth,
        }
    }
}

// ─── Loaders ────────────────────────────────────────────────────

/// The three typed track lists pulled from `stream` in a single
/// pass. Callers that only need one of the three pay the same DB
/// round-trip — in practice the `/prepare` handler reads all
/// three together.
#[derive(Debug, Clone, Default)]
pub struct LoadedStreams {
    pub video: Option<VideoTrackInfo>,
    pub audio: Vec<AudioTrack>,
    pub subtitles: Vec<SubtitleTrack>,
}

/// Load all streams for a media row and partition by type.
///
/// * Audio + subtitle lists keep ffprobe's natural ordering
///   (`stream_index` ascending), so the "default" track is
///   typically first. Callers that want explicit default-first
///   ordering should sort by `is_default`.
/// * `kind` + `entity_id` are the play-route identity used to
///   build each `SubtitleTrack.vtt_url`. Must match the route the
///   client will use — `/api/v1/play/movie/42/…` for movies,
///   `/api/v1/play/episode/17/…` for episodes.
pub async fn load_streams(
    pool: &SqlitePool,
    media_id: i64,
    kind: &str,
    entity_id: i64,
) -> Result<LoadedStreams, sqlx::Error> {
    let rows = sqlx::query_as::<_, Stream>(
        "SELECT * FROM stream WHERE media_id = ? ORDER BY stream_index",
    )
    .bind(media_id)
    .fetch_all(pool)
    .await?;

    let mut out = LoadedStreams::default();
    for row in &rows {
        match row.stream_type.as_str() {
            "video" => {
                // Keep only the first video stream — additional
                // video streams in a media file are rare and
                // usually thumbnails ffprobe lists as streams.
                if out.video.is_none() {
                    out.video = Some(VideoTrackInfo::from_row(row));
                }
            }
            "audio" => out.audio.push(AudioTrack::from_row(row)),
            "subtitle" => out
                .subtitles
                .push(SubtitleTrack::from_row(row, kind, entity_id)),
            _ => {} // ffprobe sometimes reports "attachment" / "data" — ignore
        }
    }
    Ok(out)
}

/// Build a `LoadedStreams` directly from an ffprobe result —
/// the streaming-source counterpart to `load_streams`. Used by
/// the streaming `/prepare` arm so it can surface the same
/// typed track lists the library arm surfaces from the DB,
/// without needing DB rows (which only exist post-import).
///
/// Internally this converts each `ProbeStream` into the same
/// `crate::playback::stream_model::Stream` row shape the DB path uses,
/// then feeds it through the existing `from_row` constructors.
/// Everything downstream (`AudioTrack::label`, Atmos detection,
/// subtitle VTT URL, HDR detection) behaves identically across
/// the two paths.
#[must_use]
pub fn load_streams_from_probe(
    probe: &crate::import::ffprobe::ProbeResult,
    kind: &str,
    entity_id: i64,
) -> LoadedStreams {
    let mut out = LoadedStreams::default();
    let Some(streams) = probe.streams.as_ref() else {
        return out;
    };
    for (i, s) in streams.iter().enumerate() {
        let Some(stream_type) = s.codec_type.as_deref() else {
            continue;
        };
        let row = probe_to_stream_row(s, stream_type, i64::try_from(i).unwrap_or(i64::MAX));
        match stream_type {
            "video" => {
                if out.video.is_none() {
                    out.video = Some(VideoTrackInfo::from_row(&row));
                }
            }
            "audio" => out.audio.push(AudioTrack::from_row(&row)),
            "subtitle" => out
                .subtitles
                .push(SubtitleTrack::from_row(&row, kind, entity_id)),
            _ => {}
        }
    }
    out
}

/// Synthesise a `models::stream::Stream` row from an
/// `import::ffprobe::ProbeStream` so the existing `from_row`
/// constructors can be reused for the probe-driven streaming
/// path. Mirrors the INSERT in `import::pipeline::create_stream_entity`
/// — the same fields, same derivations (HDR detection via
/// `ffprobe::detect_hdr`, Atmos via `detect_atmos`, framerate
/// via the numerator/denominator parser).
fn probe_to_stream_row(
    s: &crate::import::ffprobe::ProbeStream,
    stream_type: &str,
    fallback_index: i64,
) -> Stream {
    use crate::import::ffprobe;
    let disposition = s.disposition.as_ref();
    let is_default = disposition.and_then(|d| d.default).unwrap_or(0) != 0;
    let is_forced = disposition.and_then(|d| d.forced).unwrap_or(0) != 0;
    let is_hi = disposition.and_then(|d| d.hearing_impaired).unwrap_or(0) != 0;
    let bitrate = s.bit_rate.as_deref().and_then(|b| b.parse::<i64>().ok());
    let sample_rate = s.sample_rate.as_deref().and_then(|s| s.parse::<i64>().ok());
    let bit_depth = s
        .bits_per_raw_sample
        .as_deref()
        .and_then(|b| b.parse::<i64>().ok());
    let framerate = s.r_frame_rate.as_deref().and_then(parse_framerate);
    let hdr = (stream_type == "video").then(|| ffprobe::detect_hdr(s).to_owned());
    let is_atmos = stream_type == "audio" && ffprobe::detect_atmos(s);
    Stream {
        id: 0,
        media_id: 0,
        stream_index: if s.index > 0 { s.index } else { fallback_index },
        stream_type: stream_type.to_owned(),
        codec: s.codec_name.clone(),
        language: s.tags.as_ref().and_then(|t| t.language.clone()),
        title: s.tags.as_ref().and_then(|t| t.title.clone()),
        is_external: false,
        is_default,
        is_forced,
        is_hearing_impaired: is_hi,
        path: None,
        bitrate,
        width: s.width,
        height: s.height,
        framerate,
        pixel_format: s.pix_fmt.clone(),
        color_space: s.color_space.clone(),
        color_transfer: s.color_transfer.clone(),
        color_primaries: s.color_primaries.clone(),
        hdr_format: hdr,
        channels: s.channels,
        channel_layout: s.channel_layout.clone(),
        sample_rate,
        bit_depth,
        is_atmos,
        profile: s.profile.clone(),
    }
}

/// "30000/1001" → 29.97; "24/1" → 24.0; anything we can't parse
/// falls through to `None`. Duplicated from `import::pipeline`
/// because both modules use it and the shared home is
/// `services` — but neither module imports from there.
fn parse_framerate(rate: &str) -> Option<f64> {
    let parts: Vec<&str> = rate.split('/').collect();
    if parts.len() == 2 {
        let num: f64 = parts[0].parse().ok()?;
        let den: f64 = parts[1].parse().ok()?;
        if den > 0.0 {
            return Some(num / den);
        }
    }
    rate.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_row(stream_type: &str) -> Stream {
        Stream {
            id: 1,
            media_id: 1,
            stream_index: 0,
            stream_type: stream_type.into(),
            codec: None,
            language: None,
            title: None,
            is_external: false,
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
            path: None,
            bitrate: None,
            width: None,
            height: None,
            framerate: None,
            pixel_format: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
            hdr_format: None,
            channels: None,
            channel_layout: None,
            sample_rate: None,
            bit_depth: None,
            is_atmos: false,
            profile: None,
        }
    }

    // ── Audio label composition ────────────────────────────────

    #[test]
    fn audio_label_full() {
        let mut row = mk_row("audio");
        row.language = Some("eng".into());
        row.channel_layout = Some("5.1".into());
        row.codec = Some("ac3".into());
        let t = AudioTrack::from_row(&row);
        assert_eq!(t.label, "eng · 5.1 · AC3");
    }

    #[test]
    fn audio_label_falls_back_to_channel_count() {
        let mut row = mk_row("audio");
        row.language = Some("eng".into());
        row.channels = Some(6);
        row.codec = Some("ac3".into());
        let t = AudioTrack::from_row(&row);
        assert_eq!(t.label, "eng · 6 ch · AC3");
    }

    #[test]
    fn audio_label_minimal() {
        // Codec-only row — we still render something usable.
        let mut row = mk_row("audio");
        row.codec = Some("aac".into());
        let t = AudioTrack::from_row(&row);
        assert_eq!(t.label, "AAC");
    }

    #[test]
    fn audio_label_empty_falls_back() {
        let row = mk_row("audio");
        let t = AudioTrack::from_row(&row);
        assert_eq!(t.label, "Audio track");
    }

    #[test]
    fn audio_atmos_label_appended_after_codec() {
        // Atmos renders alongside the codec (as a separate
        // segment) rather than replacing it — "5.1 · EAC-3"
        // is the decoder-visible shape, "Atmos" is a property
        // on top.
        let mut row = mk_row("audio");
        row.language = Some("eng".into());
        row.channel_layout = Some("5.1".into());
        row.codec = Some("eac3".into());
        row.is_atmos = true;
        let t = AudioTrack::from_row(&row);
        assert_eq!(t.label, "eng · 5.1 · EAC3 · Atmos");
        assert!(t.is_atmos);
    }

    #[test]
    fn audio_atmos_flag_survives_row_round_trip() {
        let mut row = mk_row("audio");
        row.codec = Some("truehd".into());
        row.is_atmos = true;
        let t = AudioTrack::from_row(&row);
        assert!(t.is_atmos);
    }

    #[test]
    fn audio_commentary_detection() {
        let mut row = mk_row("audio");
        row.codec = Some("aac".into());
        row.title = Some("Director Commentary".into());
        let t = AudioTrack::from_row(&row);
        assert!(t.is_commentary, "commentary title should flag");

        row.title = Some("Main Dialogue".into());
        let t = AudioTrack::from_row(&row);
        assert!(!t.is_commentary);
    }

    #[test]
    fn audio_codec_normalised_lowercase() {
        let mut row = mk_row("audio");
        row.codec = Some("AC3".into()); // mixed case from somewhere upstream
        let t = AudioTrack::from_row(&row);
        assert_eq!(t.codec, "ac3", "codec field is lowercase");
        assert!(t.label.contains("AC3"), "label uses uppercase for display");
    }

    // ── Subtitle VTT URL gating ────────────────────────────────

    #[test]
    fn subtitle_text_gets_vtt_url() {
        let mut row = mk_row("subtitle");
        row.codec = Some("subrip".into());
        row.stream_index = 3;
        let t = SubtitleTrack::from_row(&row, "movie", 42);
        assert_eq!(
            t.vtt_url.as_deref(),
            Some("/api/v1/play/movie/42/subtitles/3")
        );
    }

    #[test]
    fn subtitle_pgs_has_no_vtt_url() {
        // PGS / image subs must burn in — no sidecar URL.
        let mut row = mk_row("subtitle");
        row.codec = Some("hdmv_pgs_subtitle".into());
        let t = SubtitleTrack::from_row(&row, "movie", 42);
        assert!(t.vtt_url.is_none());
    }

    #[test]
    fn subtitle_vobsub_has_no_vtt_url() {
        let mut row = mk_row("subtitle");
        row.codec = Some("dvd_subtitle".into());
        let t = SubtitleTrack::from_row(&row, "episode", 7);
        assert!(t.vtt_url.is_none());
    }

    #[test]
    fn subtitle_label_flags() {
        let mut row = mk_row("subtitle");
        row.language = Some("eng".into());
        row.is_forced = true;
        row.codec = Some("subrip".into());
        let t = SubtitleTrack::from_row(&row, "movie", 1);
        assert_eq!(t.label, "eng (forced)");

        row.is_hearing_impaired = true;
        let t = SubtitleTrack::from_row(&row, "movie", 1);
        assert_eq!(t.label, "eng (forced, SDH)");
    }

    #[test]
    fn subtitle_label_falls_back_to_title_when_no_language() {
        let mut row = mk_row("subtitle");
        row.language = None;
        row.title = Some("Signs & Songs".into());
        row.codec = Some("ass".into());
        let t = SubtitleTrack::from_row(&row, "movie", 1);
        assert_eq!(t.label, "Signs & Songs");
    }

    #[test]
    fn subtitle_label_fallback_when_everything_empty() {
        let mut row = mk_row("subtitle");
        row.codec = Some("subrip".into());
        let t = SubtitleTrack::from_row(&row, "movie", 1);
        assert_eq!(t.label, "Subtitle");
    }
}
