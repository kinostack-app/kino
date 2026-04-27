#![allow(dead_code)] // Used in Phase 3+ for paginated list endpoints

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Pagination query parameters.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PaginationParams {
    /// Maximum number of results to return.
    #[param(default = 25, minimum = 1, maximum = 100)]
    pub limit: Option<i64>,

    /// Opaque cursor from a previous response.
    pub cursor: Option<String>,

    /// Sort field name.
    pub sort: Option<String>,

    /// Sort direction: "asc" or "desc".
    #[param(default = "asc")]
    pub order: Option<String>,
}

impl PaginationParams {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(25).clamp(1, 100)
    }

    pub fn order_is_desc(&self) -> bool {
        self.order.as_deref() == Some("desc")
    }
}

/// Opaque cursor encoding the last item's sort key and ID.
#[derive(Debug, Serialize, Deserialize)]
pub struct Cursor {
    pub id: i64,
    pub sort_value: Option<String>,
}

impl Cursor {
    pub fn encode(&self) -> String {
        let json = serde_json::to_string(self).expect("cursor serialize");
        URL_SAFE_NO_PAD.encode(json)
    }

    pub fn decode(s: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(s).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Paginated response envelope.
#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedResponse<T: Serialize> {
    pub results: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

impl<T: Serialize> PaginatedResponse<T> {
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn new(mut results: Vec<T>, limit: i64, cursor_fn: impl Fn(&T) -> Cursor) -> Self {
        let has_more = results.len() as i64 > limit;
        if has_more {
            results.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            results.last().map(|item| cursor_fn(item).encode())
        } else {
            None
        };
        Self {
            results,
            next_cursor,
            has_more,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip() {
        let cursor = Cursor {
            id: 42,
            sort_value: Some("The Matrix".to_owned()),
        };
        let encoded = cursor.encode();
        let decoded = Cursor::decode(&encoded).unwrap();
        assert_eq!(decoded.id, 42);
        assert_eq!(decoded.sort_value.as_deref(), Some("The Matrix"));
    }

    #[test]
    fn cursor_decode_invalid() {
        assert!(Cursor::decode("not-valid-base64!!!").is_none());
        assert!(Cursor::decode("").is_none());
    }

    #[test]
    fn paginated_response_no_more() {
        let items: Vec<i64> = vec![1, 2, 3];
        let resp = PaginatedResponse::new(items, 5, |i| Cursor {
            id: *i,
            sort_value: None,
        });
        assert_eq!(resp.results.len(), 3);
        assert!(!resp.has_more);
        assert!(resp.next_cursor.is_none());
    }

    #[test]
    fn paginated_response_has_more() {
        let items: Vec<i64> = vec![1, 2, 3, 4, 5, 6];
        let resp = PaginatedResponse::new(items, 5, |i| Cursor {
            id: *i,
            sort_value: None,
        });
        assert_eq!(resp.results.len(), 5);
        assert!(resp.has_more);
        assert!(resp.next_cursor.is_some());
    }

    #[test]
    fn pagination_params_defaults() {
        let params = PaginationParams {
            limit: None,
            cursor: None,
            sort: None,
            order: None,
        };
        assert_eq!(params.limit(), 25);
        assert!(!params.order_is_desc());
    }

    #[test]
    fn pagination_params_clamp() {
        let params = PaginationParams {
            limit: Some(500),
            cursor: None,
            sort: None,
            order: Some("desc".to_owned()),
        };
        assert_eq!(params.limit(), 100);
        assert!(params.order_is_desc());
    }
}
