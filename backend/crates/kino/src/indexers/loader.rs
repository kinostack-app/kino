//! Definition loader — loads and caches Cardigann YAML definitions from disk.
//! Definitions are sourced from the Prowlarr/Indexers community repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::definition::CardigannDefinition;

/// Manages loading and caching of indexer definitions.
///
/// The in-memory map sits behind a `parking_lot::RwLock` so the
/// weekly refresh task (`definitions_refresh`) can swap in a freshly
/// pulled YAML set without a restart. Lookups go through a read lock
/// and the write happens at the end of `update_from_remote` once the
/// disk write completes; readers never see a half-built map.
#[derive(Debug)]
pub struct DefinitionLoader {
    definitions: parking_lot::RwLock<HashMap<String, CardigannDefinition>>,
    definitions_dir: PathBuf,
}

impl DefinitionLoader {
    /// Create a new loader pointing at a definitions directory.
    pub fn new(definitions_dir: PathBuf) -> Self {
        Self {
            definitions: parking_lot::RwLock::new(HashMap::new()),
            definitions_dir,
        }
    }

    /// Load all .yml files from the definitions directory.
    pub fn load_all(&self) -> anyhow::Result<usize> {
        let dir = &self.definitions_dir;
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            return Ok(0);
        }

        let mut fresh: HashMap<String, CardigannDefinition> = HashMap::new();
        let mut count = 0;
        let mut seen = 0_usize;
        let mut skipped = 0_usize;
        let entries = std::fs::read_dir(dir)?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yml") {
                seen += 1;
                match load_definition(&path) {
                    Ok(def) => {
                        tracing::debug!(id = %def.id, name = %def.name, "loaded definition");
                        fresh.insert(def.id.clone(), def);
                        count += 1;
                    }
                    Err(e) => {
                        skipped += 1;
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        tracing::warn!(file = %name, error = %e, "failed to parse definition");
                    }
                }
            }
        }
        // Atomic swap — readers either see the old set or the new
        // set, never a partial merge.
        *self.definitions.write() = fresh;

        // Skipped files are not just parse errors — an fd exhaustion
        // (EMFILE) at startup silently drops a chunk of definitions
        // and the only visible symptom is "searches return 0 results"
        // much later. Surface the shortfall explicitly so the
        // operator sees it before a user does.
        if skipped > 0 {
            tracing::warn!(
                loaded = count,
                skipped,
                seen,
                dir = %dir.display(),
                "indexer loader skipped definitions — search coverage is partial. \
                 EMFILE / 'Too many open files' at boot is the usual cause; raise \
                 the container's nofile soft limit."
            );
        } else {
            tracing::info!(count, dir = %dir.display(), "loaded indexer definitions");
        }
        Ok(count)
    }

    /// Get a definition by ID. Returns a cloned definition so the
    /// caller doesn't hold the internal `RwLock` across `await`s —
    /// search paths are async and holding the guard would serialise
    /// every Cardigann search behind a single lock.
    pub fn get(&self, id: &str) -> Option<CardigannDefinition> {
        self.definitions.read().get(id).cloned()
    }

    /// List all available definitions (id, name, type, language, top-level categories).
    pub fn list(&self) -> Vec<DefinitionSummary> {
        let guard = self.definitions.read();
        let mut list: Vec<_> = guard
            .values()
            .map(|d| DefinitionSummary {
                id: d.id.clone(),
                name: d.name.clone(),
                description: d.description.clone().unwrap_or_default(),
                indexer_type: IndexerDefinitionType::from_opt(d.indexer_type.as_deref()),
                language: d.language.clone().unwrap_or_else(|| "en-US".into()),
                categories: top_level_categories(d),
            })
            .collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        list
    }

    /// Number of loaded definitions.
    pub fn count(&self) -> usize {
        self.definitions.read().len()
    }

    pub async fn update_from_remote(
        &self,
        progress: Option<std::sync::Arc<dyn Fn(u32, u32) + Send + Sync>>,
    ) -> anyhow::Result<usize> {
        let url = "https://codeload.github.com/Prowlarr/Indexers/tar.gz/refs/heads/master";

        let client = reqwest::Client::builder().user_agent("kino/0.1").build()?;
        tracing::info!(%url, "fetching definitions tarball");

        let bytes = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        tracing::info!(size = bytes.len(), "tarball received");

        std::fs::create_dir_all(&self.definitions_dir)?;
        let dir = self.definitions_dir.clone();

        let count = tokio::task::spawn_blocking(move || -> anyhow::Result<u32> {
            extract_v11_yamls(&bytes, &dir, progress.as_deref())
        })
        .await??;

        tracing::info!(count, "definition update complete");
        self.load_all()?;
        Ok(count as usize)
    }
}

const ESTIMATED_TOTAL: u32 = 550;

fn extract_v11_yamls(
    tarball: &[u8],
    dir: &Path,
    progress: Option<&(dyn Fn(u32, u32) + Send + Sync)>,
) -> anyhow::Result<u32> {
    if let Some(cb) = progress {
        cb(0, ESTIMATED_TOTAL);
    }

    let cursor = std::io::Cursor::new(tarball);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    let mut written: u32 = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(filename) = v11_yaml_filename(&path) else {
            continue;
        };
        let dest = dir.join(filename);
        let mut buf = Vec::with_capacity(8 * 1024);
        std::io::copy(&mut entry, &mut buf)?;
        let tmp = dest.with_extension("yml.tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, &dest)?;
        written = written.saturating_add(1);
        if let Some(cb) = progress {
            cb(written, ESTIMATED_TOTAL.max(written));
        }
    }

    if let Some(cb) = progress {
        cb(written, written);
    }
    Ok(written)
}

