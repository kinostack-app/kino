/// Token classification categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Unknown,
    Year,
    Resolution,
    Source,
    VideoCodec,
    AudioCodec,
    Hdr,
    Flag,
    Language,
    Edition,
    TvSeason,  // S01 (season only)
    TvEpisode, // S01E05, 1x05
}

/// Classify a single token against the known vocabulary.
pub fn classify_token(token: &str) -> TokenKind {
    // TV patterns first (most specific)
    if is_tv_pattern(token) {
        if is_season_only(token) {
            return TokenKind::TvSeason;
        }
        return TokenKind::TvEpisode;
    }

    let lower = token.to_ascii_lowercase();

    // Year: 4-digit 19xx/20xx not followed by p/i
    if is_year(token) {
        return TokenKind::Year;
    }

    // Resolution
    if is_resolution(&lower) {
        return TokenKind::Resolution;
    }

    // Source
    if is_source(&lower) {
        return TokenKind::Source;
    }

    // Video codec
    if is_video_codec(&lower) {
        return TokenKind::VideoCodec;
    }

    // Audio codec
    if is_audio_codec(&lower) {
        return TokenKind::AudioCodec;
    }

    // HDR
    if is_hdr(&lower) {
        return TokenKind::Hdr;
    }

    // Language
    if is_language(&lower) {
        return TokenKind::Language;
    }

    // Edition
    if is_edition(&lower) {
        return TokenKind::Edition;
    }

    // Flags
    if is_flag(&lower) {
        return TokenKind::Flag;
    }

    TokenKind::Unknown
}

fn is_tv_pattern(token: &str) -> bool {
    let upper = token.to_ascii_uppercase();

    // S01E05, S01E05E06, S01
    if let Some(rest) = upper.strip_prefix('S')
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return true;
    }

    // 1x05
    if let Some(x_pos) = upper.find('X')
        && x_pos > 0
        && upper[..x_pos].chars().all(|c| c.is_ascii_digit())
        && upper[x_pos + 1..].chars().all(|c| c.is_ascii_digit())
        && !upper[x_pos + 1..].is_empty()
    {
        return true;
    }

    false
}

fn is_season_only(token: &str) -> bool {
    let upper = token.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix('S') {
        // S01 (no E) = season only
        return rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty();
    }
    false
}

fn is_year(token: &str) -> bool {
    if token.len() != 4 {
        return false;
    }
    if let Ok(n) = token.parse::<u16>() {
        return (1900..=2099).contains(&n);
    }
    false
}

fn is_resolution(token: &str) -> bool {
    matches!(
        token,
        "2160p"
            | "1080p"
            | "1080i"
            | "720p"
            | "480p"
            | "480i"
            | "576p"
            | "540p"
            | "360p"
            | "3840x2160"
            | "1920x1080"
            | "1280x720"
            | "640x480"
            | "848x480"
            | "fhd"
            | "4k"
            | "uhd"
    )
}

fn is_source(token: &str) -> bool {
    matches!(
        token,
        "bluray"
            | "blu-ray"
            | "bdrip"
            | "brrip"
            | "bd"
            | "remux"
            | "web-dl"
            | "webdl"
            | "webrip"
            | "web-rip"
            | "web"
            | "hdtv"
            | "pdtv"
            | "dsr"
            | "sdtv"
            | "tvrip"
            | "dvdrip"
            | "dvd"
            | "dvdr"
            | "cam"
            | "camrip"
            | "hdcam"
            | "ts"
            | "telesync"
            | "hdts"
            | "tc"
            | "telecine"
            | "hdtc"
            | "scr"
            | "screener"
            | "dvdscr"
    )
}

fn is_video_codec(token: &str) -> bool {
    matches!(
        token,
        "x264"
            | "h264"
            | "h.264"
            | "avc"
            | "x265"
            | "h265"
            | "h.265"
            | "hevc"
            | "av1"
            | "vp9"
            | "xvid"
            | "divx"
            | "mpeg2"
            | "mpeg-2"
            | "vc-1"
            | "vc1"
    )
}

fn is_audio_codec(token: &str) -> bool {
    matches!(
        token,
        "aac"
            | "aac2.0"
            | "aac5.1"
            | "ac3"
            | "dd5.1"
            | "dd2.0"
            | "eac3"
            | "ddp"
            | "ddp5"
            | "ddp5.1"
            | "ddp2.0"
            | "dd+"
            | "dts"
            | "dts-hd"
            | "dtshdma"
            | "dts-hdma"
            | "truehd"
            | "atmos"
            | "flac"
            | "opus"
            | "lpcm"
            | "pcm"
    )
}

fn is_hdr(token: &str) -> bool {
    matches!(
        token,
        "hdr"
            | "hdr10"
            | "hdr10+"
            | "hdr10plus"
            | "dv"
            | "dovi"
            | "dolbyvision"
            | "hlg"
            | "hlg10"
            | "sdr"
    )
}

fn is_language(token: &str) -> bool {
    matches!(
        token,
        "multi"
            | "multi-"
            | "dual"
            | "french"
            | "truefrench"
            | "vff"
            | "vfq"
            | "vf"
            | "german"
            | "deutsch"
            | "spanish"
            | "latino"
            | "castellano"
            | "italian"
            | "ita"
            | "portuguese"
            | "dublado"
            | "russian"
            | "rus"
            | "japanese"
            | "jap"
            | "jpn"
            | "korean"
            | "kor"
            | "chinese"
            | "chi"
            | "chs"
            | "cht"
            | "hindi"
            | "turkish"
            | "tur"
            | "polish"
            | "pldub"
            | "dutch"
            | "nl"
            | "swedish"
            | "swe"
            | "nordic"
            | "norse"
    )
}

fn is_edition(token: &str) -> bool {
    matches!(
        token,
        "remastered"
            | "extended"
            | "uncut"
            | "unrated"
            | "uncensored"
            | "dc"
            | "directors"
            | "imax"
    )
}

fn is_flag(token: &str) -> bool {
    matches!(
        token,
        "proper"
            | "repack"
            | "rerip"
            | "complete"
            | "dubbed"
            | "dub"
            | "subbed"
            | "sub"
            | "hardsub"
            | "hc"
    )
}
