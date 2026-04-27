mod tokens;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use tokens::{TokenKind, classify_token};
use utoipa::ToSchema;

/// Structured metadata extracted from a release title.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParsedRelease {
    // Content identification
    pub title: String,
    pub year: Option<u16>,
    pub season: Option<u16>,
    pub episodes: Vec<u16>,
    pub is_season_pack: bool,
    pub episode_title: Option<String>,

    // Quality
    pub resolution: Option<String>,
    pub source: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hdr_format: Option<String>,

    // Flags
    pub is_remux: bool,
    pub is_proper: bool,
    pub is_repack: bool,
    pub is_remastered: bool,

    // Metadata
    pub release_group: Option<String>,
    pub languages: Vec<String>,
    pub edition: Option<String>,
}

/// Parse a release title into structured metadata.
#[allow(clippy::too_many_lines)]
pub fn parse(title: &str) -> ParsedRelease {
    // trace-level: off by default, but flip on to follow a specific
    // release end-to-end when debugging a mis-parse. The parser is
    // pure and called thousands of times per search so we never log
    // at debug or above here.
    tracing::trace!(raw = title, "parse release title");

    let mut result = ParsedRelease::default();

    let cleaned = normalize(title);
    let raw_tokens = tokenize(&cleaned);

    if raw_tokens.is_empty() {
        // Callers should never pass an empty title, but if they do we
        // still return something rather than panic — log a warn so
        // the bad caller shows up in traces.
        tracing::warn!(raw = title, "parse called with empty/whitespace title");
        title.clone_into(&mut result.title);
        return result;
    }

    // Extract release group from the last hyphen-separated segment
    result.release_group = extract_release_group(&cleaned);

    // Classify each token
    let classified: Vec<(&str, TokenKind)> =
        raw_tokens.iter().map(|t| (*t, classify_token(t))).collect();

    // Find the first non-year metadata token (resolution / source /
    // codec / TV pattern / etc.). Years alone don't close the title
    // because some titles *contain* a year — "2001 A Space Odyssey
    // 1968 2160p BluRay" has two years, and the real release year is
    // the last one before the quality block, not the first.
    let mut first_non_year_meta: Option<usize> = None;
    for (i, (tok, kind)) in classified.iter().enumerate() {
        match kind {
            TokenKind::Resolution
            | TokenKind::Source
            | TokenKind::VideoCodec
            | TokenKind::AudioCodec
            | TokenKind::Hdr
            | TokenKind::Flag
            | TokenKind::Language
            | TokenKind::Edition => {
                if first_non_year_meta.is_none() {
                    first_non_year_meta = Some(i);
                }
            }
            TokenKind::TvSeason | TokenKind::TvEpisode => {
                if first_non_year_meta.is_none() {
                    first_non_year_meta = Some(i);
                }
                parse_tv_pattern(tok, &mut result);
            }
            TokenKind::Year | TokenKind::Unknown => {}
        }
    }

    // Pick the release year: the last Year token strictly before the
    // first non-year metadata token. If only one year exists, that's
    // the pick; if two (`title_year ... release_year ... 1080p`), the
    // later one wins. Fall back to the first year overall if none
    // precedes non-year meta (movies with no quality info).
    let year_picked_at: Option<usize> = {
        let meta_cut = first_non_year_meta.unwrap_or(classified.len());
        let candidate = classified[..meta_cut]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (_, k))| matches!(k, TokenKind::Year))
            .map(|(i, _)| i);
        candidate.or_else(|| {
            classified
                .iter()
                .enumerate()
                .find(|(_, (_, k))| matches!(k, TokenKind::Year))
                .map(|(i, _)| i)
        })
    };

    // Title ends at whichever comes first: the picked year, or the
    // first non-year metadata token. The picked year itself is
    // excluded from the title.
    let title_end = match (year_picked_at, first_non_year_meta) {
        (Some(y), Some(m)) => y.min(m),
        (Some(y), None) => y,
        (None, Some(m)) => m,
        (None, None) => classified.len(),
    };
    let first_meta = year_picked_at.or(first_non_year_meta);

    // Build title from tokens before first metadata, excluding release group
    let mut title_tokens = &raw_tokens[..title_end];
    if let Some(ref group) = result.release_group {
        // If the last title token matches the release group, exclude it
        if title_tokens
            .last()
            .is_some_and(|t| t.eq_ignore_ascii_case(group))
        {
            title_tokens = &title_tokens[..title_tokens.len() - 1];
        }
    }
    result.title = title_tokens.join(" ");

    // Extract episode title for TV: tokens between TV pattern and next quality token
    if result.season.is_some() && !result.episodes.is_empty() {
        extract_episode_title(&classified, &raw_tokens, &mut result);
    }

    // Seed the picked release year before the metadata walk. The
    // loop below would otherwise take the first Year token it meets,
    // which for a title-with-year ("2001 A Space Odyssey 1968 ...")
    // is the wrong one. `year_picked_at` captures the post-title
    // year we want.
    if let Some(idx) = year_picked_at {
        result.year = classified[idx].0.parse().ok();
    }

    // Process all classified tokens for metadata
    let mut i = first_meta.unwrap_or(0);
    while i < classified.len() {
        let (tok, kind) = &classified[i];
        match kind {
            TokenKind::Year => {
                if result.year.is_none() {
                    result.year = tok.parse().ok();
                }
            }
            TokenKind::Resolution => {
                if result.resolution.is_none() {
                    result.resolution = Some(normalize_resolution(tok));
                }
            }
            TokenKind::Source => {
                // Peek ahead: WEB followed by DL = webdl (not web + dual-language)
                if tok.eq_ignore_ascii_case("WEB")
                    && let Some((next_tok, _)) = classified.get(i + 1)
                    && next_tok.eq_ignore_ascii_case("DL")
                {
                    if result.source.is_none() {
                        result.source = Some("webdl".to_owned());
                    }
                    i += 2;
                    continue;
                }
                let (source, remux) = normalize_source(tok);
                if result.source.is_none() {
                    result.source = Some(source);
                }
                if remux {
                    result.is_remux = true;
                }
            }
            TokenKind::VideoCodec => {
                if result.video_codec.is_none() {
                    result.video_codec = Some(normalize_video_codec(tok));
                }
            }
            TokenKind::AudioCodec => {
                let codec = normalize_audio_codec(tok);
                // Atmos is a modifier — combine with existing codec
                if codec == "atmos" {
                    // Atmos modifies TrueHD or EAC3
                } else if result.audio_codec.is_none() {
                    result.audio_codec = Some(codec);
                }
                // Peek ahead for multi-token audio: DTS followed by HD/MA
                if tok.eq_ignore_ascii_case("DTS")
                    && let Some((next_tok, _)) = classified.get(i + 1)
                {
                    let nl = next_tok.to_ascii_lowercase();
                    if nl == "hd" || nl == "ma" || nl == "hdma" {
                        result.audio_codec = Some("dtshd".to_owned());
                        i += 1;
                    }
                }
            }
            TokenKind::Hdr => {
                if result.hdr_format.is_none() {
                    result.hdr_format = Some(normalize_hdr(tok));
                }
            }
            TokenKind::Flag => apply_flag(tok, &mut result),
            TokenKind::Language => {
                let lang = normalize_language(tok);
                if !result.languages.contains(&lang) {
                    result.languages.push(lang);
                }
            }
            TokenKind::Edition => {
                if result.edition.is_none() {
                    result.edition = Some(normalize_edition(tok));
                }
                // Remastered is both an edition and a flag
                if tok.eq_ignore_ascii_case("remastered") {
                    result.is_remastered = true;
                }
            }
            TokenKind::TvSeason | TokenKind::TvEpisode => {}
            TokenKind::Unknown => {
                // Peekahead: "H" + "264"/"265" = video codec
                if tok.eq_ignore_ascii_case("H")
                    && let Some((next_tok, _)) = classified.get(i + 1)
                    && (*next_tok == "264" || *next_tok == "265")
                {
                    if result.video_codec.is_none() {
                        result.video_codec = Some(format!("h{next_tok}"));
                    }
                    i += 1;
                }
            }
        }
        i += 1;
    }

    // Signal when a parse produces nothing structurally useful.
    // An empty title + no year + no TV season usually means the
    // tokenizer lost the signal (weird separators, non-ASCII
    // wrapping, numeric-only releases). Warn once per such parse
    // so call-site logs surface the raw title.
    if result.title.is_empty() && result.year.is_none() && result.season.is_none() {
        tracing::warn!(raw = title, "parse produced no title/year/season");
    } else {
        tracing::trace!(
            title = %result.title,
            year = ?result.year,
            season = ?result.season,
            episodes = ?result.episodes,
            "parse result"
        );
    }

    result
}

