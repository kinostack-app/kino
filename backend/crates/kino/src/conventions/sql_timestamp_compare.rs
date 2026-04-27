//! Convention test: every SQL comparison against a known timestamp
//! column wraps both sides in `datetime(...)`.
//!
//! Plain text comparison (`last_searched_at < ?`) silently relies
//! on lexicographic ordering, which fails the moment a row holds a
//! value missing leading zeros, a `Z` suffix, or a sub-second
//! component. Wrapping in `datetime(?)` parses both sides as
//! datetimes and compares those — invariant the codebase already
//! follows. This test guards against new code regressing it.
//!
//! See `docs/architecture/conventions.md` for the rule and
//! [`crate::time::SQL_TIMESTAMP_COMPARE_GUIDE`] for the in-source
//! pointer.

use std::fs;
use std::path::{Path, PathBuf};

/// Known TEXT timestamp columns. A new column added to the schema
/// that participates in SQL comparison should be added here.
const TIMESTAMP_COLUMNS: &[&str] = &[
    "last_searched_at",
    "watched_at",
    "last_played_at",
    "last_metadata_refresh",
    "first_seen_at",
    "grabbed_at",
    "last_attempt_at",
    "publish_date",
    "expires_at",
    "last_seen_at",
    "consumed_at",
    "completed_at",
    "first_air_date",
    "last_air_date",
    "blocklisted_at",
    "intro_analysis_at",
    "air_date_utc",
    "added_at",
    "date_added",
    "created_at",
];

#[test]
fn timestamp_comparisons_use_datetime_function() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = crate_root.join("src");
    let mut violations: Vec<String> = Vec::new();

    walk_rs_files(&src, &mut |path| {
        let Ok(content) = fs::read_to_string(path) else {
            return;
        };
        // This file would otherwise trip itself — every column name
        // appears in the constant array above without a `datetime(`
        // wrapper.
        if path.ends_with("conventions/sql_timestamp_compare.rs") {
            return;
        }
        for (idx, line) in content.lines().enumerate() {
            // Skip Rust comment lines outright — `// last_searched_at < cutoff`
            // mentions a column next to a comparison without being SQL.
            // Doc comments (`///`, `//!`) and inline `//` both qualify.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // SQL strings live in Rust source; their comparison
            // operators are bare `<`, `>`, `<=`, `>=`. Skip lines
            // that are pure Rust syntax (assertions, lambdas) by
            // requiring the column name + a comparison operator
            // appear together AND `datetime(` does not appear in
            // the same line.
            for column in TIMESTAMP_COLUMNS {
                if !line_compares_column(line, column) {
                    continue;
                }
                if line.contains("datetime(") {
                    continue;
                }
                violations.push(format!(
                    "{}:{} — column `{column}` compared without `datetime()`:\n    {}",
                    path.display(),
                    idx + 1,
                    line.trim()
                ));
            }
        }
    });

    assert!(
        violations.is_empty(),
        "SQL timestamp comparison must wrap both sides in datetime(...). Violations:\n{}",
        violations.join("\n")
    );
}

fn line_compares_column(line: &str, column: &str) -> bool {
    // Find every occurrence of the column name, then look at the
    // next non-whitespace non-alpha char for a comparison operator.
    // Stops false-positives on `last_searched_at = ?` (assignment,
    // not comparison) and on accidental column-name substrings in
    // identifiers.
    let bytes = line.as_bytes();
    let needle = column.as_bytes();
    let mut start = 0;
    while let Some(rel) = find_subslice(&bytes[start..], needle) {
        let abs = start + rel;
        let after = abs + needle.len();
        // Reject identifier suffixes (e.g. `last_searched_at_x`).
        let next = bytes.get(after).copied();
        let prev = if abs == 0 {
            None
        } else {
            bytes.get(abs - 1).copied()
        };
        let is_identifier_char =
            |b: Option<u8>| matches!(b, Some(c) if c.is_ascii_alphanumeric() || c == b'_');
        if !is_identifier_char(prev) && !is_identifier_char(next) {
            // Skip past whitespace + `IS NOT NULL` / `IS NULL`
            // before checking for a comparison operator.
            let mut i = after;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            // `IS NULL` / `IS NOT NULL` aren't comparisons we care about.
            if line[i..].starts_with("IS ") {
                start = after;
                continue;
            }
            // `=` alone (column = ?) is assignment in UPDATE / INSERT;
            // we only flag ordered comparisons. `<`, `>`, `<=`, `>=`,
            // `<>`, `BETWEEN` qualify.
            if matches!(bytes.get(i), Some(b'<' | b'>')) {
                return true;
            }
            if line[i..].starts_with("BETWEEN") {
                return true;
            }
        }
        start = after;
    }
    false
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn walk_rs_files(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, visit);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            visit(&path);
        }
    }
}
