//! Profile chain — ordered list of fallback playback profiles.
//!
//! A profile captures *how* a single session will produce HLS: the
//! playback method (remux vs. re-encode) and, when re-encoding, the
//! hardware backend driving the encoder. The chain is an ordered
//! list of profiles; the front rung is the one the session starts
//! on. On a known mid-stream failure (hardware driver reload, `PCIe`
//! reset, encoder OOM) the chain pops to the next rung and the
//! session is respawned.
//!
//! The chain design is deliberately simple — a `VecDeque` of
//! profiles — because the policy lives in [`ProfileChain::build`].
//! Callers don't build rungs by hand; they hand `build` a
//! decision-engine plan + the configured HWA + the probe result,
//! and the builder decides which rungs make sense. That keeps all
//! fallback policy in one auditable function instead of scattered
//! across API handlers.
//!
//! # Chain shapes
//!
//! | Plan method  | Configured HWA | Probe says      | Rungs                                         |
//! |--------------|----------------|-----------------|-----------------------------------------------|
//! | `DirectPlay`   | any            | any             | (empty — caller serves `/direct` instead)     |
//! | Remux        | any            | any             | `[Remux, SoftwareTranscode]`                  |
//! | Transcode    | HW             | HW available    | `[HardwareTranscode(HW), SoftwareTranscode]`  |
//! | Transcode    | HW             | HW unavailable  | `[SoftwareTranscode]`                         |
//! | Transcode    | None           | n/a             | `[SoftwareTranscode]`                         |
//!
//! Software transcode is the universal terminal rung — every
//! modern ffmpeg build ships libx264, so it's never absent from a
//! non-empty chain. The probe validates libx264's presence at
//! startup; if it's genuinely missing we fail loudly there, not
//! silently here.
//!
//! The `Remux → SoftwareTranscode` fallback catches the rare case
//! where the source's bitstream trips up the stream-copy muxer
//! (malformed SEI, exotic NAL unit, broken MKV segment) so that
//! the user gets playable bytes instead of a permanent failure.

use std::collections::VecDeque;

use super::HwCapabilities;
use super::PlaybackMethod;
use super::PlaybackPlan;
use super::transcode::HwAccel;

/// How a single session will produce HLS. A chain rung.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Remux (stream-copy into fMP4) vs. full re-encode. Derived
    /// from the plan for the primary rung; the fallback rung for
    /// a Remux plan is always `Transcode + HwAccel::None`.
    pub method: PlaybackMethod,
    /// Active hardware backend. `HwAccel::None` for software
    /// transcode + for remux (remux ignores the field).
    pub hw_accel: HwAccel,
    /// Which role this rung plays in the chain — informational;
    /// read by logs + the future `HealthWarning` emission on
    /// fallback ("fell back from HW to SW on session X").
    pub kind: ProfileKind,
}

/// Role of a chain rung. Purely informational — actual behavior
/// is driven by `Profile::method` + `Profile::hw_accel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// Hardware-accelerated re-encode. Primary rung for transcode
    /// plans on HW-capable hosts.
    HardwareTranscode,
    /// Stream-copy into fMP4 HLS (`-c:v copy -c:a copy`). Primary
    /// rung when only the container is incompatible.
    Remux,
    /// libx264 software re-encode. Terminal fallback — always the
    /// last rung of a non-empty chain.
    SoftwareTranscode,
}

/// Ordered list of profiles to try. Popped from the front on
/// failure.
#[derive(Debug, Clone)]
pub struct ProfileChain {
    rungs: VecDeque<Profile>,
}

impl ProfileChain {
    /// Build the chain for a playback plan + configured HW
    /// backend + probe result.
    ///
    /// See the module-level table for the per-plan chain shapes.
    /// The builder never produces an empty chain for a
    /// Remux / Transcode plan — software transcode is always
    /// appended as the terminal rung.
    #[must_use]
    pub fn build(plan: &PlaybackPlan, configured_hwa: &HwAccel, caps: &HwCapabilities) -> Self {
        let mut rungs: VecDeque<Profile> = VecDeque::new();
        match plan.method {
            PlaybackMethod::DirectPlay => {
                // No session needed — direct play serves raw bytes
                // via `/direct`, never goes through the transcode
                // manager. Empty chain is the correct signal.
            }
            PlaybackMethod::Remux => {
                rungs.push_back(Profile {
                    method: PlaybackMethod::Remux,
                    hw_accel: HwAccel::None,
                    kind: ProfileKind::Remux,
                });
                rungs.push_back(Profile {
                    method: PlaybackMethod::Transcode,
                    hw_accel: HwAccel::None,
                    kind: ProfileKind::SoftwareTranscode,
                });
            }
            PlaybackMethod::Transcode => {
                if let Some(backend) = configured_hwa.backend()
                    && caps.is_available(backend)
                {
                    rungs.push_back(Profile {
                        method: PlaybackMethod::Transcode,
                        hw_accel: configured_hwa.clone(),
                        kind: ProfileKind::HardwareTranscode,
                    });
                }
                rungs.push_back(Profile {
                    method: PlaybackMethod::Transcode,
                    hw_accel: HwAccel::None,
                    kind: ProfileKind::SoftwareTranscode,
                });
            }
        }
        Self { rungs }
    }

    /// The currently-active profile (the front of the chain).
    /// `None` only when the chain is empty (`DirectPlay` plan, or
    /// an exhausted chain after repeated failures).
    #[must_use]
    pub fn current(&self) -> Option<&Profile> {
        self.rungs.front()
    }

