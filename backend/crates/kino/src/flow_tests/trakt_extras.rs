//! Additional Trakt read endpoints — recommendations + trending.
//! These read from the local sync cache (zero network calls), so
//! they're always safe to call. Empty cache → `{ items: [] }`.

use crate::test_support::{TestAppBuilder, json_body};

#[tokio::test]
async fn recommendations_empty_when_disconnected() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/integrations/trakt/recommendations").await).await;
    assert!(
        body.get("items").is_some(),
        "shape is {{ items: [] }}; got {body}"
    );
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "no cached recommendations on disconnected install"
    );
}

#[tokio::test]
async fn trending_empty_when_disconnected() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/integrations/trakt/trending").await).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "no cached trending on disconnected install; got {body}"
    );
}
