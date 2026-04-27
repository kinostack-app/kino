//! Per-entity clearlogo ingest — subsystem 29.
//!
//! Called from the metadata refresh sweep; one call per movie or
//! show, best-effort (logos are non-critical polish). Downloads the
//! best SVG/PNG candidate from TMDB, sanitises SVG via `usvg`
//! (text→paths, `<use>` resolve, `<foreignObject>` dropped, filters
//! flattened where possible, `viewBox` normalised), palette-
//! classifies the result, and persists to the on-disk cache + two
//! new columns on `movie`/`show`.
//!
//! Sanitisation chain:
//!   1. `usvg::Tree::from_str` — parses + normalises + drops the
//!      DOM primitives that would let a hostile SVG reach the
//!      browser (scripts, external refs, `<foreignObject>`, raw
//!      `<style>`). This is the primary defence; the later
//!      substring checks are belt-and-braces.
//!   2. Re-serialise to a clean SVG string via
//!      `Tree::to_string`. usvg's serialiser only emits a
//!      constrained subset of SVG primitives, so any
//!      `<script>` / `onload=` / JS-URL `<image>` etc. that the
//!      parser accepted and didn't strip is guaranteed not to
//!      survive the round-trip.
//!   3. Validate: at least one path with a non-empty `d`, bounding
//!      box > 0×0, size < 500 KB.
//!   4. Palette-classify by scanning distinct fill colours in both
//!      attribute form (`fill="X"`) and CSS-style form
//!      (`fill:X` inside `style=`), plus gradient stops.
//!   5. Mono logos: rewrite every `fill=` / `fill:` declaration
//!      (non-`none`) to `currentColor` so CSS owns the tint.
//!
//! The frontend additionally runs `DOMPurify` before inlining, as
//! defence-in-depth, but the server is authoritative.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::tmdb::TmdbClient;
use crate::tmdb::types::TmdbImageEntry;

/// Palette classification. `Mono` logos get their `fill` rewritten to
/// `currentColor` so CSS owns the tint; `Multi` logos are stored as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    Mono,
    Multi,
}

impl Palette {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mono => "mono",
            Self::Multi => "multi",
        }
    }
}

/// What came back after processing a downloaded logo.
#[derive(Debug, Clone)]
pub struct ProcessedLogo {
    /// Bytes to write to disk. For SVG this may differ from the
    /// originally-downloaded bytes (mono → `currentColor` rewrite).
    pub bytes: Vec<u8>,
    /// `"svg"` or `"png"` — drives the stored filename extension.
    pub extension: &'static str,
    /// `mono` vs `multi` classification. PNGs are always `Multi` —
    /// we don't palette-scan rasters.
    pub palette: Palette,
}

/// Top-level: fetch logos from TMDB, pick the best candidate that
/// survives download + sanitisation, save it, and update DB columns.
/// No-op on any failure — logos are non-critical; the refresh sweep
/// continues with the next entity.
pub async fn refresh_entity_logo(
    pool: &SqlitePool,
    tmdb: &TmdbClient,
    http: &reqwest::Client,
    data_path: &Path,
    content_type: ContentType,
    db_id: i64,
    tmdb_id: i64,
) -> anyhow::Result<()> {
    let images = match content_type {
        ContentType::Movie => tmdb.movie_logos(tmdb_id).await,
        ContentType::Show => tmdb.show_logos(tmdb_id).await,
    };
    let Ok(images) = images else {
        // TMDB 404s are common (obscure titles) — not worth bubbling.
        tracing::debug!(?content_type, tmdb_id, "no TMDB logos available");
        return Ok(());
    };

    let candidates = select_candidates(&images.logos);
    for candidate in candidates {
        let Some(bytes) = download(http, &candidate.file_path).await else {
            continue;
        };
        let processed = if is_svg_path(&candidate.file_path) {
            process_svg(&bytes)
        } else {
            Some(ProcessedLogo {
                bytes,
                extension: "png",
                palette: Palette::Multi,
            })
        };
        let Some(processed) = processed else {
            continue;
        };

        let rel_path = save(data_path, content_type, tmdb_id, &processed)
            .await
            .map_err(|e| anyhow::anyhow!("save logo: {e}"))?;
        update_db(pool, content_type, db_id, &rel_path, processed.palette).await?;
        return Ok(());
    }
    Ok(())
}

