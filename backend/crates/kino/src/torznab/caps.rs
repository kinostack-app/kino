#![allow(dead_code)] // Wired into indexer_health + search in subsequent commits

//! Parser for the Torznab `?t=caps` response. Extracts only the
//! fields kino acts on today:
//!
//! - `<tv-search available="yes" supportedParams="q,season,ep,…"/>`
//! - `<movie-search available="yes" supportedParams="q,imdbid,…"/>`
//! - `<categories><category id="2000" … /></categories>`
//!
//! Everything else (server version, limits, audio/book modes,
//! Prowlarr `<tags>`) is ignored — see `docs/subsystems/02-search.md`
//! for the rationale. Storing less means less to break on quirky
//! indexer responses.

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

/// Per-mode (tv/movie) capability as declared by the indexer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchMode {
    pub available: bool,
    /// Lowercase Torznab parameter names, e.g. `q`, `season`, `ep`,
    /// `imdbid`, `tmdbid`, `tvdbid`, `tvmazeid`, `rid`.
    pub supported_params: Vec<String>,
}

/// The shape we persist into `indexer.supported_search_params` as
/// JSON and read back in `search.rs`. Missing modes = `None` so we
/// can distinguish "never probed" from "probed but mode unavailable".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorznabCapabilities {
    pub tv_search: Option<SearchMode>,
    pub movie_search: Option<SearchMode>,
    /// Newznab category IDs the indexer serves (2000 = Movies,
    /// 5000 = TV, narrower buckets like 2040 = Movies/HD).
    pub categories: Vec<i64>,
}

impl TorznabCapabilities {
    /// True when the indexer declared tv-search available. Callers
    /// with no capability data yet (None) should assume support —
    /// matches the pre-caps-probe behaviour.
    pub fn tv_available(&self) -> bool {
        self.tv_search.as_ref().is_some_and(|m| m.available)
    }

    pub fn movie_available(&self) -> bool {
        self.movie_search.as_ref().is_some_and(|m| m.available)
    }

    /// Whether a given parameter (e.g. `"imdbid"`) is declared
    /// supported in the tv-search mode. Returns `true` when we have
    /// no capability data (legacy indexers pre-caps-probe) so we
    /// don't regress.
    pub fn tv_supports(&self, param: &str) -> bool {
        self.tv_search
            .as_ref()
            .is_none_or(|m| m.supported_params.iter().any(|p| p == param))
    }

    pub fn movie_supports(&self, param: &str) -> bool {
        self.movie_search
            .as_ref()
            .is_none_or(|m| m.supported_params.iter().any(|p| p == param))
    }
}

