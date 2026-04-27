//! Rust structs for deserializing Cardigann YAML indexer definitions.
//!
//! 100% compatible with the Prowlarr v11 YAML format. Field names use
//! `serde(alias)` to accept both the canonical lowercase YAML keys and
//! any camelCase variants found in the wild.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

// ---------------------------------------------------------------------------
// Top-level definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CardigannDefinition {
    pub id: String,
    pub name: String,

    #[serde(default)]
    pub description: Option<String>,

    /// Indexer type — `public`, `private`, `semi-private`.
    /// Renamed because `type` is a Rust keyword.
    #[serde(default, rename = "type")]
    pub indexer_type: Option<String>,

    #[serde(default)]
    pub language: Option<String>,

    #[serde(default)]
    pub encoding: Option<String>,

    #[serde(default, alias = "requestDelay")]
    pub request_delay: Option<f64>,

    #[serde(default)]
    pub links: Vec<String>,

    #[serde(default)]
    pub legacylinks: Vec<String>,

    #[serde(default)]
    pub followredirect: bool,

    /// When true, test the first link with a torrent download.
    #[serde(default = "default_true", alias = "testlinktorrent")]
    pub test_link_torrent: bool,

    #[serde(default)]
    pub certificates: Vec<String>,

    #[serde(default)]
    pub settings: Vec<SettingsField>,

    #[serde(default)]
    pub caps: Option<CapsBlock>,

    #[serde(default)]
    pub login: Option<LoginBlock>,

    #[serde(default)]
    pub ratio: Option<RatioBlock>,

    #[serde(default)]
    pub search: Option<SearchBlock>,

    #[serde(default)]
    pub download: Option<DownloadBlock>,
}

