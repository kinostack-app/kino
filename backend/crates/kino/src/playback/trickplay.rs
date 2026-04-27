//! Trickplay thumbnail generation.
//!
//! Extracts one frame every 10s (by default) from a video, composites
//! them into 10×10 sprite sheets via `FFmpeg`'s `tile` filter, and writes
//! a `WebVTT` cue file with `xywh` fragments so a player can render hover
//! previews on the seek bar.
//!
//! Output layout (per media id):
//! ```text
//!   data/trickplay/{media_id}/
//!     trickplay.vtt
//!     sprite_001.jpg
//!     sprite_002.jpg
//!     ...
//! ```
//!
//! The VTT emitted here uses relative sprite names; the HTTP handler
//! rewrites them to full API URLs before serving.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

/// Generation parameters.
#[derive(Debug, Clone)]
pub struct Params {
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    /// Frame every N seconds. Default 10.
    pub interval_secs: u32,
    /// Thumbnail width in pixels. Default 320 (height auto-scales
    /// to the source aspect). The default used to be 160 — fine for
    /// the seek-bar hover preview where a tile renders 1:1, but the
    /// resume dialog upscales to ~440px-wide which was visibly soft.
    /// 320 × 180 is Plex's default and matches roughly the native
    /// pixel density of every surface we render it in. A 2h film
    /// lands around 3-4 MB of JPEG on disk — unnoticeable against
    /// the media itself.
    pub thumb_width: u32,
    /// Tile grid size (e.g. 10 → 10×10 = 100 thumbs per sheet).
    pub tile_size: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            ffmpeg_path: "ffmpeg".into(),
            ffprobe_path: "ffprobe".into(),
            interval_secs: 10,
            thumb_width: 320,
            tile_size: 10,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TrickplayError {
    #[error("probe: {0}")]
    Probe(String),
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    #[error("io: {0}")]
    Io(String),
    #[error("video too short ({0}s)")]
    TooShort(f64),
}

impl TrickplayError {
    /// A *permanent* error is one where re-running the same
    /// pipeline against the same file will always fail the same
    /// way — today that's `TooShort` (the file is shorter than
    /// two thumbnail intervals and no ffmpeg invocation can fix
    /// that). The sweep marks these `trickplay_generated = 1`
    /// immediately so we stop retrying.
    ///
    /// Everything else (`Probe`, `Ffmpeg`, `Io`) is classified
    /// transient: a disk blip, a momentarily-missing ffmpeg, an
    /// OOM, a corrupt probe against an in-flight write. The
    /// sweep resets the row to `trickplay_generated = 0` so the
    /// next tick can try again, capped by `trickplay_attempts`
    /// so a genuinely-broken file doesn't loop forever.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::TooShort(_))
    }
}

/// Software tone-map chain for HDR → SDR thumbnail rendering.
/// Same shape as `transcode::TONEMAP_HABLE_SW` but terminated
/// at yuv420p — the subsequent tile/scale/jpeg encoder consumes
/// that directly. Without this stage, an HDR10 / HLG / DV source
/// produces washed-out or purple thumbnails because the PQ / HLG
/// transfer curve gets interpreted as BT.709.
///
/// This is the CPU fallback — when the active ffmpeg build
/// carries libplacebo (jellyfin-ffmpeg, or a vanilla build
/// configured with `--enable-libplacebo`), `TONEMAP_LIBPLACEBO`
/// below is selected instead for the same speedup the transcode
/// path gets.
const TONEMAP_HABLE_SW: &str = concat!(
    "zscale=t=linear:npl=100,",
    "format=gbrpf32le,",
    "zscale=p=bt709,",
    "tonemap=tonemap=hable:desat=0,",
    "zscale=t=bt709:m=bt709:r=tv,",
    "format=yuv420p"
);

