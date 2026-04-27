//! Playback decision engine.
//!
//! Given `(source, client, options)`, produce a `PlaybackPlan`
//! describing what the server should do — direct play, remux, or
//! full transcode — plus the set of `TranscodeReason`s that led to
//! anything less than direct play.
//!
//! This module is the heart of the playback subsystem. It is a
//! pure function: no I/O, no `AppState`, no ffmpeg. That's
//! deliberate. The decision engine must be unit-testable in
//! isolation with hundreds of `(source, client)` fixtures, and
//! that's only cheap if nothing it touches talks to the DB or the
//! filesystem.
//!
//! # Extension points
//!
//! Today the engine decides direct-play vs. transcode on
//! container + video codec + audio codec. Follow-up commits layer
//! in remux (`PlaybackMethod::Remux` when only the container
//! mismatches, emitting `-c:v copy -c:a copy` via the profile
//! chain), HDR / DV / 10-bit / profile / level awareness (extending
//! `SourceInfo` + `ClientCapabilities` with the relevant fields and
//! adding the matching `TranscodeReason` arms), and multi-audio
//! compat (`SourceInfo::audio_tracks` so a `TrueHD` + AC-3 MKV
//! picks AC-3 without transcoding).
//!
//! Each extension adds fields; the public function signature is
//! stable.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{TranscodeReason, TranscodeReasons};

// ─── Input ────────────────────────────────────────────────────────

/// What we know about the source from ffprobe + import metadata.
/// All codec strings are lowercase (ffprobe's form).
///
/// Empty `audio_tracks` is valid for clips with no audio — the
/// engine treats "no audio stream" as automatically compatible
/// rather than forcing a transcode for a silent test file.
#[derive(Debug, Clone, Default)]
pub struct SourceInfo {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    /// All audio tracks on the source, in ffprobe's `stream_index`
    /// order (first entry is conventionally the default / primary).
    /// The engine scans this list for a client-compatible track
    /// before flagging `AudioCodecNotSupported`, so a dual-track
    /// source like `[TrueHD, AC-3]` on a client that can't decode
    /// `TrueHD` picks the AC-3 track and stays on the non-transcode
    /// path instead of re-encoding needlessly.
    pub audio_tracks: Vec<AudioCandidate>,
    /// ffprobe color-transfer string (`bt709` = SDR, `smpte2084`
    /// = HDR10 PQ, `arib-std-b67` = HLG). Feeds the HDR branch:
    /// any non-SDR transfer against an SDR-only client triggers
    /// `VideoRangeTypeNotSupported`.
    pub color_transfer: Option<String>,
    /// ffprobe pixel format (`yuv420p` = 8-bit, `yuv420p10le` =
    /// 10-bit, etc.). 10-bit H.264 sources flag
    /// `VideoBitDepthNotSupported` against clients that only
    /// decode 8-bit (Safari is the notable offender).
    pub pix_fmt: Option<String>,
    /// Import-layer summary of the HDR format (e.g.,
    /// `"Dolby Vision Profile 5"`, `"HDR10"`, `"HLG"`). Used as a
    /// fallback signal when `color_transfer` alone doesn't
    /// distinguish DV profile 5 (same `smpte2084` transfer as
    /// HDR10 but IPT-PQ-C2 — no HDR10 fallback possible).
    pub hdr_format: Option<String>,
}

/// Minimal per-audio-track shape the decision engine needs. Narrow
/// subset of `playback::stream::AudioTrack` — omits display label,
/// `is_default`, `is_commentary` because those are UI concerns that
/// don't affect compatibility. Stays small + easy to construct in
/// tests without dragging the full track type through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioCandidate {
    /// ffprobe stream index. Fed to ffmpeg as `-map 0:a:N` when
    /// the engine selects this track.
    pub stream_index: i64,
    /// Lowercase codec string (`aac`, `ac3`, `eac3`, `truehd`, …).
    pub codec: String,
    /// Channel count if known. Not yet used by the engine, but
    /// carried for the future "prefer 5.1 over stereo when
    /// passthrough is an option" policy that lands with the audio
    /// passthrough profiles work.
    pub channels: Option<i64>,
    /// Human layout string ("stereo", "5.1", "7.1", "5.1(side)").
    /// Read by the API layer after planning to drive the BS.775
    /// downmix filter when the track is re-encoded. Decision
    /// engine itself doesn't consult it — codec compatibility is
    /// codec-only.
    pub channel_layout: Option<String>,
    /// Raw ffprobe `profile` string — `"DTS-HD MA"`, `"LC"`,
    /// `"Main 10"`, etc. Decision engine doesn't consult it;
    /// the API layer reads it after planning so the HLS
    /// `CODECS` emitter can promote DTS-HD MA (`dtsh`) over
    /// DTS Core (`dtsc`).
    pub profile: Option<String>,
}

/// Browser family detected from the `User-Agent` header. Drives
/// `ClientCapabilities::from_user_agent` — paired with `ClientOs`
/// to pick the right codec matrix. Smart-TV / Cast variants live
/// in the same enum so the detection code can return a single
/// typed value regardless of whether the client is a desktop
/// browser or a TV browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFamily {
    Firefox,
    Chromium,
    Edge,
    Safari,
    FireTv,
    Chromecast,
    AppleTv,
    LgWebos,
    SamsungTizen,
    Unknown,
}

/// Operating system detected from the `User-Agent` header. Only
/// meaningful for the desktop browser families — TV / Cast
/// variants encode their "OS" in `BrowserFamily` already.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientOs {
    Windows,
    MacOs,
    Linux,
    Ios,
    Android,
    Other,
}

/// What we concluded about the client — driven by the UA header.
/// Surfaced on `PlayPrepareReply` so the info chip can show
/// "Detected: Firefox on Linux · firefox profile" and reviewers
/// can audit the decision without reading logs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DetectedClient {
    pub family: BrowserFamily,
    pub os: ClientOs,
    /// Preset name the decision engine picked (`"firefox"`,
    /// `"chromium_windows"`, `"safari_macos"`, ...).
    pub preset: String,
    /// Truncated UA for display — full UAs are long and
    /// fingerprinty, so we show enough to verify detection
    /// without dumping the whole header into the UI.
    pub ua_display: Option<String>,
}

impl DetectedClient {
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            family: BrowserFamily::Unknown,
            os: ClientOs::Other,
            preset: "browser_defaults".into(),
            ua_display: None,
        }
    }

    /// Parse a `User-Agent` string into a typed family + OS. The
    /// order of checks is deliberate — `Edg/` must match before
    /// `Chrome/` because Edge's UA contains both; TV / Cast
    /// tokens must match before the generic browser ones because
    /// e.g. a Tizen TV's UA also contains `AppleWebKit`.
    #[must_use]
    pub fn from_ua(ua: &str) -> Self {
        let lc = ua.to_ascii_lowercase();
        // TV / Cast detection — match first, they carry browser
        // tokens too.
        let family = if lc.contains("crkey") || lc.contains("chromecast") {
            BrowserFamily::Chromecast
        } else if lc.contains("appletv") {
            BrowserFamily::AppleTv
        } else if lc.contains("web0s") || lc.contains("webos") {
            BrowserFamily::LgWebos
        } else if lc.contains("tizen") || lc.contains("smart-tv") || lc.contains("smarttv") {
            BrowserFamily::SamsungTizen
        } else if lc.contains("silk/") || lc.contains("aftm") || lc.contains("afts") {
            BrowserFamily::FireTv
        } else if lc.contains("firefox/") || lc.contains("fxios/") {
            // FxiOS is Firefox-branded Safari WebKit under iOS
            // WebKit-mandate; the codec matrix is Safari iOS's,
            // not Firefox's. Handled below by the OS branch.
            BrowserFamily::Firefox
        } else if lc.contains("edg/") || lc.contains("edge/") {
            BrowserFamily::Edge
        } else if lc.contains("chrome/") || lc.contains("chromium/") || lc.contains("crios/") {
            // CriOS = Chrome for iOS, also WebKit under the hood
            // (see FxiOS note above).
            BrowserFamily::Chromium
        } else if lc.contains("safari/") {
            // Real Safari: has "Safari/" but NOT "Chrome/" / "Edg/"
            // / "Firefox/", all gated above.
            BrowserFamily::Safari
        } else {
            BrowserFamily::Unknown
        };

        // OS detection. iPhone / iPad override generic "like Mac
        // OS X" since every iOS UA mimics macOS for compat.
        let os = if lc.contains("iphone") || lc.contains("ipad") || lc.contains("ipod") {
            ClientOs::Ios
        } else if lc.contains("android") {
            ClientOs::Android
        } else if lc.contains("windows") {
            ClientOs::Windows
        } else if lc.contains("mac os x") || lc.contains("macintosh") {
            ClientOs::MacOs
        } else if lc.contains("linux") || lc.contains("x11") {
            ClientOs::Linux
        } else {
            ClientOs::Other
        };

        // iOS Firefox / iOS Chrome are WebKit under the hood —
        // promote to Safari so we pick the Safari codec matrix.
        let family = if os == ClientOs::Ios
            && matches!(family, BrowserFamily::Firefox | BrowserFamily::Chromium)
        {
            BrowserFamily::Safari
        } else {
            family
        };

        let preset = match (family, os) {
            (BrowserFamily::Firefox, _) => "firefox",
            (BrowserFamily::Edge, _) => "edge_windows",
            (BrowserFamily::Chromium, ClientOs::Windows) => "chromium_windows",
            (BrowserFamily::Chromium, ClientOs::MacOs) => "chromium_macos",
            (BrowserFamily::Chromium, _) => "chromium_linux",
            (BrowserFamily::Safari, ClientOs::Ios) => "safari_ios",
            (BrowserFamily::Safari, _) => "safari_macos",
            (BrowserFamily::FireTv, _) => "fire_tv",
            (BrowserFamily::Chromecast, _) => "chromecast_gtv",
            (BrowserFamily::AppleTv, _) => "apple_tv_4k",
            (BrowserFamily::LgWebos, _) => "lg_webos",
            (BrowserFamily::SamsungTizen, _) => "samsung_tizen",
            (BrowserFamily::Unknown, _) => "browser_defaults",
        };

        // Trim the UA for display — most are ~200 chars, most
        // of which is vendor boilerplate.
        let ua_display = Some(ua.chars().take(96).collect::<String>());

        Self {
            family,
            os,
            preset: preset.to_owned(),
            ua_display,
        }
    }
}

