#![allow(dead_code)] // Used by indexer engine in later phases

use chrono::{Duration, NaiveDateTime, TimeZone, Utc};

/// Apply a named filter to an input string with optional arguments.
///
/// Each filter is a pure transformation: `input` in, `String` out. Filters are
/// chained sequentially in Cardigann YAML definitions to transform extracted
/// field values (e.g., parse dates, clean up titles, extract URLs).
pub fn apply_filter(input: &str, name: &str, args: &[String]) -> Result<String, FilterError> {
    match name {
        // String manipulation
        "replace" => filter_replace(input, args),
        "re_replace" => filter_re_replace(input, args),
        "regexp" => filter_regexp(input, args),
        "split" => filter_split(input, args),
        "trim" => filter_trim(input, args),
        "append" => filter_append(input, args),
        "prepend" => filter_prepend(input, args),
        "tolower" => Ok(input.to_lowercase()),
        "toupper" => Ok(input.to_uppercase()),

        // Encoding
        "urlencode" => Ok(urlencoding::encode(input).into_owned()),
        "urldecode" => Ok(urlencoding::decode(input)
            .unwrap_or(std::borrow::Cow::Borrowed(input))
            .into_owned()),
        "htmldecode" => Ok(decode_html_entities(input)),
        "htmlencode" => Ok(encode_html_entities(input)),
        "hexdump" => Ok(hex::encode(input.as_bytes())),

        // Date
        "dateparse" | "timeparse" => filter_dateparse(input, args),
        "timeago" | "reltime" => filter_timeago(input),
        "fuzzytime" => filter_fuzzytime(input),

        // Unicode
        "diacritics" => Ok(filter_diacritics(input, args)),

        // Utility
        "querystring" => filter_querystring(input, args),
        "validfilename" => Ok(filter_validfilename(input)),
        "jsonjoinarray" => filter_jsonjoinarray(input, args),
        // `strdump` is a debug no-op. Cardigann also has several names that
        // only make sense as template/config helpers (boolean logic, UI
        // field types). When they show up inside a `filters:` chain we
        // pass the value through so the chain doesn't blow up — the real
        // semantics live in the template engine.
        "strdump" | "contains" | "has" | "eq" | "ne" | "and" | "or" | "not" | "select" | "text"
        | "info" | "validate" | "checkbox" | "password" => Ok(input.to_owned()),

        _ => Err(FilterError::UnknownFilter(name.to_owned())),
    }
}

/// Errors that can occur during filter application.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("unknown filter: {0}")]
    UnknownFilter(String),
    #[error("filter '{filter}' requires {expected} argument(s), got {got}")]
    MissingArgs {
        filter: String,
        expected: usize,
        got: usize,
    },
    #[error("regex error in filter '{filter}': {source}")]
    Regex {
        filter: String,
        source: regex::Error,
    },
    #[error("date parse error in filter '{filter}': {message}")]
    DateParse { filter: String, message: String },
    #[error("json error in filter '{filter}': {message}")]
    Json { filter: String, message: String },
}

// ---------------------------------------------------------------------------
// String manipulation filters
// ---------------------------------------------------------------------------

/// `replace(from, to)` — literal string replacement.
fn filter_replace(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("replace", args, 2)?;
    Ok(input.replace(args[0].as_str(), args[1].as_str()))
}

/// `re_replace(pattern, replacement)` — regex-based replacement.
fn filter_re_replace(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("re_replace", args, 2)?;
    let re = regex::Regex::new(&args[0]).map_err(|e| FilterError::Regex {
        filter: "re_replace".to_owned(),
        source: e,
    })?;
    Ok(re.replace_all(input, args[1].as_str()).into_owned())
}

/// `regexp(pattern)` — extract first capture group (or full match if no groups).
fn filter_regexp(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("regexp", args, 1)?;
    let re = regex::Regex::new(&args[0]).map_err(|e| FilterError::Regex {
        filter: "regexp".to_owned(),
        source: e,
    })?;
    match re.captures(input) {
        Some(caps) => {
            // Return first capture group if it exists, otherwise the full match.
            let result = caps
                .get(1)
                .or_else(|| caps.get(0))
                .map_or("", |m| m.as_str());
            Ok(result.to_owned())
        }
        None => Ok(String::new()),
    }
}