    /// Advance to the next rung. Returns the new current, or
    /// `None` if the chain is now exhausted. Callers surface
    /// exhaustion as a user-visible failure; we've run out of
    /// things to try.
    pub fn advance(&mut self) -> Option<&Profile> {
        self.rungs.pop_front();
        self.rungs.front()
    }

    /// Number of rungs remaining (including the current one).
    #[must_use]
    pub fn len(&self) -> usize {
        self.rungs.len()
    }

    /// `true` when no rungs remain. Holds for a `DirectPlay` plan
    /// and for a chain that's been fully popped via `advance`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rungs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::TranscodeReasons;
    use crate::playback::hw_probe::{BackendState, BackendStatus, HwBackend};

    fn plan(method: PlaybackMethod) -> PlaybackPlan {
        PlaybackPlan {
            method,
            transcode_reasons: TranscodeReasons::new(),
            selected_audio_stream: None,
            video_bitstream_filter: None,
            audio_passthrough: false,
        }
    }

    fn caps_with(available: &[HwBackend]) -> HwCapabilities {
        HwCapabilities {
            ffmpeg_ok: true,
            ffmpeg_version: Some("ffmpeg version 7.0".into()),
            ffmpeg_major: Some(7),
            is_jellyfin_build: false,
            has_libplacebo: false,
            has_libass: false,
            software_codecs: vec!["libx264".into(), "aac".into()],
            backends: HwBackend::all()
                .into_iter()
                .map(|b| BackendStatus {
                    backend: b,
                    state: if available.contains(&b) {
                        BackendState::Available {
                            driver_fingerprint: None,
                            device: None,
                        }
                    } else {
                        BackendState::NotCompiled
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn direct_play_yields_empty_chain() {
        let chain = ProfileChain::build(
            &plan(PlaybackMethod::DirectPlay),
            &HwAccel::None,
            &caps_with(&[]),
        );
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
        assert!(chain.current().is_none());
    }

    #[test]
    fn remux_chain_falls_back_to_sw_transcode() {
        let chain = ProfileChain::build(
            &plan(PlaybackMethod::Remux),
            &HwAccel::None,
            &caps_with(&[]),
        );
        assert_eq!(chain.len(), 2);
        let first = chain.current().unwrap();
        assert_eq!(first.kind, ProfileKind::Remux);
        assert!(matches!(first.method, PlaybackMethod::Remux));
        assert!(matches!(first.hw_accel, HwAccel::None));

        let mut chain = chain;
        let second = chain.advance().unwrap();
        assert_eq!(second.kind, ProfileKind::SoftwareTranscode);
        assert!(matches!(second.method, PlaybackMethod::Transcode));

        assert!(chain.advance().is_none());
    }

    #[test]
    fn transcode_with_available_hw_picks_hw_first() {
        let chain = ProfileChain::build(
            &plan(PlaybackMethod::Transcode),
            &HwAccel::Nvenc,
            &caps_with(&[HwBackend::Nvenc]),
        );
        assert_eq!(chain.len(), 2);
        let first = chain.current().unwrap();
        assert_eq!(first.kind, ProfileKind::HardwareTranscode);
        assert!(matches!(first.hw_accel, HwAccel::Nvenc));

        let mut chain = chain;
        let second = chain.advance().unwrap();
        assert_eq!(second.kind, ProfileKind::SoftwareTranscode);
        assert!(matches!(second.hw_accel, HwAccel::None));
    }

    #[test]
    fn transcode_skips_hw_rung_when_probe_says_unavailable() {
        // User configured NVENC but the probe found the driver
        // missing — the HW rung is filtered out entirely. Going
        // straight to SW is cheaper than burning a session on a
        // predictable failure.
        let chain = ProfileChain::build(
            &plan(PlaybackMethod::Transcode),
            &HwAccel::Nvenc,
            &caps_with(&[]),
        );
        assert_eq!(chain.len(), 1);
        let only = chain.current().unwrap();
        assert_eq!(only.kind, ProfileKind::SoftwareTranscode);
    }

    #[test]
    fn transcode_with_hwa_none_skips_hw_rung() {
        let chain = ProfileChain::build(
            &plan(PlaybackMethod::Transcode),
            &HwAccel::None,
            &caps_with(&[HwBackend::Nvenc]),
        );
        assert_eq!(chain.len(), 1);
        assert_eq!(
            chain.current().unwrap().kind,
            ProfileKind::SoftwareTranscode
        );
    }

    #[test]
    fn transcode_hwa_mismatched_with_probe_skips_hw() {
        // Configured AMF but only VAAPI is available — filter out
        // the unavailable rung.
        let chain = ProfileChain::build(
            &plan(PlaybackMethod::Transcode),
            &HwAccel::Amf,
            &caps_with(&[HwBackend::Vaapi]),
        );
        assert_eq!(chain.len(), 1);
        assert_eq!(
            chain.current().unwrap().kind,
            ProfileKind::SoftwareTranscode
        );
    }

    #[test]
    fn advance_on_empty_chain_stays_none() {
        let mut chain = ProfileChain::build(
            &plan(PlaybackMethod::DirectPlay),
            &HwAccel::None,
            &caps_with(&[]),
        );
        assert!(chain.advance().is_none());
        assert!(chain.advance().is_none());
    }
}
