use super::parse;

// ==================== Standard Movies ====================

#[test]
fn movie_standard() {
    let r = parse("The.Matrix.1999.1080p.BluRay.x264-GROUP");
    assert_eq!(r.title, "The Matrix");
    assert_eq!(r.year, Some(1999));
    assert_eq!(r.resolution.as_deref(), Some("1080"));
    assert_eq!(r.source.as_deref(), Some("bluray"));
    assert_eq!(r.video_codec.as_deref(), Some("h264"));
    assert_eq!(r.release_group.as_deref(), Some("GROUP"));
}

#[test]
fn movie_2160p_uhd_remux() {
    let r = parse("The.Matrix.1999.Remastered.2160p.UHD.BluRay.Remux.HEVC.DTS-HD.MA.7.1-FGT");
    assert_eq!(r.title, "The Matrix");
    assert_eq!(r.year, Some(1999));
    assert_eq!(r.resolution.as_deref(), Some("2160"));
    assert_eq!(r.source.as_deref(), Some("bluray"));
    assert_eq!(r.video_codec.as_deref(), Some("h265"));
    assert!(r.is_remux);
    assert!(r.is_remastered);
    assert_eq!(r.release_group.as_deref(), Some("FGT"));
}

#[test]
fn movie_web_dl() {
    let r = parse("Oppenheimer.2023.2160p.WEB-DL.DDP5.1.Atmos.DV.H.265-FLUX");
    assert_eq!(r.title, "Oppenheimer");
    assert_eq!(r.year, Some(2023));
    assert_eq!(r.resolution.as_deref(), Some("2160"));
    assert_eq!(r.video_codec.as_deref(), Some("h265"));
    assert_eq!(r.hdr_format.as_deref(), Some("dolby_vision"));
    assert_eq!(r.audio_codec.as_deref(), Some("eac3"));
    assert_eq!(r.release_group.as_deref(), Some("FLUX"));
}

#[test]
fn movie_webrip() {
    let r = parse("The.Holdovers.2023.1080p.WEBRip.x264.AAC-AOC");
    assert_eq!(r.title, "The Holdovers");
    assert_eq!(r.year, Some(2023));
    assert_eq!(r.source.as_deref(), Some("webrip"));
    assert_eq!(r.audio_codec.as_deref(), Some("aac"));
}

#[test]
fn movie_proper() {
    let r = parse("Inception.2010.1080p.BluRay.x264.PROPER-GROUP");
    assert_eq!(r.title, "Inception");
    assert!(r.is_proper);
}

#[test]
fn movie_repack() {
    let r = parse("Dune.Part.Two.2024.2160p.WEB-DL.DDP5.1.H.265.REPACK-GROUP");
    assert_eq!(r.title, "Dune Part Two");
    assert!(r.is_repack);
}

#[test]
fn movie_extended_edition() {
    let r =
        parse("The.Lord.of.the.Rings.The.Return.of.the.King.2003.EXTENDED.2160p.BluRay.x265-GROUP");
    assert_eq!(r.title, "The Lord of the Rings The Return of the King");
    assert_eq!(r.year, Some(2003));
    assert_eq!(r.edition.as_deref(), Some("Extended"));
}

#[test]
fn movie_hdr10plus() {
    let r = parse("Blade.Runner.2049.2017.2160p.UHD.BluRay.HDR10+.x265-TERMiNAL");
    // Two years: 2049 is the title year ("Blade Runner 2049"), 2017
    // is the actual release year. The last year before the quality
    // block wins; the earlier one stays in the title.
    assert_eq!(r.title, "Blade Runner 2049");
    assert_eq!(r.year, Some(2017));
    assert_eq!(r.hdr_format.as_deref(), Some("hdr10plus"));
}

#[test]
fn movie_dts_hd() {
    let r = parse("Interstellar.2014.2160p.BluRay.x265.DTS-HD.MA-GROUP");
    assert_eq!(r.title, "Interstellar");
    assert_eq!(r.audio_codec.as_deref(), Some("dtshd"));
}

#[test]
fn movie_cam() {
    let r = parse("New.Movie.2024.CAM-RARBG");
    assert_eq!(r.source.as_deref(), Some("cam"));
}

