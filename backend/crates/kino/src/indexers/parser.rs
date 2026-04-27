//! Response parser for the Cardigann indexer engine.
//!
//! Parses HTML, JSON, and XML responses from indexer sites into normalized
//! `ParsedRelease` structs using CSS selectors, dot-path navigation, and
//! filter chains defined in the Cardigann YAML definition.

#![allow(dead_code)]

use std::collections::HashMap;

use super::definition::{CardigannDefinition, FilterBlock, SearchBlock, SelectorBlock};
use super::filters::apply_filter;
use super::template::{TemplateContext, render};

/// A release parsed from an indexer response.
///
/// Field names mirror the Cardigann field names. The upstream `mod.rs`
/// converts these into `TorznabRelease` for the rest of the system.
#[derive(Debug, Clone, Default)]
pub struct ParsedRelease {
    pub title: String,
    pub details: Option<String>,
    pub download: Option<String>,
    pub magnet_url: Option<String>,
    pub info_hash: Option<String>,
    pub size: Option<i64>,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    pub grabs: Option<i64>,
    pub publish_date: Option<String>,
    pub category: Option<String>,
    pub download_volume_factor: Option<f64>,
    pub upload_volume_factor: Option<f64>,
}

/// Parse a response body into releases, dispatching to the appropriate parser.
///
/// `response_type` is one of "html" (default), "json", or "xml".
pub fn parse_response(
    definition: &CardigannDefinition,
    response_body: &str,
    response_type: &str,
    context: &TemplateContext,
) -> anyhow::Result<Vec<ParsedRelease>> {
    let Some(ref search) = definition.search else {
        return Ok(Vec::new());
    };

    match response_type {
        "json" => parse_json(search, response_body, context),
        "xml" => {
            // No Cardigann v11 definition currently sets
            // `responsetype: xml` — Torznab RSS (the main XML caller
            // in the pipeline) has its own `quick_xml`-based parser
            // in `torznab::parse`. If a future definition does
            // surface an XML response here, we'd want to swap in a
            // namespaced + CDATA-aware parse path. For now fall
            // through to `parse_html` (scraper handles trivial XML)
            // and log so the gap is visible rather than silent.
            tracing::warn!(
                "cardigann XML responsetype invoked — no namespaced XML parser is wired. \
                 Falling back to the HTML selector engine; releases may be incomplete."
            );
            parse_html(search, response_body, context)
        }
        // HTML (default) and any unknown type use the scraper-
        // backed selector engine.
        _ => parse_html(search, response_body, context),
    }
}

// ── HTML parser ─────────────────────────────────────────────────────

/// Parse an HTML response using CSS selectors from the search block.
fn parse_html(
    search: &SearchBlock,
    html: &str,
    context: &TemplateContext,
) -> anyhow::Result<Vec<ParsedRelease>> {
    let document = scraper::Html::parse_document(html);

    let Some(ref rows_block) = search.rows else {
        return Ok(Vec::new());
    };

    let Some(ref row_selector_str) = rows_block.selector else {
        return Ok(Vec::new());
    };

    let row_selector = scraper::Selector::parse(row_selector_str)
        .map_err(|e| anyhow::anyhow!("invalid row selector '{row_selector_str}': {e:?}"))?;

    if search.fields.is_empty() {
        return Ok(Vec::new());
    }

    // `after` skips N rows from the start (header rows).
    let after = usize::try_from(rows_block.after.max(0)).unwrap_or(0);

    // Track the most recent date header for `dateheaders` mode.
    // When `dateheaders` is set, it's a SelectorBlock describing how to
    // extract dates from header rows that precede groups of data rows.
    let uses_date_headers = rows_block.dateheaders.is_some();
    let mut current_date_header: Option<String> = None;

    let mut releases = Vec::new();

    for (idx, row) in document.select(&row_selector).enumerate() {
        // Skip header rows based on `after`.
        if idx < after {
            continue;
        }

        // Date headers: check if this row looks like a date header rather
        // than a data row. We test by checking if the row has fewer cells
        // than expected or matches the dateheaders selector pattern.
        if uses_date_headers
            && let Some(ref dh_block) = rows_block.dateheaders
            && let Some(ref dh_selector_str) = dh_block.selector
            && let Ok(dh_sel) = scraper::Selector::parse(dh_selector_str)
            && let Some(dh_el) = row.select(&dh_sel).next()
        {
            // This is a date header row — extract the date.
            let raw_date = extract_selector_block_value(&dh_el, dh_block);
            if let Some(date) = raw_date {
                current_date_header = Some(date);
            }
            continue;
        }

        // Extract fields in definition order (important: later fields can
        // reference earlier ones via .Result.fieldname).
        let mut result: HashMap<String, String> = HashMap::new();

        // Inject date from header row if applicable.
        if let Some(ref date) = current_date_header {
            result.insert("date".to_string(), date.clone());
        }

        let mut row_context = context.clone();
        let mut title_found = true;

        for (field_name, selector_block) in &search.fields {
            let value = extract_html_field(&row, selector_block, &row_context);

            match value {
                Some(v) => {
                    result.insert(field_name.clone(), v.clone());
                    row_context.result.insert(field_name.clone(), v);
                }
                None => {
                    // Use default value if specified.
                    if let Some(ref default_val) = selector_block.default {
                        let rendered = render(default_val, &row_context);
                        result.insert(field_name.clone(), rendered.clone());
                        row_context.result.insert(field_name.clone(), rendered);
                    } else if !selector_block.optional && field_name == "title" {
                        // Title is mandatory — skip this row.
                        title_found = false;
                        break;
                    }
                }
            }
        }

        if !title_found {
            continue;
        }

        // Skip rows where title extraction failed or is empty.
        let Some(title) = result.get("title").cloned() else {
            continue;
        };

        if title.is_empty() {
            continue;
        }

        releases.push(build_release(&result));
    }

    Ok(releases)
}

