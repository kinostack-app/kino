//! Trakt list resolver — covers both user-pasted custom lists
//! (`trakt.tv/users/{user}/lists/{slug}`) and the auto-managed user
//! watchlist. Reuses [`TraktClient`] for OAuth, so no separate auth
//! plumbing here.

use serde::Deserialize;
use sqlx::SqlitePool;

use super::{ListMetadata, ListsError, RawItem};
use crate::integrations::trakt::{TraktClient, TraktError};

#[derive(Debug, Deserialize)]
struct TraktListMeta {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    item_count: Option<i64>,
}

/// Wrapper for `/users/{user}/lists/{slug}/items` and watchlist items.
#[derive(Debug, Deserialize)]
struct TraktListItem {
    #[serde(default)]
    rank: Option<i64>,
    #[serde(default)]
    listed_at: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    movie: Option<TraktItemBody>,
    #[serde(default)]
    show: Option<TraktItemBody>,
}

#[derive(Debug, Deserialize)]
struct TraktItemBody {
    title: String,
    #[serde(default)]
    ids: TraktItemIds,
}

#[derive(Debug, Default, Deserialize)]
struct TraktItemIds {
    #[serde(default)]
    tmdb: Option<i64>,
}

pub async fn fetch_metadata(db: &SqlitePool, source_id: &str) -> Result<ListMetadata, ListsError> {
    let client = trakt_client(db).await?;
    let (user, slug) = split_source_id(source_id)?;
    // `?extended=full` carries `item_count` in Trakt's response —
    // without it, the core shape omits the count and the UI rendered
    // "0 items" on Trakt custom lists until the first poll landed.
    let meta: TraktListMeta = client
        .get(&format!("/users/{user}/lists/{slug}?extended=full"))
        .await
        .map_err(map_err)?;
    // Belt and braces: if Trakt still omits the field (older shape,
    // private list, etc.), count items ourselves with one extra call.
    let item_count = if let Some(c) = meta.item_count {
        c
    } else {
        let items: Vec<TraktListItem> = client
            .get(&format!("/users/{user}/lists/{slug}/items"))
            .await
            .map_err(map_err)?;
        i64::try_from(items.len()).unwrap_or(i64::MAX)
    };
    Ok(ListMetadata {
        title: meta.name,
        description: meta.description,
        item_count,
        item_type: "mixed".into(),
    })
}

pub async fn fetch_items(db: &SqlitePool, source_id: &str) -> Result<Vec<RawItem>, ListsError> {
    let client = trakt_client(db).await?;
    let (user, slug) = split_source_id(source_id)?;
    let raw: Vec<TraktListItem> = client
        .get(&format!("/users/{user}/lists/{slug}/items"))
        .await
        .map_err(map_err)?;
    Ok(map_items(raw))
}

/// `source_id` is stored as `{user}/{slug}` from the parser; Trakt's
/// API path shape is `/users/{user}/lists/{slug}`, so we split here.
fn split_source_id(source_id: &str) -> Result<(&str, &str), ListsError> {
    source_id.split_once('/').ok_or_else(|| {
        ListsError::Parse(format!(
            "trakt list source_id must be `user/slug`, got {source_id}"
        ))
    })
}

pub async fn fetch_watchlist_metadata(db: &SqlitePool) -> Result<ListMetadata, ListsError> {
    let client = trakt_client(db).await?;
    let raw: Vec<TraktListItem> = client.get("/sync/watchlist").await.map_err(map_err)?;
    let count = i64::try_from(raw.len()).unwrap_or(i64::MAX);
    Ok(ListMetadata {
        title: "Trakt watchlist".into(),
        description: Some("Items you've added to your Trakt watchlist.".into()),
        item_count: count,
        item_type: "mixed".into(),
    })
}

pub async fn fetch_watchlist_items(db: &SqlitePool) -> Result<Vec<RawItem>, ListsError> {
    let client = trakt_client(db).await?;
    let raw: Vec<TraktListItem> = client.get("/sync/watchlist").await.map_err(map_err)?;
    Ok(map_items(raw))
}

fn map_items(raw: Vec<TraktListItem>) -> Vec<RawItem> {
    let now = crate::time::Timestamp::now().to_rfc3339();
    raw.into_iter()
        .filter_map(|i| {
            let (item_type, body) = match (i.movie, i.show, i.kind.as_deref()) {
                (Some(m), _, _) => ("movie", m),
                (_, Some(s), _) => ("show", s),
                _ => return None,
            };
            let tmdb_id = body.ids.tmdb?;
            Some(RawItem {
                tmdb_id,
                item_type: item_type.into(),
                title: body.title,
                poster_path: None,
                position: i.rank,
                added_at: i.listed_at.unwrap_or_else(|| now.clone()),
            })
        })
        .collect()
}

async fn trakt_client(db: &SqlitePool) -> Result<TraktClient, ListsError> {
    TraktClient::from_db(db.clone())
        .await
        .map_err(|_| ListsError::TraktNotConnected)
}

fn map_err(e: TraktError) -> ListsError {
    match e {
        TraktError::NotConfigured | TraktError::NotConnected => ListsError::TraktNotConnected,
        TraktError::Api { status, message } => {
            if status == 404 {
                ListsError::NotFound(message)
            } else if status == 401 || status == 403 {
                ListsError::Auth(message)
            } else {
                ListsError::Network(format!("trakt {status}: {message}"))
            }
        }
        other => ListsError::Network(other.to_string()),
    }
}