fn normalize(title: &str) -> String {
    let mut s = title.to_owned();

    // Strip common website prefixes/suffixes
    if let Some(idx) = s.find("www.")
        && let Some(end) = s[idx..].find([' ', '.', '-'])
    {
        // Check if this looks like a website domain
        let segment = &s[idx..idx + end];
        if segment.contains('.') {
            s = s[idx + end..]
                .trim_start_matches(['.', '-', ' '])
                .to_owned();
        }
    }

    // Strip leading bracket content like [Group]
    if s.starts_with('[')
        && let Some(end) = s.find(']')
    {
        s = s[end + 1..].trim_start_matches(['.', '-', ' ']).to_owned();
    }

    s
}

fn tokenize(title: &str) -> Vec<&str> {
    // First split on . _ and space, then further split each token on hyphens.
    // This handles "x264-GROUP" → ["x264", "GROUP"] and "WEB-DL" → ["WEB", "DL"]
    // while the release group is extracted separately before tokenization.
    title
        .split(['.', '_', ' '])
        .flat_map(|t| t.split('-'))
        .filter(|t| !t.is_empty())
        .collect()
}

fn extract_release_group(title: &str) -> Option<String> {
    // Find the last hyphen that separates the release group
    let last_hyphen = title.rfind('-')?;
    let group = &title[last_hyphen + 1..];

    // Strip trailing bracket content like [rarbg]
    let group = if let Some(bracket) = group.find('[') {
        &group[..bracket]
    } else {
        group
    };

    // Strip trailing extension
    let group = group.trim_end_matches('.');
    let group = group
        .strip_suffix(".mkv")
        .or_else(|| group.strip_suffix(".mp4"))
        .or_else(|| group.strip_suffix(".avi"))
        .unwrap_or(group);

    if group.is_empty() {
        return None;
    }

    // Don't return known tokens as release groups
    let kind = classify_token(group);
    if kind != TokenKind::Unknown {
        return None;
    }

    Some(group.to_owned())
}

