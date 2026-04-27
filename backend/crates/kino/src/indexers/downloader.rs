//! Download URL resolution for cardigann indexers.
//!
//! Many indexer definitions use a two-step download flow: the search
//! result row gives you a link to a *details page* (typically under the
//! `download:` field of the row), and the definition's top-level
//! `download:` block describes how to extract the real magnet/torrent
//! URL from that details page via CSS selectors.
//!
//! If we skip that step and hand the details URL straight to librqbit,
//! it fetches the HTML and fails to bencode-decode it. This module
//! performs the second fetch and runs the selectors so the magnet URL
//! we store can actually be consumed by the torrent client.
//!
//! Supports the full cardigann download vocabulary:
//!   - `selectors` — CSS selectors that yield a magnet/href from the page.
//!   - `before:` — optional intermediate request before extraction (e.g. a
//!     download-token endpoint); may be driven by a `pathselector` that
//!     first scrapes the details page for the before-path.
//!   - `infohash:` — synthesise a public magnet from extracted hash+title.
//!   - `usebeforeresponse` — selector flag telling us to run against the
//!     before-response instead of the main details page.

use std::collections::HashMap;

use scraper::{Html, Selector};

use super::definition::{
    BeforeBlock, CardigannDefinition, DownloadBlock, InfohashBlock, SelectorField,
};
use super::filters::apply_filter;
use super::request::{IndexerClient, RequestSpec};
use super::template::{SearchQuery, TemplateContext, render};

/// Public trackers to seed synthesised magnets with. Neutral, widely-used
/// public announce URLs — same defaults qBittorrent ships with.
const PUBLIC_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://explodie.org:6969/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
];

/// Resolve a stored download URL into one librqbit can consume.
///
/// - `magnet:…` URLs pass through untouched.
/// - Definitions without a `download:` block return the URL as-is — we
///   assume it's a direct `.torrent` download that librqbit can fetch.
/// - Otherwise we follow the cardigann download flow: optional `before:`
///   request, then either `infohash:` synthesis or selector extraction.
pub async fn resolve_download_url<S: std::hash::BuildHasher + Default>(
    definition: &CardigannDefinition,
    client: &IndexerClient,
    config: &HashMap<String, String, S>,
    url: &str,
) -> anyhow::Result<String> {
    if url.starts_with("magnet:") {
        return Ok(url.to_owned());
    }

    let Some(ref block) = definition.download else {
        // No download block — trust the URL as a direct torrent link.
        return Ok(url.to_owned());
    };

    if block.selectors.is_empty() && block.infohash.is_none() {
        return Ok(url.to_owned());
    }

    let context = TemplateContext {
        config: config.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        query: SearchQuery::default(),
        result: HashMap::new(),
    };

    // Fetch bodies lazily — prefer the before-response for selectors
    // flagged with `usebeforeresponse`.
    let mut details_body: Option<String> = None;
    let mut before_body: Option<String> = None;

    if let Some(before) = &block.before {
        before_body = Some(execute_before(client, before, url, &context, &mut details_body).await?);
    }

    // Infohash path takes precedence when present — synthesise a public magnet.
    if let Some(infohash) = &block.infohash {
        return resolve_via_infohash(
            client,
            url,
            infohash,
            &context,
            &mut details_body,
            before_body.as_deref(),
        )
        .await;
    }

    // Selector path — first non-empty href wins.
    resolve_via_selectors(
        client,
        url,
        block,
        &context,
        &mut details_body,
        before_body.as_deref(),
    )
    .await
}