/// What the client can play without help. Presets (`firefox()`,
/// `chromium_macos()`, `safari_macos()`, `apple_tv_4k()`, ...) are
/// below; pick the right one by hand or call
/// `from_user_agent(ua)` for UA-driven selection.
#[derive(Debug, Clone)]
pub struct ClientCapabilities {
    /// Container extensions the client accepts (`mp4`, `mkv`, ...).
    pub containers: Vec<&'static str>,
    /// Video codecs the client can decode.
    pub video_codecs: Vec<&'static str>,
    /// Audio codecs the client can decode.
    pub audio_codecs: Vec<&'static str>,
    /// True when the client can render HDR10 / HLG natively.
    /// Browsers in `<video>` pipelines overwhelmingly cannot —
    /// they need tone-map to SDR even on HDR-capable displays.
    /// Future Cast / Apple TV presets will flip this.
    pub hdr_support: bool,
    /// True when the client can decode Dolby Vision natively
    /// (implies `hdr_support`). Apple TV 4K, most LG OLEDs,
    /// some Chromecast devices. When `false` against a DV source
    /// with an HDR10 fallback layer, the engine sets a bitstream
    /// filter that strips the RPU so the output is pure HDR10 —
    /// capable HDR clients still get full HDR, DV-only metadata
    /// is silently dropped.
    pub dv_support: bool,
    /// True when the client uses HDR10+ dynamic metadata.
    /// Samsung TVs, some Panasonic, Chromecast with Google TV.
    /// When `false`, HDR10+ sources get their dynamic metadata
    /// stripped so the output is plain HDR10 (static metadata
    /// baseline) — the `hdr_support`-capable client sees the
    /// same range without the tone-curve-per-scene benefit.
    pub hdr10_plus_support: bool,
    /// True when the client's video decoder handles 10-bit
    /// pixel formats (HEVC Main10, 10-bit H.264). Safari /
    /// `VideoToolbox` can't; most Chromium builds can for HEVC but
    /// not H.264. Drives the `VideoBitDepthNotSupported` branch.
    pub ten_bit_support: bool,
}

impl ClientCapabilities {
    /// Safe conservative floor for any unknown browser — the
    /// intersection of what every modern browser plays without
    /// asking. Use `from_user_agent(ua)` to pick a more specific
    /// profile when the UA identifies a known family/OS pair.
    ///
    /// Deliberately excludes codecs with any cross-browser
    /// ambiguity:
    ///
    /// * **MKV** — no browser natively direct-plays MKV in
    ///   `<video>`. A Remux-to-fMP4 session is essentially free
    ///   (stream-copy into HLS fMP4) and actually plays.
    /// * **HEVC / H.265** — Firefox has no decoder on any OS;
    ///   Chrome / Edge rely on OS codec plugins that vary.
    /// * **AC-3 / EAC-3** — Firefox has no decoder; Chrome is
    ///   platform-dependent. Common in scene releases; the class
    ///   of "silent video" bugs lives here.
    /// * **DTS** — essentially never works in-browser.
    #[must_use]
    pub fn browser_defaults() -> Self {
        Self {
            // No MKV — see module doc on the ClientCapabilities
            // struct. Every browser profile below omits MKV; the
            // Remux path handles it cheaply.
            containers: vec!["mp4", "m4v", "webm"],
            video_codecs: vec!["h264", "vp9", "vp8"],
            audio_codecs: vec!["aac", "mp3", "opus", "flac", "vorbis"],
            hdr_support: false,
            dv_support: false,
            hdr10_plus_support: false,
            ten_bit_support: false,
        }
    }