/// Extract a single field value against an HTML document's root element,
/// e.g. selectorinputs on a login landing page. Thin wrapper around
/// `extract_html_field` that finds the document root.
pub fn extract_field_from_document(
    document: &scraper::Html,
    block: &SelectorBlock,
    context: &TemplateContext,
) -> Option<String> {
    let root = document.root_element();
    extract_html_field(&root, block, context)
}

/// Extract a single field value from an HTML row element using a `SelectorBlock`.
pub(super) fn extract_html_field(
    row: &scraper::ElementRef<'_>,
    block: &SelectorBlock,
    context: &TemplateContext,
) -> Option<String> {
    // If there's a `text` template, render it directly from context (no CSS selection).
    if let Some(ref text_template) = block.text {
        let rendered = render(text_template, context);
        let filtered = apply_filters_chain(&rendered, &block.filters);
        return non_empty(apply_case_mapping(&filtered, block.case.as_ref()));
    }

    // Select element within the row.
    let element = if let Some(ref selector_str) = block.selector {
        let selector = scraper::Selector::parse(selector_str).ok()?;

        // Handle `remove` — we find the element first, then work with
        // cleaned text that excludes matching sub-elements.
        if let Some(ref remove_str) = block.remove {
            let el = row.select(&selector).next()?;

            // Get the HTML content, parse it, remove matching elements.
            let inner_html = el.html();
            let fragment = scraper::Html::parse_fragment(&inner_html);

            if let Ok(remove_sel) = scraper::Selector::parse(remove_str) {
                let root_sel =
                    scraper::Selector::parse(":root > *").expect("valid static selector");
                let root = fragment.select(&root_sel).next();

                if let Some(root_el) = root {
                    let text = collect_text_without(&fragment, &root_el, &remove_sel);
                    let trimmed = text.trim().to_string();
                    let filtered = apply_filters_chain(&trimmed, &block.filters);
                    return non_empty(apply_case_mapping(&filtered, block.case.as_ref()));
                }
            }

            Some(el)
        } else {
            row.select(&selector).next()
        }
    } else {
        // No selector — use the row element itself.
        Some(*row)
    };

    let el = element?;

    // Extract value: attribute or text content.
    let raw = if let Some(ref attr_name) = block.attribute {
        el.value().attr(attr_name)?.to_string()
    } else {
        element_text(&el)
    };

    let trimmed = raw.trim().to_string();

    // Apply filter chain.
    let filtered = apply_filters_chain(&trimmed, &block.filters);

    non_empty(apply_case_mapping(&filtered, block.case.as_ref()))
}

