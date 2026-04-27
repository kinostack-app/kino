//! `Timestamp` — kino's canonical wall-clock instant.
//!
//! Wraps `chrono::DateTime<Utc>` so every timestamp shares one
//! representation across the codebase: RFC3339 on the wire, TEXT in
//! `SQLite`, typed arithmetic in code. SQL comparisons go through
//! `datetime(?)` on both sides — see [`SQL_TIMESTAMP_COMPARE_GUIDE`].
//!
//! Library boundaries that natively speak `chrono::DateTime<Utc>`
//! (chrono APIs, third-party crates) keep their native types;
//! convert at the boundary.

use std::fmt;

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::Sqlite;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};

/// A UTC wall-clock instant. See module docs for the rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Current wall-clock instant.
    #[must_use]
    pub fn now() -> Self {
        Self(Utc::now())
    }

    /// `now()` minus a duration. Common pattern for cutoff comparisons:
    /// "rows older than 72 hours" → `Timestamp::now_minus(Duration::hours(72))`.
    #[must_use]
    pub fn now_minus(delta: Duration) -> Self {
        Self(Utc::now() - delta)
    }

    /// `now()` plus a duration. Common for expiry computation.
    #[must_use]
    pub fn now_plus(delta: Duration) -> Self {
        Self(Utc::now() + delta)
    }

    /// Construct from an existing `chrono::DateTime<Utc>` (boundary
    /// conversion from a chrono-returning API).
    #[must_use]
    pub const fn from_datetime(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }

    /// Try to parse from any of the formats kino historically writes:
    /// canonical RFC3339, RFC3339 without subseconds, or `SQLite`'s
    /// native `YYYY-MM-DD HH:MM:SS` shape (no T, no zone).
    ///
    /// Returns `None` for unparseable input.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(Self(dt.with_timezone(&Utc)));
        }
        // SQLite's `datetime('now')` produces "2026-04-25 12:34:56".
        // Treat as UTC since every kino write site uses UTC clocks.
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Self(naive.and_utc()));
        }
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
            return Some(Self(naive.and_utc()));
        }
        None
    }

    /// Underlying `chrono::DateTime<Utc>` for interop.
    #[must_use]
    pub const fn as_datetime(self) -> DateTime<Utc> {
        self.0
    }

    /// Canonical RFC3339 rendering. The single serialisation format
    /// kino emits for timestamps.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        self.0.to_rfc3339()
    }

    /// Duration since `other`. Negative if `self` is earlier.
    #[must_use]
    pub fn duration_since(self, other: Self) -> Duration {
        self.0 - other.0
    }

    /// Time elapsed since `self` per the system clock. Common
    /// pattern: `created_at.elapsed() > Duration::hours(24)`.
    #[must_use]
    pub fn elapsed(self) -> Duration {
        Utc::now() - self.0
    }

    /// `self + delta`. Useful for computing expiry from a base.
    #[must_use]
    pub fn plus(self, delta: Duration) -> Self {
        Self(self.0 + delta)
    }

    /// `self - delta`.
    #[must_use]
    pub fn minus(self, delta: Duration) -> Self {
        Self(self.0 - delta)
    }
}

impl fmt::Display for Timestamp {
    /// Display always renders RFC3339 — matches the wire format so
    /// log lines and JSON look identical.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.to_rfc3339())
    }
}

impl From<DateTime<Utc>> for Timestamp {
    fn from(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }
}

impl From<Timestamp> for DateTime<Utc> {
    fn from(t: Timestamp) -> Self {
        t.0
    }
}

// ── serde ──────────────────────────────────────────────────────────

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    /// Accepts canonical RFC3339, RFC3339 without subseconds, and
    /// SQLite-native `YYYY-MM-DD HH:MM:SS`. Mirrors [`Timestamp::parse`]
    /// so JSON and SQL row payloads use the same tolerant
    /// dispatch.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid timestamp: {s:?}")))
    }
}

// ── sqlx ───────────────────────────────────────────────────────────

impl sqlx::Type<Sqlite> for Timestamp {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for Timestamp {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        // Always emit canonical RFC3339. Existing rows that hold the
        // SQLite-native shape stay readable via Decode below.
        let s = self.0.to_rfc3339();
        <String as sqlx::Encode<'q, Sqlite>>::encode(s, buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Timestamp {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s: String = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Self::parse(&s).ok_or_else(|| format!("invalid timestamp from sqlx: {s:?}").into())
    }
}

// ── SQL comparison convention ──────────────────────────────────────