    /// Firefox on any OS. Policy-constrained — Mozilla doesn't
    /// license HEVC, AC-3, EAC-3, DTS, so the codec matrix is
    /// the smallest of the desktop browsers. AV1 has been on by
    /// default since Firefox 100 (2022).
    #[must_use]
    pub fn firefox() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "webm"],
            video_codecs: vec!["h264", "vp9", "vp8", "av1"],
            audio_codecs: vec!["aac", "mp3", "opus", "flac", "vorbis"],
            hdr_support: false,
            dv_support: false,
            hdr10_plus_support: false,
            ten_bit_support: false,
        }
    }

    /// Chrome / Chromium / Edge on Linux. HEVC support requires
    /// system libavcodec with H.265 which can't be assumed; we
    /// conservatively omit it. AV1 + VP9 are bundled.
    /// AC-3/EAC-3 depend on the ffmpeg library the distro ships
    /// with — also conservatively omitted.
    #[must_use]
    pub fn chromium_linux() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "webm"],
            video_codecs: vec!["h264", "vp9", "vp8", "av1"],
            audio_codecs: vec!["aac", "mp3", "opus", "flac", "vorbis"],
            hdr_support: false,
            dv_support: false,
            hdr10_plus_support: false,
            ten_bit_support: false,
        }
    }

    /// Chrome / Chromium on Windows. Windows 11's built-in HEVC
    /// codec + the Media Foundation path give HEVC + AC-3/EAC-3
    /// playback. HDR renders via Windows HDR mode when the
    /// display is HDR-capable — but we can't know the display
    /// state from UA, so we leave `hdr_support` off.
    #[must_use]
    pub fn chromium_windows() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "webm"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "vp8", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac", "vorbis"],
            hdr_support: false,
            dv_support: false,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Chrome / Chromium on macOS. `VideoToolbox` gives HEVC
    /// (including 10-bit) + AC-3/EAC-3 at the OS layer.
    #[must_use]
    pub fn chromium_macos() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "webm"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "vp8", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac", "alac"],
            hdr_support: false,
            dv_support: false,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Microsoft Edge on Windows. Chromium under the hood; the
    /// HEVC story is identical to Chrome on Windows.
    #[must_use]
    pub fn edge_windows() -> Self {
        Self::chromium_windows()
    }

    /// Safari on macOS. Full Apple codec matrix — H.264/HEVC
    /// (up to 10-bit), VP9 (recent Safari), AV1 (Safari 17+,
    /// 2023). HDR10 + Dolby Vision render natively on capable
    /// displays. Notable omissions: no `WebM` video direct-play
    /// (audio only), no MKV.
    #[must_use]
    pub fn safari_macos() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "mov"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "flac", "alac"],
            // Safari is the outlier among browsers — it actually
            // does pipe HDR10/DV through the compositor on a
            // capable display. True here because even on an SDR
            // display Safari tone-maps in the compositor rather
            // than refusing the stream.
            hdr_support: true,
            dv_support: true,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Safari on iOS / iPadOS. Same `WebKit` engine as macOS
    /// Safari; iOS mandates `WebKit` even for "Chrome iOS" and
    /// "Firefox iOS" apps, so this preset covers those too.
    #[must_use]
    pub fn safari_ios() -> Self {
        Self::safari_macos()
    }

    /// Amazon Fire TV browser (Silk). Chromium-derived Android
    /// TV variant — HEVC, AC-3/EAC-3 via the TV stack, HDR10 on
    /// 4K Fire sticks. Used when the user is running our web UI
    /// on a Fire TV natively (rare); most Fire TV playback would
    /// go through a future Cast/Android-TV native app instead.
    #[must_use]
    pub fn fire_tv() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "webm", "ts"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac"],
            hdr_support: true,
            dv_support: false,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Pick the best-fit preset for an incoming `User-Agent`
    /// header. Returns `(caps, detected)` so the caller can log
    /// what was picked and surface it in the player info chip.
    ///
    /// Falls back to `browser_defaults()` for unrecognised UAs —
    /// the safe conservative floor. A user on a privacy browser
    /// with a randomised UA gets slightly-over-transcoded output,
    /// which is the correct failure mode.
    #[must_use]
    pub fn from_user_agent(ua: Option<&str>) -> (Self, DetectedClient) {
        let Some(ua) = ua.filter(|s| !s.is_empty()) else {
            return (Self::browser_defaults(), DetectedClient::unknown());
        };
        let detected = DetectedClient::from_ua(ua);
        let caps = match (detected.family, detected.os) {
            (BrowserFamily::Firefox, _) => Self::firefox(),
            (BrowserFamily::Edge, _) => Self::edge_windows(),
            (BrowserFamily::Chromium, ClientOs::Windows) => Self::chromium_windows(),
            (BrowserFamily::Chromium, ClientOs::MacOs) => Self::chromium_macos(),
            // `from_ua` promotes iOS Chromium/Firefox to Safari,
            // so (Chromium, Ios) shouldn't reach this arm via the
            // UA path. Handled here too for completeness — the
            // Safari matrix is the correct fallback given
            // WebKit-mandate.
            (BrowserFamily::Chromium, ClientOs::Ios) => Self::safari_ios(),
            (BrowserFamily::Chromium, _) => Self::chromium_linux(),
            (BrowserFamily::Safari, _) => Self::safari_macos(),
            (BrowserFamily::FireTv, _) => Self::fire_tv(),
            (BrowserFamily::Chromecast, _) => Self::chromecast_gtv(),
            (BrowserFamily::AppleTv, _) => Self::apple_tv_4k(),
            (BrowserFamily::LgWebos, _) => Self::lg_webos(),
            (BrowserFamily::SamsungTizen, _) => Self::samsung_tizen(),
            (BrowserFamily::Unknown, _) => Self::browser_defaults(),
        };
        (caps, detected)
    }

    /// Pick a preset by explicit name — used by the Cast
    /// target-override flow. The browser doing the controlling
    /// has its own UA, but the receiver (what actually plays)
    /// is a different device; the frontend passes its name
    /// through here.
    #[must_use]
    pub fn from_target_override(target: &str) -> Option<(Self, DetectedClient)> {
        let (caps, family) = match target {
            "chromecast_gtv" => (Self::chromecast_gtv(), BrowserFamily::Chromecast),
            "chromecast_ultra" => (Self::chromecast_ultra(), BrowserFamily::Chromecast),
            "apple_tv_4k" => (Self::apple_tv_4k(), BrowserFamily::AppleTv),
            "lg_webos" => (Self::lg_webos(), BrowserFamily::LgWebos),
            "samsung_tizen" => (Self::samsung_tizen(), BrowserFamily::SamsungTizen),
            "fire_tv" => (Self::fire_tv(), BrowserFamily::FireTv),
            _ => return None,
        };
        Some((
            caps,
            DetectedClient {
                family,
                os: ClientOs::Other,
                preset: target.to_owned(),
                ua_display: None,
            },
        ))
    }

    /// Chromecast with Google TV (4K HDR Chromecast released
    /// 2020). Full HDR10 / DV profile 5 + 8 / HLG support,
    /// HEVC Main10, AV1, AC-3 + EAC-3 passthrough including
    /// Atmos-in-EAC3. Source:
    /// <https://developers.google.com/cast/docs/media>.
    ///
    /// Deliberately enabled codecs the browser defaults omit:
    /// HEVC, AC-3, EAC-3 — these are where the big
    /// over-transcoding wins live for a Cast-with-Google-TV
    /// library.
    #[must_use]
    pub fn chromecast_gtv() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "mkv", "webm", "ts"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac", "vorbis"],
            hdr_support: true,
            dv_support: true,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Chromecast Ultra (pre-GTV, 2016-era). 4K HDR10 / DV,
    /// HEVC Main10, no AV1. AC-3 / EAC-3 passthrough. No
    /// HDR10+.
    #[must_use]
    pub fn chromecast_ultra() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "mkv", "webm", "ts"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac", "vorbis"],
            hdr_support: true,
            dv_support: true,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Apple TV 4K (all generations). HEVC Main10, HDR10 + DV +
    /// HLG, AV1 on the 3rd-gen. Includes DTS + DTS-HD MA — the
    /// tvOS media stack decodes DTS-in-fMP4 natively, so we can
    /// stream-copy the source audio track instead of re-encoding
    /// to AAC stereo. No other mainstream HLS client handles this
    /// reliably today.
    #[must_use]
    pub fn apple_tv_4k() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "mkv", "mov", "ts"],
            video_codecs: vec!["h264", "hevc", "h265", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "dts", "mp3", "alac", "flac"],
            hdr_support: true,
            dv_support: true,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// LG OLED webOS (2018+). HEVC Main10, HDR10 + DV + HLG,
    /// AV1 on 2022+ panels. Aligned with the Dolby
    /// ecosystem — HDR10+ is Samsung's fight, LG doesn't back
    /// it. AC-3 / EAC-3 passthrough works over TV browser
    /// playback.
    #[must_use]
    pub fn lg_webos() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "mkv", "webm", "ts"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac", "vorbis"],
            hdr_support: true,
            dv_support: true,
            hdr10_plus_support: false,
            ten_bit_support: true,
        }
    }

    /// Samsung Tizen (2018+). HEVC Main10, HDR10 +
    /// **HDR10+** (Samsung-backed format), HLG. No Dolby
    /// Vision — Samsung explicitly refuses to license it.
    /// AC-3 / EAC-3 passthrough.
    #[must_use]
    pub fn samsung_tizen() -> Self {
        Self {
            containers: vec!["mp4", "m4v", "mkv", "webm", "ts"],
            video_codecs: vec!["h264", "hevc", "h265", "vp9", "av1"],
            audio_codecs: vec!["aac", "ac3", "eac3", "mp3", "opus", "flac", "vorbis"],
            hdr_support: true,
            dv_support: false,
            hdr10_plus_support: true,
            ten_bit_support: true,
        }
    }
}

/// User-level knobs that can bias the decision. Empty today —
/// preferred audio language, subtitle stream, user bitrate cap
/// land here.
#[derive(Debug, Clone, Default)]
pub struct PlaybackOptions {}

// ─── Output ───────────────────────────────────────────────────────

/// How the server will serve this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMethod {
    /// Serve the source bytes as-is over the `/direct` endpoint
    /// with `Range` support. No ffmpeg, no session.
    DirectPlay,
    /// Spawn an ffmpeg HLS session but stream-copy both video
    /// and audio (`-c:v copy -c:a copy`) into fMP4. Near-zero
    /// CPU. Picked when the video + audio codecs are
    /// direct-playable but the container isn't — the canonical
    /// "HEVC in MKV → Safari" case.
    Remux,
    /// Spawn an ffmpeg HLS session with full video / audio
    /// re-encode. Filter chain (HW backend, tone-mapping,
    /// subtitle burn-in) comes from the profile chain at
    /// session creation time.
    Transcode,
}