/// `split(separator, index)` — split by separator, pick element at index.
fn filter_split(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("split", args, 2)?;
    let idx: usize = args[1].parse().unwrap_or(0);
    let parts: Vec<&str> = input.split(args[0].as_str()).collect();
    Ok(parts.get(idx).unwrap_or(&"").to_string())
}

/// `trim(cutset)` — trim characters. If no cutset, trim whitespace.
#[allow(clippy::unnecessary_wraps)] // consistent Result return for all filters
fn filter_trim(input: &str, args: &[String]) -> Result<String, FilterError> {
    if args.is_empty() || args[0].is_empty() {
        return Ok(input.trim().to_owned());
    }
    let chars: Vec<char> = args[0].chars().collect();
    Ok(input.trim_matches(chars.as_slice()).to_owned())
}

/// `append(text)` — append literal text.
fn filter_append(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("append", args, 1)?;
    Ok(format!("{input}{}", args[0]))
}

/// `prepend(text)` — prepend literal text.
fn filter_prepend(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("prepend", args, 1)?;
    Ok(format!("{}{input}", args[0]))
}

// ---------------------------------------------------------------------------
// Encoding filters
// ---------------------------------------------------------------------------

/// Decode common HTML entities.
fn decode_html_entities(input: &str) -> String {
    let mut result = input.to_owned();

    // Named entities — most common ones used in torrent tracker output
    let entities = [
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&nbsp;", " "),
        ("&ndash;", "\u{2013}"),
        ("&mdash;", "\u{2014}"),
        ("&laquo;", "\u{00AB}"),
        ("&raquo;", "\u{00BB}"),
        ("&copy;", "\u{00A9}"),
        ("&reg;", "\u{00AE}"),
        ("&trade;", "\u{2122}"),
        ("&hellip;", "\u{2026}"),
    ];

    for (entity, replacement) in entities {
        result = result.replace(entity, replacement);
    }

    // Numeric entities: &#123; or &#x1F; (decimal or hex)
    let re_decimal = regex::Regex::new(r"&#(\d+);").expect("valid regex");
    result = re_decimal
        .replace_all(&result, |caps: &regex::Captures| {
            caps.get(1)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .and_then(char::from_u32)
                .map_or_else(|| caps[0].to_owned(), |c| c.to_string())
        })
        .into_owned();

    let re_hex = regex::Regex::new(r"&#[xX]([0-9a-fA-F]+);").expect("valid regex");
    result = re_hex
        .replace_all(&result, |caps: &regex::Captures| {
            caps.get(1)
                .and_then(|m| u32::from_str_radix(m.as_str(), 16).ok())
                .and_then(char::from_u32)
                .map_or_else(|| caps[0].to_owned(), |c| c.to_string())
        })
        .into_owned();

    result
}

/// HTML-encode the common special characters. Inverse of `decode_html_entities`.
/// We encode the minimal safe set (&, <, >, ", ') — enough for text contexts
/// and attribute values. Cardigann definitions only use this for URL params
/// or search queries where the tracker expects escaped input.
fn encode_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Normalize diacritics (accented characters) to plain ASCII. Used by
/// several European trackers for search-query normalization so "Poupée"
/// matches against "Poupee" in their database. The optional `args`
/// element is a string mode:
///   - `"replace"` (default) — transliterate to ASCII via deunicode.
///   - `"strip"`             — drop combining marks, leave base chars.
///   - `"keep"`              — passthrough (the filter is a no-op).
fn filter_diacritics(input: &str, args: &[String]) -> String {
    let mode = args.first().map_or("replace", String::as_str);
    match mode {
        "keep" => input.to_owned(),
        "strip" => input
            .chars()
            .filter(|c| {
                // Drop common combining-mark ranges without pulling in a
                // full unicode tables crate.
                !matches!(*c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF)
            })
            .collect(),
        _ => deunicode::deunicode(input),
    }
}

// ---------------------------------------------------------------------------
// Date filters
// ---------------------------------------------------------------------------

