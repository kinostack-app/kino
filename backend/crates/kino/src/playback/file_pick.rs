//! Torrent-file pickers shared by the old `stream_*` endpoints and
//! the unified `play` dispatcher. Extracted so one module owns the
//! "which file inside a season pack matches `SxxExx`" logic.
//!
//! The picker operates on `&[(file_idx, PathBuf, file_size)]` which
//! is the shape `TorrentSession::files` returns.

use std::path::{Path, PathBuf};

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "m4v", "avi", "ts", "mov", "mpg", "mpeg",
];

/// Pick the "main" playable video file from a torrent's file list.
/// Largest-video-by-bytes wins. Returns `(idx, path, size)`.
pub fn pick_largest(files: &[(usize, PathBuf, u64)]) -> Option<(usize, PathBuf, u64)> {
    files
        .iter()
        .filter(|(_, p, _)| is_video_path(p))
        .max_by_key(|(_, _, len)| *len)
        .cloned()
}

/// Pick the file that matches a given `(season, episode)`. Filenames
/// are matched against `SxxExx` / `NxNN` patterns. Among multiple
/// matches (rare; oddly-structured packs) the largest wins — favours
/// the main cut over sample/preview clips.
pub fn pick_episode(
    files: &[(usize, PathBuf, u64)],
    season: i64,
    episode: i64,
) -> Option<(usize, PathBuf, u64)> {
    files
        .iter()
        .filter(|(_, p, _)| is_video_path(p))
        .filter(|(_, p, _)| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(parse_episode_from_filename)
                .is_some_and(|(s, e)| s == season && e == episode)
        })
        .max_by_key(|(_, _, len)| *len)
        .cloned()
}

/// Number of plausibly-playable video files in a torrent file list.
/// Used by the episode picker's pack guard — when no `SxxExx` match
/// exists and the count is `> 1`, the picker refuses to guess rather
/// than streaming the largest (wrong) episode.
pub fn video_file_count(files: &[(usize, PathBuf, u64)]) -> usize {
    files.iter().filter(|(_, p, _)| is_video_path(p)).count()
}

pub fn is_video_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Parse a season + episode pair out of a torrent filename.
/// Handles both `SxxExx` (optional separators between S and E) and
/// `NxNN` season-cross-episode forms. Returns the first match —
/// for `Show.101.S01E01.mkv` that means `SxxExx` wins.
pub fn parse_episode_from_filename(name: &str) -> Option<(i64, i64)> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)(?:s(\d{1,4})[ ._\-]*e(\d{1,4})|\b(\d{1,2})x(\d{1,3})\b)")
            .expect("valid regex")
    });
    let caps = re.captures(name)?;
    let (s, e) = match (caps.get(1), caps.get(2), caps.get(3), caps.get(4)) {
        (Some(s), Some(e), _, _) | (_, _, Some(s), Some(e)) => (s.as_str(), e.as_str()),
        _ => return None,
    };
    Some((s.parse().ok()?, e.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sxxexx() {
        assert_eq!(
            parse_episode_from_filename("Better.Call.Saul.S01E01.1080p.mkv"),
            Some((1, 1))
        );
        assert_eq!(
            parse_episode_from_filename("show.s03e12.720p.mkv"),
            Some((3, 12))
        );
        assert_eq!(
            parse_episode_from_filename("Show.S01 E02.mkv"),
            Some((1, 2))
        );
    }

    #[test]
    fn parse_nxnn() {
        assert_eq!(parse_episode_from_filename("Show 1x02.mkv"), Some((1, 2)));
        assert_eq!(
            parse_episode_from_filename("Show.12x105.mkv"),
            Some((12, 105))
        );
    }

    #[test]
    fn parse_sxxexx_wins_over_nxnn() {
        assert_eq!(
            parse_episode_from_filename("Show.S01E01.1x02.mkv"),
            Some((1, 1))
        );
    }

    #[test]
    fn parse_no_match() {
        assert_eq!(parse_episode_from_filename("Some.Movie.2024.mkv"), None);
    }

    fn fake_files(names: &[(&str, u64)]) -> Vec<(usize, PathBuf, u64)> {
        names
            .iter()
            .enumerate()
            .map(|(i, (n, s))| (i, PathBuf::from(n), *s))
            .collect()
    }

    #[test]
    fn pick_episode_from_pack() {
        let files = fake_files(&[
            ("Show.S01E01.mkv", 800_000_000),
            ("Show.S01E02.mkv", 1_200_000_000),
            ("Show.S01E03.mkv", 900_000_000),
            ("readme.nfo", 1_024),
        ]);
        assert_eq!(
            pick_episode(&files, 1, 1).map(|(i, _, _)| i),
            Some(0),
            "pilot wins even though it isn't the largest file"
        );
    }

    #[test]
    fn pick_episode_falls_back_via_caller() {
        let files = fake_files(&[
            ("Show.S01E01.mkv", 800_000_000),
            ("Show.S01E02.mkv", 1_200_000_000),
        ]);
        assert!(pick_episode(&files, 2, 1).is_none());
        assert_eq!(pick_largest(&files).map(|(i, _, _)| i), Some(1));
    }

    #[test]
    fn pick_episode_ignores_non_video() {
        let files = fake_files(&[
            ("Show.S01E01.sample.txt", 1_024),
            ("Show.S01E01.mkv", 800_000_000),
        ]);
        assert_eq!(pick_episode(&files, 1, 1).map(|(i, _, _)| i), Some(1));
    }

    #[test]
    fn video_file_count_skips_non_video() {
        // The pack-guard in `pick_file_for_entity` reads this count to
        // decide whether the largest-fallback is safe. Sidecar files
        // (.nfo, .srt, .txt) must not inflate the count or every
        // single-video pack would refuse to play.
        let files = fake_files(&[
            ("Show.S01E01.mkv", 800_000_000),
            ("Show.S01E01.srt", 30_000),
            ("readme.nfo", 1_024),
            ("sample.txt", 256),
        ]);
        assert_eq!(video_file_count(&files), 1);
    }

    #[test]
    fn video_file_count_counts_packs() {
        // Bug #30: when `pick_episode` returned None for a season pack
        // with no SxxExx filenames, the old code fell through to
        // `pick_largest` and streamed the wrong episode. The pack
        // guard relies on this count to refuse instead of guessing.
        let files = fake_files(&[
            ("Show.S01.Disk1.mkv", 800_000_000), // pack with non-parseable names
            ("Show.S01.Disk2.mkv", 1_200_000_000),
            ("Show.S01.Disk3.mkv", 900_000_000),
        ]);
        assert_eq!(video_file_count(&files), 3);
        // None of the files have parseable SxxExx — the picker must
        // see this count and refuse to guess.
        assert!(pick_episode(&files, 1, 1).is_none());
    }
}