/// The engine's verdict on a request. `transcode_reasons` is
/// `empty()` iff `method == DirectPlay`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlaybackPlan {
    pub method: PlaybackMethod,
    pub transcode_reasons: TranscodeReasons,
    /// Stream index of the audio track the engine picked as the
    /// best compatibility match for this client. `None` when the
    /// source has no audio at all, or when no track is
    /// client-compatible (in which case a re-encode of the
    /// primary track is the fallback). API callers treat this as
    /// the default — an explicit `?audio_stream=N` user pick
    /// still wins.
    #[serde(default)]
    pub selected_audio_stream: Option<i64>,
    /// Optional ffmpeg bitstream-filter spec applied to the
    /// video stream on the Remux path. Strips dynamic HDR
    /// metadata the client can't use so the output is a
    /// cleaner baseline: DV profile 8.x without the RPU →
    /// pure HDR10, HDR10+ → plain HDR10 with static metadata.
    /// Near-zero CPU (bitstream-level, no decode / re-encode).
    /// `None` when stripping isn't needed (SDR source, or
    /// client is DV / HDR10+ native). Only meaningful when
    /// `method == Remux`; Transcode produces fresh output
    /// without dynamic metadata anyway.
    #[serde(default)]
    pub video_bitstream_filter: Option<String>,
    /// When `true`, stream-copy the selected audio track
    /// (`-c:a copy`) instead of re-encoding to stereo AAC.
    /// Preserves 5.1 / 7.1 channels + Atmos-in-EAC-3 for
    /// clients that can decode the source codec in fMP4 HLS
    /// (Apple TV / Chromecast with Google TV / smart-TV
    /// browsers). Only meaningful when
    /// `method == Transcode` — `Remux` already copies audio
    /// via `-c:a copy` unconditionally, `DirectPlay` bypasses
    /// ffmpeg entirely.
    #[serde(default)]
    pub audio_passthrough: bool,
}

// ─── Engine ───────────────────────────────────────────────────────

/// Plan a playback request.
///
/// Walks container → video codec → audio codec, collecting one
/// `TranscodeReason` per mismatch. If every check passes the
/// result is `PlaybackMethod::DirectPlay`; otherwise `Transcode`.
/// The set of reasons is stable for a given `(source, client)`
/// pair — callers can persist it on a session and round-trip it
/// via the `?tr=` query parameter.
#[must_use]
pub fn plan_playback(
    source: &SourceInfo,
    client: &ClientCapabilities,
    _options: &PlaybackOptions,
) -> PlaybackPlan {
    let mut reasons = TranscodeReasons::new();

    // Container. Missing == unplayable; we can't even open it.
    match source.container.as_deref() {
        Some(c) if has_lc(&client.containers, c) => {}
        _ => reasons.add(TranscodeReason::ContainerNotSupported),
    }

    // Video codec. Missing video is treated as incompatible — a
    // pure-audio file has no business going through this path, so
    // we flag it rather than silently direct-playing garbage.
    match source.video_codec.as_deref() {
        Some(c) if has_lc(&client.video_codecs, c) => {}
        _ => reasons.add(TranscodeReason::VideoCodecNotSupported),
    }

    // HDR: any non-SDR transfer function against an SDR-only
    // client forces tone-map. We consult both `color_transfer`
    // (authoritative when ffprobe surfaced it) and the fallback
    // `hdr_format` string (Dolby Vision + HDR10+ detection).
    if is_hdr_source(source) && !client.hdr_support {
        reasons.add(TranscodeReason::VideoRangeTypeNotSupported);
    }

    // 10-bit pixel formats: Safari rejects 10-bit H.264
    // outright; most browsers have patchy support. Conservative
    // branch forces an 8-bit re-encode.
    if is_ten_bit(source.pix_fmt.as_deref()) && !client.ten_bit_support {
        reasons.add(TranscodeReason::VideoBitDepthNotSupported);
    }

    // Audio compat. Empty track list is legitimate (silent clip)
    // and compatible by default — no reason, no selection.
    //
    // Otherwise scan the full list: if any track is
    // client-compatible, pick the first one found in ffprobe
    // order (conventionally the default track). Only flag
    // `AudioCodecNotSupported` when *every* track is
    // incompatible — that's when a re-encode is actually forced.
    //
    // This is the whole point: a UHD rip with [TrueHD, AC-3]
    // against a client that can play AC-3 should pick track 1
    // and stay on Remux/DirectPlay instead of paying for a
    // transcode of the TrueHD primary.
    let selected_audio_stream = if source.audio_tracks.is_empty() {
        None
    } else if let Some(compat) = source
        .audio_tracks
        .iter()
        .find(|t| has_lc(&client.audio_codecs, &t.codec))
    {
        Some(compat.stream_index)
    } else {
        reasons.add(TranscodeReason::AudioCodecNotSupported);
        None
    };

    let method = pick_method(&reasons);

    // Dynamic HDR metadata stripping. Only meaningful on the
    // Remux path (Transcode produces fresh output, DirectPlay
    // has no ffmpeg to apply the filter). A DV profile 8.x
    // source streamed to an HDR-capable-but-not-DV-native
    // client gets its RPU stripped via `hevc_metadata=remove_dovi=1`
    // — the HDR10 base layer survives and plays natively.
    // Likewise HDR10+ → HDR10 via `remove_hdr10plus=1` for
    // clients without HDR10+ dynamic metadata support.
    //
    // Profile 5 DV is deliberately skipped — its colour data is
    // IPT-PQ-C2, not BT.2020, so stripping the RPU leaves bytes
    // that would render as purple/green on an HDR10 display.
    // That case routes through the transcode / tonemap path
    // (which runs the generic HDR branch above).
    let video_bitstream_filter = if matches!(method, PlaybackMethod::Remux) {
        detect_hdr_metadata_strip(source, client)
    } else {
        None
    };

    // Audio passthrough: when we're re-encoding video (Transcode
    // method) but the selected audio track is client-compatible
    // AND safe to carry in fMP4 HLS, use `-c:a copy` instead of
    // re-encoding to stereo AAC. Preserves 5.1 / 7.1 channels +
    // Atmos for clients that advertise the codec. Remux already
    // copies both streams unconditionally; DirectPlay bypasses
    // ffmpeg so this flag doesn't apply.
    let audio_passthrough = matches!(method, PlaybackMethod::Transcode)
        && selected_audio_stream.is_some_and(|idx| {
            source
                .audio_tracks
                .iter()
                .find(|t| t.stream_index == idx)
                .is_some_and(|t| is_hls_fmp4_passthrough_safe(&t.codec))
        });

    PlaybackPlan {
        method,
        transcode_reasons: reasons,
        selected_audio_stream,
        video_bitstream_filter,
        audio_passthrough,
    }
}

/// Codecs safe to stream-copy into an fMP4 HLS segment. The
/// intersection of "client advertises support" and "the fMP4
/// HLS muxer + common clients handle the codec in an MP4
/// wrapper."
///
/// Included:
/// * `aac` — lingua franca, universally safe.
/// * `ac3` → emitted as `ac-3` in CODECS. Supported by Apple
///   platforms, Chromecast with Google TV, modern smart TVs.
/// * `eac3` → emitted as `ec-3`. Same support matrix as AC-3,
///   plus the Atmos-in-EAC3 payload when present.
///
/// Deliberately excluded:
/// * `dts` / `dts-hd ma` — works on Apple TV over HLS but
///   broken on most browsers; the DTS passthrough profile
///   work lives in a separate tracker item so we can tailor
///   the detection without poisoning the safe list.
/// * `truehd` / `mlp` — requires HEVC-style specialist
///   handling, not generally safe in fMP4.
/// * `opus` / `vorbis` / `flac` — technically valid in MP4
///   per RFC 6381 / ISOBMFF but HLS tooling support is
///   patchy; re-encode to AAC for safety. Worth revisiting
///   per-client as presets grow.
fn is_hls_fmp4_passthrough_safe(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        // AAC / AC-3 / EAC-3 are the broadly-safe HLS fMP4 audio
        // family — every "HEVC + AC-3/EAC-3 + Atmos" Blu-ray rip
        // streams through to capable clients with -c:a copy.
        "aac" | "ac3" | "eac3"
        // DTS / DTS-HD MA / DTS-X all surface as codec="dts"
        // from ffprobe (profile differentiates). HLS fMP4 support
        // for DTS is patchy outside Apple — the client-side gate
        // (`client.audio_codecs` must include "dts") is the
        // authoritative check; this list just says "the container
        // can carry it." Apple TV decodes DTS-in-fMP4 natively;
        // Chrome / Firefox can't, so their presets don't advertise
        // it and the selection check above never picks a DTS
        // track for them regardless of this flag.
        | "dts"
    )
}