/// libplacebo tone-map for HDR → SDR thumbnails. Single filter
/// that replaces the five-stage CPU chain and (on capable hosts)
/// runs on the GPU. Output is pinned to BT.709 / tv-range /
/// yuv420p so the downstream tile/jpeg stages see a consistent
/// SDR surface regardless of source metadata fidelity.
///
/// Selected by `tonemap_filter()` when `hw_probe::cached()`
/// reports `has_libplacebo`; otherwise `TONEMAP_HABLE_SW` is
/// used. Mirrors `transcode::TONEMAP_LIBPLACEBO` so the two
/// paths stay in lockstep.
const TONEMAP_LIBPLACEBO: &str = "libplacebo=tonemapping=hable:colorspace=bt709:\
     color_primaries=bt709:color_trc=bt709:range=tv:format=yuv420p";

/// Pick the right tone-map filter for the current ffmpeg build.
/// `has_libplacebo` is read from the cached HW probe (populated
/// at startup + after an ffmpeg bundle swap), so the decision
/// tracks jellyfin-ffmpeg installs / removals without needing
/// a re-probe on every trickplay generation.
fn tonemap_filter() -> &'static str {
    if crate::playback::hw_probe_cache::cached()
        .as_deref()
        .is_some_and(|c| c.has_libplacebo)
    {
        TONEMAP_LIBPLACEBO
    } else {
        TONEMAP_HABLE_SW
    }
}

/// Probe the primary video stream's `color_transfer` to decide
/// whether to prepend the tone-map stage. PQ (`smpte2084`) +
/// HLG (`arib-std-b67`) are the two transfer functions that
/// would render wrong without it.
fn needs_tonemap(probe: &crate::import::ffprobe::ProbeResult) -> bool {
    probe
        .streams
        .as_ref()
        .and_then(|streams| {
            streams
                .iter()
                .find(|s| s.codec_type.as_deref() == Some("video"))
        })
        .and_then(|s| s.color_transfer.as_deref())
        .is_some_and(|t| {
            let lc = t.to_ascii_lowercase();
            lc == "smpte2084" || lc == "arib-std-b67"
        })
}

/// Generate sprite sheets + VTT for `input` into `output_dir`.
/// Returns the number of sprite sheets written.
pub async fn generate(
    input: &Path,
    output_dir: &Path,
    params: &Params,
) -> Result<u32, TrickplayError> {
    // One probe, two uses: duration check + HDR detection for
    // the tone-map branch. Avoids a second ffprobe subprocess
    // for the same file.
    let probe = crate::import::ffprobe::probe(input, &params.ffprobe_path)
        .map_err(|e| TrickplayError::Probe(e.to_string()))?;
    let duration = probe
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .ok_or_else(|| TrickplayError::Probe("no duration in probe".into()))?;

    // Don't bother with very short clips — one frame is useless.
    #[allow(clippy::cast_precision_loss)]
    let min_duration = f64::from(params.interval_secs * 2);
    if duration < min_duration {
        return Err(TrickplayError::TooShort(duration));
    }

    tokio::fs::create_dir_all(output_dir)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;

    // FFmpeg filter graph. Base shape is
    // `fps=1/N,scale=W:-1,tile=NxN`. For HDR sources we
    // prepend a tone-map stage so thumbs render in BT.709
    // instead of squashing PQ / HLG into SDR space. The
    // filter choice (libplacebo vs. SW zscale chain) mirrors
    // the transcode path's `use_libplacebo` gating so both
    // pipelines track the active ffmpeg build together.
    let sprite_pattern = output_dir.join("sprite_%03d.jpg");
    let base_chain = format!(
        "fps=1/{},scale={}:-1,tile={}x{}",
        params.interval_secs, params.thumb_width, params.tile_size, params.tile_size
    );
    let vf = if needs_tonemap(&probe) {
        let tonemap = tonemap_filter();
        tracing::debug!(
            input = %input.display(),
            use_libplacebo = std::ptr::eq(tonemap, TONEMAP_LIBPLACEBO),
            "trickplay: HDR source detected, prepending tonemap stage"
        );
        format!("{tonemap},{base_chain}")
    } else {
        base_chain
    };

    // `-threads 2` caps ffmpeg at 2 cores. Trickplay is background
    // work — without the cap a 4K H.265 decode fans out across every
    // core and dominates the machine while the user is trying to
    // watch something on the same host.
    let status = Command::new(&params.ffmpeg_path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-threads",
            "2",
            "-i",
        ])
        .arg(input)
        .args(["-vf", &vf, "-vsync", "vfr", "-qscale:v", "5"])
        .arg(&sprite_pattern)
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| TrickplayError::Ffmpeg(format!("spawn: {e}")))?;
    if !status.success() {
        return Err(TrickplayError::Ffmpeg(format!("exit {status}")));
    }

    // Count how many sprite sheets ended up on disk.
    let mut sheets = 0u32;
    let mut read = tokio::fs::read_dir(output_dir)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;
    while let Ok(Some(entry)) = read.next_entry().await {
        let path = entry.path();
        let is_sprite_jpeg = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("sprite_"))
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg"));
        if is_sprite_jpeg {
            sheets += 1;
        }
    }
    if sheets == 0 {
        return Err(TrickplayError::Ffmpeg("no sprites produced".into()));
    }

    // Read one sprite to learn exact thumb height (FFmpeg scale=w:-1 keeps
    // aspect ratio, so we need to derive h from the actual image).
    let first_sprite = output_dir.join("sprite_001.jpg");
    let sample = tokio::fs::read(&first_sprite)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;
    let (sheet_w, sheet_h) = probe_jpeg_size(&sample)
        .ok_or_else(|| TrickplayError::Ffmpeg("could not parse sprite JPEG".into()))?;
    let thumb_w = sheet_w / params.tile_size;
    let thumb_h = sheet_h / params.tile_size;

    // Write VTT.
    write_vtt(output_dir, duration, params, sheets, thumb_w, thumb_h).await?;

    Ok(sheets)
}