/// `dateparse(go_layout)` — parse a date string using a Go-style layout and
/// return a Unix timestamp as a string.
///
/// Go reference time: Mon Jan 2 15:04:05 MST 2006
fn filter_dateparse(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("dateparse", args, 1)?;
    let go_layout = &args[0];
    let chrono_fmt = go_layout_to_chrono(go_layout);
    let input = input.trim();

    // Try parsing as NaiveDateTime first (no timezone)
    if let Ok(dt) = NaiveDateTime::parse_from_str(input, &chrono_fmt) {
        let utc = Utc.from_utc_datetime(&dt);
        return Ok(utc.timestamp().to_string());
    }

    // Try with timezone info via chrono::DateTime
    if let Ok(dt) = chrono::DateTime::parse_from_str(input, &chrono_fmt) {
        return Ok(dt.timestamp().to_string());
    }

    // Try just a date (no time component)
    if let Ok(d) = chrono::NaiveDate::parse_from_str(input, &chrono_fmt) {
        let dt = d.and_hms_opt(0, 0, 0).expect("midnight is always valid");
        let utc = Utc.from_utc_datetime(&dt);
        return Ok(utc.timestamp().to_string());
    }

    Err(FilterError::DateParse {
        filter: "dateparse".to_owned(),
        message: format!(
            "could not parse '{input}' with layout '{go_layout}' (chrono: '{chrono_fmt}')"
        ),
    })
}

/// Convert a Go time format layout to a chrono strftime format string.
///
/// Go's reference time is: Mon Jan 2 15:04:05 MST 2006
/// Each component in the reference time maps to a specific format specifier.
///
/// Replacements must be applied longest-first to avoid partial matches
/// (e.g., "2006" before "06", "January" before "Jan", "Monday" before "Mon").
fn go_layout_to_chrono(go_layout: &str) -> String {
    let mut result = go_layout.to_owned();

    // Order matters: longest tokens first to prevent partial replacement.
    // Each entry is (go_token, chrono_token, unique_placeholder).
    // Placeholders use Unicode private use area sequences that cannot appear
    // in normal text or in Go format tokens, ensuring no cross-contamination.
    let replacements: &[(&str, &str)] = &[
        // Timezone (longest first to prevent partial matches)
        ("-07:00", "%:z"),
        ("-0700", "%z"),
        ("-07", "%:z"),
        ("Z07:00", "%:z"),
        ("Z0700", "%z"),
        ("MST", "%Z"),
        // Year (4-digit before 2-digit)
        ("2006", "%Y"),
        // Month text (before numeric, longest first)
        ("January", "%B"),
        ("Jan", "%b"),
        // Day text (before numeric, longest first)
        ("Monday", "%A"),
        ("Mon", "%a"),
        // Two-digit tokens with leading zero — must come before single-digit
        // and before shorter numeric tokens to avoid partial matches.
        ("05", "%S"),
        ("04", "%M"),
        ("03", "%-I"),
        ("02", "%d"),
        ("01", "%m"),
        ("15", "%H"),
        // AM/PM
        ("PM", "%p"),
        ("pm", "%P"),
        // Two-digit year (after 2006 is already replaced)
        ("06", "%y"),
        // Single-digit tokens — only match after all multi-char tokens are
        // replaced. These are rare in practice.
        ("5", "%-S"),
        ("4", "%-M"),
        ("3", "%-I"),
    ];

    // Two-pass replacement using Unicode Private Use Area placeholders.
    // Each placeholder is a unique sequence: \u{F0000} + index-as-PUA-char + \u{F0001}.
    // Since the index is encoded as a PUA codepoint (not ASCII digits), no
    // replacement token can accidentally match inside another placeholder.
    let placeholders: Vec<String> = (0..replacements.len())
        .map(|i| {
            let idx_char = char::from_u32(0xF0100 + u32::try_from(i).expect("index fits in u32"))
                .unwrap_or('\u{FFFD}');
            format!("\u{F0000}{idx_char}\u{F0001}")
        })
        .collect();

    // Pass 1: replace all Go tokens with unique placeholders
    for (i, (go_token, _)) in replacements.iter().enumerate() {
        result = result.replace(go_token, &placeholders[i]);
    }

    // Pass 2: replace placeholders with chrono format specifiers
    for (i, (_, chrono_token)) in replacements.iter().enumerate() {
        result = result.replace(&placeholders[i], chrono_token);
    }

    result
}

