//! Intro + credits detection (subsystem 15).
//!
//! Runs per-season fingerprint matching via Chromaprint to find the
//! shared audio segment across episodes (the theme song / outro).
//! For each pair of episodes we feed ~10 min of intro audio (or ~7
//! min of credits audio) into `Fingerprinter`, then let
//! `match_fingerprints` find the longest aligned region. Boundaries
//! are then refined to the nearest silence and keyframe so the Skip
//! button lands on a natural break rather than mid-dialogue.
//!
//! Storage:
//!   - Timing is persisted per-episode on the `episode` row
//!     (`intro_start_ms` / `intro_end_ms` / `credits_start_ms` /
//!     `credits_end_ms`). `intro_analysis_at` marks when analysis
//!     last ran — so NULL timings + NULL `intro_analysis_at` means
//!     "never tried", whereas NULL timings + a set
//!     `intro_analysis_at` means "ran, found nothing".
//!   - Raw Chromaprint fingerprints cache to disk under
//!     `{data_path}/fingerprints/{episode_id}-{mode}.bin`. Re-analysis
//!     reuses these so only the (dominant-cost) `FFmpeg` decode
//!     happens once per episode.
//!
//! Concurrency: analysis takes a permit from `state.media_processing_sem`,
//! the same budget `trickplay_gen` uses. Playback transcoding has its
//! own semaphore so it always takes priority.

use std::io::Read;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use rusty_chromaprint::{Configuration, Fingerprinter, match_fingerprints};
use sqlx::SqlitePool;
use tokio::process::Command;

use crate::state::AppState;

/// Which end of the episode we're looking at. Drives the audio
/// extraction window and which columns we write on success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Intro,
    Credits,
}

impl Mode {
    fn cache_suffix(self) -> &'static str {
        match self {
            Self::Intro => "intro",
            Self::Credits => "credits",
        }
    }
}

#[derive(Debug, Clone)]
struct Settings {
    ffmpeg_path: String,
    intro_enabled: bool,
    credits_enabled: bool,
    intro_analysis_limit_s: i64,
    credits_analysis_limit_s: i64,
    match_score_threshold: f64,
}

async fn load_settings(db: &SqlitePool) -> Result<Settings> {
    let row: (Option<String>, bool, bool, i64, i64, f64) = sqlx::query_as(
        "SELECT ffmpeg_path, intro_detect_enabled, credits_detect_enabled,
                intro_analysis_limit_s, credits_analysis_limit_s,
                intro_match_score_threshold
         FROM config WHERE id = 1",
    )
    .fetch_one(db)
    .await
    .context("load intro-skipper config")?;
    Ok(Settings {
        ffmpeg_path: row
            .0
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "ffmpeg".into()),
        intro_enabled: row.1,
        credits_enabled: row.2,
        intro_analysis_limit_s: row.3,
        credits_analysis_limit_s: row.4,
        match_score_threshold: row.5,
    })
}

/// Episode shape we need to drive analysis — plain struct, not the
/// full `Episode` model (we don't need most of it).
#[derive(Debug, Clone, sqlx::FromRow)]
struct EpSnap {
    id: i64,
    episode_number: i64,
    runtime: Option<i64>,
    file_path: Option<String>,
    runtime_ticks: Option<i64>,
}