// ==================== TV Episodes ====================

#[test]
fn tv_standard_episode() {
    let r = parse("Breaking.Bad.S05E14.Ozymandias.1080p.BluRay.x264-DON");
    assert_eq!(r.title, "Breaking Bad");
    assert_eq!(r.season, Some(5));
    assert_eq!(r.episodes, vec![14]);
    assert_eq!(r.episode_title.as_deref(), Some("Ozymandias"));
    assert_eq!(r.resolution.as_deref(), Some("1080"));
    assert_eq!(r.source.as_deref(), Some("bluray"));
    assert_eq!(r.release_group.as_deref(), Some("DON"));
}

#[test]
fn tv_multi_episode() {
    let r = parse("The.Office.S03E01E02.720p.BluRay.x264-GROUP");
    assert_eq!(r.title, "The Office");
    assert_eq!(r.season, Some(3));
    assert_eq!(r.episodes, vec![1, 2]);
}

#[test]
fn tv_season_pack() {
    let r = parse("Game.of.Thrones.S08.1080p.BluRay.x264-GROUP");
    assert_eq!(r.title, "Game of Thrones");
    assert_eq!(r.season, Some(8));
    assert!(r.is_season_pack);
    assert!(r.episodes.is_empty());
}

#[test]
fn tv_season_pack_complete() {
    let r = parse("Silo.S01.COMPLETE.2160p.WEB-DL.DDP5.1.H.265-NTb");
    assert_eq!(r.title, "Silo");
    assert_eq!(r.season, Some(1));
    assert!(r.is_season_pack);
    assert_eq!(r.resolution.as_deref(), Some("2160"));
}

#[test]
fn tv_1x05_format() {
    let r = parse("Seinfeld.4x11.The.Contest.DVDRip.x264-GROUP");
    assert_eq!(r.title, "Seinfeld");
    assert_eq!(r.season, Some(4));
    assert_eq!(r.episodes, vec![11]);
    assert_eq!(r.source.as_deref(), Some("dvd"));
}

#[test]
fn tv_web_dl_hdr() {
    let r = parse("Severance.S02E01.2160p.WEB-DL.DDP5.1.DV.H.265-NTb");
    assert_eq!(r.title, "Severance");
    assert_eq!(r.season, Some(2));
    assert_eq!(r.episodes, vec![1]);
    assert_eq!(r.hdr_format.as_deref(), Some("dolby_vision"));
}

// ==================== Languages ====================

#[test]
fn movie_french() {
    let r = parse("The.Matrix.1999.FRENCH.1080p.BluRay.x264-GROUP");
    assert!(r.languages.contains(&"fr".to_owned()));
}

#[test]
fn movie_multi_language() {
    let r = parse("The.Matrix.1999.MULTi.1080p.BluRay.x264-GROUP");
    assert!(r.languages.contains(&"multi".to_owned()));
}

#[test]
fn movie_german() {
    let r = parse("The.Matrix.1999.GERMAN.DL.1080p.BluRay.x264-GROUP");
    assert!(r.languages.contains(&"de".to_owned()));
}

// ==================== HDR Formats ====================

#[test]
fn hdr10() {
    let r = parse("Movie.2024.2160p.BluRay.HDR10.x265-GROUP");
    assert_eq!(r.hdr_format.as_deref(), Some("hdr10"));
}

#[test]
fn dolby_vision() {
    let r = parse("Movie.2024.2160p.WEB-DL.DV.H.265-GROUP");
    assert_eq!(r.hdr_format.as_deref(), Some("dolby_vision"));
}

#[test]
fn hlg() {
    let r = parse("Movie.2024.2160p.BluRay.HLG.x265-GROUP");
    assert_eq!(r.hdr_format.as_deref(), Some("hlg"));
}

// ==================== Edge Cases ====================

#[test]
fn no_quality_info() {
    let r = parse("Some.Random.Release-GROUP");
    assert_eq!(r.title, "Some Random Release");
    assert!(r.resolution.is_none());
    assert!(r.source.is_none());
    assert_eq!(r.release_group.as_deref(), Some("GROUP"));
}