/// Execute the `before:` block, returning its response body.
///
/// The before block can have its path computed from a `pathselector`
/// applied to the details page — in that case we fetch the details page
/// first and cache it for later reuse.
async fn execute_before(
    client: &IndexerClient,
    before: &BeforeBlock,
    details_url: &str,
    context: &TemplateContext,
    details_body: &mut Option<String>,
) -> anyhow::Result<String> {
    // If a pathselector is present, scrape the details page for the
    // before-path.
    let before_path = if let Some(ref pathsel) = before.pathselector {
        let body = ensure_details_body(client, details_url, details_body).await?;

        extract_from_html(body, pathsel, context)
            .ok_or_else(|| anyhow::anyhow!("before-block pathselector produced no value"))?
    } else {
        before.path.clone().unwrap_or_default()
    };

    let before_url = make_absolute(&before_path, details_url);

    let method = before.method.as_deref().unwrap_or("GET").to_uppercase();
    let rendered_inputs: HashMap<String, String> = before
        .inputs
        .iter()
        .map(|(k, v)| (k.clone(), render(v, context)))
        .collect();

    let (url, body) = if method == "POST" {
        (before_url, Some(form_encode(&rendered_inputs)))
    } else if rendered_inputs.is_empty() {
        (before_url, None)
    } else {
        let sep = before.query_separator.as_deref().unwrap_or("&");
        let qs = encode_query(&rendered_inputs, sep);
        let u = if before_url.contains('?') {
            format!("{before_url}&{qs}")
        } else {
            format!("{before_url}?{qs}")
        };
        (u, None)
    };

    let mut headers = reqwest::header::HeaderMap::new();
    if method == "POST" && body.is_some() {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    }

    client
        .execute(&RequestSpec {
            method,
            url,
            headers,
            body,
            response_type: Some("html".into()),
        })
        .await
}

/// Fetch the details page body if we haven't already.
async fn ensure_details_body<'a>(
    client: &IndexerClient,
    url: &str,
    slot: &'a mut Option<String>,
) -> anyhow::Result<&'a str> {
    if slot.is_none() {
        let body = client
            .execute(&RequestSpec {
                method: "GET".into(),
                url: url.to_owned(),
                headers: reqwest::header::HeaderMap::new(),
                body: None,
                response_type: Some("html".into()),
            })
            .await?;
        *slot = Some(body);
    }
    Ok(slot.as_deref().unwrap())
}

async fn resolve_via_infohash(
    client: &IndexerClient,
    url: &str,
    infohash: &InfohashBlock,
    context: &TemplateContext,
    details_body: &mut Option<String>,
    before_body: Option<&str>,
) -> anyhow::Result<String> {
    let body = pick_body(
        client,
        url,
        details_body,
        before_body,
        infohash.usebeforeresponse,
    )
    .await?;

    let hash_sel = infohash
        .hash
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("infohash block missing hash selector"))?;
    let title_sel = infohash
        .title
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("infohash block missing title selector"))?;

    let hash = extract_from_html(&body, hash_sel, context)
        .ok_or_else(|| anyhow::anyhow!("infohash hash selector produced no value"))?;
    let title = extract_from_html(&body, title_sel, context)
        .ok_or_else(|| anyhow::anyhow!("infohash title selector produced no value"))?;

    Ok(build_public_magnet(&hash, &title))
}

async fn resolve_via_selectors(
    client: &IndexerClient,
    url: &str,
    block: &DownloadBlock,
    context: &TemplateContext,
    details_body: &mut Option<String>,
    before_body: Option<&str>,
) -> anyhow::Result<String> {
    for field in &block.selectors {
        let body = pick_body(
            client,
            url,
            details_body,
            before_body,
            field.usebeforeresponse,
        )
        .await?;

        if let Some(value) = extract_from_html(&body, field, context) {
            return Ok(make_absolute(&value, url));
        }
    }

    anyhow::bail!("no download selector produced a value for {url}")
}

/// Decide which response body to feed a selector: the before-response if
/// `usebeforeresponse` is set and a before-response exists; otherwise the
/// details page (fetching it if necessary).
async fn pick_body<'a>(
    client: &IndexerClient,
    url: &str,
    details_body: &'a mut Option<String>,
    before_body: Option<&'a str>,
    use_before: bool,
) -> anyhow::Result<std::borrow::Cow<'a, str>> {
    if use_before && let Some(b) = before_body {
        return Ok(std::borrow::Cow::Borrowed(b));
    }
    let body = ensure_details_body(client, url, details_body).await?;
    Ok(std::borrow::Cow::Borrowed(body))
}