/// Analyse one season. Pulls every imported episode in the season,
/// computes (or loads cached) fingerprints per mode, then walks
/// pairs to detect the shared intro/credits. On success, writes the
/// timings back to each episode row; `intro_analysis_at` is always
/// stamped so the daily catch-up knows the season has been tried.
pub async fn analyse_season(state: &AppState, show_id: i64, season_number: i64) -> Result<()> {
    let _permit = state
        .media_processing_sem
        .clone()
        .acquire_owned()
        .await
        .context("acquire media-processing permit")?;

    let settings = load_settings(&state.db).await?;
    if !settings.intro_enabled && !settings.credits_enabled {
        return Ok(());
    }

    // Join episode → media_episode → media so we get the on-disk path
    // for every imported episode in the season.
    let eps: Vec<EpSnap> = sqlx::query_as(
        "SELECT e.id, e.episode_number, e.runtime,
                m.file_path AS file_path,
                m.runtime_ticks AS runtime_ticks
         FROM episode e
         JOIN media_episode me ON me.episode_id = e.id
         JOIN media m ON m.id = me.media_id
         WHERE e.show_id = ? AND e.season_number = ?
         ORDER BY e.episode_number ASC",
    )
    .bind(show_id)
    .bind(season_number)
    .fetch_all(&state.db)
    .await
    .context("load episodes for intro analysis")?;

    if eps.len() < 2 {
        // Not enough episodes to pair — defer to catch-up once more arrive.
        tracing::debug!(
            show_id,
            season_number,
            count = eps.len(),
            "intro analysis skipped: season has <2 imported episodes"
        );
        return Ok(());
    }

    let fp_dir = state.data_path.join("fingerprints");
    tokio::fs::create_dir_all(&fp_dir)
        .await
        .context("create fingerprint cache dir")?;

    // Track which episodes successfully extracted (or already had
    // cached) fingerprints across both modes. Only those get their
    // `intro_analysis_at` stamped — transient ffmpeg failures leave
    // the stamp NULL so the daily catch-up retries.
    let mut fp_success: std::collections::HashSet<i64> = std::collections::HashSet::new();
    if settings.intro_enabled {
        analyse_mode(
            state,
            &settings,
            &fp_dir,
            &eps,
            Mode::Intro,
            &mut fp_success,
        )
        .await?;
    }
    if settings.credits_enabled {
        analyse_mode(
            state,
            &settings,
            &fp_dir,
            &eps,
            Mode::Credits,
            &mut fp_success,
        )
        .await?;
    }

    let now = crate::time::Timestamp::now().to_rfc3339();
    let mut stamped = 0usize;
    let mut deferred = 0usize;
    for ep in &eps {
        if !fp_success.contains(&ep.id) {
            deferred += 1;
            continue;
        }
        sqlx::query("UPDATE episode SET intro_analysis_at = ? WHERE id = ?")
            .bind(&now)
            .bind(ep.id)
            .execute(&state.db)
            .await
            .ok();
        stamped += 1;
    }
    tracing::info!(
        show_id,
        season_number,
        episodes = eps.len(),
        stamped,
        deferred,
        "intro analysis complete"
    );
    Ok(())
}