/// Extract a value from an element using a `SelectorBlock` (for dateheaders etc.).
fn extract_selector_block_value(
    element: &scraper::ElementRef<'_>,
    block: &SelectorBlock,
) -> Option<String> {
    let raw = if let Some(ref attr_name) = block.attribute {
        element.value().attr(attr_name)?.to_string()
    } else {
        element_text(element)
    };

    let trimmed = raw.trim().to_string();
    let filtered = apply_filters_chain(&trimmed, &block.filters);
    non_empty(filtered)
}

/// Collect text content from an element, excluding descendants that match
/// the given remove selector.
///
/// Works by getting the full text, then subtracting text from removed elements.
fn collect_text_without(
    _document: &scraper::Html,
    element: &scraper::ElementRef<'_>,
    remove_selector: &scraper::Selector,
) -> String {
    // Get the full text content.
    let full_text = element_text(element);

    // Get text from elements that should be removed.
    let mut removed_text = String::new();
    for removed_el in element.select(remove_selector) {
        removed_text.push_str(&element_text(&removed_el));
    }

    // If there's nothing to remove, return the full text.
    if removed_text.is_empty() {
        return full_text;
    }

    // Remove the removed text from the full text. This is an approximation
    // that works for the common case where removed elements contain distinct
    // text fragments (e.g., removing a badge/label from a title cell).
    let result = full_text.replace(&removed_text, "");
    result.trim().to_string()
}

// ── JSON parser ─────────────────────────────────────────────────────

/// Parse a JSON response using dot-path navigation from the search block.
fn parse_json(
    search: &SearchBlock,
    json_str: &str,
    context: &TemplateContext,
) -> anyhow::Result<Vec<ParsedRelease>> {
    let json: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| anyhow::anyhow!("failed to parse JSON response: {e}"))?;

    let Some(ref rows_block) = search.rows else {
        return Ok(Vec::new());
    };

    let Some(ref rows_selector) = rows_block.selector else {
        return Ok(Vec::new());
    };

    if search.fields.is_empty() {
        return Ok(Vec::new());
    }

    // Navigate to the array of items using the rows selector as a dot-path.
    let items = json_navigate(&json, rows_selector)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!("JSON rows selector '{rows_selector}' did not resolve to an array")
        })?;

    let mut releases = Vec::new();

    for item in items {
        let mut result: HashMap<String, String> = HashMap::new();
        let mut row_context = context.clone();
        let mut title_found = true;

        for (field_name, selector_block) in &search.fields {
            let value = extract_json_field(item, selector_block, &row_context);

            match value {
                Some(v) => {
                    result.insert(field_name.clone(), v.clone());
                    row_context.result.insert(field_name.clone(), v);
                }
                None => {
                    if let Some(ref default_val) = selector_block.default {
                        let rendered = render(default_val, &row_context);
                        result.insert(field_name.clone(), rendered.clone());
                        row_context.result.insert(field_name.clone(), rendered);
                    } else if !selector_block.optional && field_name == "title" {
                        title_found = false;
                        break;
                    }
                }
            }
        }

        if !title_found {
            continue;
        }

        let Some(title) = result.get("title").cloned() else {
            continue;
        };

        if title.is_empty() {
            continue;
        }

        releases.push(build_release(&result));
    }

    Ok(releases)
}

/// Extract a field value from a JSON object.
fn extract_json_field(
    item: &serde_json::Value,
    block: &SelectorBlock,
    context: &TemplateContext,
) -> Option<String> {
    // If there's a `text` template, render it.
    if let Some(ref text_template) = block.text {
        let rendered = render(text_template, context);
        let filtered = apply_filters_chain(&rendered, &block.filters);
        return non_empty(apply_case_mapping(&filtered, block.case.as_ref()));
    }

    // Use the selector as a dot-path into the JSON object.
    let raw = if let Some(ref selector_str) = block.selector {
        let value = json_navigate(item, selector_str)?;
        json_value_to_string(value)
    } else {
        json_value_to_string(item)
    };

    let trimmed = raw.trim().to_string();
    let filtered = apply_filters_chain(&trimmed, &block.filters);

    non_empty(apply_case_mapping(&filtered, block.case.as_ref()))
}

