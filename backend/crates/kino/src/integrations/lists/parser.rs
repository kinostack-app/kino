//! URL → `ParsedList`. Pure function so it's exhaustively testable.

use super::{ListsError, ParsedList, SourceType};

/// Detect a list source from a user-pasted URL.
///
/// Handles HTTP/HTTPS, optional `www.`, optional trailing slash, and
/// path variants. Trakt watchlist URLs are *only* parsed as such —
/// the system list is auto-managed; users can't re-add it manually.
pub fn parse_list_url(url: &str) -> Result<ParsedList, ListsError> {
    let raw = url.trim();
    let lower = raw.to_lowercase();
    // Strip protocol + www. for matching.
    let stripped = lower
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_end_matches('/');

    if let Some(rest) = stripped.strip_prefix("mdblist.com/lists/") {
        // Format: {user}/{slug}
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(ListsError::UnsupportedUrl(raw.into()));
        }
        let id = format!("{}/{}", parts[0], parts[1]);
        return Ok(ParsedList {
            source_type: SourceType::Mdblist,
            source_id: id.clone(),
            source_url: format!("https://mdblist.com/lists/{id}"),
        });
    }

    if let Some(rest) = stripped.strip_prefix("themoviedb.org/list/") {
        // Format: {numeric_id}[?params]
        let id_part = rest.split(['?', '/']).next().unwrap_or("");
        if id_part.is_empty() || id_part.parse::<u64>().is_err() {
            return Err(ListsError::UnsupportedUrl(raw.into()));
        }
        return Ok(ParsedList {
            source_type: SourceType::TmdbList,
            source_id: id_part.to_string(),
            source_url: format!("https://www.themoviedb.org/list/{id_part}"),
        });
    }

    if let Some(rest) = stripped.strip_prefix("trakt.tv/users/") {
        // Possible forms:
        //   {user}/lists/{slug}            → custom list
        //   {user}/watchlist               → watchlist (system list)
        let parts: Vec<&str> = rest.splitn(4, '/').collect();
        if parts.is_empty() || parts[0].is_empty() {
            return Err(ListsError::UnsupportedUrl(raw.into()));
        }
        let user = parts[0];
        if parts.len() >= 2 && parts[1] == "watchlist" {
            return Ok(ParsedList {
                source_type: SourceType::TraktWatchlist,
                source_id: user.to_string(),
                source_url: format!("https://trakt.tv/users/{user}/watchlist"),
            });
        }
        if parts.len() >= 3 && parts[1] == "lists" && !parts[2].is_empty() {
            let slug = parts[2].split('?').next().unwrap_or("");
            if slug.is_empty() {
                return Err(ListsError::UnsupportedUrl(raw.into()));
            }
            return Ok(ParsedList {
                source_type: SourceType::TraktList,
                source_id: format!("{user}/{slug}"),
                source_url: format!("https://trakt.tv/users/{user}/lists/{slug}"),
            });
        }
    }

    Err(ListsError::UnsupportedUrl(raw.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdblist_canonical() {
        let p = parse_list_url("https://mdblist.com/lists/myuser/best-of-a24").unwrap();
        assert_eq!(p.source_type, SourceType::Mdblist);
        assert_eq!(p.source_id, "myuser/best-of-a24");
    }

    #[test]
    fn mdblist_trailing_slash_and_http() {
        let p = parse_list_url("http://mdblist.com/lists/u/x/").unwrap();
        assert_eq!(p.source_id, "u/x");
    }

    #[test]
    fn tmdb_list_numeric() {
        let p = parse_list_url("https://www.themoviedb.org/list/12345").unwrap();
        assert_eq!(p.source_type, SourceType::TmdbList);
        assert_eq!(p.source_id, "12345");
    }

    #[test]
    fn tmdb_list_with_query() {
        let p = parse_list_url("https://www.themoviedb.org/list/12345?language=en").unwrap();
        assert_eq!(p.source_id, "12345");
    }

    #[test]
    fn tmdb_list_non_numeric_rejected() {
        assert!(parse_list_url("https://www.themoviedb.org/list/abc").is_err());
    }

    #[test]
    fn trakt_custom_list() {
        let p = parse_list_url("https://trakt.tv/users/alice/lists/comfort-films").unwrap();
        assert_eq!(p.source_type, SourceType::TraktList);
        assert_eq!(p.source_id, "alice/comfort-films");
    }

    #[test]
    fn trakt_watchlist() {
        let p = parse_list_url("https://trakt.tv/users/alice/watchlist").unwrap();
        assert_eq!(p.source_type, SourceType::TraktWatchlist);
        assert_eq!(p.source_id, "alice");
    }

    #[test]
    fn trakt_no_user_rejected() {
        assert!(parse_list_url("https://trakt.tv/users/").is_err());
    }

    #[test]
    fn unrelated_url_rejected() {
        assert!(parse_list_url("https://example.com/lists/foo").is_err());
    }

    #[test]
    fn whitespace_trimmed() {
        let p = parse_list_url("  https://mdblist.com/lists/u/x/  ").unwrap();
        assert_eq!(p.source_id, "u/x");
    }
}