fn parse_tv_pattern(token: &str, result: &mut ParsedRelease) {
    let upper = token.to_ascii_uppercase();

    // S01E05, S01E05E06, S01E05-E08
    if let Some(rest) = upper.strip_prefix('S') {
        let parts: Vec<&str> = rest.split('E').collect();
        if parts.len() >= 2 {
            // S{season}E{ep1}[E{ep2}...]
            if let Ok(season) = parts[0].parse::<u16>() {
                result.season = Some(season);
                // A hyphen in any part signals range syntax (E05-E08),
                // where the listed numbers are endpoints rather than
                // discrete episodes. Without a hyphen, each E{n} is a
                // separate episode — a release like S01E05E10 is two
                // episodes, not the six-episode span 5..10.
                let has_range_marker = parts[1..].iter().any(|p| p.contains('-'));
                for ep_str in &parts[1..] {
                    let clean = ep_str.split('-').next().unwrap_or(ep_str);
                    if let Ok(ep) = clean.parse::<u16>() {
                        result.episodes.push(ep);
                    }
                }
                if has_range_marker && result.episodes.len() == 2 {
                    let start = result.episodes[0];
                    let end = result.episodes[1];
                    if end > start + 1 {
                        result.episodes.clear();
                        for ep in start..=end {
                            result.episodes.push(ep);
                        }
                    }
                }
            }
        } else if let Ok(season) = parts[0].parse::<u16>() {
            // S01 with no episode = season pack
            result.season = Some(season);
            result.is_season_pack = true;
        }
    }
    // 1x05 format
    else if let Some(x_pos) = upper.find('X')
        && let (Ok(season), Ok(ep)) = (
            upper[..x_pos].parse::<u16>(),
            upper[x_pos + 1..].parse::<u16>(),
        )
    {
        result.season = Some(season);
        result.episodes.push(ep);
    }
    // "Season" keyword handled via flag/edition
}