/// Navigate a JSON value using a dot-separated path (e.g., "data.items").
fn json_navigate<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        // Try as object key first.
        if let Some(next) = current.get(segment) {
            current = next;
        } else if let Ok(idx) = segment.parse::<usize>() {
            // Try as array index.
            current = current.get(idx)?;
        } else {
            return None;
        }
    }
    Some(current)
}

/// Convert a JSON value to a string representation.
fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ── Field building helpers ──────────────────────────────────────────

/// Apply a chain of filters to a value.
fn apply_filters_chain(value: &str, filters: &[FilterBlock]) -> String {
    if filters.is_empty() {
        return value.to_string();
    }

    let mut result = value.to_string();
    for filter in filters {
        match apply_filter(&result, &filter.name, &filter.args) {
            Ok(filtered) => result = filtered,
            Err(e) => {
                tracing::warn!(filter = %filter.name, error = %e, "filter failed, keeping original value");
            }
        }
    }
    result
}

/// Apply a case mapping (switch/case) if present.
///
/// The case map translates extracted values to different output values.
/// Keys are the potential input values; the map value is the output.
/// A `"*"` key acts as a default/wildcard.
fn apply_case_mapping(value: &str, case: Option<&HashMap<String, String>>) -> String {
    let Some(case_map) = case else {
        return value.to_string();
    };

    if let Some(mapped) = case_map.get(value) {
        return mapped.clone();
    }

    // Wildcard fallback.
    if let Some(default) = case_map.get("*") {
        return default.clone();
    }

    value.to_string()
}

/// Return `Some(s)` if `s` is non-empty, `None` otherwise.
fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// Build a `ParsedRelease` from extracted field values.
///
/// Maps Cardigann field names to struct fields. Fields starting with `_`
/// are internal (used in templates but not included in the output).
fn build_release(fields: &HashMap<String, String>) -> ParsedRelease {
    ParsedRelease {
        title: fields.get("title").cloned().unwrap_or_default(),
        details: fields.get("details").cloned(),
        download: fields.get("download").cloned(),
        magnet_url: fields
            .get("magneturl")
            .cloned()
            .or_else(|| fields.get("magnet_url").cloned())
            .or_else(|| fields.get("magnetUri").cloned()),
        info_hash: fields
            .get("infohash")
            .cloned()
            .or_else(|| fields.get("info_hash").cloned()),
        size: fields.get("size").and_then(|s| parse_size(s)),
        seeders: fields.get("seeders").and_then(|s| parse_int(s)),
        leechers: fields
            .get("leechers")
            .or_else(|| fields.get("peers"))
            .and_then(|s| parse_int(s)),
        grabs: fields.get("grabs").and_then(|s| parse_int(s)),
        publish_date: fields
            .get("date")
            .cloned()
            .or_else(|| fields.get("publish_date").cloned()),
        category: fields
            .get("category")
            .cloned()
            .or_else(|| fields.get("cat").cloned()),
        download_volume_factor: fields
            .get("downloadvolumefactor")
            .and_then(|s| s.parse::<f64>().ok()),
        upload_volume_factor: fields
            .get("uploadvolumefactor")
            .and_then(|s| s.parse::<f64>().ok()),
    }
}

/// Extract the visible text content from an HTML element.
fn element_text(element: &scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string()
}

/// Parse a human-readable size string (e.g., "1.5 GB", "500 MB") into bytes.
fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', "");

    // If it's already a plain integer, return it directly.
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }

    // Also try as float for cases like "14500000000.0".
    #[expect(clippy::cast_possible_truncation)]
    if let Ok(n) = s.parse::<f64>()
        && n >= 0.0
    {
        return Some(n as i64);
    }

    // Try to parse as "number unit" format.
    let s_upper = s.to_uppercase();
    let (num_str, unit) = split_number_unit(&s_upper)?;
    let number: f64 = num_str.parse().ok()?;

    let multiplier: f64 = match unit {
        "B" | "BYTES" | "BYTE" => 1.0,
        "KB" | "KIB" | "K" => 1024.0,
        "MB" | "MIB" | "M" => 1024.0 * 1024.0,
        "GB" | "GIB" | "G" => 1024.0 * 1024.0 * 1024.0,
        "TB" | "TIB" | "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    #[expect(clippy::cast_possible_truncation)]
    Some((number * multiplier) as i64)
}