/// Which entity table we're refreshing. Drives both the TMDB endpoint
/// (movie vs tv) and the DB table (movie vs show).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Movie,
    Show,
}

impl ContentType {
    pub const fn dir_segment(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Show => "show",
        }
    }

    pub const fn table(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Show => "show",
        }
    }
}

/// Filter + order candidates per spec §Selection scoring.
///   1. Drop entries with <3 votes (noisy community submissions)
///   2. Drop wide banners (aspect > 4.0 is usually a banner not wordmark)
///   3. Prefer SVG — all SVG candidates sorted ahead of any PNG
///   4. Within each format, sort by `vote_average` desc, then width desc
fn select_candidates(logos: &[TmdbImageEntry]) -> Vec<&TmdbImageEntry> {
    let mut filtered: Vec<&TmdbImageEntry> = logos
        .iter()
        .filter(|l| l.vote_count >= 3)
        .filter(|l| l.aspect_ratio <= 4.0)
        .collect();
    filtered.sort_by(|a, b| {
        let a_svg = is_svg_path(&a.file_path);
        let b_svg = is_svg_path(&b.file_path);
        b_svg.cmp(&a_svg).then_with(|| {
            b.vote_average
                .partial_cmp(&a.vote_average)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.width.cmp(&a.width))
        })
    });
    filtered
}

/// Case-insensitive `.svg` extension check. TMDB paths come back
/// lowercase in practice, but the clippy lint insists we handle
/// any capitalisation correctly.
fn is_svg_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
}

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

async fn download(http: &reqwest::Client, tmdb_path: &str) -> Option<Vec<u8>> {
    let url = format!("{TMDB_IMAGE_BASE}{tmdb_path}");
    let resp = http.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok().map(|b| b.to_vec())
}

/// Cap on the serialised, post-sanitisation SVG size. Per spec §SVG
/// sanitization "sanity cap, not enforcement — 99% will be <50KB";
/// anything past this is either pathological or hostile so we drop it.
const MAX_SANITISED_SVG_BYTES: usize = 500 * 1024;

/// Process an SVG end-to-end. Returns `None` (caller falls through to
/// the next candidate) when:
///   - the bytes aren't UTF-8
///   - `usvg` refuses to parse (malformed / unsupported)
///   - the sanitised output has no rendered geometry (empty `d`)
///   - the bounding box is zero-width or zero-height
///   - the sanitised output exceeds `MAX_SANITISED_SVG_BYTES`
fn process_svg(bytes: &[u8]) -> Option<ProcessedLogo> {
    let text = std::str::from_utf8(bytes).ok()?;

    // Parse with usvg. `resources_dir = None` + the default options
    // disable external fetches; text nodes convert to paths using the
    // bundled fontdb (empty by default — TMDB logos are almost always
    // already path-based, so zero cost in the common case). usvg
    // drops <script>, <foreignObject>, external <use>, and JS-URL
    // image refs during parse.
    let tree = match usvg::Tree::from_str(text, &usvg::Options::default()) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "usvg parse rejected SVG candidate");
            return None;
        }
    };

    // Validate: bounding box must have positive width + height. usvg's
    // root size is already viewBox-normalised.
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        tracing::debug!(
            w = size.width(),
            h = size.height(),
            "svg rejected: empty bbox"
        );
        return None;
    }
    if !tree_has_rendered_geometry(tree.root()) {
        tracing::debug!("svg rejected: no non-empty path geometry");
        return None;
    }

    // Re-serialise the sanitised tree. This is the authoritative
    // defence: usvg's writer only emits a whitelisted subset of SVG
    // primitives (paths, groups, clips, masks, gradients, images as
    // base64 data), so scripts / event-handler attributes / external
    // refs that hostile SVGs try to smuggle in cannot survive the
    // round-trip even if they slipped past the parser.
    let serialised = tree.to_string(&usvg::WriteOptions::default());
    if serialised.len() > MAX_SANITISED_SVG_BYTES {
        tracing::debug!(
            bytes = serialised.len(),
            cap = MAX_SANITISED_SVG_BYTES,
            "svg rejected: exceeds size cap"
        );
        return None;
    }

    let palette = classify_palette(&serialised);
    let output = match palette {
        Palette::Mono => rewrite_fills_to_currentcolor(&serialised),
        Palette::Multi => serialised,
    };
    Some(ProcessedLogo {
        bytes: output.into_bytes(),
        extension: "svg",
        palette,
    })
}

