#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Indexer {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub api_key: Option<String>,
    pub priority: i64,
    pub enabled: bool,
    pub supports_rss: bool,
    pub supports_search: bool,
    pub supported_categories: Option<String>,
    pub supported_search_params: Option<String>,
    pub initial_failure_time: Option<String>,
    pub most_recent_failure_time: Option<String>,
    pub escalation_level: i64,
    pub disabled_until: Option<String>,
    pub indexer_type: String,
    pub definition_id: Option<String>,
    pub settings_json: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateIndexer {
    pub name: String,
    pub url: String,
    pub api_key: Option<String>,
    pub priority: Option<i64>,
    pub enabled: Option<bool>,
    pub indexer_type: Option<String>,
    pub definition_id: Option<String>,
    pub settings_json: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateIndexer {
    pub name: Option<String>,
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub priority: Option<i64>,
    pub enabled: Option<bool>,
    pub indexer_type: Option<String>,
    pub definition_id: Option<String>,
    pub settings_json: Option<String>,
}
