#![allow(dead_code)] // Used by search subsystem

use quick_xml::Reader;
use quick_xml::events::Event;

/// A release parsed from a Torznab RSS/XML response.
#[derive(Debug, Clone, Default)]
pub struct TorznabRelease {
    pub title: String,
    pub guid: String,
    pub size: Option<i64>,
    pub download_url: Option<String>,
    pub magnet_url: Option<String>,
    pub info_url: Option<String>,
    pub info_hash: Option<String>,
    pub publish_date: Option<String>,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub grabs: Option<i64>,
    pub categories: Vec<i64>,
}

/// Parse a Torznab XML response into a list of releases.
pub fn parse_torznab_response(xml: &str) -> Result<Vec<TorznabRelease>, String> {
    let mut reader = Reader::from_str(xml);
    let mut releases = Vec::new();
    let mut current: Option<TorznabRelease> = None;
    let mut current_tag = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                tag.clone_into(&mut current_tag);

                if tag == "item" {
                    current = Some(TorznabRelease::default());
                }

                if tag.contains("attr")
                    && let Some(ref mut item) = current
                {
                    parse_attr_element(e, item);
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag.contains("attr")
                    && let Some(ref mut item) = current
                {
                    parse_attr_element(e, item);
                }
                if tag == "enclosure"
                    && let Some(ref mut item) = current
                {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        match key.as_str() {
                            "url" => {
                                if item.download_url.is_none() {
                                    item.download_url = Some(val);
                                }
                            }
                            "length" => {
                                if item.size.is_none() {
                                    item.size = val.parse().ok();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Some(ref mut item) = current {
                    let text = e.unescape().unwrap_or_default().to_string();
                    apply_text_to_field(item, &current_tag, text);
                }
            }
            // CDATA — quick_xml exposes `<![CDATA[...]]>` as its own
            // event variant, distinct from plain text. Many indexers
            // (Jackett, Prowlarr, raw Newznab feeds) wrap titles,
            // links, and pubDates in CDATA; without this arm those
            // payloads silently drop on the floor and the item ends
            // up with an empty `title`, which the End-arm filter then
            // throws away. Item disappears from results.
            Ok(Event::CData(ref e)) => {
                if let Some(ref mut item) = current {
                    let text = String::from_utf8_lossy(e.as_ref()).to_string();
                    apply_text_to_field(item, &current_tag, text);
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "item"
                    && let Some(item) = current.take()
                    && !item.title.is_empty()
                {
                    releases.push(item);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    Ok(releases)
}

/// Dispatch a textual payload (from `Event::Text` or `Event::CData`)
/// to the right `TorznabRelease` field based on the current XML tag.
/// Empty / whitespace-only payloads are ignored so XML formatting
/// whitespace doesn't clobber a field set on a prior event of the
/// same item.
fn apply_text_to_field(item: &mut TorznabRelease, current_tag: &str, text: String) {
    if text.trim().is_empty() {
        return;
    }
    match current_tag {
        "title" => item.title = text,
        "guid" => {
            // Many indexers put the magnet URI in the guid field.
            if text.starts_with("magnet:") && item.magnet_url.is_none() {
                item.magnet_url = Some(text.clone());
            }
            item.guid = text;
        }
        "link" => {
            if item.download_url.is_none() {
                item.download_url = Some(text);
            }
        }
        "size" => item.size = text.parse().ok(),
        "pubDate" => item.publish_date = Some(text),
        "comments" => item.info_url = Some(text),
        _ => {}
    }
}

fn parse_attr_element(e: &quick_xml::events::BytesStart<'_>, item: &mut TorznabRelease) {
    let mut name = String::new();
    let mut value = String::new();

    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let val = String::from_utf8_lossy(&attr.value).to_string();
        match key.as_str() {
            "name" => name = val,
            "value" => value = val,
            _ => {}
        }
    }

    match name.as_str() {
        "seeders" => item.seeders = value.parse().ok(),
        "leechers" | "peers" => item.leechers = value.parse().ok(),
        "grabs" => item.grabs = value.parse().ok(),
        "size" => {
            if item.size.is_none() {
                item.size = value.parse().ok();
            }
        }
        "infohash" => item.info_hash = Some(value),
        "magneturl" => item.magnet_url = Some(value),
        "category" => {
            if let Ok(cat) = value.parse::<i64>() {
                item.categories.push(cat);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_torznab_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>The.Matrix.1999.1080p.BluRay.x264-GROUP</title>
      <guid>abc123</guid>
      <size>14500000000</size>
      <link>https://example.com/download/abc123</link>
      <pubDate>Mon, 01 Jan 2024 00:00:00 +0000</pubDate>
      <torznab:attr name="seeders" value="42"/>
      <torznab:attr name="leechers" value="5"/>
      <torznab:attr name="infohash" value="deadbeef"/>
      <torznab:attr name="category" value="2040"/>
    </item>
    <item>
      <title>The.Matrix.1999.2160p.UHD.BluRay.Remux-OTHER</title>
      <guid>def456</guid>
      <enclosure url="https://example.com/download/def456" length="45000000000" type="application/x-bittorrent"/>
      <torznab:attr name="seeders" value="100"/>
    </item>
  </channel>
</rss>"#;

        let results = parse_torznab_response(xml).unwrap();
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].title, "The.Matrix.1999.1080p.BluRay.x264-GROUP");
        assert_eq!(results[0].guid, "abc123");
        assert_eq!(results[0].size, Some(14_500_000_000));
        assert_eq!(results[0].seeders, Some(42));
        assert_eq!(results[0].leechers, Some(5));
        assert_eq!(results[0].info_hash.as_deref(), Some("deadbeef"));
        assert_eq!(results[0].categories, vec![2040]);

        assert_eq!(results[1].seeders, Some(100));
        assert_eq!(results[1].size, Some(45_000_000_000));
        assert!(results[1].download_url.is_some());
    }

    #[test]
    fn parse_empty_response() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel></channel></rss>"#;
        let results = parse_torznab_response(xml).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn parse_cdata_wrapped_fields() {
        // Bug #41: many indexers (Jackett, Prowlarr, raw Newznab)
        // wrap title / link / pubDate / guid in CDATA. The old
        // parser only matched `Event::Text`, so the CDATA payload
        // dropped silently — leaving the title empty, which the
        // End-of-item filter then threw away. Result: indexers that
        // actually returned hits looked like they returned nothing.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title><![CDATA[The.Matrix.1999.1080p.BluRay.x264-GROUP]]></title>
      <guid><![CDATA[abc123]]></guid>
      <link><![CDATA[https://example.com/dl/abc?token=xyz&format=torrent]]></link>
      <pubDate><![CDATA[Mon, 01 Jan 2024 00:00:00 +0000]]></pubDate>
      <comments><![CDATA[https://example.com/comments/abc]]></comments>
      <torznab:attr name="seeders" value="42"/>
      <torznab:attr name="infohash" value="deadbeef"/>
    </item>
  </channel>
</rss>"#;
        let results = parse_torznab_response(xml).unwrap();
        assert_eq!(results.len(), 1, "CDATA-wrapped item must survive");
        let r = &results[0];
        assert_eq!(r.title, "The.Matrix.1999.1080p.BluRay.x264-GROUP");
        assert_eq!(r.guid, "abc123");
        assert_eq!(
            r.download_url.as_deref(),
            Some("https://example.com/dl/abc?token=xyz&format=torrent"),
        );
        assert_eq!(
            r.publish_date.as_deref(),
            Some("Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        assert_eq!(
            r.info_url.as_deref(),
            Some("https://example.com/comments/abc"),
        );
        assert_eq!(r.seeders, Some(42));
    }

    #[test]
    fn parse_cdata_magnet_in_guid() {
        // Magnet-in-guid pattern (some indexers ship magnets there
        // instead of in `<link>` or as an attr) must also be picked
        // up from CDATA, otherwise the item lands without a
        // download URL and gets rejected later in the pipeline.
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <item>
      <title><![CDATA[Some.Show.S01E01]]></title>
      <guid><![CDATA[magnet:?xt=urn:btih:abcdef]]></guid>
    </item>
  </channel>
</rss>"#;
        let results = parse_torznab_response(xml).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].magnet_url.as_deref(),
            Some("magnet:?xt=urn:btih:abcdef"),
        );
    }

    #[test]
    fn parse_newznab_attr() {
        let xml = r#"<?xml version="1.0"?>
<rss xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
<channel>
  <item>
    <title>Release Title</title>
    <guid>guid1</guid>
    <newznab:attr name="seeders" value="10"/>
    <newznab:attr name="magneturl" value="magnet:?xt=urn:btih:abc"/>
  </item>
</channel>
</rss>"#;
        let results = parse_torznab_response(xml).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].seeders, Some(10));
        assert_eq!(
            results[0].magnet_url.as_deref(),
            Some("magnet:?xt=urn:btih:abc")
        );
    }
}