/// Walk the usvg tree and return true once we find any path with a
/// non-empty `d` attribute (equivalent in usvg's model: a non-empty
/// `Data` segment list). Empty-`d` SVGs render as blank space and
/// should be rejected.
fn tree_has_rendered_geometry(group: &usvg::Group) -> bool {
    for node in group.children() {
        match node {
            usvg::Node::Path(p) => {
                if !p.data().is_empty() {
                    return true;
                }
            }
            usvg::Node::Group(g) => {
                if tree_has_rendered_geometry(g) {
                    return true;
                }
            }
            usvg::Node::Image(_) | usvg::Node::Text(_) => {
                // usvg converts text to paths before emitting it into
                // the tree in the common path, so a raw `Text` node
                // here means the fontdb didn't have a glyph — still
                // something rendered. Images count as geometry too.
                return true;
            }
        }
    }
    false
}

/// Collect distinct non-`none` fill values from attribute form
/// (`fill="X"`), CSS-style form (`fill:X` inside `style=…` or a
/// `<style>` block), and gradient stops (`stop-color="X"` +
/// `stop-color:X`). One opaque fill → mono; any gradient or 2+
/// distinct fills → multi.
///
/// Runs after usvg serialisation so the input is already normalised,
/// but scans textually to keep the logic independent of usvg's
/// internal fill representation (which uses enums we'd have to walk
/// the tree to collect).
fn classify_palette(svg: &str) -> Palette {
    // Any gradient element → multi, regardless of stop count. A
    // gradient is always a non-flat fill from the user's POV.
    if svg.contains("<linearGradient") || svg.contains("<radialGradient") {
        return Palette::Multi;
    }
    let mut distinct: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_attr_values(svg, "fill=\"", &mut distinct);
    collect_attr_values(svg, "stop-color=\"", &mut distinct);
    collect_css_values(svg, "fill:", &mut distinct);
    collect_css_values(svg, "stop-color:", &mut distinct);
    if distinct.len() <= 1 {
        Palette::Mono
    } else {
        Palette::Multi
    }
}

/// Pull the value out of every `{key}X"` attribute occurrence and
/// insert the lowercased, non-`none` value into `out`.
fn collect_attr_values(svg: &str, key: &str, out: &mut std::collections::HashSet<String>) {
    for piece in svg.split(key).skip(1) {
        if let Some(end) = piece.find('"') {
            let val = piece[..end].trim().to_ascii_lowercase();
            if !val.is_empty() && val != "none" {
                out.insert(val);
            }
        }
    }
}