/// SQL fragment helper. Use in queries that compare timestamps so
/// `SQLite` parses both sides as actual datetimes rather than as
/// raw text.
///
/// The discipline:
///
/// ```text
/// // WRONG — text compare. Two RFC3339 strings sort by lexical
/// // order; subseconds + timezone variations break equality.
/// "SELECT * FROM x WHERE last_ran_at < ?"
///
/// // RIGHT — both sides parsed as datetimes by SQLite.
/// "SELECT * FROM x WHERE datetime(last_ran_at) < datetime(?)"
/// ```
///
/// This is enforced at code-review time. The CI grep gate flags
/// any timestamp comparison that doesn't wrap both sides.
///
/// This constant exists so PRs can reference one place when the
/// convention is questioned.
pub const SQL_TIMESTAMP_COMPARE_GUIDE: &str =
    "compare timestamps via `datetime(column) <op> datetime(?)`; see crate::time module docs";

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        Timestamp::parse(s).expect("parseable fixture")
    }

    #[test]
    fn parse_canonical_rfc3339() {
        let t = Timestamp::parse("2026-04-25T12:34:56Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-04-25T12:34:56+00:00");
    }

    #[test]
    fn parse_rfc3339_with_subseconds() {
        let t = Timestamp::parse("2026-04-25T12:34:56.789Z").unwrap();
        assert_eq!(t.as_datetime().timestamp_millis(), 1_777_120_496_789);
    }

    #[test]
    fn parse_sqlite_native_format() {
        // SQLite's `datetime('now')` produces this shape.
        let t = Timestamp::parse("2026-04-25 12:34:56").unwrap();
        assert_eq!(t.as_datetime().to_rfc3339(), "2026-04-25T12:34:56+00:00");
    }

    #[test]
    fn parse_sqlite_native_format_with_subseconds() {
        let t = Timestamp::parse("2026-04-25 12:34:56.789").unwrap();
        assert_eq!(t.as_datetime().timestamp_millis(), 1_777_120_496_789);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Timestamp::parse("not a timestamp").is_none());
        assert!(Timestamp::parse("").is_none());
        assert!(Timestamp::parse("2026").is_none());
    }

    #[test]
    fn ordering_matches_chrono() {
        let early = at("2026-04-25T12:00:00Z");
        let late = at("2026-04-25T13:00:00Z");
        assert!(early < late);
        assert!(late > early);
        assert_eq!(early, early);
    }

    #[test]
    fn duration_since_is_signed() {
        let early = at("2026-04-25T12:00:00Z");
        let late = at("2026-04-25T13:00:00Z");
        assert_eq!(late.duration_since(early), Duration::hours(1));
        assert_eq!(early.duration_since(late), Duration::hours(-1));
    }

    #[test]
    fn arithmetic_round_trips() {
        let base = at("2026-04-25T12:00:00Z");
        let later = base.plus(Duration::minutes(30));
        let earlier = base.minus(Duration::minutes(30));
        assert_eq!(later.duration_since(base), Duration::minutes(30));
        assert_eq!(base.duration_since(earlier), Duration::minutes(30));
    }

    #[test]
    fn now_minus_yields_past() {
        let cutoff = Timestamp::now_minus(Duration::hours(1));
        assert!(cutoff < Timestamp::now());
    }

    #[test]
    fn serde_roundtrips_rfc3339() {
        let t = at("2026-04-25T12:34:56Z");
        let json = serde_json::to_string(&t).unwrap();
        // JSON should always emit RFC3339 form, regardless of input format.
        assert!(json.contains("2026-04-25T12:34:56"));
        let back: Timestamp = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn serde_accepts_sqlite_native_input() {
        // History rows that came from SQLite-native writes must still
        // deserialize cleanly via the API layer.
        let json = "\"2026-04-25 12:34:56\"";
        let t: Timestamp = serde_json::from_str(json).unwrap();
        assert_eq!(t.to_rfc3339(), "2026-04-25T12:34:56+00:00");
    }

    #[test]
    fn serde_rejects_invalid() {
        let result: Result<Timestamp, _> = serde_json::from_str("\"not a timestamp\"");
        assert!(result.is_err());
    }

    #[test]
    fn display_matches_wire_format() {
        let t = at("2026-04-25T12:34:56Z");
        assert_eq!(t.to_string(), t.to_rfc3339());
    }

    #[test]
    fn from_chrono_round_trips() {
        let dt = chrono::Utc::now();
        let t: Timestamp = dt.into();
        let back: DateTime<Utc> = t.into();
        assert_eq!(dt, back);
    }
}