fn v11_yaml_filename(path: &Path) -> Option<&std::ffi::OsStr> {
    let mut comps = path.components();
    comps.next()?;
    if comps.next()?.as_os_str() != "definitions" {
        return None;
    }
    if comps.next()?.as_os_str() != "v11" {
        return None;
    }
    let filename = path.file_name()?;
    let ext = path.extension()?;
    if !ext.eq_ignore_ascii_case("yml") {
        return None;
    }
    Some(filename)
}

/// Scheduler entry-point: daily sweep that refreshes indexer
/// definitions from the Prowlarr repo. Routed through the same
/// `start_refresh` path as the manual UI button so both share the
/// tracker, the WS event stream, and the timestamp write — only
/// the trigger differs. Registered as `definitions_refresh` in
/// `scheduler::register_defaults` with a 24h interval.
///
/// **Consent gate.** The catalogue source is a third-party repo
/// (Prowlarr/Indexers); kino doesn't reach out to it without an
/// explicit user signal. The flag is `definitions_auto_refresh_enabled`
/// on the config row, set by the manual refresh path the first
/// time the user clicks "Download catalogue" in the wizard / Settings.
/// Pre-consent boots are completely silent — no scheduled fetch,
/// no log spam, no surprise outbound traffic. Once the user has
/// asked at least once, the daily refresh keeps the catalogue
/// current; users can opt back out via Settings → Indexers (TODO).
pub async fn refresh_sweep(state: &crate::state::AppState) -> anyhow::Result<()> {
    if state.definitions.is_none() {
        tracing::debug!("definitions_refresh: no loader configured — skipping");
        return Ok(());
    }
    let consented: bool =
        sqlx::query_scalar("SELECT definitions_auto_refresh_enabled FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .map(|n: i64| n != 0)
            .unwrap_or(false);
    if !consented {
        tracing::debug!(
            "definitions_refresh: user has not opted in — skipping (set via manual refresh)"
        );
        return Ok(());
    }
    crate::indexers::refresh::start_refresh(
        state.definitions_refresh.clone(),
        state.definitions.clone(),
        state.event_tx.clone(),
        state.db.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("definitions_refresh start failed: {e}"))?;
    Ok(())
}

/// Derive the set of top-level categories ("Movies", "TV", "Audio", "Books",
/// "Anime", "XXX", "Other") the indexer supports, based on the `cat:` values
/// in its `categorymappings`. The cardigann YAMLs use a `"Top/Sub"` shape
/// (e.g. `"Movies/HD"`, `"TV/Anime"`), so the first segment is what we want.
/// Returns a deduped, sorted list.
fn top_level_categories(d: &CardigannDefinition) -> Vec<String> {
    let Some(ref caps) = d.caps else {
        return Vec::new();
    };
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for mapping in &caps.categorymappings {
        let top = mapping.cat.split('/').next().unwrap_or("").trim();
        if top.is_empty() {
            continue;
        }
        // Map "Console" / "PC" / "Other" / etc. to "Other" so the UI has a
        // small, predictable taxonomy. This keeps the filter pills short
        // without throwing away the long tail.
        let normalised = match top {
            "Movies" | "TV" | "Audio" | "Books" | "Anime" | "XXX" => top.to_string(),
            _ => "Other".to_string(),
        };
        seen.insert(normalised);
    }
    seen.into_iter().collect()
}

fn load_definition(path: &Path) -> anyhow::Result<CardigannDefinition> {
    let content = std::fs::read_to_string(path)?;
    // Some community YAML files are saved with a UTF-8 BOM, which serde_yaml
    // treats as part of the first key and rejects. Strip it.
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let def: CardigannDefinition = serde_yaml::from_str(content)?;
    Ok(def)
}

/// Whether a Cardigann definition describes a private tracker (login
/// required) or a public one. Exposed as a typed enum so the frontend
/// can branch on it without `.toLowerCase() === 'private'` stringly
/// checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum IndexerDefinitionType {
    Public,
    SemiPrivate,
    Private,
}

impl IndexerDefinitionType {
    fn from_opt(s: Option<&str>) -> Self {
        let lowered = s.map(str::to_ascii_lowercase);
        match lowered.as_deref() {
            Some("private") => Self::Private,
            Some("semi-private" | "semiprivate") => Self::SemiPrivate,
            _ => Self::Public,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DefinitionSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub indexer_type: IndexerDefinitionType,
    /// BCP-47 language tag of the tracker's interface. Free-form
    /// string — Cardigann YAMLs cover a long tail (en-US, zh-cn,
    /// multi, etc.) that doesn't cleanly fit a typed enum without
    /// losing fidelity.
    pub language: String,
    /// Top-level categories derived from the definition's categorymappings,
    /// normalised to a small taxonomy (Movies / TV / Audio / Books / Anime /
    /// XXX / Other). Used by the settings UI to let users narrow the 500+
    /// list to the ~80 indexers that actually serve TV+movies.
    pub categories: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_all_prowlarr_definitions() {
        let defs_dir = std::path::PathBuf::from("/workspace/ref/prowlarr-indexers/definitions/v11");
        if !defs_dir.exists() {
            eprintln!("skipping: prowlarr-indexers not cloned");
            return;
        }

        let loader = DefinitionLoader::new(defs_dir);
        let count = loader.load_all().unwrap();

        // Should load the vast majority (some may have edge-case YAML we don't handle)
        let total_yml = std::fs::read_dir(loader.definitions_dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("yml"))
            })
            .count();

        #[allow(clippy::cast_precision_loss)]
        let pct = (count as f64 / total_yml as f64) * 100.0;
        eprintln!("{count}/{total_yml} definitions loaded ({pct:.1}%)");

        // We should parse at least 90% successfully
        assert!(
            pct > 90.0,
            "only {pct:.1}% of definitions parsed — expected >90%"
        );
    }
}