/// Split a string like "1.5 GB" into ("1.5", "GB").
fn split_number_unit(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let unit_start = s.find(|c: char| c.is_ascii_alphabetic())?;

    let num_part = s[..unit_start].trim();
    let unit_part = s[unit_start..].trim();

    if num_part.is_empty() || unit_part.is_empty() {
        return None;
    }

    Some((num_part, unit_part))
}

/// Parse a string to i64, handling comma-separated numbers and whitespace.
fn parse_int(s: &str) -> Option<i64> {
    let cleaned: String = s.trim().replace(',', "");

    // Try direct parse first.
    if let Ok(n) = cleaned.parse::<i64>() {
        return Some(n);
    }

    // Handle floats like "42.0" by truncating.
    #[expect(clippy::cast_possible_truncation)]
    cleaned.parse::<f64>().ok().map(|f| f as i64)
}

#[cfg(test)]
mod tests {
    use super::super::definition::RowsBlock;
    use super::super::template::SearchQuery;
    use super::*;
    use indexmap::IndexMap;

    fn make_search_block(row_selector: &str, fields: Vec<(&str, SelectorBlock)>) -> SearchBlock {
        let mut field_map = IndexMap::new();
        for (name, block) in fields {
            field_map.insert(name.to_string(), block);
        }
        SearchBlock {
            rows: Some(RowsBlock {
                selector: Some(row_selector.to_string()),
                ..Default::default()
            }),
            fields: field_map,
            ..Default::default()
        }
    }

