#![allow(dead_code)] // Used by search subsystem in later phases

use std::fmt::Write;

use reqwest::Client;

use super::parse::{TorznabRelease, parse_torznab_response};

/// Torznab API client for a single indexer.
#[derive(Debug, Clone)]
pub struct TorznabClient {
    http: Client,
}

/// Parameters for a Torznab search query.
#[derive(Debug, Default)]
pub struct TorznabQuery {
    pub q: Option<String>,
    pub imdbid: Option<String>,
    pub tvdbid: Option<i64>,
    pub tmdbid: Option<i64>,
    pub season: Option<i64>,
    pub ep: Option<i64>,
    pub cat: Option<String>,
}

impl TorznabClient {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
        }
    }

    /// Search a Torznab indexer.
    pub async fn search(
        &self,
        base_url: &str,
        api_key: Option<&str>,
        query: &TorznabQuery,
    ) -> Result<Vec<TorznabRelease>, TorznabError> {
        let mut url = format!("{base_url}?t=search");

        if let Some(key) = api_key {
            let _ = write!(url, "&apikey={key}");
        }
        if let Some(ref q) = query.q {
            let _ = write!(url, "&q={}", urlencoding::encode(q));
        }
        if let Some(ref imdbid) = query.imdbid {
            let _ = write!(url, "&imdbid={imdbid}");
        }
        if let Some(tvdbid) = query.tvdbid {
            let _ = write!(url, "&tvdbid={tvdbid}");
        }
        if let Some(tmdbid) = query.tmdbid {
            let _ = write!(url, "&tmdbid={tmdbid}");
        }
        if let Some(season) = query.season {
            let _ = write!(url, "&season={season}");
        }
        if let Some(ep) = query.ep {
            let _ = write!(url, "&ep={ep}");
        }
        if let Some(ref cat) = query.cat {
            let _ = write!(url, "&cat={cat}");
        }

        // Redact apikey from the URL before logging.
        let log_url = url.replace(api_key.unwrap_or("__nope__"), "[REDACTED]");
        let start = std::time::Instant::now();
        tracing::debug!(url = %log_url, "torznab GET");

        let resp = self.http.get(&url).send().await.map_err(|e| {
            tracing::warn!(url = %log_url, error = %e, "torznab request failed");
            TorznabError::Network(e.to_string())
        })?;

        let status = resp.status();
        if !status.is_success() {
            tracing::warn!(url = %log_url, status = status.as_u16(), "torznab non-success");
            return Err(TorznabError::Http(status.as_u16()));
        }

        let body = resp.text().await.map_err(|e| {
            tracing::warn!(url = %log_url, error = %e, "torznab body read failed");
            TorznabError::Network(e.to_string())
        })?;

        let body_bytes = body.len();
        let parsed = parse_torznab_response(&body).map_err(|e| {
            tracing::warn!(url = %log_url, body_bytes, error = %e, "torznab parse failed");
            TorznabError::Parse(e.clone())
        })?;
        tracing::debug!(
            url = %log_url,
            status = status.as_u16(),
            body_bytes,
            releases = parsed.len(),
            duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "torznab ok",
        );
        Ok(parsed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TorznabError {
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP {0}")]
    Http(u16),
    #[error("parse error: {0}")]
    Parse(String),
}