async fn analyse_mode(
    state: &AppState,
    settings: &Settings,
    fp_dir: &Path,
    eps: &[EpSnap],
    mode: Mode,
    fingerprint_success: &mut std::collections::HashSet<i64>,
) -> Result<()> {
    // 1. Ensure every episode has a fingerprint cached for this mode.
    for ep in eps {
        let Some(ref path) = ep.file_path else {
            continue;
        };
        let cache = fp_dir.join(format!("{}-{}.bin", ep.id, mode.cache_suffix()));
        if cache.exists() {
            // Cache hit — treat as success for stamping purposes.
            fingerprint_success.insert(ep.id);
            continue;
        }
        match compute_fingerprint(settings, Path::new(path), mode, ep.runtime_sec()).await {
            Ok(fp) => {
                if let Err(e) = write_fingerprint(&cache, &fp).await {
                    tracing::warn!(error = %e, episode_id = ep.id, "failed to cache fingerprint");
                    continue;
                }
                fingerprint_success.insert(ep.id);
            }
            Err(e) => {
                // Don't insert into `fingerprint_success` — a transient
                // ffmpeg failure should leave this episode's
                // `intro_analysis_at` NULL so the daily catch-up
                // retries it rather than permanently burying the
                // season on one decode error.
                tracing::warn!(
                    error = %e,
                    episode_id = ep.id,
                    mode = ?mode,
                    "fingerprint extraction failed — leaving intro_analysis_at NULL for retry"
                );
            }
        }
    }

    // 2. Pair each episode with its nearest neighbour by episode
    //    number. Try up to three alternative partners if the adjacent
    //    pair fails to produce a valid segment.
    let config = Configuration::preset_test2();
    for ep in eps {
        let Some(my_fp) = load_fingerprint(fp_dir, ep.id, mode).await else {
            continue;
        };

        // Pair candidate order: ±1, ±2, ±3, ±4.
        let mut candidates: Vec<&EpSnap> = eps.iter().filter(|o| o.id != ep.id).collect();
        candidates.sort_by_key(|o| (o.episode_number - ep.episode_number).abs());
        candidates.truncate(4);

        let mut best: Option<(i64, i64)> = None; // (start_ms, end_ms)
        for other in candidates {
            let Some(other_fp) = load_fingerprint(fp_dir, other.id, mode).await else {
                continue;
            };
            let Some((start_s, end_s)) = run_match(&config, &my_fp, &other_fp, mode, ep, settings)
            else {
                continue;
            };
            best = Some((secs_to_ms(start_s), secs_to_ms(end_s)));
            break;
        }

        let Some((raw_start_ms, raw_end_ms)) = best else {
            continue;
        };

        // Refine the END boundary (intro_end / credits_start) — silence
        // snap first, then keyframe snap. The START of the intro is
        // almost always t=0 (cold opens are rare) so refinement there
        // isn't worth an extra FFmpeg pass; similarly credits END is
        // usually the end of the file.
        let refined_end_ms = match mode {
            Mode::Intro => refine_boundary(settings, ep, raw_end_ms)
                .await
                .unwrap_or(raw_end_ms),
            Mode::Credits => refine_boundary(settings, ep, raw_start_ms)
                .await
                .unwrap_or(raw_start_ms),
        };

        match mode {
            Mode::Intro => {
                sqlx::query("UPDATE episode SET intro_start_ms = ?, intro_end_ms = ? WHERE id = ?")
                    .bind(raw_start_ms)
                    .bind(refined_end_ms)
                    .bind(ep.id)
                    .execute(&state.db)
                    .await
                    .ok();
            }
            Mode::Credits => {
                sqlx::query(
                    "UPDATE episode SET credits_start_ms = ?, credits_end_ms = ? WHERE id = ?",
                )
                .bind(refined_end_ms)
                .bind(raw_end_ms)
                .bind(ep.id)
                .execute(&state.db)
                .await
                .ok();
            }
        }
    }
    Ok(())
}

/// Run `match_fingerprints` and filter the returned segments down to
/// a valid intro / credits window. Returns (`start_sec`, `end_sec`)
/// relative to the episode's start-of-file; caller converts to ms.
///
/// The `u32 -> f64` casts below are safe: fingerprint offsets are
/// bounded by the 10-minute analysis window at 8 Hz (well under 2^52).
#[allow(clippy::cast_precision_loss)]
fn run_match(
    config: &Configuration,
    my_fp: &[u32],
    other_fp: &[u32],
    mode: Mode,
    ep: &EpSnap,
    settings: &Settings,
) -> Option<(f64, f64)> {
    // For credits we reverse both arrays so the "shared segment at
    // t=0 in both streams" model still applies. After matching, we
    // un-reverse the timestamps relative to episode_duration.
    let (a, b): (Vec<u32>, Vec<u32>) = match mode {
        Mode::Intro => (my_fp.to_vec(), other_fp.to_vec()),
        Mode::Credits => (
            my_fp.iter().copied().rev().collect(),
            other_fp.iter().copied().rev().collect(),
        ),
    };

    let segments = match_fingerprints(&a, &b, config).ok()?;
    let item_dur = config.item_duration_in_seconds();

    for seg in segments {
        if seg.score > settings.match_score_threshold {
            continue;
        }
        let seg_start = seg.offset1 as f64 * f64::from(item_dur);
        let seg_end = (seg.offset1 + seg.items_count) as f64 * f64::from(item_dur);
        let duration = seg_end - seg_start;
        if duration < 15.0 {
            continue;
        }

        let (start_s, end_s) = match mode {
            Mode::Intro => {
                if seg_start > 30.0 || duration > 120.0 {
                    continue;
                }
                (seg_start, seg_end)
            }
            Mode::Credits => {
                // Un-reverse: if the episode is 30 min and we matched
                // seg_start=0..seg_end=60 in the reversed stream, the
                // real segment is (30*60 - 60, 30*60 - 0).
                let Some(runtime_sec) = ep.runtime_sec() else {
                    continue;
                };
                // Runtime fits comfortably in f64's 52-bit mantissa
                // (max ~14 years of seconds).
                #[allow(clippy::cast_precision_loss)]
                let runtime_f = runtime_sec as f64;
                let real_start = runtime_f - seg_end;
                let real_end = runtime_f - seg_start;
                if real_start < 0.0 {
                    continue;
                }
                (real_start, real_end)
            }
        };
        return Some((start_s, end_s));
    }
    None
}