    #[test]
    fn parse_html_basic() {
        let html = r#"
        <html><body>
        <table>
            <tr>
                <td class="name"><a href="/torrent/123">Test.Release.720p</a></td>
                <td class="size">1.5 GB</td>
                <td class="seeds">42</td>
                <td class="leeches">5</td>
            </tr>
            <tr>
                <td class="name"><a href="/torrent/456">Another.Release.1080p</a></td>
                <td class="size">3.2 GB</td>
                <td class="seeds">100</td>
                <td class="leeches">10</td>
            </tr>
        </table>
        </body></html>
        "#;

        let search = make_search_block(
            "table tr",
            vec![
                (
                    "title",
                    SelectorBlock {
                        selector: Some("td.name a".into()),
                        ..Default::default()
                    },
                ),
                (
                    "details",
                    SelectorBlock {
                        selector: Some("td.name a".into()),
                        attribute: Some("href".into()),
                        ..Default::default()
                    },
                ),
                (
                    "size",
                    SelectorBlock {
                        selector: Some("td.size".into()),
                        ..Default::default()
                    },
                ),
                (
                    "seeders",
                    SelectorBlock {
                        selector: Some("td.seeds".into()),
                        ..Default::default()
                    },
                ),
                (
                    "leechers",
                    SelectorBlock {
                        selector: Some("td.leeches".into()),
                        ..Default::default()
                    },
                ),
            ],
        );

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].title, "Test.Release.720p");
        assert_eq!(results[0].details.as_deref(), Some("/torrent/123"));
        assert_eq!(results[0].size, Some(1_610_612_736)); // 1.5 GB
        assert_eq!(results[0].seeders, Some(42));
        assert_eq!(results[0].leechers, Some(5));

        assert_eq!(results[1].title, "Another.Release.1080p");
        assert_eq!(results[1].size, Some(3_435_973_836)); // 3.2 GB
    }

    #[test]
    fn parse_html_with_optional_field() {
        let html = r#"
        <html><body>
        <table>
            <tr>
                <td class="name">Title Here</td>
            </tr>
        </table>
        </body></html>
        "#;

        let search = make_search_block(
            "table tr",
            vec![
                (
                    "title",
                    SelectorBlock {
                        selector: Some("td.name".into()),
                        ..Default::default()
                    },
                ),
                (
                    "seeders",
                    SelectorBlock {
                        selector: Some("td.seeds".into()),
                        optional: true,
                        ..Default::default()
                    },
                ),
            ],
        );

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Title Here");
        assert!(results[0].seeders.is_none());
    }

    #[test]
    fn parse_json_basic() {
        let json = r#"{
            "data": {
                "items": [
                    {
                        "name": "Test.Release.720p",
                        "link": "/download/123",
                        "size": 1610612736,
                        "seed": 42,
                        "leech": 5
                    },
                    {
                        "name": "Another.Release.1080p",
                        "link": "/download/456",
                        "size": 3435973837,
                        "seed": 100,
                        "leech": 10
                    }
                ]
            }
        }"#;

        let mut fields = IndexMap::new();
        fields.insert(
            "title".into(),
            SelectorBlock {
                selector: Some("name".into()),
                ..Default::default()
            },
        );
        fields.insert(
            "download".into(),
            SelectorBlock {
                selector: Some("link".into()),
                ..Default::default()
            },
        );
        fields.insert(
            "size".into(),
            SelectorBlock {
                selector: Some("size".into()),
                ..Default::default()
            },
        );
        fields.insert(
            "seeders".into(),
            SelectorBlock {
                selector: Some("seed".into()),
                ..Default::default()
            },
        );
        fields.insert(
            "leechers".into(),
            SelectorBlock {
                selector: Some("leech".into()),
                ..Default::default()
            },
        );

        let search = SearchBlock {
            rows: Some(RowsBlock {
                selector: Some("data.items".into()),
                ..Default::default()
            }),
            fields,
            ..Default::default()
        };

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_json(&search, json, &ctx).unwrap();
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].title, "Test.Release.720p");
        assert_eq!(results[0].download.as_deref(), Some("/download/123"));
        assert_eq!(results[0].size, Some(1_610_612_736));
        assert_eq!(results[0].seeders, Some(42));
        assert_eq!(results[0].leechers, Some(5));
    }

    #[test]
    fn parse_size_various_formats() {
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("1.5 GB"), Some(1_610_612_736));
        assert_eq!(parse_size("500 MB"), Some(524_288_000));
        assert_eq!(parse_size("1 TB"), Some(1_099_511_627_776));
        assert_eq!(parse_size("100 KB"), Some(102_400));
        assert_eq!(parse_size("1,024"), Some(1024));
        assert_eq!(parse_size("  2.0 GB  "), Some(2_147_483_648));
        assert!(parse_size("invalid").is_none());
    }

    #[test]
    fn parse_int_with_formatting() {
        assert_eq!(parse_int("42"), Some(42));
        assert_eq!(parse_int("1,234"), Some(1234));
        assert_eq!(parse_int("  100  "), Some(100));
        assert!(parse_int("abc").is_none());
    }

    #[test]
    fn json_navigate_nested() {
        let json: serde_json::Value = serde_json::json!({
            "data": {
                "items": [
                    {"name": "first"},
                    {"name": "second"}
                ]
            }
        });

        let items = json_navigate(&json, "data.items").unwrap();
        assert!(items.is_array());
        assert_eq!(items.as_array().unwrap().len(), 2);

        let first_name = json_navigate(&json, "data.items.0.name").unwrap();
        assert_eq!(first_name.as_str(), Some("first"));
    }

    #[test]
    fn apply_filters_chain_empty() {
        let result = apply_filters_chain("hello", &[]);
        assert_eq!(result, "hello");
    }

    #[test]
    fn parse_html_with_filters() {
        let html = r#"
        <html><body>
        <table>
            <tr>
                <td class="name">  Test.Release  </td>
                <td class="size">1.5 GB</td>
            </tr>
        </table>
        </body></html>
        "#;

        let search = make_search_block(
            "table tr",
            vec![
                (
                    "title",
                    SelectorBlock {
                        selector: Some("td.name".into()),
                        filters: vec![FilterBlock {
                            name: "trim".into(),
                            args: vec![],
                        }],
                        ..Default::default()
                    },
                ),
                (
                    "size",
                    SelectorBlock {
                        selector: Some("td.size".into()),
                        ..Default::default()
                    },
                ),
            ],
        );

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Test.Release");
    }

    #[test]
    fn parse_html_skip_after_rows() {
        let html = r#"
        <html><body>
        <table>
            <tr><td class="name">Header Row</td></tr>
            <tr><td class="name">Actual Data</td></tr>
        </table>
        </body></html>
        "#;

        let mut fields = IndexMap::new();
        fields.insert(
            "title".into(),
            SelectorBlock {
                selector: Some("td.name".into()),
                ..Default::default()
            },
        );

        let search = SearchBlock {
            rows: Some(RowsBlock {
                selector: Some("table tr".into()),
                after: 1,
                ..Default::default()
            }),
            fields,
            ..Default::default()
        };

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Actual Data");
    }

    #[test]
    fn parse_html_attribute_extraction() {
        let html = r#"
        <html><body>
        <table>
            <tr>
                <td class="name">Some Title</td>
                <td class="cat" data-id="5">Movies</td>
            </tr>
        </table>
        </body></html>
        "#;

        let search = make_search_block(
            "table tr",
            vec![
                (
                    "title",
                    SelectorBlock {
                        selector: Some("td.name".into()),
                        ..Default::default()
                    },
                ),
                (
                    "category",
                    SelectorBlock {
                        selector: Some("td.cat".into()),
                        attribute: Some("data-id".into()),
                        ..Default::default()
                    },
                ),
            ],
        );

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].category.as_deref(), Some("5"));
    }

    #[test]
    fn build_release_maps_field_names() {
        let mut fields = HashMap::new();
        fields.insert("title".to_string(), "My Release".to_string());
        fields.insert(
            "magneturl".to_string(),
            "magnet:?xt=urn:btih:abc".to_string(),
        );
        fields.insert("size".to_string(), "1024".to_string());
        fields.insert("seeders".to_string(), "50".to_string());
        fields.insert("date".to_string(), "2024-01-01".to_string());
        fields.insert("downloadvolumefactor".to_string(), "0".to_string());
        fields.insert("uploadvolumefactor".to_string(), "1".to_string());

        let release = build_release(&fields);
        assert_eq!(release.title, "My Release");
        assert_eq!(
            release.magnet_url.as_deref(),
            Some("magnet:?xt=urn:btih:abc")
        );
        assert_eq!(release.size, Some(1024));
        assert_eq!(release.seeders, Some(50));
        assert_eq!(release.publish_date.as_deref(), Some("2024-01-01"));
        assert_eq!(release.download_volume_factor, Some(0.0));
        assert_eq!(release.upload_volume_factor, Some(1.0));
    }

    #[test]
    fn case_mapping_exact_match() {
        let mut case = HashMap::new();
        case.insert("1".into(), "Movies".into());
        case.insert("2".into(), "TV".into());

        assert_eq!(apply_case_mapping("1", Some(&case)), "Movies");
        assert_eq!(apply_case_mapping("2", Some(&case)), "TV");
    }

    #[test]
    fn case_mapping_wildcard_fallback() {
        let mut case = HashMap::new();
        case.insert("1".into(), "Movies".into());
        case.insert("*".into(), "Other".into());

        assert_eq!(apply_case_mapping("1", Some(&case)), "Movies");
        assert_eq!(apply_case_mapping("99", Some(&case)), "Other");
    }

    #[test]
    fn case_mapping_none() {
        assert_eq!(apply_case_mapping("hello", None), "hello");
    }

    #[test]
    fn parse_html_with_default_value() {
        let html = r#"
        <html><body>
        <table>
            <tr>
                <td class="name">Title Here</td>
            </tr>
        </table>
        </body></html>
        "#;

        let search = make_search_block(
            "table tr",
            vec![
                (
                    "title",
                    SelectorBlock {
                        selector: Some("td.name".into()),
                        ..Default::default()
                    },
                ),
                (
                    "downloadvolumefactor",
                    SelectorBlock {
                        selector: Some("td.freeleech".into()),
                        optional: true,
                        default: Some("1".into()),
                        ..Default::default()
                    },
                ),
            ],
        );

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].download_volume_factor, Some(1.0));
    }

    #[test]
    fn parse_html_text_template() {
        let html = r#"
        <html><body>
        <table>
            <tr>
                <td class="name">My Title</td>
                <td class="link" data-url="/dl/123">Download</td>
            </tr>
        </table>
        </body></html>
        "#;

        let search = make_search_block(
            "table tr",
            vec![
                (
                    "title",
                    SelectorBlock {
                        selector: Some("td.name".into()),
                        ..Default::default()
                    },
                ),
                (
                    "_link",
                    SelectorBlock {
                        selector: Some("td.link".into()),
                        attribute: Some("data-url".into()),
                        ..Default::default()
                    },
                ),
                (
                    "download",
                    SelectorBlock {
                        text: Some("{{ .Result._link }}".into()),
                        ..Default::default()
                    },
                ),
            ],
        );

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let results = parse_html(&search, html, &ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].download.as_deref(), Some("/dl/123"));
    }
}
