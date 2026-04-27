//! Built-in Cardigann-compatible indexer engine.
//!
//! Executes Prowlarr/Jackett YAML indexer definitions natively in Rust.
//! Supports HTML, JSON, and XML response parsing with CSS selectors,
//! Go-style templates, and 25+ filter functions.
//!
//! ## Public API
//!
//! - `model::{Indexer, CreateIndexer, UpdateIndexer}` — DB row + DTOs
//!   used by acquisition's search loops + the HTTP handlers
//! - `loader::DefinitionLoader` — startup-loaded definition cache
//!   `AppState` carries; consumed by acquisition + by handlers' "test
//!   indexer" path
//! - `request::IndexerClient` — the per-indexer HTTP client (cookies
//!   + cf solver state) `AppState` caches
//! - `cloudflare::CloudflareSolver` — turnstile/cf-clearance solver
//!   the request module uses
//! - `health::health_sweep` — scheduler entry; pings each enabled
//!   indexer with a real search, manages the backoff ladder
//! - `handlers` — HTTP CRUD + test-button + retry, registered via
//!   main.rs
//!
//! Internal: `definition`, `downloader`, `filters`, `parser`,
//! `template` — the Cardigann engine's working parts.

pub mod cloudflare;
pub mod definition;
pub mod downloader;
pub mod filters;
pub mod handlers;
pub mod health;
pub mod loader;
pub mod model;
pub mod parser;
pub mod request;
pub mod template;

use std::collections::HashMap;

use definition::CardigannDefinition;
use request::IndexerClient;
use template::{SearchQuery, TemplateContext};

use crate::torznab::parse::TorznabRelease;

/// Execute a Cardigann search against an indexer definition, returning results
/// in the same `TorznabRelease` format used by the Torznab path.
#[allow(clippy::implicit_hasher)]
#[tracing::instrument(skip_all, fields(indexer = %definition.id, q = %query.q))]
pub async fn search(
    client: &IndexerClient,
    definition: &CardigannDefinition,
    settings: &HashMap<String, String>,
    query: &SearchQuery,
) -> anyhow::Result<Vec<TorznabRelease>> {
    let start = std::time::Instant::now();

    // Ensure we are authenticated (no-op for public trackers).
    client.ensure_login(definition, settings).await?;

    // Build template context for request building and response parsing.
    let context = TemplateContext::new(settings.clone(), query.clone());

    // Build search requests from the definition.
    let request_specs = request::build_search_requests(definition, &context)?;
    if request_specs.is_empty() {
        tracing::debug!("no search requests produced");
        return Ok(Vec::new());
    }
    tracing::debug!(requests = request_specs.len(), "cardigann search start");

    let base_url = definition.links.first().cloned().unwrap_or_default();
    let mut all_releases = Vec::new();

    for spec in &request_specs {
        let response_type = spec.response_type.as_deref().unwrap_or("html");

        let body = match client.execute(spec).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(url = %spec.url, error = %e, "cardigann request failed");
                continue;
            }
        };

        let parsed = parser::parse_response(definition, &body, response_type, &context)?;
        tracing::debug!(
            url = %spec.url,
            response_type,
            parsed = parsed.len(),
            "cardigann parse",
        );

        for p in parsed {
            let download_url = p.download.as_ref().map(|d| {
                if d.starts_with("http://") || d.starts_with("https://") {
                    d.clone()
                } else {
                    format!(
                        "{}/{}",
                        base_url.trim_end_matches('/'),
                        d.trim_start_matches('/')
                    )
                }
            });

            // Cardigann definitions expose category as a single string
            // (tracker-native id); the Torznab wire format carries a
            // list of numeric ids. Fan the single id out into the list
            // so downstream cap-probing / category filters see it. We
            // only forward values that parse cleanly — a non-numeric
            // tracker label has no meaning against Newznab caps.
            let categories = p
                .category
                .as_deref()
                .and_then(|c| c.trim().parse::<i64>().ok())
                .map(|c| vec![c])
                .unwrap_or_default();

            all_releases.push(TorznabRelease {
                title: p.title.clone(),
                guid: p
                    .details
                    .clone()
                    .or_else(|| p.download.clone())
                    .unwrap_or_else(|| p.title.clone()),
                size: p.size,
                download_url,
                magnet_url: p.magnet_url,
                info_url: p.details,
                info_hash: p.info_hash,
                publish_date: p.publish_date,
                seeders: p.seeders,
                leechers: p.leechers,
                grabs: p.grabs,
                categories,
            });
        }
    }

    tracing::info!(
        releases = all_releases.len(),
        duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        "cardigann search done",
    );
    Ok(all_releases)
}
