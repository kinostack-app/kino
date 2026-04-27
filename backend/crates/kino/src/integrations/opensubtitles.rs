//! Minimal `OpenSubtitles` REST client for fetching subtitles after import.
//!
//! Endpoint: <https://api.opensubtitles.com/api/v1>
//! Flow:
//!   1. `login(username, password)` → access token
//!   2. `search_by_imdb(imdb_id, languages)` → list of files
//!   3. `download(file_id)` → signed URL → bytes
//!
//! The client caches the login token for its lifetime. The rate-limit on
//! `OpenSubtitles` is 5/s authenticated — `reqwest::Client` keeps its own
//! connection pool; this module does no extra concurrency limiting.
//!
//! Subtitle files are saved alongside the media file as `<stem>.<lang>.srt`.

use std::path::Path;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

const BASE: &str = "https://api.opensubtitles.com/api/v1";
const USER_AGENT: &str = "kino/0.1";

/// `OpenSubtitles` credentials + API key.
#[derive(Debug, Clone)]
pub struct OsCredentials {
    pub api_key: String,
    pub username: String,
    pub password: String,
}

/// Minimal client. Re-login on 401; otherwise reuses the token.
#[derive(Debug, Clone)]
pub struct OpenSubtitlesClient {
    http: reqwest::Client,
    creds: OsCredentials,
    token: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum OsError {
    #[error("http: {0}")]
    Http(String),
    #[error("api {0}: {1}")]
    Api(u16, String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("no subtitles found")]
    NotFound,
}

impl OpenSubtitlesClient {
    pub fn new(creds: OsCredentials) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("build reqwest client"),
            creds,
            token: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn base_headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static("api-key"),
            HeaderValue::from_str(&self.creds.api_key).unwrap_or(HeaderValue::from_static("")),
        );
        h.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );
        h
    }

    /// Test credentials without caching state — primarily used by the
    /// settings page's "Test Connection" button. A successful response
    /// means the API key + username + password are all valid.
    pub async fn test_login(&self) -> Result<(), OsError> {
        self.ensure_token().await.map(|_| ())
    }

    async fn ensure_token(&self) -> Result<String, OsError> {
        {
            let guard = self.token.lock().await;
            if let Some(ref t) = *guard {
                return Ok(t.clone());
            }
        }
        let body = serde_json::json!({
            "username": self.creds.username,
            "password": self.creds.password,
        });
        let resp = self
            .http
            .post(format!("{BASE}/login"))
            .headers(self.base_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| OsError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OsError::Api(status.as_u16(), body));
        }
        let parsed: LoginResponse = resp
            .json()
            .await
            .map_err(|e| OsError::Parse(e.to_string()))?;
        *self.token.lock().await = Some(parsed.token.clone());
        Ok(parsed.token)
    }

    /// Search subtitles by IMDB id + comma-separated language codes.
    pub async fn search_by_imdb(
        &self,
        imdb_id: &str,
        languages: &str,
    ) -> Result<Vec<SubtitleHit>, OsError> {
        // IMDB id from TMDB is like "tt1234567"; the numeric form is also
        // accepted, but we pass as-is — OpenSubtitles tolerates both.
        let url = format!(
            "{BASE}/subtitles?imdb_id={}&languages={}",
            imdb_id.trim_start_matches("tt"),
            languages
        );
        let resp = self
            .http
            .get(&url)
            .headers(self.base_headers())
            .send()
            .await
            .map_err(|e| OsError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OsError::Api(status.as_u16(), body));
        }
        let parsed: SearchResponse = resp
            .json()
            .await
            .map_err(|e| OsError::Parse(e.to_string()))?;
        Ok(parsed
            .data
            .into_iter()
            .filter_map(SubtitleHit::from_response)
            .collect())
    }

    /// Request a signed download URL for a subtitle file.
    pub async fn download_link(&self, file_id: i64) -> Result<String, OsError> {
        let token = self.ensure_token().await?;
        let mut headers = self.base_headers();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .unwrap_or(HeaderValue::from_static("")),
        );
        let body = serde_json::json!({ "file_id": file_id });
        let resp = self
            .http
            .post(format!("{BASE}/download"))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| OsError::Http(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            *self.token.lock().await = None;
            return Err(OsError::Api(401, "token expired".into()));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OsError::Api(status.as_u16(), body));
        }
        let parsed: DownloadResponse = resp
            .json()
            .await
            .map_err(|e| OsError::Parse(e.to_string()))?;
        Ok(parsed.link)
    }

    /// Fetch a subtitle and write it next to `media_file` as `<stem>.<lang>.srt`.
    pub async fn download_to(
        &self,
        hit: &SubtitleHit,
        media_file: &Path,
    ) -> Result<std::path::PathBuf, OsError> {
        let link = self.download_link(hit.file_id).await?;
        let bytes = self
            .http
            .get(&link)
            .send()
            .await
            .map_err(|e| OsError::Http(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| OsError::Http(e.to_string()))?;

        let stem = media_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("subtitle");
        let lang = &hit.language;
        let target = media_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}.{lang}.srt"));

        tokio::fs::write(&target, &bytes)
            .await
            .map_err(|e| OsError::Http(e.to_string()))?;
        Ok(target)
    }
}

/// A single subtitle match — the minimal fields we need to download.
#[derive(Debug, Clone)]
pub struct SubtitleHit {
    pub file_id: i64,
    pub language: String,
    pub release: Option<String>,
}

impl SubtitleHit {
    fn from_response(item: SearchItem) -> Option<Self> {
        let attrs = item.attributes?;
        let file = attrs.files?.into_iter().next()?;
        Some(Self {
            file_id: file.file_id,
            language: attrs.language.unwrap_or_else(|| "en".into()),
            release: attrs.release,
        })
    }
}

// ── Response types (only the fields we care about) ──

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

#[derive(Deserialize)]
struct DownloadResponse {
    link: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    attributes: Option<SearchAttributes>,
}

#[derive(Deserialize)]
struct SearchAttributes {
    language: Option<String>,
    release: Option<String>,
    files: Option<Vec<SearchFile>>,
}

#[derive(Deserialize)]
struct SearchFile {
    file_id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_response_extracts_file_id() {
        let json = r#"{
            "data": [
              { "attributes": { "language": "en", "release": "release-group", "files": [{"file_id": 12345}] } },
              { "attributes": { "language": "fr", "files": [{"file_id": 6789}] } }
            ]
        }"#;
        let parsed: SearchResponse = serde_json::from_str(json).unwrap();
        let hits: Vec<SubtitleHit> = parsed
            .data
            .into_iter()
            .filter_map(SubtitleHit::from_response)
            .collect();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].file_id, 12345);
        assert_eq!(hits[0].language, "en");
        assert_eq!(hits[0].release.as_deref(), Some("release-group"));
        assert_eq!(hits[1].file_id, 6789);
        assert_eq!(hits[1].language, "fr");
    }

    #[test]
    fn items_without_files_are_skipped() {
        let json = r#"{ "data": [ { "attributes": { "language": "en" } } ] }"#;
        let parsed: SearchResponse = serde_json::from_str(json).unwrap();
        let hits: Vec<SubtitleHit> = parsed
            .data
            .into_iter()
            .filter_map(SubtitleHit::from_response)
            .collect();
        assert!(hits.is_empty());
    }
}
