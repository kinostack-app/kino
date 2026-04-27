//! `MDBList` API resolver. Single key per kino install (user-provided),
//! 60 req/min documented limit. We keep our touch minimal: one
//! metadata fetch per list refresh + one items fetch.

use serde::Deserialize;
use sqlx::SqlitePool;

use super::{ListMetadata, ListsError, RawItem};

const BASE: &str = "https://mdblist.com/api";

async fn api_key(db: &SqlitePool) -> Result<String, ListsError> {
    let key: Option<String> = sqlx::query_scalar("SELECT mdblist_api_key FROM config WHERE id = 1")
        .fetch_optional(db)
        .await?
        .flatten();
    let Some(k) = key.filter(|s| !s.is_empty()) else {
        return Err(ListsError::MissingMdblistKey);
    };
    Ok(k)
}

#[derive(Debug, Deserialize)]
struct MdbList {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    items: Option<i64>,
    #[serde(default)]
    mediatype: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MdbItem {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    tmdb_id: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    poster: Option<String>,
    #[serde(default)]
    mediatype: Option<String>,
    #[serde(default)]
    rank: Option<i64>,
}

pub async fn fetch_metadata(db: &SqlitePool, source_id: &str) -> Result<ListMetadata, ListsError> {
    let key = api_key(db).await?;
    let url = format!("{BASE}/lists/{source_id}?apikey={key}");
    let lists: Vec<MdbList> = http_get_json(&url).await?;
    let Some(list) = lists.into_iter().next() else {
        return Err(ListsError::NotFound(source_id.into()));
    };
    let item_type = match list.mediatype.as_deref() {
        Some("movie") => "movies",
        Some("show") => "shows",
        _ => "mixed",
    };
    Ok(ListMetadata {
        title: list.name,
        description: list.description,
        item_count: list.items.unwrap_or(0),
        item_type: item_type.into(),
    })
}

pub async fn fetch_items(db: &SqlitePool, source_id: &str) -> Result<Vec<RawItem>, ListsError> {
    let key = api_key(db).await?;
    let url = format!("{BASE}/lists/{source_id}/items?apikey={key}");
    let items: Vec<MdbItem> = http_get_json(&url).await?;
    let now = crate::time::Timestamp::now().to_rfc3339();
    let mapped = items
        .into_iter()
        .filter_map(|i| {
            let tmdb_id = i.tmdb_id.or(i.id)?;
            let item_type = match i.mediatype.as_deref() {
                Some("movie") => "movie",
                Some("show") => "show",
                _ => return None,
            };
            Some(RawItem {
                tmdb_id,
                item_type: item_type.into(),
                title: i.title.unwrap_or_default(),
                poster_path: i.poster,
                position: i.rank,
                added_at: now.clone(),
            })
        })
        .collect();
    Ok(mapped)
}

async fn http_get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, ListsError> {
    let resp = reqwest::Client::new()
        .get(url)
        .header("User-Agent", concat!("kino/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .map_err(|e| ListsError::Network(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ListsError::Auth(format!("MDBList returned {status}")));
    }
    if !status.is_success() {
        return Err(ListsError::Network(format!("MDBList returned {status}")));
    }
    resp.json::<T>()
        .await
        .map_err(|e| ListsError::Parse(e.to_string()))
}