impl Default for CardigannDefinition {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: None,
            indexer_type: None,
            language: None,
            encoding: None,
            request_delay: None,
            links: Vec::new(),
            legacylinks: Vec::new(),
            followredirect: false,
            test_link_torrent: true,
            certificates: Vec::new(),
            settings: Vec::new(),
            caps: None,
            login: None,
            ratio: None,
            search: None,
            download: None,
        }
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsField {
    #[serde(default)]
    pub name: String,

    #[serde(default, rename = "type")]
    pub field_type: Option<String>,

    #[serde(default)]
    pub label: Option<String>,

    #[serde(default)]
    pub default: Option<String>,

    #[serde(default)]
    pub defaults: Option<Vec<String>>,

    #[serde(default)]
    pub options: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Capabilities / Caps
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CapsBlock {
    #[serde(default)]
    pub categories: Option<HashMap<String, String>>,

    #[serde(default)]
    pub categorymappings: Vec<CategoryMapping>,

    #[serde(default)]
    pub modes: HashMap<String, Vec<String>>,

    #[serde(default)]
    pub allowrawsearch: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CategoryMapping {
    pub id: String,
    pub cat: String,

    #[serde(default)]
    pub desc: Option<String>,

    #[serde(default)]
    pub default: Option<bool>,
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

/// Login method enum — maps the YAML string values to typed variants.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoginMethod {
    Post,
    Form,
    Cookie,
    Get,
    OneUrl,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub submitpath: Option<String>,

    #[serde(default)]
    pub method: Option<LoginMethod>,

    #[serde(default)]
    pub form: Option<String>,

    #[serde(default)]
    pub selectors: bool,

    #[serde(default)]
    pub inputs: HashMap<String, String>,

    #[serde(default)]
    pub selectorinputs: HashMap<String, SelectorBlock>,

    #[serde(default)]
    pub getselectorinputs: HashMap<String, SelectorBlock>,

    #[serde(default)]
    pub cookies: Vec<String>,

    #[serde(default)]
    pub error: Vec<ErrorBlock>,

    #[serde(default)]
    pub test: Option<PageTestBlock>,

    #[serde(default)]
    pub captcha: Option<CaptchaBlock>,

    #[serde(default)]
    pub headers: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptchaBlock {
    #[serde(default, rename = "type")]
    pub captcha_type: Option<String>,

    #[serde(default)]
    pub selector: Option<String>,

    #[serde(default)]
    pub input: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub selector: Option<String>,

    #[serde(default)]
    pub message: Option<SelectorBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PageTestBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub selector: Option<String>,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub paths: Vec<SearchPathBlock>,

    #[serde(default)]
    pub headers: HashMap<String, Vec<String>>,

    #[serde(default)]
    pub keywordsfilters: Vec<FilterBlock>,

    #[serde(default, alias = "allowEmptyInputs")]
    pub allow_empty_inputs: bool,

    #[serde(default)]
    pub inputs: HashMap<String, String>,

    #[serde(default)]
    pub error: Vec<ErrorBlock>,

    #[serde(default)]
    pub preprocessingfilters: Vec<FilterBlock>,

    #[serde(default)]
    pub rows: Option<RowsBlock>,

    /// Ordered map of field name -> selector. Order matters because later
    /// fields can reference earlier fields via `{{ .Result.field_name }}`.
    #[serde(default)]
    pub fields: IndexMap<String, SelectorBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchPathBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub method: Option<String>,

    #[serde(default)]
    pub inputs: HashMap<String, String>,

    #[serde(default, alias = "queryseparator")]
    pub query_separator: Option<String>,

    #[serde(default)]
    pub categories: Vec<String>,

    #[serde(default = "default_true")]
    pub inheritinputs: bool,

    #[serde(default)]
    pub followredirect: bool,

    #[serde(default)]
    pub response: Option<ResponseBlock>,
}

impl Default for SearchPathBlock {
    fn default() -> Self {
        Self {
            path: None,
            method: None,
            inputs: HashMap::new(),
            query_separator: None,
            categories: Vec::new(),
            inheritinputs: true,
            followredirect: false,
            response: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResponseBlock {
    #[serde(default, rename = "type")]
    pub response_type: Option<String>,

    #[serde(default, alias = "noresultsmessage")]
    pub no_results_message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RowsBlock {
    #[serde(default)]
    pub selector: Option<String>,

    #[serde(default)]
    pub after: i64,

    #[serde(default)]
    pub dateheaders: Option<SelectorBlock>,

    #[serde(default)]
    pub count: Option<SelectorBlock>,

    #[serde(default)]
    pub multiple: bool,

    #[serde(default, alias = "missingAttributeEqualsNoResults")]
    pub missing_attribute_equals_no_results: bool,

    // RowsBlock inherits SelectorBlock fields in C#; flatten them here.
    #[serde(default)]
    pub optional: bool,

    #[serde(default)]
    pub text: Option<String>,

    #[serde(default)]
    pub attribute: Option<String>,

    #[serde(default)]
    pub remove: Option<String>,

    #[serde(default)]
    pub filters: Vec<FilterBlock>,

    #[serde(default)]
    pub case: Option<HashMap<String, String>>,

    #[serde(default)]
    pub default: Option<String>,
}

// ---------------------------------------------------------------------------
// Selector & Filter (core building blocks)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SelectorBlock {
    #[serde(default)]
    pub selector: Option<String>,

    #[serde(default)]
    pub optional: bool,

    #[serde(default)]
    pub default: Option<String>,

    #[serde(default)]
    pub text: Option<String>,

    #[serde(default)]
    pub attribute: Option<String>,

    #[serde(default)]
    pub remove: Option<String>,

    #[serde(default)]
    pub filters: Vec<FilterBlock>,

    /// Map from selector match -> output value (like a switch/case).
    #[serde(default)]
    pub case: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FilterBlock {
    #[serde(default)]
    pub name: String,

    /// Filter arguments. In YAML this can be a single string or a list of
    /// strings — the custom deserializer normalises both forms to `Vec<String>`.
    #[serde(default, deserialize_with = "deserialize_filter_args")]
    pub args: Vec<String>,
}

/// Deserialize filter args that may be a single string, a list of strings,
/// or absent (empty vec).
fn deserialize_filter_args<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct FilterArgsVisitor;

    impl<'de> Visitor<'de> for FilterArgsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string, a list of strings, or null")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_owned()])
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(vec![v])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(elem) = seq.next_element::<StringOrNumber>()? {
                out.push(elem.into_string());
            }
            Ok(out)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(FilterArgsVisitor)
}

/// Helper for deserializing sequence elements that can be strings or numbers.
#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrNumber {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl StringOrNumber {
    fn into_string(self) -> String {
        match self {
            Self::Str(s) => s,
            Self::Int(n) => n.to_string(),
            Self::Float(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadBlock {
    #[serde(default)]
    pub selectors: Vec<SelectorField>,

    #[serde(default)]
    pub method: Option<String>,

    #[serde(default)]
    pub before: Option<BeforeBlock>,

    #[serde(default)]
    pub infohash: Option<InfohashBlock>,

    #[serde(default)]
    pub headers: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SelectorField {
    #[serde(default)]
    pub selector: Option<String>,

    #[serde(default)]
    pub attribute: Option<String>,

    #[serde(default)]
    pub usebeforeresponse: bool,

    #[serde(default)]
    pub filters: Vec<FilterBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InfohashBlock {
    #[serde(default)]
    pub hash: Option<SelectorField>,

    #[serde(default)]
    pub title: Option<SelectorField>,

    #[serde(default)]
    pub usebeforeresponse: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BeforeBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub method: Option<String>,

    #[serde(default)]
    pub inputs: HashMap<String, String>,

    #[serde(default, alias = "queryseparator")]
    pub query_separator: Option<String>,

    #[serde(default)]
    pub pathselector: Option<SelectorField>,
}

// ---------------------------------------------------------------------------
// Ratio (inherits SelectorBlock fields + path)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct RatioBlock {
    #[serde(default)]
    pub path: Option<String>,

    #[serde(default)]
    pub selector: Option<String>,

    #[serde(default)]
    pub optional: bool,

    #[serde(default)]
    pub default: Option<String>,

    #[serde(default)]
    pub text: Option<String>,

    #[serde(default)]
    pub attribute: Option<String>,

    #[serde(default)]
    pub remove: Option<String>,

    #[serde(default)]
    pub filters: Vec<FilterBlock>,

    #[serde(default)]
    pub case: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Type aliases (for compatibility with other indexer modules)
// ---------------------------------------------------------------------------

pub type FilterDef = FilterBlock;
pub type SearchPath = SearchPathBlock;
pub type LoginTest = PageTestBlock;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_args_string() {
        let yaml = r#"
name: dateparse
args: "MM/dd/yyyy"
"#;
        let fb: FilterBlock = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(fb.args, vec!["MM/dd/yyyy"]);
    }

    #[test]
    fn filter_args_array() {
        let yaml = r#"
name: replace
args: ["-", " "]
"#;
        let fb: FilterBlock = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(fb.args, vec!["-", " "]);
    }

    #[test]
    fn filter_args_absent() {
        let yaml = "name: fuzzytime\n";
        let fb: FilterBlock = serde_yaml::from_str(yaml).unwrap();
        assert!(fb.args.is_empty());
    }

    #[test]
    fn filter_args_numeric_in_array() {
        let yaml = r#"
name: split
args: ["/", 3]
"#;
        let fb: FilterBlock = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(fb.args, vec!["/", "3"]);
    }

    #[test]
    fn minimal_definition() {
        let yaml = r#"
id: test
name: Test Indexer
links:
  - https://example.com
caps:
  modes:
    search: [q]
  categorymappings:
    - {id: "1", cat: Movies, desc: Movies}
search:
  paths:
    - path: /search
  rows:
    selector: tr
  fields:
    title:
      selector: a
    size:
      selector: td.size
"#;
        let def: CardigannDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id, "test");
        assert_eq!(def.links.len(), 1);

        let caps = def.caps.unwrap();
        assert_eq!(caps.categorymappings.len(), 1);
        assert_eq!(caps.modes.get("search").unwrap(), &["q"]);

        let search = def.search.unwrap();
        assert_eq!(search.fields.len(), 2);
        // Order is preserved
        let keys: Vec<&String> = search.fields.keys().collect();
        assert_eq!(keys, vec!["title", "size"]);
    }

    #[test]
    fn category_id_numeric_or_string() {
        // YAML may represent unquoted numbers as integers; our id is String.
        let yaml = r"
id: 28
cat: TV/Anime
desc: Anime
";
        let cm: CategoryMapping = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cm.id, "28");
    }

    #[test]
    fn parse_synthetic_tracker_definition() {
        // Exercises the wide-shape parser surface: caps +
        // categorymappings + multi-mode `modes`, settings list with
        // text/select/info widgets, ordered `search.fields` (insertion
        // order is load-bearing — Cardigann fields reference earlier
        // ones via `{{ .Result.x }}` templates), multi-step
        // `download.selectors`. See test_fixtures/README.md for the
        // synthetic-vs-real-fixture rationale.
        let yaml = include_str!("test_fixtures/synthetic-tracker.yml");
        let def: CardigannDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id, "synthetic-tracker");
        assert_eq!(def.name, "SyntheticTracker");
        assert_eq!(def.indexer_type.as_deref(), Some("public"));
        assert_eq!(def.language.as_deref(), Some("en-US"));
        assert_eq!(def.encoding.as_deref(), Some("UTF-8"));
        assert_eq!(def.request_delay, Some(2.0));
        assert!(!def.links.is_empty());
        assert!(!def.legacylinks.is_empty());
        assert!(!def.settings.is_empty());

        let caps = def.caps.unwrap();
        assert!(!caps.categorymappings.is_empty());
        assert!(caps.modes.contains_key("search"));
        assert!(caps.modes.contains_key("tv-search"));
        assert!(caps.allowrawsearch);

        let search = def.search.unwrap();
        assert!(!search.paths.is_empty());
        assert!(!search.keywordsfilters.is_empty());
        assert!(search.rows.is_some());
        assert!(!search.fields.is_empty());
        // Fields preserve insertion order
        let first_key = search.fields.keys().next().unwrap();
        assert_eq!(first_key, "title_default");

        let download = def.download.unwrap();
        assert_eq!(download.selectors.len(), 2);
    }

    #[test]
    fn parse_synthetic_magnet_tracker_definition() {
        // Exercises the alternative `download.infohash` block (vs
        // the more common `download.selectors`). The parser must
        // accept either shape at the same level.
        let yaml = include_str!("test_fixtures/synthetic-magnet-tracker.yml");
        let def: CardigannDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id, "synthetic-magnet-tracker");
        let download = def.download.unwrap();
        let infohash = download.infohash.unwrap();
        assert!(infohash.hash.is_some());
        assert!(infohash.title.is_some());
    }
}