/// `timeago` — parse relative time expressions like "2 hours ago", "5 days ago"
/// into a Unix timestamp string.
fn filter_timeago(input: &str) -> Result<String, FilterError> {
    let input = input.trim().to_lowercase();

    if input == "now" || input == "just now" {
        return Ok(Utc::now().timestamp().to_string());
    }

    if input == "today" {
        return Ok(Utc::now().timestamp().to_string());
    }

    if input == "yesterday" {
        let dt = Utc::now() - Duration::days(1);
        return Ok(dt.timestamp().to_string());
    }

    // Pattern: "<number> <unit> ago"
    let re = regex::Regex::new(r"(\d+)\s*(second|minute|hour|day|week|month|year)s?\s*ago")
        .expect("valid regex");

    if let Some(caps) = re.captures(&input) {
        let amount: i64 = caps[1].parse().unwrap_or(0);
        let unit = &caps[2];

        let duration = match unit {
            "second" => Duration::seconds(amount),
            "minute" => Duration::minutes(amount),
            "hour" => Duration::hours(amount),
            "day" => Duration::days(amount),
            "week" => Duration::weeks(amount),
            "month" => Duration::days(amount * 30),
            "year" => Duration::days(amount * 365),
            _ => Duration::zero(),
        };

        let dt = Utc::now() - duration;
        return Ok(dt.timestamp().to_string());
    }

    Err(FilterError::DateParse {
        filter: "timeago".to_owned(),
        message: format!("could not parse relative time '{input}'"),
    })
}

/// `fuzzytime` — try `timeago` first, then common absolute date formats.
fn filter_fuzzytime(input: &str) -> Result<String, FilterError> {
    // Try timeago first
    if let Ok(result) = filter_timeago(input) {
        return Ok(result);
    }

    let input = input.trim();

    // Try common date/time formats
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%SZ",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%d-%m-%Y %H:%M:%S",
        "%d-%m-%Y %H:%M",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M",
        "%b %d, %Y",
        "%B %d, %Y",
        "%d %b %Y",
        "%d %B %Y",
        "%Y-%m-%d",
        "%d-%m-%Y",
        "%d/%m/%Y",
        "%m/%d/%Y",
    ];

    // Try parsing with timezone
    for fmt in &formats {
        if let Ok(dt) = chrono::DateTime::parse_from_str(input, fmt) {
            return Ok(dt.timestamp().to_string());
        }
    }

    // Try parsing as naive datetime
    for fmt in &formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(input, fmt) {
            let utc = Utc.from_utc_datetime(&dt);
            return Ok(utc.timestamp().to_string());
        }
    }

    // Try parsing as naive date
    for fmt in &formats {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(input, fmt) {
            let dt = d.and_hms_opt(0, 0, 0).expect("midnight is always valid");
            let utc = Utc.from_utc_datetime(&dt);
            return Ok(utc.timestamp().to_string());
        }
    }

    // Try Unix timestamp (already a number)
    if let Ok(ts) = input.parse::<i64>() {
        return Ok(ts.to_string());
    }

    Err(FilterError::DateParse {
        filter: "fuzzytime".to_owned(),
        message: format!("could not parse date '{input}'"),
    })
}

// ---------------------------------------------------------------------------
// Utility filters
// ---------------------------------------------------------------------------

/// `querystring(param)` — extract a query parameter from a URL.
fn filter_querystring(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("querystring", args, 1)?;
    let param = &args[0];

    // Find the query string portion
    let query_part = if let Some(pos) = input.find('?') {
        &input[pos + 1..]
    } else {
        // Maybe the input is just the query string already
        input
    };

    // Strip fragment
    let query_part = query_part.split('#').next().unwrap_or(query_part);

    for pair in query_part.split('&') {
        let mut kv = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (kv.next(), kv.next())
            && key == param.as_str()
        {
            return Ok(urlencoding::decode(value)
                .unwrap_or(std::borrow::Cow::Borrowed(value))
                .into_owned());
        }
    }

    Ok(String::new())
}

/// `validfilename` — replace characters invalid in file paths.
fn filter_validfilename(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => result.push('_'),
            c if c.is_control() => result.push('_'),
            c => result.push(c),
        }
    }
    result
}