/// Run a selector field against an HTML document, applying filters.
fn extract_from_html(
    body: &str,
    field: &SelectorField,
    context: &TemplateContext,
) -> Option<String> {
    let document = Html::parse_document(body);
    let selector_str = field.selector.as_ref()?;
    let rendered = render(selector_str, context);

    let css = match Selector::parse(&rendered) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(
                selector = %rendered,
                error = ?e,
                "invalid download selector, skipping"
            );
            return None;
        }
    };

    for el in document.select(&css) {
        // Download / infohash blocks default to the `href` attribute when
        // none is specified (unlike search row extractors, which default
        // to text content). For infohash sub-selectors the hash might not
        // live in an href — if the author wants text they set attribute
        // explicitly, so fall back to text when the href is absent.
        let raw = if let Some(ref attr_name) = field.attribute {
            el.value().attr(attr_name).map(str::to_owned)
        } else {
            el.value()
                .attr("href")
                .map(str::to_owned)
                .or_else(|| Some(el.text().collect::<Vec<_>>().concat()))
        }?;

        let mut value = raw;
        for filter in &field.filters {
            match apply_filter(&value, &filter.name, &filter.args) {
                Ok(next) => value = next,
                Err(e) => {
                    tracing::warn!(
                        filter = %filter.name,
                        error = %e,
                        "download-link filter failed, keeping previous value"
                    );
                }
            }
        }

        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    None
}

/// Build a public magnet URI from an info hash + display name, seeded with
/// a list of well-known public trackers so it's usable even when the
/// release page doesn't expose a tracker list.
fn build_public_magnet(hash: &str, title: &str) -> String {
    let mut s = format!(
        "magnet:?xt=urn:btih:{}&dn={}",
        hash,
        urlencoding::encode(title),
    );
    for tr in PUBLIC_TRACKERS {
        s.push_str("&tr=");
        s.push_str(&urlencoding::encode(tr));
    }
    s
}

/// Encode key/value pairs with a custom separator (`query_separator`).
fn encode_query(params: &HashMap<String, String>, sep: &str) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join(sep)
}

/// URL-encode for form bodies.
fn form_encode(params: &HashMap<String, String>) -> String {
    encode_query(params, "&")
}

/// Resolve a potentially-relative URL against the page it was scraped from.
fn make_absolute(candidate: &str, base_url: &str) -> String {
    if candidate.starts_with("magnet:") {
        return candidate.to_owned();
    }
    if candidate.starts_with("//") {
        let scheme = base_url.split_once("://").map_or("https", |(s, _)| s);
        return format!("{scheme}:{candidate}");
    }
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return candidate.to_owned();
    }
    let Ok(base) = reqwest::Url::parse(base_url) else {
        return candidate.to_owned();
    };
    match base.join(candidate) {
        Ok(resolved) => resolved.to_string(),
        Err(_) => candidate.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_magnet_links_through_unchanged() {
        let url = "magnet:?xt=urn:btih:ABC123&dn=movie";
        assert_eq!(make_absolute(url, "https://example.com/page"), url);
    }

    #[test]
    fn resolves_relative_urls() {
        assert_eq!(
            make_absolute("/dl/xyz", "https://example.com/torrent/1"),
            "https://example.com/dl/xyz",
        );
    }

    #[test]
    fn extracts_href_from_html() {
        let html = r#"
            <html><body>
                <a class="other" href="/details">details</a>
                <a class="csprite_dltorrent" href="magnet:?xt=urn:btih:ABC&dn=movie">grab</a>
            </body></html>
        "#;
        let field = SelectorField {
            selector: Some("a.csprite_dltorrent".into()),
            attribute: None, // defaults to href
            usebeforeresponse: false,
            filters: vec![],
        };
        let context = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };
        let result = extract_from_html(html, &field, &context);
        assert_eq!(result.as_deref(), Some("magnet:?xt=urn:btih:ABC&dn=movie"));
    }

    #[test]
    fn synthesises_public_magnet() {
        let magnet = build_public_magnet("DEADBEEF", "Some Release 2025 1080p");
        assert!(magnet.starts_with("magnet:?xt=urn:btih:DEADBEEF&dn="));
        assert!(magnet.contains("&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce"));
    }

    #[test]
    fn extracts_text_when_no_href_present() {
        // Infohash extraction often grabs a table cell's text, not an href.
        let html = "<html><body><table><tr><td class='infohash'>ABC123HASH</td></tr></table></body></html>";
        let field = SelectorField {
            selector: Some("td.infohash".into()),
            attribute: None,
            usebeforeresponse: false,
            filters: vec![],
        };
        let context = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };
        assert_eq!(
            extract_from_html(html, &field, &context).as_deref(),
            Some("ABC123HASH"),
        );
    }
}