// ── Audio extraction + fingerprinting ─────────────────────────────

async fn compute_fingerprint(
    settings: &Settings,
    file: &Path,
    mode: Mode,
    runtime_sec: Option<i64>,
) -> Result<Vec<u32>> {
    let (start, duration) = audio_window(settings, mode, runtime_sec);
    let ffmpeg = settings.ffmpeg_path.clone();
    let file = file.to_path_buf();

    // Fingerprinter is !Send, so we keep both the FFmpeg pipe read
    // and the fingerprint consumption on a blocking thread. Gives us
    // back a Send Vec<u32> at the end.
    tokio::task::spawn_blocking(move || -> Result<Vec<u32>> {
        let mut child = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &start.to_string(),
                "-i",
            ])
            .arg(&file)
            .args([
                "-t",
                &duration.to_string(),
                "-ac",
                "2",
                "-ar",
                "44100",
                "-vn",
                "-sn",
                "-dn",
                "-f",
                "s16le",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn ffmpeg for audio extraction")?;

        let mut stdout = child.stdout.take().context("ffmpeg stdout missing")?;

        let config = Configuration::preset_test2();
        let mut fingerprinter = Fingerprinter::new(&config);
        fingerprinter
            .start(44100, 2)
            .context("fingerprinter start")?;

        // Carry over at most one byte across reads so odd-sized chunks
        // don't split an i16 sample. 64 KiB blocks are a compromise
        // between syscall overhead and memory churn.
        let mut buf = vec![0u8; 64 * 1024];
        let mut leftover: Option<u8> = None;
        loop {
            let n = stdout.read(&mut buf).context("read ffmpeg stdout")?;
            if n == 0 {
                break;
            }
            let mut bytes: &[u8] = &buf[..n];
            let mut tmp: Vec<u8> = Vec::new();
            if let Some(b) = leftover.take() {
                tmp.push(b);
                tmp.extend_from_slice(bytes);
                bytes = &tmp[..];
            }
            let even = bytes.len() & !1;
            let samples: Vec<i16> = bytes[..even]
                .chunks_exact(2)
                .map(|p| i16::from_le_bytes([p[0], p[1]]))
                .collect();
            fingerprinter.consume(&samples);
            if bytes.len() > even {
                leftover = Some(bytes[even]);
            }
        }

        // FFmpeg can exit non-zero with partial audio (corrupt file,
        // early EOF) but the fingerprint we got is still usable.
        let _ = child.wait();

        fingerprinter.finish();
        Ok(fingerprinter.fingerprint().to_vec())
    })
    .await
    .context("join blocking fingerprint task")?
}

fn audio_window(settings: &Settings, mode: Mode, runtime_sec: Option<i64>) -> (i64, i64) {
    match mode {
        Mode::Intro => {
            // min(intro_limit, runtime / 4) — never analyse more than
            // the first quarter of the episode (dialogue-rich content
            // produces spurious matches).
            let limit = runtime_sec.map_or(settings.intro_analysis_limit_s, |r| {
                (r / 4).min(settings.intro_analysis_limit_s)
            });
            (0, limit.max(30))
        }
        Mode::Credits => {
            let r = runtime_sec.unwrap_or(60 * 60);
            let limit = settings.credits_analysis_limit_s;
            ((r - limit).max(0), limit)
        }
    }
}