/// Parse a Torznab `?t=caps` response. Unknown tags are ignored.
/// Returns an empty capabilities set on malformed input rather than
/// an error — a health probe that finds a reachable indexer with
/// broken caps XML is still "healthy enough to try a search", it
/// just can't narrow params until the next probe.
#[allow(clippy::too_many_lines)]
pub fn parse_caps(xml: &str) -> TorznabCapabilities {
    let mut reader = Reader::from_str(xml);
    let mut caps = TorznabCapabilities::default();
    let mut buf = Vec::new();
    let mut inside_categories = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "tv-search" => {
                        caps.tv_search = Some(parse_search_mode(e));
                    }
                    "movie-search" => {
                        caps.movie_search = Some(parse_search_mode(e));
                    }
                    "categories" => {
                        inside_categories = true;
                    }
                    "category" | "subcat" => {
                        if inside_categories
                            && let Some(id) =
                                extract_attr(e, "id").and_then(|v| v.parse::<i64>().ok())
                            && !caps.categories.contains(&id)
                        {
                            caps.categories.push(id);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"categories" {
                    inside_categories = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!(error = %e, "torznab caps: xml parse error, returning partial");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    caps
}

fn parse_search_mode(e: &quick_xml::events::BytesStart) -> SearchMode {
    let available = extract_attr(e, "available").is_some_and(|v| v.eq_ignore_ascii_case("yes"));
    let supported_params = extract_attr(e, "supportedParams")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    SearchMode {
        available,
        supported_params,
    }
}

fn extract_attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == key.as_bytes() {
            return Some(String::from_utf8_lossy(&attr.value).to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Realistic Prowlarr-style caps response, lightly trimmed.
    const PROWLARR_CAPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server version="1.0" title="Prowlarr" />
  <limits default="100" max="100" />
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="yes" supportedParams="q,season,ep,imdbid,tvdbid,tmdbid,tvmazeid,rid" />
    <movie-search available="yes" supportedParams="q,imdbid,tmdbid" />
    <audio-search available="no" />
    <book-search available="no" />
  </searching>
  <categories>
    <category id="2000" name="Movies">
      <subcat id="2040" name="HD" />
      <subcat id="2045" name="UHD" />
    </category>
    <category id="5000" name="TV">
      <subcat id="5070" name="Anime" />
    </category>
  </categories>
</caps>"#;

    // LimeTorrents-style: no ID params anywhere, category set mapped
    // through the Cardigann definition.
    const LIMETORRENTS_CAPS: &str = r#"<caps>
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="yes" supportedParams="q,season,ep" />
    <movie-search available="yes" supportedParams="q" />
  </searching>
  <categories>
    <category id="2000" name="Movies" />
    <category id="5000" name="TV" />
    <category id="8000" name="Other" />
  </categories>
</caps>"#;

    #[test]
    fn prowlarr_caps_parses_full_shape() {
        let caps = parse_caps(PROWLARR_CAPS);
        let tv = caps.tv_search.as_ref().expect("tv-search present");
        assert!(tv.available);
        assert!(tv.supported_params.contains(&"imdbid".to_string()));
        assert!(tv.supported_params.contains(&"tvdbid".to_string()));
        let mv = caps.movie_search.as_ref().expect("movie-search present");
        assert!(mv.available);
        assert!(mv.supported_params.contains(&"tmdbid".to_string()));
        assert!(!mv.supported_params.contains(&"tvdbid".to_string()));
        assert!(caps.categories.contains(&2000));
        assert!(caps.categories.contains(&5000));
        assert!(caps.categories.contains(&2040));
    }

    #[test]
    fn limetorrents_caps_has_only_q() {
        let caps = parse_caps(LIMETORRENTS_CAPS);
        let tv = caps.tv_search.as_ref().unwrap();
        assert!(tv.available);
        assert_eq!(tv.supported_params, vec!["q", "season", "ep"]);
        let mv = caps.movie_search.as_ref().unwrap();
        assert_eq!(mv.supported_params, vec!["q"]);
        assert!(!mv.supported_params.contains(&"imdbid".to_string()));
    }

    #[test]
    fn capability_checks_fall_back_when_no_data() {
        // Never-probed indexer: assume everything works so we don't
        // regress to strict-deny against legacy rows.
        let caps = TorznabCapabilities::default();
        assert!(caps.tv_supports("imdbid"));
        assert!(caps.movie_supports("tmdbid"));
        // But availability is strictly false when mode is absent.
        assert!(!caps.tv_available());
        assert!(!caps.movie_available());
    }

    #[test]
    fn capability_checks_enforce_when_data_present() {
        let caps = parse_caps(LIMETORRENTS_CAPS);
        assert!(caps.tv_supports("q"));
        assert!(!caps.tv_supports("imdbid"));
        assert!(caps.movie_supports("q"));
        assert!(!caps.movie_supports("imdbid"));
    }

    #[test]
    fn malformed_xml_returns_empty_not_panic() {
        let caps = parse_caps("<caps><unclosed>");
        assert!(caps.tv_search.is_none());
        assert!(caps.movie_search.is_none());
    }

    #[test]
    fn unavailable_mode_parses_without_params() {
        let xml = r#"<caps><searching><tv-search available="no" /></searching></caps>"#;
        let caps = parse_caps(xml);
        let tv = caps.tv_search.as_ref().unwrap();
        assert!(!tv.available);
        assert!(tv.supported_params.is_empty());
    }
}