/// Pull the value out of every `{key}X` CSS-style declaration —
/// handles both `style="…"` attributes and bare `<style>` blocks.
/// A value ends at `;`, a closing attribute quote, or whitespace.
fn collect_css_values(svg: &str, key: &str, out: &mut std::collections::HashSet<String>) {
    for piece in svg.split(key).skip(1) {
        let val: String = piece
            .trim_start()
            .chars()
            .take_while(|c| *c != ';' && *c != '"' && *c != '}' && !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if !val.is_empty() && val != "none" {
            out.insert(val);
        }
    }
}

/// Replace every `fill="X"` and `fill:X` declaration (except
/// `none`) with `currentColor`. Leaves `none` alone so `stroke`-only
/// path geometry isn't accidentally filled by CSS.
///
/// Two-pass: attribute form (`fill="…"`), then CSS form (`fill:…`) to
/// cover both inline `style="…"` attributes and `<style>` blocks.
fn rewrite_fills_to_currentcolor(svg: &str) -> String {
    let attr_rewritten = rewrite_attr_value(svg, "fill=\"", "currentColor");
    rewrite_css_value(&attr_rewritten, "fill:", "currentColor")
}

/// Rewrite every `{key}X"` attribute value (non-`none`) to `{key}{replacement}"`.
fn rewrite_attr_value(svg: &str, key: &str, replacement: &str) -> String {
    let key_len = key.len();
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(idx) = rest.find(key) {
        out.push_str(&rest[..idx]);
        out.push_str(key);
        let after = &rest[idx + key_len..];
        if let Some(close) = after.find('"') {
            let value = &after[..close];
            if value.eq_ignore_ascii_case("none") {
                out.push_str(value);
            } else {
                out.push_str(replacement);
            }
            out.push('"');
            rest = &after[close + 1..];
        } else {
            out.push_str(after);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Rewrite every `{key}X` CSS-style declaration (non-`none`) to
/// `{key}{replacement}`, stopping the value at `;`, `"`, `}`, or
/// whitespace. Preserves the trailing delimiter.
fn rewrite_css_value(svg: &str, key: &str, replacement: &str) -> String {
    let key_len = key.len();
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(idx) = rest.find(key) {
        out.push_str(&rest[..idx]);
        out.push_str(key);
        let after = &rest[idx + key_len..];
        // Preserve any leading whitespace the author wrote between `:`
        // and the value so the output matches input style.
        let ws_len = after.chars().take_while(|c| c.is_whitespace()).count();
        out.push_str(&after[..ws_len]);
        let after_ws = &after[ws_len..];
        let value_len = after_ws
            .chars()
            .take_while(|c| *c != ';' && *c != '"' && *c != '}' && !c.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        let value = &after_ws[..value_len];
        if value.eq_ignore_ascii_case("none") || value.is_empty() {
            out.push_str(value);
        } else {
            out.push_str(replacement);
        }
        rest = &after_ws[value_len..];
    }
    out.push_str(rest);
    out
}

/// Persist the processed logo to disk. Returns the relative path
/// stored in the DB, which is rooted at `{data_path}/images/logos/`
/// and scoped by content type + `tmdb_id`.
async fn save(
    data_path: &Path,
    content_type: ContentType,
    tmdb_id: i64,
    processed: &ProcessedLogo,
) -> std::io::Result<String> {
    let rel = format!(
        "logos/{ct}/{tmdb_id}.{ext}",
        ct = content_type.dir_segment(),
        ext = processed.extension
    );
    let abs: PathBuf = data_path.join("images").join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&abs, &processed.bytes).await?;
    Ok(rel)
}

async fn update_db(
    pool: &SqlitePool,
    content_type: ContentType,
    db_id: i64,
    rel_path: &str,
    palette: Palette,
) -> sqlx::Result<()> {
    let sql = format!(
        "UPDATE {} SET logo_path = ?, logo_palette = ? WHERE id = ?",
        content_type.table()
    );
    sqlx::query(&sql)
        .bind(rel_path)
        .bind(palette.as_str())
        .bind(db_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_mono_single_fill() {
        let svg = r##"<svg><path fill="#ffffff" d="M0 0"/></svg>"##;
        assert_eq!(classify_palette(svg), Palette::Mono);
    }

    #[test]
    fn classify_multi_two_fills() {
        let svg = r##"<svg>
            <path fill="#ff0000" d="M0 0"/>
            <path fill="#00ff00" d="M0 0"/>
        </svg>"##;
        assert_eq!(classify_palette(svg), Palette::Multi);
    }

    #[test]
    fn classify_multi_on_gradient() {
        let svg = r##"<svg>
            <linearGradient id="g"><stop stop-color="#000"/></linearGradient>
            <path fill="url(#g)" d="M0 0"/>
        </svg>"##;
        assert_eq!(classify_palette(svg), Palette::Multi);
    }

    #[test]
    fn classify_ignores_fill_none() {
        let svg = r##"<svg><path fill="none" stroke="#fff" d="M0 0"/></svg>"##;
        // No opaque fill at all → treated as mono (no distinct colours).
        assert_eq!(classify_palette(svg), Palette::Mono);
    }

    #[test]
    fn rewrite_swaps_fill_to_currentcolor() {
        let svg = r##"<svg><path fill="#ffffff" d="M0 0"/></svg>"##;
        let out = rewrite_fills_to_currentcolor(svg);
        assert!(out.contains("fill=\"currentColor\""));
        assert!(!out.contains("#ffffff"));
    }

    #[test]
    fn rewrite_preserves_fill_none() {
        let svg = r##"<svg><path fill="none" stroke="#fff" d="M0 0"/></svg>"##;
        let out = rewrite_fills_to_currentcolor(svg);
        assert!(out.contains("fill=\"none\""));
    }

    #[test]
    fn process_svg_strips_script() {
        // Hostile payload — a <script> element alongside a real path.
        // usvg's writer never emits <script>, so the serialised output
        // must not contain the tag regardless of how the parser
        // handled it.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <script>alert(1)</script>
            <path fill="#ffffff" d="M0 0 L100 0 L100 100 L0 100 Z"/>
          </svg>"##;
        let out = process_svg(svg).expect("processes");
        let serialised = String::from_utf8_lossy(&out.bytes);
        assert!(
            !serialised.to_ascii_lowercase().contains("<script"),
            "script tag must not survive sanitisation, got:\n{serialised}"
        );
    }

    #[test]
    fn process_svg_rejects_empty_geometry() {
        // No `<path>` at all — must be rejected by the geometry
        // validation step.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"/>"#;
        assert!(process_svg(svg).is_none());
    }

    #[test]
    fn process_svg_mono_rewrites_fill() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <path fill="#ffffff" d="M0 0 L100 0 L100 100 L0 100 Z"/>
          </svg>"##;
        let out = process_svg(svg).expect("processes");
        assert_eq!(out.palette, Palette::Mono);
        assert_eq!(out.extension, "svg");
        assert!(String::from_utf8_lossy(&out.bytes).contains("currentColor"));
    }

    #[test]
    fn select_drops_low_vote_and_wide_banners() {
        let logos = vec![
            TmdbImageEntry {
                file_path: "/good.svg".into(),
                aspect_ratio: 2.0,
                vote_average: 5.0,
                vote_count: 10,
                width: 500,
                height: 250,
                iso_639_1: None,
            },
            TmdbImageEntry {
                file_path: "/unvetted.svg".into(),
                aspect_ratio: 2.0,
                vote_average: 5.0,
                vote_count: 1,
                width: 500,
                height: 250,
                iso_639_1: None,
            },
            TmdbImageEntry {
                file_path: "/banner.svg".into(),
                aspect_ratio: 6.0,
                vote_average: 5.0,
                vote_count: 10,
                width: 1200,
                height: 200,
                iso_639_1: None,
            },
        ];
        let out = select_candidates(&logos);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].file_path, "/good.svg");
    }

    #[test]
    fn select_prefers_svg_over_png() {
        let logos = vec![
            TmdbImageEntry {
                file_path: "/high-vote.png".into(),
                aspect_ratio: 2.0,
                vote_average: 9.0,
                vote_count: 10,
                width: 500,
                height: 250,
                iso_639_1: None,
            },
            TmdbImageEntry {
                file_path: "/low-vote.svg".into(),
                aspect_ratio: 2.0,
                vote_average: 3.0,
                vote_count: 10,
                width: 500,
                height: 250,
                iso_639_1: None,
            },
        ];
        let out = select_candidates(&logos);
        // SVG first despite lower vote_average.
        assert_eq!(out[0].file_path, "/low-vote.svg");
        assert_eq!(out[1].file_path, "/high-vote.png");
    }
}