fn extract_episode_title(
    classified: &[(&str, TokenKind)],
    raw_tokens: &[&str],
    result: &mut ParsedRelease,
) {
    // Find the TV pattern position
    let tv_pos = classified
        .iter()
        .position(|(_, k)| matches!(k, TokenKind::TvSeason | TokenKind::TvEpisode));

    if let Some(pos) = tv_pos {
        // Collect tokens after TV pattern until next quality/metadata token
        let mut ep_title_tokens = Vec::new();
        for j in (pos + 1)..classified.len() {
            match classified[j].1 {
                TokenKind::Unknown => ep_title_tokens.push(raw_tokens[j]),
                _ => break,
            }
        }
        if !ep_title_tokens.is_empty() {
            result.episode_title = Some(ep_title_tokens.join(" "));
        }
    }
}

fn normalize_resolution(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "2160p" | "3840x2160" | "4k" | "uhd" => "2160".to_owned(),
        "1080p" | "1080i" | "1920x1080" | "fhd" => "1080".to_owned(),
        "720p" | "1280x720" => "720".to_owned(),
        "480p" | "480i" | "640x480" | "848x480" => "480".to_owned(),
        "576p" => "576".to_owned(),
        "540p" => "540".to_owned(),
        "360p" => "360".to_owned(),
        _ => lower.trim_end_matches(['p', 'i']).to_owned(),
    }
}

fn normalize_source(token: &str) -> (String, bool) {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "remux" => ("bluray".to_owned(), true),
        "bluray" | "blu-ray" | "bdrip" | "brrip" | "bd" => ("bluray".to_owned(), false),
        "web-dl" | "webdl" | "web" => ("webdl".to_owned(), false),
        "webrip" | "web-rip" => ("webrip".to_owned(), false),
        "hdtv" | "pdtv" | "dsr" | "sdtv" | "tvrip" => ("hdtv".to_owned(), false),
        "dvdrip" | "dvd" | "dvdr" => ("dvd".to_owned(), false),
        "cam" | "camrip" | "hdcam" => ("cam".to_owned(), false),
        "ts" | "telesync" | "hdts" => ("telesync".to_owned(), false),
        "tc" | "telecine" | "hdtc" => ("telecine".to_owned(), false),
        "scr" | "screener" | "dvdscr" => ("screener".to_owned(), false),
        _ => (lower, false),
    }
}

fn normalize_video_codec(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "x264" | "h264" | "h.264" | "avc" => "h264".to_owned(),
        "x265" | "h265" | "h.265" | "hevc" => "h265".to_owned(),
        "av1" => "av1".to_owned(),
        "vp9" => "vp9".to_owned(),
        "xvid" | "divx" => "xvid".to_owned(),
        "mpeg2" | "mpeg-2" => "mpeg2".to_owned(),
        "vc-1" | "vc1" => "vc1".to_owned(),
        _ => lower,
    }
}

fn normalize_audio_codec(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "aac" | "aac2.0" | "aac5.1" => "aac".to_owned(),
        "ac3" | "dd5.1" | "dd2.0" => "ac3".to_owned(),
        "eac3" | "ddp" | "ddp5" | "ddp5.1" | "ddp2.0" | "dd+" => "eac3".to_owned(),
        "dts" => "dts".to_owned(),
        "dts-hd" | "dtshdma" | "dts-hdma" => "dtshd".to_owned(),
        "truehd" => "truehd".to_owned(),
        "atmos" => "atmos".to_owned(),
        "flac" => "flac".to_owned(),
        "opus" => "opus".to_owned(),
        "lpcm" | "pcm" => "lpcm".to_owned(),
        _ => lower,
    }
}

