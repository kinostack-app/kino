//! `GET /api/v1/calendar.ics` — iCalendar feed for external calendar
//! subscribers. Content-type + the wrapping `BEGIN:VCALENDAR` /
//! `END:VCALENDAR` pair are part of the public contract.

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn calendar_ics_returns_text_calendar_with_ical_envelope() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/calendar.ics").await;
    assert_status(&resp, axum::http::StatusCode::OK);

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("text/calendar"),
        "must be text/calendar; got {content_type}"
    );

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body_bytes).expect("utf-8 ICS body");
    assert!(
        body.starts_with("BEGIN:VCALENDAR"),
        "ICS envelope opens; got {body:?}"
    );
    assert!(
        body.contains("END:VCALENDAR"),
        "ICS envelope closes; got {body:?}"
    );
}