/// Decide whether a Remux session should apply a bitstream
/// filter to strip dynamic HDR metadata.
///
/// Returns the filter spec as an ffmpeg `-bsf:v` value, or
/// `None` when no stripping is needed (SDR source, or client is
/// DV / HDR10+ native).
fn detect_hdr_metadata_strip(source: &SourceInfo, client: &ClientCapabilities) -> Option<String> {
    let fmt = source.hdr_format.as_deref()?.to_ascii_lowercase();

    // DV first — it's the more specific HDR variant and its
    // strip takes priority when both are present (profile 8.1
    // + HDR10+ is rare but possible).
    if (fmt.contains("dolby vision") || fmt.contains("dovi")) && !client.dv_support {
        let profile = dv_profile(&fmt);
        // Profile 5 has no HDR10 fallback in the stream; strip
        // produces garbage. Let the generic HDR branch force a
        // transcode / tonemap for this case (today via
        // `hdr_support=false`) or, in the future, through a
        // libplacebo-backed DV-aware tonemap.
        if profile != Some(5) && client.hdr_support {
            return Some("hevc_metadata=remove_dovi=1".into());
        }
    }

    if fmt.contains("hdr10+") && !client.hdr10_plus_support && client.hdr_support {
        return Some("hevc_metadata=remove_hdr10plus=1".into());
    }

    None
}

/// Extract the DV profile number from an `hdr_format` string
/// like `"Dolby Vision Profile 8.1"` → `Some(8)`. Used by the
/// strip-vs-transcode decision + the `SUPPLEMENTAL-CODECS`
/// encoder. Best-effort: returns `None` when the string is
/// malformed.
fn dv_profile(hdr_format_lc: &str) -> Option<u8> {
    hdr_format_lc.split_whitespace().find_map(|tok| {
        let head: String = tok.chars().take_while(char::is_ascii_digit).collect();
        head.parse::<u8>().ok()
    })
}

/// Map an accumulated reason set to a `PlaybackMethod`.
///
/// * Empty → `DirectPlay` (no conversion needed at all).
/// * Only `ContainerNotSupported` → `Remux` (video + audio
///   codecs are direct-playable; just repackage into fMP4
///   HLS, near-zero CPU).
/// * Anything else → `Transcode` (at least one stream needs
///   re-encoding; the profile chain decides HW vs SW at
///   session-creation time).
///
/// Factored out so the decision policy is testable without
/// constructing a full `SourceInfo` fixture and so a future
/// commit can extend it (e.g., "audio-only transcode" when the
/// video is OK but only audio fails) without reshuffling
/// `plan_playback`.
fn pick_method(reasons: &TranscodeReasons) -> PlaybackMethod {
    if reasons.is_empty() {
        return PlaybackMethod::DirectPlay;
    }
    if reasons.len() == 1 && reasons.contains(TranscodeReason::ContainerNotSupported) {
        return PlaybackMethod::Remux;
    }
    PlaybackMethod::Transcode
}