/// Minimal JPEG size reader — walks SOF segments. Enough for our `FFmpeg`
/// output; avoids pulling `image` just for dimensions.
fn probe_jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 4 < bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        // SOF0..SOF3, SOF5..SOF7, etc.: 0xC0..0xCF except 0xC4, 0xC8, 0xCC.
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            if i + 9 >= bytes.len() {
                return None;
            }
            let h = u32::from(bytes[i + 5]) << 8 | u32::from(bytes[i + 6]);
            let w = u32::from(bytes[i + 7]) << 8 | u32::from(bytes[i + 8]);
            return Some((w, h));
        }
        // skip this segment: length = big-endian u16 at i+2
        let seg_len = (u32::from(bytes[i + 2]) << 8 | u32::from(bytes[i + 3])) as usize;
        i += 2 + seg_len;
    }
    None
}

/// Dimensions (in pixels) of a single thumbnail cell inside a sheet.
/// Returned by [`generate_sheet`] so callers can write matching VTT
/// cues without re-probing the sprite JPEG.
#[derive(Debug, Clone, Copy)]
pub struct SheetDims {
    pub thumb_w: u32,
    pub thumb_h: u32,
}

/// Generate a single sprite sheet covering `[start_sec, start_sec + span_sec]`
/// of `input` into `output_dir/sprite_{sheet_idx+1:03}.jpg`. Used by the
/// streaming trickplay task to incrementally seal sheets as coverage
/// grows — only the currently-growing sheet is regenerated each tick,
/// sealed sheets are written once and left alone.
///
/// `input` may be an HTTP URL or a filesystem path. For HTTP inputs,
/// `-ss > 0` requires the container's seek index to be readable — MKV
/// stores cues at the end of the file, so incremental seek only works
/// reliably once the file is fully downloaded (post-import).
///
/// HDR tone-mapping note: this path deliberately does *not*
/// probe + tonemap. The streaming trickplay task runs on a
/// mid-download file where ffprobe is unreliable, and the user
/// sees these sheets for minutes at most before the post-import
/// `generate` path runs (which *does* tonemap) and replaces
/// them with correct-colour versions. Washed-out thumbnails
/// during a live torrent download are an acceptable trade for
/// not gating the streaming UX on an HDR probe that might
/// fail against the partial file.
pub async fn generate_sheet(
    input: &str,
    sheet_idx: u32,
    start_sec: f64,
    span_sec: f64,
    output_dir: &Path,
    params: &Params,
) -> Result<SheetDims, TrickplayError> {
    tokio::fs::create_dir_all(output_dir)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;

    let sprite_path = output_dir.join(format!("sprite_{:03}.jpg", sheet_idx + 1));
    let vf = format!(
        "fps=1/{},scale={}:-1,tile={}x{}",
        params.interval_secs, params.thumb_width, params.tile_size, params.tile_size
    );

    let output = Command::new(&params.ffmpeg_path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-threads",
            "2",
            "-ss",
        ])
        .arg(format!("{start_sec}"))
        .arg("-t")
        .arg(format!("{span_sec}"))
        .args(["-i", input])
        .args(["-vf", &vf, "-vsync", "vfr", "-qscale:v", "5"])
        .arg(&sprite_path)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| TrickplayError::Ffmpeg(format!("spawn: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(6).collect::<Vec<_>>().join(" | ");
        return Err(TrickplayError::Ffmpeg(format!(
            "sheet {sheet_idx}: exit {} — {tail}",
            output.status
        )));
    }

    let bytes = tokio::fs::read(&sprite_path)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;
    let (sheet_w, sheet_h) = probe_jpeg_size(&bytes)
        .ok_or_else(|| TrickplayError::Ffmpeg("could not parse sprite JPEG".into()))?;
    Ok(SheetDims {
        thumb_w: sheet_w / params.tile_size,
        thumb_h: sheet_h / params.tile_size,
    })
}

