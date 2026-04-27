# Indexer engine

Built-in Cardigann-compatible YAML engine that replaces Prowlarr as an external dependency. Loads community-maintained indexer definitions, handles authentication, builds search requests, and parses HTML/JSON/XML responses into normalized releases.

## Why

Kino's goal is a single binary replacing the *arr stack. Prowlarr is the last external service dependency. Its actual value is 500+ YAML indexer definitions maintained by the community — the C# runtime that executes them is ~3,100 lines buried in ~29,000 lines of enterprise framework. Kino reimplements that runtime in ~2,400 lines of Rust while consuming the exact same YAML definitions.

## Architecture

```
YAML definition (from Prowlarr/Indexers repo)
    ↓
DefinitionLoader (parse + cache)
    ↓
SearchEngine::search(definition, config, query)
    ├── TemplateEngine (expand {{ .Query.Q }} etc.)
    ├── RequestBuilder (construct HTTP request, handle login)
    ├── HTTP client (reqwest, cookies, redirects)
    ├── ResponseParser (CSS selectors / JSONPath / XML)
    ├── Filters (regexp, dateparse, split, etc.)
    └── → Vec<TorznabRelease>
```

No plugin system, no provider factory, no app sync layer. YAML in, releases out.

## Definition format

Each indexer is a YAML file. Format is 100% compatible with Prowlarr's Cardigann v9+ definitions from [Prowlarr/Indexers](https://github.com/Prowlarr/Indexers).

### Top-level structure

```yaml
id: example-tracker
name: Example Tracker
type: public                # public | semi-private | private
language: en-US
encoding: UTF-8
links:
  - https://example.com/
legacylinks:
  - https://old-example.com/

settings:                   # user-configurable fields
  - name: username
    type: text
    label: Username

caps:                       # search capabilities + category mapping
  modes:
    search: [q]
    movie-search: [q, imdbid]
    tv-search: [q, season, ep, imdbid]
  categorymappings:
    - id: "1"
      cat: Movies
      desc: Movies

login:                      # authentication (optional for public trackers)
  path: /login
  method: post
  inputs:
    username: "{{ .Config.username }}"
    password: "{{ .Config.password }}"
  test:
    path: /
    selector: ".logged-in"

search:                     # request construction + response parsing
  paths:
    - path: /search
  inputs:
    q: "{{ .Query.Q }}"
  rows:
    selector: "table.results tbody tr"
  fields:
    title:
      selector: "td.name a"
    download:
      selector: "td.download a"
      attribute: href
    size:
      selector: "td.size"
    seeders:
      selector: "td.seeders"
    date:
      selector: "td.date"
      filters:
        - name: dateparse
          args: "2006-01-02"
```

### Response types

Definitions can parse three formats, specified per search path:

| Type | Engine | Selectors |
|------|--------|-----------|
| HTML (default) | `scraper` crate | CSS3 selectors |
| JSON | `serde_json::Value` | Dot-path navigation (`.data.items`) |
| XML | `quick-xml` | Tag-path navigation |

### Login methods

| Method | Flow |
|--------|------|
| `post` | POST credentials to login path, capture cookies |
| `form` | GET login page → extract form via CSS selector → fill inputs → POST |
| `cookie` | User provides pre-obtained cookies directly in settings |

All methods store cookies per-indexer and re-authenticate on 401.

## Template engine

Minimal Go-style template interpreter. Only supports features actually used in Cardigann definitions — no general-purpose Go templates.

### Variables

```
{{ .Config.sitelink }}          Site base URL
{{ .Config.<setting> }}         User-configured value
{{ .Query.Q }}                  Search query text
{{ .Query.Season }}             TV season number
{{ .Query.Ep }}                 TV episode number
{{ .Query.IMDBID }}             IMDB ID (tt1234567)
{{ .Query.TMDBID }}             TMDB ID
{{ .Query.TVDBID }}             TVDB ID
{{ .Query.Year }}               Year
{{ .Query.Categories }}         Category list
{{ .Result.<field> }}           Parsed field from current row
{{ .Today.Year }}               Current year
```

### Functions

```
{{ re_replace .Query.Q "[^a-zA-Z0-9]+" "%" }}
{{ join .Categories "," }}
{{ if .Query.Season }}...{{ else }}...{{ end }}
{{ and .Query.Season .Query.Ep }}
{{ or .Config.field1 "default" }}
{{ eq .Result.cat "1" }}
{{ ne .Result.cat "2" }}
```

