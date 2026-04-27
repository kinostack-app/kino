//! TMDB list resolver. We already ship a TMDB key, so no extra auth
//! setup needed. Lists API is `/list/{id}` with up to 20 items per
//! page; we paginate until exhausted.

use serde::Deserialize;
use sqlx::SqlitePool;

use super::{ListMetadata, ListsError, RawItem};

const BASE: &str = "https://api.themoviedb.org/3";

async fn token(db: &SqlitePool) -> Result<String, ListsError> {
    let key: Option<String> = sqlx::query_scalar("SELECT tmdb_api_key FROM config WHERE id = 1")
        .fetch_optional(db)
        .await?;
    let Some(k) = key.filter(|s| !s.is_empty()) else {
        return Err(ListsError::Auth("TMDB key not configured".into()));
    };
    Ok(k)
}

#[derive(Debug, Deserialize)]
struct ListPage {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    item_count: Option<i64>,
    #[serde(default)]
    items: Vec<ListItem>,
    #[serde(default)]
    total_pages: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ListItem {
    id: i64,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
}

pub async fn fetch_metadata(db: &SqlitePool, source_id: &str) -> Result<ListMetadata, ListsError> {
    let tk = token(db).await?;
    let page = http_get_json::<ListPage>(&tk, &format!("{BASE}/list/{source_id}?page=1")).await?;
    let item_type = derive_item_type(&page.items);
    Ok(ListMetadata {
        title: page.name,
        description: page.description,
        item_count: page.item_count.unwrap_or(0),
        item_type,
    })
}

pub async fn fetch_items(db: &SqlitePool, source_id: &str) -> Result<Vec<RawItem>, ListsError> {
    let tk = token(db).await?;
    let now = crate::time::Timestamp::now().to_rfc3339();
    let mut out: Vec<RawItem> = Vec::new();
    let mut page_n = 1;
    loop {
        let page =
            http_get_json::<ListPage>(&tk, &format!("{BASE}/list/{source_id}?page={page_n}"))
                .await?;
        for (idx, it) in page.items.iter().enumerate() {
            let item_type = match it.media_type.as_deref() {
                Some("movie") => "movie",
                Some("tv") => "show",
                _ => continue,
            };
            let title = it
                .title
                .clone()
                .or_else(|| it.name.clone())
                .unwrap_or_default();
            let position = i64::try_from(out.len() + idx + 1).ok();
            out.push(RawItem {
                tmdb_id: it.id,
                item_type: item_type.into(),
                title,
                poster_path: it.poster_path.clone(),
                position,
                added_at: now.clone(),
            });
        }
        let total = page.total_pages.unwrap_or(1);
        if page_n >= total {
            break;
        }
        page_n += 1;
        if page_n > 50 {
            // Hard guard against pathological pagination — TMDB lists
            // are user-curated and >1000 items is exceptional.
            break;
        }
    }
    Ok(out)
}

fn derive_item_type(items: &[ListItem]) -> String {
    let (mut movies, mut shows) = (0u32, 0u32);
    for i in items {
        match i.media_type.as_deref() {
            Some("movie") => movies += 1,
            Some("tv") => shows += 1,
            _ => {}
        }
    }
    match (movies > 0, shows > 0) {
        (true, false) => "movies".into(),
        (false, true) => "shows".into(),
        _ => "mixed".into(),
    }
}

async fn http_get_json<T: serde::de::DeserializeOwned>(
    bearer: &str,
    url: &str,
) -> Result<T, ListsError> {
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(bearer)
        .header("User-Agent", concat!("kino/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .map_err(|e| ListsError::Network(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ListsError::Auth("TMDB key rejected".into()));
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ListsError::NotFound(url.into()));
    }
    if !status.is_success() {
        return Err(ListsError::Network(format!("TMDB returned {status}")));
    }
    resp.json::<T>()
        .await
        .map_err(|e| ListsError::Parse(e.to_string()))
}
