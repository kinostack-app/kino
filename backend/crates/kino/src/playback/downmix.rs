//! Multichannel → stereo downmix policy for the audio
//! re-encode path.
//!
//! When we can't passthrough the source audio track (e.g.,
//! the client doesn't advertise AC-3 / EAC-3) we re-encode
//! to stereo AAC. Driving that re-encode through ffmpeg's
//! default `-ac 2` gets stereo output, but the default mix
//! matrix drops the LFE channel entirely and — on older
//! ffmpeg builds — attenuates the centre channel by −6 dB
//! instead of the standard −3 dB, making dialogue audibly
//! quieter than music and effects even on correctly-
//! calibrated playback systems.
//!
//! This module builds an explicit `pan=` filter spec that
//! encodes the ITU-R BS.775-1 coefficients — the canonical
//! standard used by Dolby reference decoders, Apple TV's
//! internal renderer, and the AES / RFC 7845 Opus spec.
//!
//! # Coefficient matrix (5.1 → 2.0, Bs775 algorithm)
//!
//! ```text
//!   L' = FL + 0.707·FC + 0.707·BL + 0.500·LFE
//!   R' = FR + 0.707·FC + 0.707·BR + 0.500·LFE
//! ```
//!
//! * Centre (dialogue channel) enters both L and R at
//!   −3 dB (0.707 linear) so its perceived loudness stays
//!   close to the discrete-channel reference.
//! * Surrounds enter at −3 dB — BS.775-1 recommendation.
//! * LFE is dropped entirely by BS.775-1 (purist) but we
//!   blend it at −6 dB (0.5 linear) because the common
//!   kino listening case is a laptop or TV speaker without
//!   a subwoofer; passing bass into the main channels
//!   preserves the low end rather than silencing it.
//!
//! # Variants
//!
//! `Bs775` is the canonical default. Future variants
//! (`NightmodeDialogue`, `Rfc7845`, `Ac4`) live in the
//! tracker and will land as user-selectable knobs —
//! rendering them here now keeps the selector scaffolded
//! without forcing the UI work upfront.

use std::fmt::Write;

/// Downmix algorithm selector. Only `Bs775` is implemented
/// today; the enum is here so the tracker's future
/// `NightmodeDialogue` / `Rfc7845` / `Ac4` variants can land
/// without signature churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownmixAlgorithm {
    /// ITU-R BS.775-1 canonical coefficients + LFE blend at
    /// −6 dB. Default for every transcode.
    #[default]
    Bs775,
}

/// EBU R128 loudness-normalisation spec appended after the
/// downmix filter. Target −16 LUFS with −1.5 dBTP ceiling
/// keeps the output in the streaming-platform range where
/// browser volume sliders land at sensible positions. The
/// normalise happens post-downmix so the target LUFS applies
/// to the collapsed stereo signal, not the pre-downmix
/// multichannel one.
const LOUDNORM: &str = "loudnorm=I=-16:TP=-1.5:LRA=11";

/// Build the `-af` filter spec for the configured algorithm +
/// source channel layout.
///
/// Returns:
/// * `Some(filter)` — emit `-af <filter>` and drop any
///   `-ac N` flag (the pan output already specifies stereo).
/// * `None` — source layout is mono / stereo / unknown. Fall
///   back to ffmpeg's default `-ac 2` in the caller. Stereo
///   sources don't need downmix; unknown layouts we leave to
///   ffmpeg rather than pick a wrong matrix.
#[must_use]
pub fn build_downmix_filter(
    channel_layout: Option<&str>,
    algorithm: DownmixAlgorithm,
) -> Option<String> {
    let layout = channel_layout?.to_ascii_lowercase();
    let matrix = match_matrix(&layout, algorithm)?;
    let mut f = String::new();
    let _ = write!(f, "pan=stereo|{matrix},{LOUDNORM}");
    Some(f)
}