fn normalize_hdr(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "hdr" | "hdr10" => "hdr10".to_owned(),
        "hdr10+" | "hdr10plus" => "hdr10plus".to_owned(),
        "dv" | "dovi" | "dolbyvision" => "dolby_vision".to_owned(),
        "hlg" | "hlg10" => "hlg".to_owned(),
        "sdr" => "sdr".to_owned(),
        _ => lower,
    }
}

fn normalize_language(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "multi" | "multi-" | "dual" | "dl" => "multi".to_owned(),
        "french" | "truefrench" | "vff" | "vfq" | "vf" => "fr".to_owned(),
        "german" | "deutsch" => "de".to_owned(),
        "spanish" | "latino" | "castellano" => "es".to_owned(),
        "italian" | "ita" => "it".to_owned(),
        "portuguese" | "dublado" => "pt".to_owned(),
        "russian" | "rus" => "ru".to_owned(),
        "japanese" | "jap" | "jpn" => "ja".to_owned(),
        "korean" | "kor" => "ko".to_owned(),
        "chinese" | "chi" | "chs" | "cht" => "zh".to_owned(),
        "hindi" => "hi".to_owned(),
        "turkish" | "tur" => "tr".to_owned(),
        "polish" | "pldub" => "pl".to_owned(),
        "dutch" | "nl" => "nl".to_owned(),
        "swedish" | "swe" => "sv".to_owned(),
        "nordic" | "norse" => "nordic".to_owned(),
        _ => lower,
    }
}

fn normalize_edition(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "remastered" => "Remastered".to_owned(),
        "extended" => "Extended".to_owned(),
        "uncut" | "unrated" | "uncensored" => "Unrated".to_owned(),
        "dc" | "directors" => "Director's Cut".to_owned(),
        "imax" => "IMAX".to_owned(),
        _ => token.to_owned(),
    }
}

fn apply_flag(token: &str, result: &mut ParsedRelease) {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "proper" => result.is_proper = true,
        "repack" | "rerip" => result.is_repack = true,
        "remastered" => result.is_remastered = true,
        "complete" => result.is_season_pack = true,
        _ => {}
    }
}

/// Resolve a parsed release to its quality tier id (`"bluray_1080p"`,
/// `"remux_2160p"`, etc). The returned id is what the user's profile
/// matches against; unrecognised combinations fall back through
/// resolution-only matches and ultimately `"sdtv"`.
#[must_use]
pub fn determine_quality_tier(release: &ParsedRelease) -> String {
    let resolution = release.resolution.as_deref().unwrap_or("unknown");
    let source = release.source.as_deref().unwrap_or("unknown");

    if release.is_remux {
        return match resolution {
            "2160" => "remux_2160p",
            _ => "remux_1080p",
        }
        .to_owned();
    }

    match (source, resolution) {
        ("bluray", "2160") => "bluray_2160p".to_owned(),
        ("bluray", "1080") => "bluray_1080p".to_owned(),
        ("bluray", "720") => "bluray_720p".to_owned(),
        ("bluray", "480") => "bluray_480p".to_owned(),
        ("webdl" | "webrip", "2160") => "web_2160p".to_owned(),
        ("webdl" | "webrip", "1080") => "web_1080p".to_owned(),
        ("webdl" | "webrip", "720") => "web_720p".to_owned(),
        ("webdl" | "webrip", "480") => "web_480p".to_owned(),
        ("hdtv", "2160") => "hdtv_2160p".to_owned(),
        ("hdtv", "1080") => "hdtv_1080p".to_owned(),
        ("hdtv", "720") => "hdtv_720p".to_owned(),
        ("dvd", _) => "dvd".to_owned(),
        ("hdtv", _) => "sdtv".to_owned(),
        ("cam" | "camrip" | "hdcam" | "screener", _) => "cam".to_owned(),
        ("telesync", _) => "telesync".to_owned(),
        ("telecine", _) => "telecine".to_owned(),
        _ => match resolution {
            "2160" => "web_2160p".to_owned(),
            "1080" => "web_1080p".to_owned(),
            "720" => "web_720p".to_owned(),
            "480" => "web_480p".to_owned(),
            _ => "sdtv".to_owned(),
        },
    }
}