No loops. No nested template calls. No partials. Definitions don't use them.

## Filter functions

Applied in sequence to extracted field values. Each filter is a pure function: `fn(input: &str, args: &[String]) -> String`.

### String manipulation

| Filter | Args | Example |
|--------|------|---------|
| `replace` | from, to | `replace " " "."` |
| `re_replace` | pattern, replacement | `re_replace "\\s+" " "` |
| `regexp` | pattern | Extract first capture group |
| `split` | separator, index | `split "\|" "0"` |
| `trim` | cutset (optional) | Strip whitespace or specific chars |
| `append` | text | Append literal text |
| `prepend` | text | Prepend literal text |
| `tolower` | — | Lowercase |
| `toupper` | — | Uppercase |

### Encoding

| Filter | Args | Example |
|--------|------|---------|
| `urlencode` | — | URL-encode |
| `urldecode` | — | URL-decode |
| `htmldecode` | — | Decode HTML entities |
| `htmlencode` | — | Encode HTML entities |

### Date parsing

| Filter | Args | Example |
|--------|------|---------|
| `dateparse` | Go time layout | `dateparse "2006-01-02 15:04:05"` |
| `timeago` | — | Parse "2 hours ago" to ISO 8601 |
| `fuzzytime` | — | Parse relative/absolute dates |

Go time layout uses the reference time `Mon Jan 2 15:04:05 MST 2006` where each component is a fixed value. The engine maps these to chrono format strings:

```
2006 → %Y    06 → %y
01 → %m      1 → %-m     January → %B    Jan → %b
02 → %d      2 → %-d
15 → %H      3 → %-I     PM → %p
04 → %M      4 → %-M
05 → %S      5 → %-S
```

### Utility

| Filter | Args | Example |
|--------|------|---------|
| `querystring` | param name | Extract URL query parameter |
| `validfilename` | — | Sanitize for filesystem |
| `diacritics` | "replace" | Remove accents |
| `jsonjoinarray` | path, separator | Join JSON array values |
| `validate` | word list | Keep only listed words |

## Definition loader

### Sources

1. **Bundled**: Ship a snapshot of common definitions in the binary (optional, for offline use)
2. **Local cache**: `{data_path}/definitions/*.yml` — persisted on disk
3. **Remote**: Fetch from Prowlarr/Indexers GitHub repo

### Update mechanism

On startup and periodically (default: daily), check the Prowlarr/Indexers repo for updated definitions. Download changed files to local cache. No full repo clone — fetch individual YAML files via GitHub raw content API or a release archive.

```
Startup:
  1. Load all .yml from {data_path}/definitions/
  2. If cache empty or stale (>24h), fetch from remote
  3. Parse each into CardigannDefinition, store in HashMap<id, Definition>

Runtime:
  search_indexer(id, config, query) → look up definition by id → execute
```

### Version compatibility

Prowlarr has shipped definition format versions v1 through v10+. The YAML structure is backwards-compatible — newer versions add fields, older ones still parse. Use `#[serde(default)]` on all optional fields. Ignore unknown fields with `#[serde(deny_unknown_fields)]` off (the default).

## Integration with kino

### Indexer model changes

Add to the `indexer` table:

```sql
ALTER TABLE indexer ADD COLUMN indexer_type TEXT NOT NULL DEFAULT 'torznab';
ALTER TABLE indexer ADD COLUMN definition_id TEXT;        -- links to YAML definition
ALTER TABLE indexer ADD COLUMN settings_json TEXT;        -- user config for this indexer
```

Three indexer types:
- `torznab` — existing behavior, URL points to Torznab endpoint
- `cardigann` — uses YAML definition, URL is auto-derived from definition
- `newznab` — Usenet (future, same protocol family)

### Search flow

```rust
for indexer in enabled_indexers {
    let releases = match indexer.indexer_type.as_str() {
        "torznab" => torznab_client.search(&indexer.url, &indexer.api_key, &query).await?,
        "cardigann" => {
            let def = definitions.get(&indexer.definition_id)?;
            let config = serde_json::from_str(&indexer.settings_json)?;
            indexer_engine::search(http_client, def, &config, &query).await?
        }
        _ => continue,
    };
    // score, deduplicate, grab best...
}
```