/// Write `trickplay.vtt` into `output_dir` covering `[0, duration]`,
/// referencing the first `sheets` sprite files. Public so the streaming
/// trickplay task can re-emit the VTT after each incremental sheet
/// lands without touching ffmpeg.
pub async fn write_vtt(
    output_dir: &Path,
    duration: f64,
    params: &Params,
    sheets: u32,
    thumb_w: u32,
    thumb_h: u32,
) -> Result<(), TrickplayError> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let total_frames = (duration / f64::from(params.interval_secs)).floor() as u32;
    let per_sheet = params.tile_size * params.tile_size;

    let mut vtt = String::with_capacity(total_frames as usize * 80 + 16);
    vtt.push_str("WEBVTT\n\n");

    for frame in 0..total_frames {
        let t_start = Duration::from_secs(u64::from(frame) * u64::from(params.interval_secs));
        let t_end = Duration::from_secs(u64::from(frame + 1) * u64::from(params.interval_secs));
        let sheet_idx = frame / per_sheet;
        let in_sheet = frame % per_sheet;
        let col = in_sheet % params.tile_size;
        let row = in_sheet / params.tile_size;
        let x = col * thumb_w;
        let y = row * thumb_h;
        let sheet_num = sheet_idx + 1;
        if sheet_num > sheets {
            break;
        }

        let _ = writeln!(vtt, "{} --> {}", format_time(t_start), format_time(t_end));
        let _ = writeln!(
            vtt,
            "sprite_{sheet_num:03}.jpg#xywh={x},{y},{thumb_w},{thumb_h}"
        );
        vtt.push('\n');
    }

    tokio::fs::write(output_dir.join("trickplay.vtt"), vtt)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;
    Ok(())
}