async fn write_fingerprint(path: &Path, fp: &[u32]) -> Result<()> {
    let mut bytes = Vec::with_capacity(fp.len() * 4);
    for v in fp {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    tokio::fs::write(path, &bytes)
        .await
        .context("write fingerprint cache")
}

async fn load_fingerprint(fp_dir: &Path, episode_id: i64, mode: Mode) -> Option<Vec<u32>> {
    let path = fp_dir.join(format!("{}-{}.bin", episode_id, mode.cache_suffix()));
    let raw = tokio::fs::read(&path).await.ok()?;
    if raw.len() % 4 != 0 {
        return None;
    }
    let fp: Vec<u32> = raw
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Some(fp)
}

// ── Refinement ────────────────────────────────────────────────────

/// Run `silencedetect` in a ±5s window around the raw boundary and
/// snap to the nearest silence ≥ 0.33s. Then run `showinfo` on key-
/// frames only and snap to the nearest keyframe so the player's seek
/// lands cleanly. Both passes are best-effort — any `FFmpeg` failure
/// returns `None` and the caller keeps the raw boundary.
async fn refine_boundary(settings: &Settings, ep: &EpSnap, raw_ms: i64) -> Option<i64> {
    let file = ep.file_path.as_deref()?;
    let window_start = ((raw_ms / 1000) - 5).max(0);
    let after_silence = silence_snap(settings, file, window_start, 10, raw_ms).await;
    let target = after_silence.unwrap_or(raw_ms);
    keyframe_snap(settings, file, window_start, 10, target)
        .await
        .or(Some(target))
}

async fn silence_snap(
    settings: &Settings,
    file: &str,
    window_start_s: i64,
    window_len_s: i64,
    raw_ms: i64,
) -> Option<i64> {
    let out = Command::new(&settings.ffmpeg_path)
        .args([
            "-hide_banner",
            "-nostats",
            "-ss",
            &window_start_s.to_string(),
            "-i",
            file,
            "-t",
            &window_len_s.to_string(),
            "-vn",
            "-sn",
            "-dn",
            "-af",
            "silencedetect=noise=-50dB:duration=0.33",
            "-f",
            "null",
            "-",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Collect `silence_end: <secs>` markers in the window.
    let win_base_ms = window_start_s.saturating_mul(1000);
    let mut best: Option<(i64, i64)> = None;
    for line in stderr.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("[silencedetect")
            && let Some(idx) = rest.find("silence_end:")
        {
            let tail = &rest[idx + "silence_end:".len()..];
            let tok = tail.split_whitespace().next()?;
            if let Ok(secs) = tok.parse::<f64>() {
                let abs_ms = win_base_ms + secs_to_ms(secs);
                let dist = (abs_ms - raw_ms).abs();
                if best.is_none_or(|(_, d)| dist < d) {
                    best = Some((abs_ms, dist));
                }
            }
        }
    }
    best.map(|(ms, _)| ms)
}

async fn keyframe_snap(
    settings: &Settings,
    file: &str,
    window_start_s: i64,
    window_len_s: i64,
    target_ms: i64,
) -> Option<i64> {
    let out = Command::new(&settings.ffmpeg_path)
        .args([
            "-hide_banner",
            "-nostats",
            "-skip_frame",
            "nokey",
            "-ss",
            &window_start_s.to_string(),
            "-i",
            file,
            "-t",
            &window_len_s.to_string(),
            "-an",
            "-dn",
            "-sn",
            "-vf",
            "showinfo",
            "-f",
            "null",
            "-",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let win_base_ms = window_start_s.saturating_mul(1000);
    let mut best: Option<(i64, i64)> = None;
    for line in stderr.lines() {
        if let Some(idx) = line.find("pts_time:") {
            let tail = &line[idx + "pts_time:".len()..];
            let tok = tail.split_whitespace().next()?;
            if let Ok(secs) = tok.parse::<f64>() {
                let abs_ms = win_base_ms + secs_to_ms(secs);
                let dist = (abs_ms - target_ms).abs();
                if best.is_none_or(|(_, d)| dist < d) {
                    best = Some((abs_ms, dist));
                }
            }
        }
    }
    best.map(|(ms, _)| ms)
}

/// Convert seconds-as-f64 to milliseconds-as-i64 with explicit
/// saturating semantics so clippy doesn't complain about a silent
/// truncation. In practice `secs` is always bounded by an episode
/// runtime (a few thousand seconds), well within i64 range.
/// 2^53 — the largest exactly-representable integer in f64. That's
/// ~285,000 years of milliseconds, so clamping before casting keeps
/// `secs_to_ms` lossless for any realistic episode timecode.
const MAX_EXACT_F64_MS: f64 = 9_007_199_254_740_992.0;

fn secs_to_ms(secs: f64) -> i64 {
    let ms = (secs * 1000.0).round();
    if !ms.is_finite() {
        return 0;
    }
    let clamped = ms.clamp(-MAX_EXACT_F64_MS, MAX_EXACT_F64_MS);
    // Safe: within the exactly-representable range above.
    #[allow(clippy::cast_possible_truncation)]
    {
        clamped as i64
    }
}

impl EpSnap {
    fn runtime_sec(&self) -> Option<i64> {
        // Prefer the actual media runtime (libvlc-ticks, 100ns units)
        // because the TMDB `runtime` field on the episode is often the
        // network-intended slot length, rounded up. Fall back to the
        // TMDB value in minutes when we haven't probed yet.
        if let Some(ticks) = self.runtime_ticks
            && ticks > 0
        {
            return Some(ticks / 10_000_000);
        }
        self.runtime.map(|m| m * 60)
    }
}

// ── Scheduler catch-up sweep ─────────────────────────────────────

/// Daily sweep: find (`show_id`, `season_number`) pairs where at least
/// one imported episode still has `intro_analysis_at IS NULL` and
/// trigger a fresh `analyse_season` for each. Catches the first-
/// episode-of-a-season race + episodes that arrived before the
/// feature was enabled.
pub async fn catch_up_sweep(state: &AppState) -> Result<()> {
    let seasons: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT DISTINCT e.show_id, e.season_number
         FROM episode e
         JOIN media_episode me ON me.episode_id = e.id
         WHERE e.intro_analysis_at IS NULL
         ORDER BY e.show_id, e.season_number",
    )
    .fetch_all(&state.db)
    .await?;
    for (show_id, season_number) in seasons {
        if let Err(e) = analyse_season(state, show_id, season_number).await {
            tracing::warn!(
                error = %e,
                show_id,
                season_number,
                "intro catch-up analysis failed"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_window_intro_caps_at_quarter_runtime() {
        let s = Settings {
            ffmpeg_path: "ffmpeg".into(),
            intro_enabled: true,
            credits_enabled: true,
            intro_analysis_limit_s: 600,
            credits_analysis_limit_s: 450,
            match_score_threshold: 10.0,
        };
        // Short 20-minute episode: quarter = 300s < 600s limit.
        let (start, dur) = audio_window(&s, Mode::Intro, Some(20 * 60));
        assert_eq!(start, 0);
        assert_eq!(dur, 300);
    }

    #[test]
    fn audio_window_intro_caps_at_limit() {
        let s = Settings {
            ffmpeg_path: "ffmpeg".into(),
            intro_enabled: true,
            credits_enabled: true,
            intro_analysis_limit_s: 600,
            credits_analysis_limit_s: 450,
            match_score_threshold: 10.0,
        };
        // 2-hour movie-length episode: quarter = 1800s but limit = 600s.
        let (start, dur) = audio_window(&s, Mode::Intro, Some(2 * 60 * 60));
        assert_eq!(start, 0);
        assert_eq!(dur, 600);
    }

    #[test]
    fn audio_window_credits_offsets_from_end() {
        let s = Settings {
            ffmpeg_path: "ffmpeg".into(),
            intro_enabled: true,
            credits_enabled: true,
            intro_analysis_limit_s: 600,
            credits_analysis_limit_s: 450,
            match_score_threshold: 10.0,
        };
        let (start, dur) = audio_window(&s, Mode::Credits, Some(30 * 60));
        assert_eq!(start, 30 * 60 - 450);
        assert_eq!(dur, 450);
    }

    #[test]
    fn audio_window_credits_clamps_when_episode_too_short() {
        let s = Settings {
            ffmpeg_path: "ffmpeg".into(),
            intro_enabled: true,
            credits_enabled: true,
            intro_analysis_limit_s: 600,
            credits_analysis_limit_s: 450,
            match_score_threshold: 10.0,
        };
        // 5-minute episode — start can't go negative.
        let (start, _) = audio_window(&s, Mode::Credits, Some(5 * 60));
        assert_eq!(start, 0);
    }
}
