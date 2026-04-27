use super::naming::{self, NamingContext, quality_string};
use super::pipeline::discover_video_files;
use std::path::Path;

#[test]
fn discover_finds_video_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("movie.mkv"), "fake").unwrap();
    std::fs::write(dir.join("movie.mp4"), "fake").unwrap();
    std::fs::write(dir.join("readme.nfo"), "fake").unwrap();
    std::fs::write(dir.join("cover.jpg"), "fake").unwrap();

    let files = discover_video_files(dir);
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|f| {
        let ext = f.extension().unwrap().to_str().unwrap();
        ext == "mkv" || ext == "mp4"
    }));
}

#[test]
fn discover_skips_sample_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("Sample")).unwrap();
    std::fs::write(dir.join("movie.mkv"), "fake").unwrap();
    std::fs::write(dir.join("Sample/sample.mkv"), "fake").unwrap();

    let files = discover_video_files(dir);
    assert_eq!(files.len(), 1);
}

#[test]
fn discover_skips_extras_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("extras")).unwrap();
    std::fs::write(dir.join("movie.mkv"), "fake").unwrap();
    std::fs::write(dir.join("extras/behind.mkv"), "fake").unwrap();

    let files = discover_video_files(dir);
    assert_eq!(files.len(), 1);
}

#[test]
fn discover_recurses_subdirs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("subdir")).unwrap();
    std::fs::write(dir.join("movie.mkv"), "fake").unwrap();
    std::fs::write(dir.join("subdir/other.mp4"), "fake").unwrap();

    let files = discover_video_files(dir);
    assert_eq!(files.len(), 2);
}

#[test]
fn quality_string_variants() {
    assert_eq!(quality_string(Some("bluray"), Some("1080")), "Bluray-1080p");
    assert_eq!(quality_string(Some("webdl"), Some("2160")), "Webdl-2160p");
    assert_eq!(quality_string(None, Some("720")), "720p");
    assert_eq!(quality_string(Some("hdtv"), None), "Hdtv");
    assert_eq!(quality_string(None, None), "Unknown");
}

#[test]
fn movie_naming_with_custom_format() {
    let ctx = NamingContext {
        title: "Inception".into(),
        show: None,
        year: Some(2010),
        season: None,
        episode: None,
        episode_title: None,
        quality: "Bluray-2160p".into(),
        resolution: Some("2160".into()),
        source: Some("bluray".into()),
        codec: Some("x265".into()),
        hdr: Some("HDR10".into()),
        audio: None,
        group: Some("FGT".into()),
        imdb_id: None,
        tmdb_id: Some(27205),
        container: "mkv".into(),
    };

    let path = naming::movie_path(Path::new("/lib"), "{title} ({year}) [{quality}]", &ctx);
    assert_eq!(
        path.to_string_lossy(),
        "/lib/Movies/Inception (2010) [Bluray-2160p].mkv"
    );
}

#[test]
fn episode_naming_double_digit_padding() {
    let ctx = NamingContext {
        title: "Pilot".into(),
        show: Some("Lost".into()),
        year: None,
        season: Some(1),
        episode: Some(1),
        episode_title: Some("Pilot".into()),
        quality: "WEB-1080p".into(),
        resolution: Some("1080".into()),
        source: Some("webdl".into()),
        codec: None,
        hdr: None,
        audio: None,
        group: None,
        imdb_id: None,
        tmdb_id: None,
        container: "mkv".into(),
    };

    let path = naming::episode_path(
        Path::new("/lib"),
        "{show} - S{season:00}E{episode:00} - {title} [{quality}]",
        "Season {season:00}",
        "Lost",
        &ctx,
    );
    assert!(path.to_string_lossy().contains("S01E01"));
    assert!(path.to_string_lossy().contains("Season 01"));
}