#[test]
fn no_year_movie() {
    let r = parse("Unknown.Movie.1080p.BluRay.x264-GROUP");
    assert_eq!(r.title, "Unknown Movie");
    assert!(r.year.is_none());
    assert_eq!(r.resolution.as_deref(), Some("1080"));
}

#[test]
fn title_with_numbers() {
    let r = parse("2001.A.Space.Odyssey.1968.2160p.BluRay.x265-GROUP");
    // Two year tokens: the title *contains* "2001" and the real
    // release year is 1968. We pick the last year before the quality
    // block, so the title keeps its leading year and `year` resolves
    // to the actual release year.
    assert_eq!(r.title, "2001 A Space Odyssey");
    assert_eq!(r.year, Some(1968));
    assert_eq!(r.resolution.as_deref(), Some("2160"));
}

#[test]
fn single_year_is_still_picked() {
    let r = parse("The.Matrix.1999.1080p.BluRay.x264-GROUP");
    assert_eq!(r.title, "The Matrix");
    assert_eq!(r.year, Some(1999));
}

#[test]
fn tv_multi_episode_discrete_not_range() {
    // `S01E05E10` is two discrete episodes, not the six-episode span
    // 5..=10. Only a hyphen marks a range (`S01E05-E10`).
    let r = parse("Show.Name.S01E05E10.720p.WEB-DL.x264-GROUP");
    assert_eq!(r.season, Some(1));
    assert_eq!(r.episodes, vec![5, 10]);
}

#[test]
fn strip_website_prefix() {
    let r = parse("www.Torrents.org - The.Matrix.1999.1080p.BluRay.x264-GROUP");
    // After normalization the www prefix should be stripped
    assert!(r.title.contains("Matrix"));
    assert_eq!(r.year, Some(1999));
}

#[test]
fn strip_leading_bracket() {
    let r = parse("[TorrentLeech] The.Matrix.1999.1080p.BluRay.x264-GROUP");
    assert_eq!(r.title, "The Matrix");
    assert_eq!(r.year, Some(1999));
}

#[test]
fn trailing_bracket_on_group() {
    let r = parse("The.Matrix.1999.1080p.BluRay.x264-GROUP[rarbg]");
    assert_eq!(r.release_group.as_deref(), Some("GROUP"));
}

#[test]
fn hdtv_720p() {
    let r = parse("Show.Name.S01E01.720p.HDTV.x264-GROUP");
    assert_eq!(r.source.as_deref(), Some("hdtv"));
    assert_eq!(r.resolution.as_deref(), Some("720"));
}

#[test]
fn flac_audio() {
    let r = parse("Album.Artist.2024.1080p.BluRay.FLAC.x264-GROUP");
    assert_eq!(r.audio_codec.as_deref(), Some("flac"));
}

#[test]
fn av1_codec() {
    let r = parse("Movie.2024.2160p.WEB-DL.AV1.Opus-GROUP");
    assert_eq!(r.video_codec.as_deref(), Some("av1"));
    assert_eq!(r.audio_codec.as_deref(), Some("opus"));
}

#[test]
fn imax_edition() {
    let r = parse("Dune.2021.IMAX.2160p.WEB-DL.DDP5.1.H.265-GROUP");
    assert_eq!(r.edition.as_deref(), Some("IMAX"));
}

#[test]
fn resolution_not_confused_with_year() {
    // 1080 should NOT be parsed as a year
    let r = parse("Movie.Name.1080p.BluRay.x264-GROUP");
    assert!(r.year.is_none());
    assert_eq!(r.resolution.as_deref(), Some("1080"));
}

#[test]
fn truehd_audio() {
    let r = parse("Movie.2024.2160p.BluRay.Remux.HEVC.TrueHD.Atmos.7.1-GROUP");
    assert_eq!(r.audio_codec.as_deref(), Some("truehd"));
    assert!(r.is_remux);
}

#[test]
fn empty_title() {
    let r = parse("");
    assert_eq!(r.title, "");
}

#[test]
fn web_source_standalone() {
    let r = parse("Movie.2024.1080p.WEB.H.265-GROUP");
    assert_eq!(r.source.as_deref(), Some("webdl"));
}