/// `jsonjoinarray(path, separator)` — parse JSON, navigate to an array at the
/// given dot-path, and join its string values with the separator.
fn filter_jsonjoinarray(input: &str, args: &[String]) -> Result<String, FilterError> {
    require_args("jsonjoinarray", args, 2)?;
    let path = &args[0];
    let separator = &args[1];

    let parsed: serde_json::Value = serde_json::from_str(input).map_err(|e| FilterError::Json {
        filter: "jsonjoinarray".to_owned(),
        message: e.to_string(),
    })?;

    let value = navigate_json(&parsed, path);

    match value {
        Some(serde_json::Value::Array(arr)) => {
            let items: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            Ok(items.join(separator))
        }
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        Some(other) => Ok(other.to_string()),
        None => Ok(String::new()),
    }
}

/// Navigate a JSON value by dot-separated path (e.g., "data.items").
fn navigate_json<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.is_empty() || path == "$" {
        return Some(value);
    }

    let path = path.trim_start_matches('$').trim_start_matches('.');

    let mut current = value;
    for segment in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(segment)?;
            }
            serde_json::Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_args(filter: &str, args: &[String], expected: usize) -> Result<(), FilterError> {
    if args.len() < expected {
        return Err(FilterError::MissingArgs {
            filter: filter.to_owned(),
            expected,
            got: args.len(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- String filters --

    #[test]
    fn test_replace() {
        let result = apply_filter("hello world", "replace", &args(&["hello", "goodbye"]));
        assert_eq!(result.unwrap(), "goodbye world");
    }

    #[test]
    fn test_re_replace() {
        let result = apply_filter("foo  bar   baz", "re_replace", &args(&[r"\s+", " "]));
        assert_eq!(result.unwrap(), "foo bar baz");
    }

    #[test]
    fn test_regexp_with_group() {
        let result = apply_filter("Size: 1.5 GB", "regexp", &args(&[r"Size:\s*(.+)"]));
        assert_eq!(result.unwrap(), "1.5 GB");
    }

    #[test]
    fn test_regexp_no_match() {
        let result = apply_filter("hello", "regexp", &args(&[r"\d+"]));
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_regexp_no_group() {
        let result = apply_filter("abc123def", "regexp", &args(&[r"\d+"]));
        assert_eq!(result.unwrap(), "123");
    }

    #[test]
    fn test_split() {
        let result = apply_filter("a|b|c", "split", &args(&["|", "1"]));
        assert_eq!(result.unwrap(), "b");
    }

    #[test]
    fn test_split_out_of_range() {
        let result = apply_filter("a|b", "split", &args(&["|", "5"]));
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_trim_default() {
        let result = apply_filter("  hello  ", "trim", &[]);
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_trim_custom() {
        let result = apply_filter("***hello***", "trim", &args(&["*"]));
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_append() {
        let result = apply_filter("hello", "append", &args(&[" world"]));
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_prepend() {
        let result = apply_filter("world", "prepend", &args(&["hello "]));
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_tolower() {
        let result = apply_filter("HELLO World", "tolower", &[]);
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_toupper() {
        let result = apply_filter("hello world", "toupper", &[]);
        assert_eq!(result.unwrap(), "HELLO WORLD");
    }

    // -- Encoding filters --

    #[test]
    fn test_urlencode() {
        let result = apply_filter("hello world&foo=bar", "urlencode", &[]);
        assert_eq!(result.unwrap(), "hello%20world%26foo%3Dbar");
    }

    #[test]
    fn test_urldecode() {
        let result = apply_filter("hello%20world%26foo", "urldecode", &[]);
        assert_eq!(result.unwrap(), "hello world&foo");
    }

    #[test]
    fn test_htmldecode_named() {
        let result = apply_filter("a &amp; b &lt; c &gt; d", "htmldecode", &[]);
        assert_eq!(result.unwrap(), "a & b < c > d");
    }

    #[test]
    fn test_htmldecode_numeric() {
        let result = apply_filter("&#65;&#66;&#67;", "htmldecode", &[]);
        assert_eq!(result.unwrap(), "ABC");
    }

    #[test]
    fn test_htmldecode_hex() {
        let result = apply_filter("&#x41;&#x42;", "htmldecode", &[]);
        assert_eq!(result.unwrap(), "AB");
    }

    // -- Date filters --

    #[test]
    fn test_dateparse_iso() {
        let result = apply_filter(
            "2024-01-15 10:30:00",
            "dateparse",
            &args(&["2006-01-02 15:04:05"]),
        );
        let ts: i64 = result.unwrap().parse().unwrap();
        // 2024-01-15 10:30:00 UTC
        assert_eq!(ts, 1_705_314_600);
    }

    #[test]
    fn test_dateparse_date_only() {
        let result = apply_filter("2024-01-15", "dateparse", &args(&["2006-01-02"]));
        let ts: i64 = result.unwrap().parse().unwrap();
        // 2024-01-15 00:00:00 UTC
        assert_eq!(ts, 1_705_276_800);
    }

    #[test]
    fn test_timeago_hours() {
        let result = apply_filter("2 hours ago", "timeago", &[]);
        let ts: i64 = result.unwrap().parse().unwrap();
        let expected = (Utc::now() - Duration::hours(2)).timestamp();
        // Allow 2 second tolerance for test execution time
        assert!((ts - expected).abs() < 2);
    }

    #[test]
    fn test_timeago_days() {
        let result = apply_filter("5 days ago", "timeago", &[]);
        let ts: i64 = result.unwrap().parse().unwrap();
        let expected = (Utc::now() - Duration::days(5)).timestamp();
        assert!((ts - expected).abs() < 2);
    }

    #[test]
    fn test_timeago_now() {
        let result = apply_filter("now", "timeago", &[]);
        let ts: i64 = result.unwrap().parse().unwrap();
        let expected = Utc::now().timestamp();
        assert!((ts - expected).abs() < 2);
    }

    #[test]
    fn test_timeago_yesterday() {
        let result = apply_filter("yesterday", "timeago", &[]);
        let ts: i64 = result.unwrap().parse().unwrap();
        let expected = (Utc::now() - Duration::days(1)).timestamp();
        assert!((ts - expected).abs() < 2);
    }

    #[test]
    fn test_fuzzytime_relative() {
        let result = apply_filter("3 hours ago", "fuzzytime", &[]);
        let ts: i64 = result.unwrap().parse().unwrap();
        let expected = (Utc::now() - Duration::hours(3)).timestamp();
        assert!((ts - expected).abs() < 2);
    }

    #[test]
    fn test_fuzzytime_absolute() {
        let result = apply_filter("2024-06-15 14:30:00", "fuzzytime", &[]);
        let ts: i64 = result.unwrap().parse().unwrap();
        assert!(ts > 0);
    }

    #[test]
    fn test_fuzzytime_unix_passthrough() {
        let result = apply_filter("1700000000", "fuzzytime", &[]);
        assert_eq!(result.unwrap(), "1700000000");
    }

    // -- Utility filters --

    #[test]
    fn test_querystring() {
        let result = apply_filter(
            "https://example.com/page?id=123&name=foo",
            "querystring",
            &args(&["id"]),
        );
        assert_eq!(result.unwrap(), "123");
    }

    #[test]
    fn test_querystring_encoded() {
        let result = apply_filter(
            "https://example.com/?q=hello%20world",
            "querystring",
            &args(&["q"]),
        );
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_querystring_missing() {
        let result = apply_filter(
            "https://example.com/?a=1",
            "querystring",
            &args(&["missing"]),
        );
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_validfilename() {
        let result = apply_filter("Movie: The *Best* <Ever>", "validfilename", &[]);
        assert_eq!(result.unwrap(), "Movie_ The _Best_ _Ever_");
    }

    #[test]
    fn test_validfilename_path_separators() {
        let result = apply_filter("path/to\\file", "validfilename", &[]);
        assert_eq!(result.unwrap(), "path_to_file");
    }

    #[test]
    fn test_jsonjoinarray_simple() {
        let json = r#"{"tags": ["action", "comedy", "drama"]}"#;
        let result = apply_filter(json, "jsonjoinarray", &args(&["tags", ", "]));
        assert_eq!(result.unwrap(), "action, comedy, drama");
    }

    #[test]
    fn test_jsonjoinarray_nested() {
        let json = r#"{"data": {"items": ["a", "b", "c"]}}"#;
        let result = apply_filter(json, "jsonjoinarray", &args(&["data.items", "|"]));
        assert_eq!(result.unwrap(), "a|b|c");
    }

    #[test]
    fn test_jsonjoinarray_missing_path() {
        let json = r#"{"data": {}}"#;
        let result = apply_filter(json, "jsonjoinarray", &args(&["data.items", ","]));
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_strdump() {
        let result = apply_filter("anything", "strdump", &[]);
        assert_eq!(result.unwrap(), "anything");
    }

    #[test]
    fn test_unknown_filter() {
        let result = apply_filter("x", "nonexistent", &[]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FilterError::UnknownFilter(_)));
    }

    #[test]
    fn test_missing_args() {
        let result = apply_filter("x", "replace", &[]);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FilterError::MissingArgs { .. }
        ));
    }

    // -- Go layout conversion --

    #[test]
    fn test_go_layout_to_chrono() {
        assert_eq!(go_layout_to_chrono("2006-01-02"), "%Y-%m-%d");
        assert_eq!(
            go_layout_to_chrono("2006-01-02 15:04:05"),
            "%Y-%m-%d %H:%M:%S"
        );
        assert_eq!(go_layout_to_chrono("Jan 02, 2006"), "%b %d, %Y");
        assert_eq!(
            go_layout_to_chrono("Monday, January 02, 2006"),
            "%A, %B %d, %Y"
        );
    }

    // -- HTML entity edge cases --

    #[test]
    fn test_htmldecode_mixed() {
        let input = "&lt;a href=&quot;url&quot;&gt;link&lt;/a&gt;";
        let result = apply_filter(input, "htmldecode", &[]);
        assert_eq!(result.unwrap(), "<a href=\"url\">link</a>");
    }

    #[test]
    fn test_htmldecode_apos() {
        let result = apply_filter("it&apos;s", "htmldecode", &[]);
        assert_eq!(result.unwrap(), "it's");
    }

    // -- backfill filters --

    #[test]
    fn test_htmlencode() {
        let result = apply_filter("a & b < c > d \"e\" 'f'", "htmlencode", &[]).unwrap();
        assert_eq!(result, "a &amp; b &lt; c &gt; d &quot;e&quot; &#39;f&#39;");
    }

    #[test]
    fn test_hexdump() {
        let result = apply_filter("hi", "hexdump", &[]).unwrap();
        assert_eq!(result, "6869");
    }

    #[test]
    fn test_diacritics_replace() {
        let result = apply_filter("Poupée café", "diacritics", &args(&["replace"])).unwrap();
        assert_eq!(result, "Poupee cafe");
    }

    #[test]
    fn test_diacritics_default_mode_is_replace() {
        let result = apply_filter("Amélie", "diacritics", &[]).unwrap();
        assert_eq!(result, "Amelie");
    }

    #[test]
    fn test_diacritics_keep() {
        let result = apply_filter("Amélie", "diacritics", &args(&["keep"])).unwrap();
        assert_eq!(result, "Amélie");
    }

    #[test]
    fn test_timeparse_alias() {
        // `timeparse` is an alias for `dateparse` — same Go layout support.
        let result = apply_filter(
            "2026-04-16 12:34:56",
            "timeparse",
            &args(&["2006-01-02 15:04:05"]),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_boolean_helper_names_pass_through() {
        // These are template/config primitives that should not fail a chain.
        for name in [
            "contains", "eq", "ne", "and", "or", "not", "checkbox", "validate",
        ] {
            let result = apply_filter("x", name, &[]).expect(name);
            assert_eq!(result, "x", "{name} should pass through");
        }
    }

    // -- filter chaining (simulated) --

    #[test]
    fn test_filter_chain() {
        // Simulate: input → trim → tolower → replace
        let input = "  Hello World  ";
        let step1 = apply_filter(input, "trim", &[]).unwrap();
        let step2 = apply_filter(&step1, "tolower", &[]).unwrap();
        let step3 = apply_filter(&step2, "replace", &args(&[" ", "-"])).unwrap();
        assert_eq!(step3, "hello-world");
    }

    /// Helper to create args slices from string literals.
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }
}