/// Case-insensitive membership test for a static string list
/// against an ffprobe-lowercased input. ffprobe already emits
/// lowercase codec names, but being lenient costs us nothing and
/// saves a class of "worked in unit test, failed in prod because
/// the fixture happened to be mixed case" bug.
fn has_lc(haystack: &[&'static str], needle: &str) -> bool {
    haystack.iter().any(|c| c.eq_ignore_ascii_case(needle))
}

/// True when the source is HDR (HDR10 / HLG / Dolby Vision / HDR10+).
/// Reads the authoritative `color_transfer` first (ffprobe emits
/// `smpte2084` for PQ, `arib-std-b67` for HLG), falls back to the
/// `hdr_format` string when that's missing (some containers don't
/// surface color metadata on the video stream but carry a
/// dovi / hdr10+ box that the import layer's summary picks up).
fn is_hdr_source(source: &SourceInfo) -> bool {
    if let Some(t) = source.color_transfer.as_deref() {
        let lc = t.to_ascii_lowercase();
        if lc == "smpte2084" || lc == "arib-std-b67" {
            return true;
        }
    }
    if let Some(f) = source.hdr_format.as_deref() {
        let lc = f.to_ascii_lowercase();
        if lc.contains("hdr") || lc.contains("dolby vision") || lc.contains("dovi") {
            return true;
        }
    }
    false
}

/// True when the source's pixel format is 10-bit (or higher
/// bit-depth). Pattern matches the ffprobe `pix_fmt` strings —
/// `yuv420p10le` / `yuv422p10le` / `yuv444p10le` are the common
/// 10-bit variants; `yuv420p12le` etc. for 12-bit.
fn is_ten_bit(pix_fmt: Option<&str>) -> bool {
    let Some(fmt) = pix_fmt else {
        return false;
    };
    let lc = fmt.to_ascii_lowercase();
    lc.contains("p10") || lc.contains("p12") || lc.contains("p16")
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a single-track audio list at the usual `stream_index`.
    /// Audio streams in most sources start at index 1 (video at 0),
    /// but the decision engine never cares about the absolute
    /// value — only that it's what ffmpeg will map to later. Use
    /// 1 here for realism without making it load-bearing.
    fn one_audio(codec: &str) -> Vec<AudioCandidate> {
        vec![AudioCandidate {
            stream_index: 1,
            codec: codec.into(),
            channels: None,
            channel_layout: None,
            profile: None,
        }]
    }

    /// Shorthand for building a plain SDR `SourceInfo` in tests
    /// with zero or one audio track — covers the bulk of the
    /// existing suite. Multi-track fixtures build `SourceInfo`
    /// by hand.
    fn src(c: &str, v: &str, a: Option<&str>) -> SourceInfo {
        SourceInfo {
            container: Some(c.into()),
            video_codec: Some(v.into()),
            audio_tracks: a.map(one_audio).unwrap_or_default(),
            color_transfer: None,
            pix_fmt: None,
            hdr_format: None,
        }
    }

    /// Shorthand for an HDR10 source. Takes the same codec spec
    /// as `src` but flips the color metadata.
    fn hdr10(c: &str, v: &str, a: Option<&str>) -> SourceInfo {
        SourceInfo {
            container: Some(c.into()),
            video_codec: Some(v.into()),
            audio_tracks: a.map(one_audio).unwrap_or_default(),
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("HDR10".into()),
        }
    }

    fn plan(s: &SourceInfo) -> PlaybackPlan {
        plan_playback(
            s,
            &ClientCapabilities::browser_defaults(),
            &PlaybackOptions::default(),
        )
    }

    #[test]
    fn mp4_h264_aac_direct_plays() {
        let p = plan(&src("mp4", "h264", Some("aac")));
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert!(p.transcode_reasons.is_empty());
    }

    #[test]
    fn mkv_h264_aac_remuxes() {
        // MKV isn't browser-direct-playable (no browser natively
        // plays MKV in `<video>`). H.264+AAC codecs are fine
        // though, so the plan is Remux — stream-copy into fMP4
        // HLS, near-zero CPU. Prior to the UA-aware capability
        // audit, this test asserted DirectPlay — which silently
        // produced a buffering `<video>` element because the
        // browser choked on the container.
        let p = plan(&src("mkv", "h264", Some("aac")));
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
    }

    #[test]
    fn mkv_h264_eac3_transcodes() {
        // MKV container + EAC-3 audio — both need fixing. Container
        // is a ContainerNotSupported; audio drives the full
        // transcode (a Remux can't re-encode audio while
        // stream-copying video).
        let p = plan(&src("mkv", "h264", Some("eac3")));
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported)
        );
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::VideoCodecNotSupported)
        );
    }

    #[test]
    fn hevc_flags_video_codec() {
        // Both `h265` and `hevc` ffprobe spellings must be caught.
        for codec in ["h265", "hevc"] {
            let p = plan(&src("mkv", codec, Some("aac")));
            assert_eq!(p.method, PlaybackMethod::Transcode);
            assert!(
                p.transcode_reasons
                    .contains(TranscodeReason::VideoCodecNotSupported)
            );
        }
    }

    #[test]
    fn avi_h264_aac_remuxes() {
        // AVI wrapping direct-playable video + audio: container
        // is the only problem, so the decision is Remux —
        // stream-copy into fMP4 HLS with near-zero CPU.
        let p = plan(&src("avi", "h264", Some("aac")));
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
        assert_eq!(p.transcode_reasons.len(), 1, "only container should flag");
    }

    #[test]
    fn avi_h264_ac3_transcodes_not_remuxes() {
        // AVI + H.264 + AC-3: two failures (container + audio
        // codec). Remux would produce audio the browser can't
        // decode; must be a full transcode.
        let p = plan(&src("avi", "h264", Some("ac3")));
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported)
        );
    }

    #[test]
    fn silent_clip_direct_plays() {
        // No audio stream must not trigger AudioCodecNotSupported —
        // regression guard against "forcing a transcode on a
        // silent test file".
        let p = plan(&src("mp4", "h264", None));
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert!(p.transcode_reasons.is_empty());
    }

    #[test]
    fn missing_container_flags_container() {
        let p = plan(&SourceInfo {
            container: None,
            video_codec: Some("h264".into()),
            audio_tracks: one_audio("aac"),
            ..Default::default()
        });
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
    }

    #[test]
    fn missing_video_flags_video() {
        let p = plan(&SourceInfo {
            container: Some("mp4".into()),
            video_codec: None,
            audio_tracks: one_audio("aac"),
            ..Default::default()
        });
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoCodecNotSupported)
        );
    }

    // ─── HDR / 10-bit branch ──────────────────────────────────

    #[test]
    fn hdr10_to_sdr_client_flags_range() {
        // HDR10 source, browser-default (SDR) client → tone-map
        // needed. Reasons also carry VideoBitDepthNotSupported
        // because the HDR10 source is 10-bit, and
        // VideoCodecNotSupported because HEVC isn't in
        // browser-default codec list yet.
        let p = plan(&hdr10("mkv", "hevc", Some("aac")));
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoRangeTypeNotSupported),
            "HDR → SDR must flag range",
        );
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoBitDepthNotSupported),
            "10-bit pix_fmt must flag bit depth",
        );
    }

    #[test]
    fn hdr_source_to_hdr_capable_client_stays_clean() {
        // Same HDR10 source, but a hypothetical HDR-capable
        // client (future Chromecast Ultra preset). HDR + 10-bit
        // reasons drop out.
        let mut client = ClientCapabilities::browser_defaults();
        client.hdr_support = true;
        client.ten_bit_support = true;
        // Still flags VideoCodecNotSupported because this client
        // doesn't have HEVC — that's fine, separate concern.
        let p = plan_playback(
            &hdr10("mkv", "hevc", Some("aac")),
            &client,
            &PlaybackOptions::default(),
        );
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::VideoRangeTypeNotSupported)
        );
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::VideoBitDepthNotSupported)
        );
    }

    #[test]
    fn hlg_source_flags_range() {
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: Some("arib-std-b67".into()),
            pix_fmt: Some("yuv420p".into()),
            hdr_format: None,
        };
        let p = plan(&s);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoRangeTypeNotSupported)
        );
        // HLG at 8-bit — no bit-depth flag.
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::VideoBitDepthNotSupported)
        );
    }

    #[test]
    fn dv_via_hdr_format_fallback_flags_range() {
        // Some containers don't emit `color_transfer` on the
        // video stream but do surface the DV / HDR10+ box in
        // the container-level metadata. The import layer
        // summarises this as a string on `media.hdr_format`.
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: None,
            pix_fmt: Some("yuv420p".into()),
            hdr_format: Some("Dolby Vision Profile 5".into()),
        };
        let p = plan(&s);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoRangeTypeNotSupported)
        );
    }

    #[test]
    fn ten_bit_h264_flags_bit_depth() {
        let s = SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("h264".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: Some("bt709".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: None,
        };
        let p = plan(&s);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoBitDepthNotSupported),
            "10-bit H.264 must force 8-bit re-encode for Safari-like clients",
        );
        // SDR source — no range flag.
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::VideoRangeTypeNotSupported)
        );
    }

    #[test]
    fn eight_bit_source_no_flag() {
        let s = src("mp4", "h264", Some("aac"));
        let p = plan(&s);
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::VideoBitDepthNotSupported)
        );
    }

    #[test]
    fn all_three_flags_fire_when_everything_is_wrong() {
        // DTS audio in an AVI wrapping HEVC is pathological but
        // good coverage for "reasons accumulate, not short-circuit".
        let p = plan(&src("avi", "hevc", Some("dts")));
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::VideoCodecNotSupported)
        );
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported)
        );
        assert_eq!(p.transcode_reasons.len(), 3);
    }

    #[test]
    fn case_insensitive_codec_match() {
        // ffprobe lowercases, but a direct caller with mixed case
        // (e.g. a log replay, a user-entered fixture) should still
        // match.
        let p = plan(&src("MP4", "H264", Some("AAC")));
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
    }

    #[test]
    fn direct_play_reasons_is_always_empty() {
        // Invariant: `method == DirectPlay` ⇔
        // `transcode_reasons.is_empty()`. Sweep a handful of
        // variations to catch any future arm that forgets to
        // either add a reason or fall through to DirectPlay.
        // MKV is no longer a browser direct-play container
        // (see `browser_defaults` docstring) so the cases use
        // browser-native containers only.
        let cases = [
            src("mp4", "h264", Some("aac")),
            src("webm", "vp9", Some("opus")),
            src("webm", "vp8", Some("vorbis")),
            src("m4v", "h264", None),
        ];
        for s in cases {
            let p = plan(&s);
            assert_eq!(p.method, PlaybackMethod::DirectPlay, "{s:?}");
            assert!(p.transcode_reasons.is_empty(), "{s:?}");
        }
    }

    // ─── Multi-audio-track compat selection ──────────────────

    fn audio(stream_index: i64, codec: &str) -> AudioCandidate {
        AudioCandidate {
            stream_index,
            codec: codec.into(),
            channels: None,
            channel_layout: None,
            profile: None,
        }
    }

    #[test]
    fn single_compat_audio_track_is_selected() {
        // Baseline: one-track AAC source — the selection is that
        // track, no reason, direct-play.
        let p = plan(&src("mp4", "h264", Some("aac")));
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert_eq!(p.selected_audio_stream, Some(1));
    }

    #[test]
    fn multi_audio_primary_compat_wins() {
        // Primary is AAC → pick it, ignore the rest. MP4 container
        // so we stay on the direct-play path. (MKV would force a
        // Remux now that browsers can't direct-play MKV, which
        // would obscure the track-selection behaviour under test.)
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            audio_tracks: vec![audio(1, "aac"), audio(2, "eac3"), audio(3, "truehd")],
            ..Default::default()
        };
        let p = plan(&s);
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert_eq!(p.selected_audio_stream, Some(1));
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported)
        );
    }

    #[test]
    fn multi_audio_picks_secondary_when_primary_incompat() {
        // The headline scenario: UHD rip with TrueHD primary +
        // AC-3 secondary. The selection logic should pick AC-3
        // so we don't force a full audio re-encode. MP4 chosen
        // to isolate track selection from container handling —
        // an MKV here would hit Remux-because-container and mask
        // the assertion we care about (audio-not-flagged).
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            audio_tracks: vec![audio(1, "truehd"), audio(2, "ac3")],
            ..Default::default()
        };
        let mut client = ClientCapabilities::browser_defaults();
        client.audio_codecs.push("ac3");
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert_eq!(p.selected_audio_stream, Some(2));
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported),
            "a compatible secondary track avoids the transcode flag",
        );
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
    }

    #[test]
    fn multi_audio_all_incompat_flags_and_no_selection() {
        // [TrueHD, DTS-HD MA] against browser_defaults — nothing
        // is compatible. Flag AudioCodecNotSupported and return
        // no selection; the API layer falls back to the primary
        // for the forced re-encode.
        let s = SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("h264".into()),
            audio_tracks: vec![audio(1, "truehd"), audio(2, "dts")],
            ..Default::default()
        };
        let p = plan(&s);
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert_eq!(p.selected_audio_stream, None);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported)
        );
    }

    #[test]
    fn no_audio_tracks_means_no_selection_no_flag() {
        // Silent clip. No flag, no selection — ffmpeg's default
        // `0:a:0` is still safe because the mapper simply skips
        // the audio stream when there isn't one.
        let p = plan(&src("mp4", "h264", None));
        assert_eq!(p.selected_audio_stream, None);
        assert!(
            !p.transcode_reasons
                .contains(TranscodeReason::AudioCodecNotSupported)
        );
    }

    #[test]
    fn multi_audio_picks_first_compat_in_ffprobe_order() {
        // Three compatible tracks — the selection is the first
        // one in list order. "First ffprobe index" is the
        // stable, defensible default; the frontend picker still
        // lets the user override.
        let s = SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("h264".into()),
            audio_tracks: vec![audio(1, "aac"), audio(2, "aac"), audio(3, "opus")],
            ..Default::default()
        };
        let p = plan(&s);
        assert_eq!(p.selected_audio_stream, Some(1));
    }

    #[test]
    fn multi_audio_skips_incompat_primary_keeps_remux_when_container_is_wrong() {
        // AVI + [TrueHD, AC-3] with an AC-3-capable client:
        // container flags remux, audio is fine via the
        // secondary → method is Remux, not Transcode.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("h264".into()),
            audio_tracks: vec![audio(1, "truehd"), audio(2, "ac3")],
            ..Default::default()
        };
        let mut client = ClientCapabilities::browser_defaults();
        client.audio_codecs.push("ac3");
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert_eq!(p.selected_audio_stream, Some(2));
    }

    // ─── Dynamic HDR metadata stripping ──────────────────────

    /// Hypothetical HDR-capable client with HEVC support —
    /// mimics what a future Apple TV / Cast preset would
    /// advertise. Deliberately does NOT add AVI to `containers`:
    /// tests that need to force the Remux branch do so by
    /// using an AVI source container, which isn't in this
    /// client's list.
    fn hdr_capable_client() -> ClientCapabilities {
        let mut c = ClientCapabilities::browser_defaults();
        c.video_codecs.push("hevc");
        c.hdr_support = true;
        c.ten_bit_support = true;
        c
    }

    #[test]
    fn dv_profile_81_remux_to_hdr10_client_strips_rpu() {
        // Profile 8.1 DV to an HDR-capable client that isn't
        // DV-native: strip the RPU so the output is pure
        // HDR10. Container + video + audio are compatible so
        // method stays Remux (no codec transcode).
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
        };
        let p = plan_playback(&s, &hdr_capable_client(), &PlaybackOptions::default());
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        // DirectPlay has no filter — filter only fires on Remux.
        assert!(p.video_bitstream_filter.is_none());
    }

    #[test]
    fn dv_profile_81_with_container_remux_strips_rpu() {
        // AVI container forces Remux; DV profile 8.1 → strip.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
        };
        let p = plan_playback(&s, &hdr_capable_client(), &PlaybackOptions::default());
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert_eq!(
            p.video_bitstream_filter.as_deref(),
            Some("hevc_metadata=remove_dovi=1")
        );
    }

    #[test]
    fn dv_profile_5_is_not_stripped() {
        // Profile 5 has no HDR10 fallback in the bitstream —
        // stripping yields IPT-PQ-C2 garbage. No filter.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("Dolby Vision Profile 5".into()),
        };
        let p = plan_playback(&s, &hdr_capable_client(), &PlaybackOptions::default());
        assert!(p.video_bitstream_filter.is_none());
    }

    #[test]
    fn dv_to_dv_native_client_does_not_strip() {
        // Client advertises DV support — keep the full
        // DV bitstream including RPU.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: one_audio("aac"),
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
            ..Default::default()
        };
        let mut client = hdr_capable_client();
        client.dv_support = true;
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert!(p.video_bitstream_filter.is_none());
    }

    #[test]
    fn hdr10plus_to_hdr10_client_strips_dynamic_metadata() {
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: one_audio("aac"),
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("HDR10+".into()),
        };
        let p = plan_playback(&s, &hdr_capable_client(), &PlaybackOptions::default());
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert_eq!(
            p.video_bitstream_filter.as_deref(),
            Some("hevc_metadata=remove_hdr10plus=1")
        );
    }

    #[test]
    fn hdr10plus_to_hdr10plus_client_does_not_strip() {
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: one_audio("aac"),
            hdr_format: Some("HDR10+".into()),
            ..Default::default()
        };
        let mut client = hdr_capable_client();
        client.hdr10_plus_support = true;
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert!(p.video_bitstream_filter.is_none());
    }

    #[test]
    fn plain_hdr10_source_does_not_need_stripping() {
        // Plain HDR10 has no dynamic metadata to strip — just
        // static metadata that every HDR10 client handles.
        let s = hdr10("avi", "hevc", Some("aac"));
        let p = plan_playback(&s, &hdr_capable_client(), &PlaybackOptions::default());
        assert!(p.video_bitstream_filter.is_none());
    }

    #[test]
    fn sdr_source_never_gets_a_strip_filter() {
        let s = src("avi", "h264", Some("aac"));
        let p = plan(&s);
        assert!(p.video_bitstream_filter.is_none());
    }

    // ─── Audio passthrough ────────────────────────────────────

    /// Transcode-forcing source (HEVC + browser-defaults): the
    /// video side has to re-encode but audio might be
    /// compatible. Helper assembles that scenario compactly.
    fn hevc_with_audio(codecs: &[&str]) -> SourceInfo {
        SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: codecs
                .iter()
                .enumerate()
                .map(|(i, c)| AudioCandidate {
                    stream_index: i64::try_from(i).expect("test vec fits in i64") + 1,
                    codec: (*c).into(),
                    channels: Some(6),
                    channel_layout: Some("5.1".into()),
                    profile: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn transcode_with_compat_eac3_enables_passthrough() {
        // HEVC forces Transcode; EAC-3 advertised by the
        // client + safe in fMP4 → passthrough fires.
        let mut client = ClientCapabilities::browser_defaults();
        client.audio_codecs.push("eac3");
        let s = hevc_with_audio(&["eac3"]);
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert!(p.audio_passthrough, "eac3 should passthrough");
        assert_eq!(p.selected_audio_stream, Some(1));
    }

    #[test]
    fn transcode_with_aac_enables_passthrough() {
        // Even AAC — normally "would direct-play if video
        // weren't broken" — benefits from passthrough when
        // video forces a re-encode. Saves the AAC decode +
        // re-encode round-trip.
        let s = hevc_with_audio(&["aac"]);
        let p = plan(&s);
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert!(p.audio_passthrough);
    }

    #[test]
    fn transcode_with_incompat_audio_skips_passthrough() {
        // TrueHD primary + client can't play it → decision
        // flags AudioCodecNotSupported, no selection, no
        // passthrough.
        let s = hevc_with_audio(&["truehd"]);
        let p = plan(&s);
        assert!(!p.audio_passthrough);
        assert_eq!(p.selected_audio_stream, None);
    }

    #[test]
    fn transcode_picks_compat_secondary_and_passes_through() {
        // Dual track [TrueHD, AC-3] + AC-3 client: selection
        // picks AC-3 (stream 2), passthrough fires on that
        // track.
        let mut client = ClientCapabilities::browser_defaults();
        client.audio_codecs.push("ac3");
        let s = hevc_with_audio(&["truehd", "ac3"]);
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert_eq!(p.selected_audio_stream, Some(2));
        assert!(p.audio_passthrough);
    }

    #[test]
    fn remux_does_not_set_audio_passthrough_flag() {
        // Remux already copies audio unconditionally; the
        // flag is only meaningful for the Transcode branch.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("h264".into()),
            audio_tracks: one_audio("aac"),
            ..Default::default()
        };
        let p = plan(&s);
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert!(
            !p.audio_passthrough,
            "Remux handles audio copy via -c:a copy, not this flag"
        );
    }

    #[test]
    fn direct_play_does_not_set_audio_passthrough_flag() {
        let s = src("mp4", "h264", Some("aac"));
        let p = plan(&s);
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert!(!p.audio_passthrough);
    }

    #[test]
    fn transcode_with_dts_advertised_enables_passthrough() {
        // Core test for the "DTS is now fMP4-safe" change: a
        // client that advertises DTS (e.g. the Apple TV preset,
        // or a user override on a receiver with passthrough)
        // paired with an HEVC video forces Transcode on the
        // video side but lets audio stream-copy via the
        // passthrough flag. Uses a browser-floor client + manual
        // DTS addition to isolate the gate from other preset
        // fields (hdr, ten_bit, etc.).
        let mut client = ClientCapabilities::browser_defaults();
        client.audio_codecs.push("dts");
        let s = hevc_with_audio(&["dts"]);
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert_eq!(p.method, PlaybackMethod::Transcode);
        assert_eq!(p.selected_audio_stream, Some(1));
        assert!(
            p.audio_passthrough,
            "DTS is now fMP4-passthrough-safe — should stream-copy"
        );
    }

    #[test]
    fn apple_tv_direct_plays_hevc_with_dts() {
        // End-to-end: Apple TV 4K + HEVC + DTS-HD MA in MKV
        // needs nothing from ffmpeg — container + video codec +
        // audio codec all compatible. DirectPlay bypasses the
        // transcode manager entirely; no `audio_passthrough`
        // flag involved.
        let s = SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "dts".into(),
                channels: Some(8),
                channel_layout: Some("7.1".into()),
                profile: None,
            }],
            ..Default::default()
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::apple_tv_4k(),
            &PlaybackOptions::default(),
        );
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert!(p.transcode_reasons.is_empty());
    }

    #[test]
    fn transcode_with_truehd_still_skips_passthrough() {
        // Negative case: TrueHD is advertised nowhere today —
        // its HLS fMP4 support is outside every mainstream
        // client's decode matrix. Confirms the fMP4-safe table
        // is still the gate (hypothetical client that adds
        // `truehd` to its codec list won't sneak around).
        let mut client = ClientCapabilities::browser_defaults();
        client.audio_codecs.push("truehd");
        let s = hevc_with_audio(&["truehd"]);
        let p = plan_playback(&s, &client, &PlaybackOptions::default());
        assert_eq!(p.selected_audio_stream, Some(1));
        assert!(
            !p.audio_passthrough,
            "TrueHD isn't on the fMP4-safe list — must re-encode"
        );
    }

    // ─── Client capability presets ───────────────────────────

    #[test]
    fn chromecast_gtv_direct_plays_hevc_eac3_hdr() {
        // UHD rip: HEVC Main10 HDR10 + EAC-3 in MKV. Browser
        // would transcode everything; Cast GTV direct-plays
        // end-to-end.
        let s = SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "eac3".into(),
                channels: Some(6),
                channel_layout: Some("5.1".into()),
                profile: None,
            }],
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("HDR10".into()),
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::chromecast_gtv(),
            &PlaybackOptions::default(),
        );
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert_eq!(p.selected_audio_stream, Some(1));
        assert!(p.transcode_reasons.is_empty());
    }

    #[test]
    fn apple_tv_4k_direct_plays_dv_profile_8_hevc() {
        // Canonical Apple TV case: DV 8.1 HEVC with EAC-3
        // Atmos. Direct plays with full HDR passthrough.
        let s = SourceInfo {
            container: Some("mp4".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "eac3".into(),
                channels: Some(6),
                channel_layout: Some("5.1".into()),
                profile: None,
            }],
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::apple_tv_4k(),
            &PlaybackOptions::default(),
        );
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
        assert!(p.transcode_reasons.is_empty());
    }

    #[test]
    fn samsung_tizen_strips_dv_rpu_on_remux() {
        // Samsung doesn't support DV — on a DV source in a
        // non-native container, we remux + strip the RPU so
        // the TV sees pure HDR10.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "eac3".into(),
                channels: Some(6),
                channel_layout: Some("5.1".into()),
                profile: None,
            }],
            color_transfer: Some("smpte2084".into()),
            pix_fmt: Some("yuv420p10le".into()),
            hdr_format: Some("Dolby Vision Profile 8.1".into()),
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::samsung_tizen(),
            &PlaybackOptions::default(),
        );
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert_eq!(
            p.video_bitstream_filter.as_deref(),
            Some("hevc_metadata=remove_dovi=1")
        );
    }

    #[test]
    fn samsung_tizen_passes_hdr10_plus_through() {
        // HDR10+ is Samsung's format — passed through, not
        // stripped.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "aac".into(),
                channels: Some(2),
                channel_layout: Some("stereo".into()),
                profile: None,
            }],
            hdr_format: Some("HDR10+".into()),
            ..Default::default()
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::samsung_tizen(),
            &PlaybackOptions::default(),
        );
        assert!(
            p.video_bitstream_filter.is_none(),
            "Samsung = HDR10+ native, no strip"
        );
    }

    #[test]
    fn lg_webos_strips_hdr10_plus() {
        // LG is Dolby-aligned — doesn't decode HDR10+ dynamic
        // metadata. On HDR10+ source we strip to plain HDR10.
        let s = SourceInfo {
            container: Some("avi".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "aac".into(),
                channels: Some(2),
                channel_layout: Some("stereo".into()),
                profile: None,
            }],
            hdr_format: Some("HDR10+".into()),
            ..Default::default()
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::lg_webos(),
            &PlaybackOptions::default(),
        );
        assert_eq!(
            p.video_bitstream_filter.as_deref(),
            Some("hevc_metadata=remove_hdr10plus=1")
        );
    }

    // ─── UA parser ──────────────────────────────────────────────

    #[test]
    fn ua_firefox_linux() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Firefox);
        assert_eq!(d.os, ClientOs::Linux);
        assert_eq!(d.preset, "firefox");
    }

    #[test]
    fn ua_firefox_windows() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Firefox);
        assert_eq!(d.os, ClientOs::Windows);
    }

    #[test]
    fn ua_chrome_windows() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Chromium);
        assert_eq!(d.os, ClientOs::Windows);
        assert_eq!(d.preset, "chromium_windows");
    }

    #[test]
    fn ua_chrome_macos() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Chromium);
        assert_eq!(d.os, ClientOs::MacOs);
        assert_eq!(d.preset, "chromium_macos");
    }

    #[test]
    fn ua_chrome_linux() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Chromium);
        assert_eq!(d.os, ClientOs::Linux);
        assert_eq!(d.preset, "chromium_linux");
    }

    #[test]
    fn ua_edge_matches_before_chrome() {
        // Edge's UA ends with "Edg/…" after "Chrome/…" — ordering
        // of substring checks must hit the Edge token first.
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Edge);
        assert_eq!(d.preset, "edge_windows");
    }

    #[test]
    fn ua_safari_macos() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Safari);
        assert_eq!(d.os, ClientOs::MacOs);
        assert_eq!(d.preset, "safari_macos");
    }

    #[test]
    fn ua_safari_ios() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Safari);
        assert_eq!(d.os, ClientOs::Ios);
        assert_eq!(d.preset, "safari_ios");
    }

    #[test]
    fn ua_firefox_ios_is_webkit_under_the_hood() {
        // iOS mandates WebKit even for third-party browsers; a
        // "Firefox for iOS" app has to use Safari's rendering.
        // Our detector should promote it to the Safari codec
        // matrix so we don't offer codecs FxiOS can't actually
        // decode.
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) FxiOS/127.0 Mobile/15E148 Safari/605.1.15";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Safari);
        assert_eq!(d.os, ClientOs::Ios);
    }

    #[test]
    fn ua_chromecast_gtv() {
        let ua = "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 (KHTML, like Gecko) CrKey/1.70 Chrome/116.0.0.0 Safari/537.36";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::Chromecast);
    }

    #[test]
    fn ua_samsung_tizen() {
        let ua = "Mozilla/5.0 (SMART-TV; LINUX; Tizen 6.0) AppleWebKit/537.36 (KHTML, like Gecko) 85.0.4183.93/6.0 TV Safari/537.36";
        let d = DetectedClient::from_ua(ua);
        assert_eq!(d.family, BrowserFamily::SamsungTizen);
    }

    #[test]
    fn ua_unknown_falls_back_to_defaults() {
        let d = DetectedClient::from_ua("some-random-fetcher/1.0");
        assert_eq!(d.family, BrowserFamily::Unknown);
        assert_eq!(d.preset, "browser_defaults");
    }

    #[test]
    fn ua_empty_returns_unknown() {
        let (_caps, d) = ClientCapabilities::from_user_agent(Some(""));
        assert_eq!(d.family, BrowserFamily::Unknown);
    }

    // ─── MKV should not be browser-direct-playable ──────────────

    #[test]
    fn browser_defaults_rejects_mkv() {
        let s = src("mkv", "h264", Some("aac"));
        let p = plan_playback(
            &s,
            &ClientCapabilities::browser_defaults(),
            &PlaybackOptions::default(),
        );
        // Container mismatch alone → Remux (not DirectPlay). Cheap
        // stream-copy into fMP4; matches the mental model of "MKV
        // container isn't a browser direct-play format."
        assert_eq!(p.method, PlaybackMethod::Remux);
        assert!(
            p.transcode_reasons
                .contains(TranscodeReason::ContainerNotSupported)
        );
    }

    #[test]
    fn firefox_rejects_mkv() {
        let s = src("mkv", "h264", Some("aac"));
        let p = plan_playback(
            &s,
            &ClientCapabilities::firefox(),
            &PlaybackOptions::default(),
        );
        assert_eq!(p.method, PlaybackMethod::Remux);
    }

    #[test]
    fn chromecast_ultra_eac3_transcode_video_passthrough_audio() {
        // Mid-resolution HEVC+EAC3 direct-plays to Cast Ultra.
        let s = SourceInfo {
            container: Some("mkv".into()),
            video_codec: Some("hevc".into()),
            audio_tracks: vec![AudioCandidate {
                stream_index: 1,
                codec: "eac3".into(),
                channels: Some(6),
                channel_layout: Some("5.1".into()),
                profile: None,
            }],
            ..Default::default()
        };
        let p = plan_playback(
            &s,
            &ClientCapabilities::chromecast_ultra(),
            &PlaybackOptions::default(),
        );
        assert_eq!(p.method, PlaybackMethod::DirectPlay);
    }
}