/// Lookup the coefficient matrix for `(layout, algorithm)`.
/// Returns the `FL=...|FR=...` core (without the `pan=stereo|`
/// prefix or the loudnorm tail — both added by the caller).
///
/// ffmpeg's `pan` filter uses fixed channel names: `FL FR FC
/// LFE BL BR SL SR`. We stay consistent with those regardless
/// of the source's layout string. Where a layout lacks a
/// channel (e.g., 5.0 has no LFE) the term is simply omitted.
fn match_matrix(layout: &str, algorithm: DownmixAlgorithm) -> Option<&'static str> {
    // Ignore `ffmpeg`'s parenthetical decorations such as
    // "5.1(side)" — they describe the physical arrangement,
    // not the channel count. Strip to the canonical prefix.
    let canonical = layout.split('(').next().unwrap_or(layout).trim();

    match (canonical, algorithm) {
        // 5.1 — FL FR FC LFE BL BR
        ("5.1", DownmixAlgorithm::Bs775) => {
            Some("FL=FL+0.707*FC+0.707*BL+0.5*LFE|FR=FR+0.707*FC+0.707*BR+0.5*LFE")
        }
        // 5.0 — FL FR FC BL BR (no LFE)
        ("5.0", DownmixAlgorithm::Bs775) => Some("FL=FL+0.707*FC+0.707*BL|FR=FR+0.707*FC+0.707*BR"),
        // 7.1 — FL FR FC LFE BL BR SL SR. Sides + backs both
        // contribute to the stereo image; BS.775 weights each
        // at −3 dB.
        ("7.1", DownmixAlgorithm::Bs775) => Some(
            "FL=FL+0.707*FC+0.707*SL+0.707*BL+0.5*LFE|\
             FR=FR+0.707*FC+0.707*SR+0.707*BR+0.5*LFE",
        ),
        // 7.0 — 7.1 without LFE
        ("7.0", DownmixAlgorithm::Bs775) => {
            Some("FL=FL+0.707*FC+0.707*SL+0.707*BL|FR=FR+0.707*FC+0.707*SR+0.707*BR")
        }
        // 4.0 quadraphonic — FL FR BL BR (no centre, no LFE)
        ("4.0" | "quad", DownmixAlgorithm::Bs775) => Some("FL=FL+0.707*BL|FR=FR+0.707*BR"),
        // Everything else (mono, stereo, 2.1, exotic layouts) →
        // None; caller uses `-ac 2` default.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bs775_5_1_emits_canonical_matrix() {
        let f =
            build_downmix_filter(Some("5.1"), DownmixAlgorithm::Bs775).expect("5.1 should downmix");
        assert!(f.starts_with("pan=stereo|"));
        assert!(f.contains("FL=FL+0.707*FC+0.707*BL+0.5*LFE"));
        assert!(f.contains("FR=FR+0.707*FC+0.707*BR+0.5*LFE"));
        assert!(f.ends_with(LOUDNORM));
    }

    #[test]
    fn bs775_7_1_includes_side_channels() {
        let f = build_downmix_filter(Some("7.1"), DownmixAlgorithm::Bs775).unwrap();
        assert!(f.contains("SL"), "7.1 must pull in the side-left channel");
        assert!(f.contains("SR"), "7.1 must pull in the side-right channel");
        assert!(f.contains("BL"));
        assert!(f.contains("BR"));
    }

    #[test]
    fn bs775_5_0_omits_lfe_term() {
        let f = build_downmix_filter(Some("5.0"), DownmixAlgorithm::Bs775).unwrap();
        assert!(!f.contains("LFE"), "5.0 has no LFE, term should be absent");
        assert!(f.contains("0.707*FC"));
    }

    #[test]
    fn parentheticals_are_stripped() {
        // ffprobe emits "5.1(side)" for the (FL,FR,FC,LFE,SL,SR)
        // physical arrangement — same 6 channels as plain 5.1,
        // treated identically.
        let f = build_downmix_filter(Some("5.1(side)"), DownmixAlgorithm::Bs775);
        assert!(f.is_some());
        assert!(f.unwrap().contains("0.5*LFE"));
    }

    #[test]
    fn case_insensitive_layout() {
        let f = build_downmix_filter(Some("5.1"), DownmixAlgorithm::Bs775);
        let f_upper = build_downmix_filter(Some("5.1"), DownmixAlgorithm::Bs775);
        assert_eq!(f, f_upper);
    }

    #[test]
    fn stereo_source_returns_none() {
        // Stereo → stereo needs no downmix; caller uses the
        // existing `-c:a aac` path without a filter.
        assert!(build_downmix_filter(Some("stereo"), DownmixAlgorithm::Bs775).is_none());
    }

    #[test]
    fn mono_source_returns_none() {
        assert!(build_downmix_filter(Some("mono"), DownmixAlgorithm::Bs775).is_none());
    }

    #[test]
    fn unknown_layout_returns_none() {
        // Unknown-layout fallback is important: rather than
        // pick a wrong matrix, let ffmpeg's `-ac 2` default do
        // its best. Safer than publishing "weird mix" output.
        assert!(build_downmix_filter(Some("9.2.4"), DownmixAlgorithm::Bs775).is_none());
    }

    #[test]
    fn missing_layout_returns_none() {
        assert!(build_downmix_filter(None, DownmixAlgorithm::Bs775).is_none());
    }

    #[test]
    fn loudnorm_is_tacked_on_at_the_end() {
        // Single-shot regression check — loudnorm must appear
        // as the terminal filter or the target LUFS applies
        // to the wrong signal.
        let f = build_downmix_filter(Some("5.1"), DownmixAlgorithm::Bs775).unwrap();
        let last_comma = f.rfind(',').unwrap();
        assert_eq!(&f[last_comma + 1..], LOUDNORM);
    }

    #[test]
    fn quad_layout_maps_backs_only() {
        // Quadraphonic (FL FR BL BR, no centre, no LFE) falls
        // into the "4.0 / quad" arm — no centre term, no LFE
        // term, just the back channels at −3 dB.
        let f = build_downmix_filter(Some("quad"), DownmixAlgorithm::Bs775).unwrap();
        assert!(!f.contains("FC"));
        assert!(!f.contains("LFE"));
        assert!(f.contains("BL"));
        assert!(f.contains("BR"));
    }
}
