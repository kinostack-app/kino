//! Library file naming from templates.

use std::path::{Path, PathBuf};

/// Context for template token replacement.
#[derive(Debug)]
pub struct NamingContext {
    pub title: String,
    pub show: Option<String>,
    pub year: Option<i64>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub episode_title: Option<String>,
    pub quality: String,
    pub resolution: Option<String>,
    pub source: Option<String>,
    pub codec: Option<String>,
    pub hdr: Option<String>,
    pub audio: Option<String>,
    pub group: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<i64>,
    pub container: String,
}

/// Generate a movie library path.
pub fn movie_path(library_root: &Path, format: &str, ctx: &NamingContext) -> PathBuf {
    let filename = apply_template(format, ctx);
    let safe = sanitize_filename(&filename);
    library_root
        .join("Movies")
        .join(format!("{safe}.{}", ctx.container))
}

/// Generate a TV episode library path.
pub fn episode_path(
    library_root: &Path,
    episode_format: &str,
    season_format: &str,
    show_name: &str,
    ctx: &NamingContext,
) -> PathBuf {
    let season_folder = apply_template(season_format, ctx);
    let filename = apply_template(episode_format, ctx);
    let safe_filename = sanitize_filename(&filename);
    let safe_show = sanitize_filename(show_name);
    let safe_season = sanitize_filename(&season_folder);

    library_root
        .join("TV")
        .join(safe_show)
        .join(safe_season)
        .join(format!("{safe_filename}.{}", ctx.container))
}

/// Apply template token substitution.
fn apply_template(template: &str, ctx: &NamingContext) -> String {
    let mut result = template.to_owned();

    result = result.replace("{title}", &ctx.title);
    if let Some(ref show) = ctx.show {
        result = result.replace("{show}", show);
    }
    if let Some(year) = ctx.year {
        result = result.replace("{year}", &year.to_string());
    }

    // Season/episode with padding support
    if let Some(season) = ctx.season {
        result = result.replace("{season:00}", &format!("{season:02}"));
        result = result.replace("{season}", &season.to_string());
    }
    if let Some(episode) = ctx.episode {
        result = result.replace("{episode:00}", &format!("{episode:02}"));
        result = result.replace("{episode}", &episode.to_string());
    }

    result = result.replace("{quality}", &ctx.quality);
    result = result.replace("{resolution}", ctx.resolution.as_deref().unwrap_or(""));
    result = result.replace("{source}", ctx.source.as_deref().unwrap_or(""));
    result = result.replace("{codec}", ctx.codec.as_deref().unwrap_or(""));
    result = result.replace("{hdr}", ctx.hdr.as_deref().unwrap_or(""));
    result = result.replace("{audio}", ctx.audio.as_deref().unwrap_or(""));
    result = result.replace("{group}", ctx.group.as_deref().unwrap_or(""));
    result = result.replace("{imdb}", ctx.imdb_id.as_deref().unwrap_or(""));
    if let Some(tmdb) = ctx.tmdb_id {
        result = result.replace("{tmdb}", &tmdb.to_string());
    }

    // Clean up empty brackets and double separators
    result = result.replace("[]", "");
    result = result.replace("()", "");
    result = result.replace("  ", " ");
    result = result.replace(" - - ", " - ");
    result.trim().to_owned()
}

/// Remove characters that are invalid in filenames.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Build a quality string like "Bluray-1080p" from components.
pub fn quality_string(source: Option<&str>, resolution: Option<&str>) -> String {
    match (source, resolution) {
        (Some(s), Some(r)) => format!("{}-{r}p", capitalize(s)),
        (Some(s), None) => capitalize(s),
        (None, Some(r)) => format!("{r}p"),
        (None, None) => "Unknown".to_owned(),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            format!("{upper}{}", chars.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> NamingContext {
        NamingContext {
            title: "The Matrix".into(),
            show: None,
            year: Some(1999),
            season: None,
            episode: None,
            episode_title: None,
            quality: "Bluray-1080p".into(),
            resolution: Some("1080".into()),
            source: Some("bluray".into()),
            codec: Some("x264".into()),
            hdr: None,
            audio: None,
            group: Some("GROUP".into()),
            imdb_id: Some("tt0133093".into()),
            tmdb_id: Some(603),
            container: "mkv".into(),
        }
    }

    #[test]
    fn movie_path_default_format() {
        let ctx = make_ctx();
        let path = movie_path(Path::new("/media"), "{title} ({year}) [{quality}]", &ctx);
        assert_eq!(
            path,
            PathBuf::from("/media/Movies/The Matrix (1999) [Bluray-1080p].mkv")
        );
    }

    #[test]
    fn episode_path_default_format() {
        let mut ctx = make_ctx();
        ctx.show = Some("Breaking Bad".into());
        ctx.title = "Ozymandias".into();
        ctx.season = Some(5);
        ctx.episode = Some(14);

        let path = episode_path(
            Path::new("/media"),
            "{show} - S{season:00}E{episode:00} - {title} [{quality}]",
            "Season {season:00}",
            "Breaking Bad",
            &ctx,
        );
        assert_eq!(
            path,
            PathBuf::from(
                "/media/TV/Breaking Bad/Season 05/Breaking Bad - S05E14 - Ozymandias [Bluray-1080p].mkv"
            )
        );
    }

    #[test]
    fn sanitize_removes_invalid_chars() {
        assert_eq!(sanitize_filename("Movie: The?Sequel"), "Movie_ The_Sequel");
    }

    #[test]
    fn quality_string_full() {
        assert_eq!(quality_string(Some("bluray"), Some("1080")), "Bluray-1080p");
    }

    #[test]
    fn quality_string_source_only() {
        assert_eq!(quality_string(Some("webdl"), None), "Webdl");
    }

    #[test]
    fn quality_string_resolution_only() {
        assert_eq!(quality_string(None, Some("2160")), "2160p");
    }
}