### UI: Add indexer flow

1. User clicks "Add Indexer" in Settings
2. UI shows searchable list of all available definitions (500+ from the YAML cache)
3. User picks one (e.g., "1337x")
4. UI shows the definition's `settings` fields (username, password, etc.)
5. User fills in settings, saves
6. Kino creates an `indexer` row with `indexer_type = 'cardigann'`, `definition_id = '1337x'`
7. Test button: runs a search for "test" and checks for results

For Torznab indexers (Prowlarr, Jackett), the existing manual URL entry still works.

## Dependencies

```toml
scraper = "0.20"          # CSS selector engine (replaces AngleSharp)
serde_yaml = "0.9"        # YAML deserialization
indexmap = "2"             # ordered HashMap (field order matters for extraction)
```

Already in project: `reqwest`, `quick-xml`, `regex`, `chrono`, `serde`, `serde_json`.

## Cloudflare bypass

Private trackers that gate behind Cloudflare interactive challenges can't be scraped with a plain `reqwest` client. kino ships a two-stage solver in `indexers::cloudflare`:

1. **TLS-fingerprint-aware HTTP client** (`wreq` + `veilus-fingerprint`) — handles the vast majority of "just make my TLS hello look like Chrome" challenges without spawning a browser.
2. **Camoufox + Node + Playwright fallback** — when the TLS-only pass still returns an interstitial, the solver boots a headless Camoufox (Firefox fork with anti-detection patches) via Node + Playwright, completes the challenge, and exports the resulting cookies back into the shared `reqwest` cookie jar. Only runs when needed; first invocation kicks off the launcher, subsequent requests reuse the same session.

Both stages are opt-in — the solver is constructed lazily from `state.data_path` and only used when an indexer search surfaces a 403 + challenge marker. kino still runs fine against public trackers and TLS-simple private ones without the Node / Camoufox layer on disk.

## Module layout

```
src/indexers/
├── mod.rs              # search() public API
├── definition.rs       # CardigannDefinition + sub-structs (serde)
├── template.rs         # {{ .Query.Q }} expansion
├── filters.rs          # 25 filter functions
├── request.rs          # HTTP request builder + login
├── parser.rs           # HTML/JSON/XML → Vec<Release>
├── cloudflare.rs       # TLS-fingerprint + Camoufox challenge solver
└── loader.rs           # load from disk, fetch from remote
```

Estimated: ~2,400 lines total.

## What this replaces

| Before | After |
|--------|-------|
| Prowlarr container (~200MB .NET image) | Built into kino binary |
| Separate config UI | Integrated in kino settings |
| Manual Prowlarr → kino Torznab wiring | Direct definition → search |
| Community definitions via Prowlarr updates | Same definitions, fetched directly |

## What this does NOT replace

- **FlareSolverr**: Cloudflare-protected sites still need a headless browser. Kino can integrate with FlareSolverr as an optional external service, same as Prowlarr does. Most public trackers don't need it.
- **Native C# indexer implementations**: Prowlarr has ~55 hand-coded indexers for complex private trackers (BTN, PTP, etc.). These use bespoke JSON-RPC/REST APIs, not Cardigann YAML. Supporting them requires per-tracker Rust code. Phase 2 — add native adapters for high-value private trackers (UNIT3D covers ~30 trackers with one adapter).

## Error handling

- **Login failure**: Mark indexer as temporarily disabled, escalation backoff (1h → 6h → 24h)
- **Rate limiting**: Respect `Retry-After` headers, per-indexer throttle
- **Parse failure**: Log warning with definition ID + response snippet, skip indexer for this search
- **Missing fields**: Fields marked `optional` in the definition are allowed to fail silently
- **Definition load failure**: Skip invalid YAMLs, log warning, continue with others

## Testing strategy

1. **Unit tests per filter**: each of the 25 filters gets a test with known inputs/outputs
2. **Template tests**: real template expressions from popular definitions
3. **Parser integration tests**: saved HTML responses from real trackers, verify extracted fields
4. **Definition compatibility**: load all 500+ YAML files from Prowlarr/Indexers, verify they parse without error (structural test, not functional)
5. **Live search test**: search a public tracker (1337x or similar) and verify real results