fn format_time(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    let ms = d.subsec_millis();
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// Path to a media item's trickplay dir inside `data_path`.
pub fn trickplay_dir(data_path: &Path, media_id: i64) -> PathBuf {
    data_path.join("trickplay").join(media_id.to_string())
}

/// Remove a media item's trickplay directory + sprites + VTT.
/// Called on media deletion (both the user-initiated
/// `DELETE /api/v1/media/{id}` path and the watched-cleanup
/// sweep) so the ~4–10 MB of sprite JPEGs per imported movie /
/// episode doesn't linger past the media's lifetime. Missing
/// directory is not an error — trickplay may never have been
/// generated for this media (generation failures, user-disabled
/// setting, pre-generation delete).
pub async fn clear_trickplay_dir(data_path: &Path, media_id: i64) -> std::io::Result<()> {
    let dir = trickplay_dir(data_path, media_id);
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Path to a streaming download's trickplay dir. Separate from
/// `trickplay/` so cleanup-on-cancel doesn't risk blowing away
/// imported-media trickplay, and so the regular sweep doesn't pick
/// up partial runs.
pub fn trickplay_stream_dir(data_path: &Path, download_id: i64) -> PathBuf {
    data_path
        .join("trickplay-stream")
        .join(download_id.to_string())
}

/// Generate sprite sheets + VTT for the `[0, duration_sec]` window
/// of a (potentially partial) video served over HTTP. Regenerates
/// wholesale — caller is expected to re-run as coverage grows.
///
/// Unlike `generate()` which probes the file for duration, this takes
/// the target window as input. The caller decides how much of the
/// file is safe to sample based on download progress.
pub async fn generate_partial(
    input_url: &str,
    duration_sec: f64,
    output_dir: &Path,
    params: &Params,
) -> Result<u32, TrickplayError> {
    #[allow(clippy::cast_precision_loss)]
    let min_duration = f64::from(params.interval_secs * 2);
    if duration_sec < min_duration {
        return Err(TrickplayError::TooShort(duration_sec));
    }

    // DO NOT wipe the dir. Sprite names (`sprite_NNN.jpg`) are
    // deterministic from the `-ss 0 -t N` layout, so successive
    // runs with growing N overwrite earlier sprites in place with
    // equivalent-or-fuller content. Wiping would leave the dir
    // empty during the 30–90s ffmpeg run on a live stream, which
    // is when the user is most likely hovering the seek bar.
    tokio::fs::create_dir_all(output_dir)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;

    let sprite_pattern = output_dir.join("sprite_%03d.jpg");
    let vf = format!(
        "fps=1/{},scale={}:-1,tile={}x{}",
        params.interval_secs, params.thumb_width, params.tile_size, params.tile_size
    );

    // `-ss 0 -t duration_sec` keeps ffmpeg bounded to the covered
    // window. `-nostdin` avoids ffmpeg consuming the parent's stdin
    // when run as a child task. The URL input uses libavformat's
    // HTTP demuxer, which sends Range requests — librqbit's FileStream
    // prioritises pieces on demand, so this pulls only what it needs.
    // `-skip_frame nokey` makes the decoder only output keyframes
    // (I-frames) — non-keyframe packets are dropped before decode.
    // On a long stream that's a 20–50× speedup: for H.264/H.265 at
    // a typical 2–5s GOP only one in 50+ frames needs decoding, and
    // `fps=1/10` then picks the closest keyframe to each 10s mark.
    // `-an` drops the audio stream entirely — the trickplay doesn't
    // need it and demuxing it off a slow HTTP source is wasted work.
    let output = Command::new(&params.ffmpeg_path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-threads",
            "2",
            "-skip_frame",
            "nokey",
            "-an",
            "-ss",
            "0",
            "-t",
        ])
        .arg(format!("{duration_sec}"))
        .args(["-i", input_url])
        .args(["-vf", &vf, "-vsync", "vfr", "-qscale:v", "5"])
        .arg(&sprite_pattern)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| TrickplayError::Ffmpeg(format!("spawn: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(6).collect::<Vec<_>>().join(" | ");
        return Err(TrickplayError::Ffmpeg(format!(
            "exit {} — {}",
            output.status, tail
        )));
    }

    let mut sheets = 0u32;
    let mut read = tokio::fs::read_dir(output_dir)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;
    while let Ok(Some(entry)) = read.next_entry().await {
        let path = entry.path();
        let is_sprite = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("sprite_"))
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg"));
        if is_sprite {
            sheets += 1;
        }
    }
    if sheets == 0 {
        return Err(TrickplayError::Ffmpeg("no sprites produced".into()));
    }

    let first_sprite = output_dir.join("sprite_001.jpg");
    let sample = tokio::fs::read(&first_sprite)
        .await
        .map_err(|e| TrickplayError::Io(e.to_string()))?;
    let (sheet_w, sheet_h) = probe_jpeg_size(&sample)
        .ok_or_else(|| TrickplayError::Ffmpeg("could not parse sprite JPEG".into()))?;
    let thumb_w = sheet_w / params.tile_size;
    let thumb_h = sheet_h / params.tile_size;

    write_vtt(output_dir, duration_sec, params, sheets, thumb_w, thumb_h).await?;
    Ok(sheets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_pads_correctly() {
        assert_eq!(format_time(Duration::from_secs(0)), "00:00:00.000");
        assert_eq!(format_time(Duration::from_millis(12_345)), "00:00:12.345");
        assert_eq!(
            format_time(Duration::from_secs(3725) + Duration::from_millis(7)),
            "01:02:05.007"
        );
    }

    #[test]
    fn jpeg_size_reads_soi_then_sof() {
        // Minimal JPEG: SOI, SOF0 (width=320, height=180), EOI
        let mut buf = vec![0xFF, 0xD8];
        buf.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xB4, 0x01, 0x40]);
        buf.extend(std::iter::repeat_n(0, 8));
        assert_eq!(probe_jpeg_size(&buf), Some((320, 180)));
    }

    #[test]
    fn jpeg_size_rejects_non_jpeg() {
        assert_eq!(probe_jpeg_size(&[0, 1, 2, 3]), None);
    }

    // ── HDR detection for trickplay ─────────────────────────

    use crate::import::ffprobe::{ProbeFormat, ProbeResult, ProbeStream};

    fn probe_with_transfer(codec_type: &str, transfer: Option<&str>) -> ProbeResult {
        ProbeResult {
            streams: Some(vec![ProbeStream {
                index: 0,
                codec_type: Some(codec_type.into()),
                codec_name: Some("hevc".into()),
                profile: None,
                width: Some(3840),
                height: Some(2160),
                r_frame_rate: None,
                pix_fmt: None,
                bits_per_raw_sample: Some("10".into()),
                color_space: None,
                color_transfer: transfer.map(str::to_owned),
                color_primaries: Some("bt2020".into()),
                channels: None,
                channel_layout: None,
                sample_rate: None,
                bit_rate: None,
                tags: None,
                disposition: None,
                side_data_list: None,
            }]),
            format: Some(ProbeFormat {
                duration: Some("120.0".into()),
                size: None,
                bit_rate: None,
                format_name: None,
            }),
            chapters: None,
        }
    }

    #[test]
    fn hdr10_pq_source_needs_tonemap() {
        assert!(needs_tonemap(&probe_with_transfer(
            "video",
            Some("smpte2084")
        )));
    }

    #[test]
    fn hlg_source_needs_tonemap() {
        assert!(needs_tonemap(&probe_with_transfer(
            "video",
            Some("arib-std-b67")
        )));
    }

    #[test]
    fn sdr_source_does_not_need_tonemap() {
        assert!(!needs_tonemap(&probe_with_transfer("video", Some("bt709"))));
    }

    #[test]
    fn missing_color_transfer_does_not_need_tonemap() {
        // Defensive — no transfer info means we can't be sure;
        // don't tonemap rather than corrupt an SDR source.
        assert!(!needs_tonemap(&probe_with_transfer("video", None)));
    }

    #[test]
    fn missing_video_stream_does_not_need_tonemap() {
        // Audio-only file or probe with no streams — no tonemap
        // path applies (generate would fail out at sprite step
        // anyway; this just keeps the pure function honest).
        let probe = probe_with_transfer("audio", Some("smpte2084"));
        assert!(!needs_tonemap(&probe));
    }
}
