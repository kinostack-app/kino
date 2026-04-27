//! Pure helper deciding whether the scheduled backup task is due.
//!
//! Two inputs from `config`:
//!
//! - `backup_schedule` — `daily` | `weekly` | `monthly` | `off` |
//!   `cron:<expr>` (cron form is reserved for Phase 2; the parser
//!   below treats unknown prefixes as `off`).
//! - `backup_time` — `HH:MM` in the host's local time. The
//!   scheduler ticks every minute; we fire when the wall clock is
//!   in the right hour:minute window for the right day.
//!
//! No state stored beyond the most-recent scheduled `backup` row's
//! `created_at` — we use that to debounce so the same minute
//! doesn't fire twice.

use chrono::{DateTime, Duration, NaiveTime, Timelike, Utc};
use sqlx::SqlitePool;

/// Configured cadence preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Off,
    Daily,
    Weekly,
    Monthly,
}

impl Frequency {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "daily" => Self::Daily,
            "weekly" => Self::Weekly,
            "monthly" => Self::Monthly,
            _ => Self::Off,
        }
    }
}

/// Decide whether a scheduled backup should fire *now*. Reads
/// config + the most recent scheduled backup row to debounce.
pub async fn is_due(pool: &SqlitePool, now: DateTime<Utc>) -> sqlx::Result<bool> {
    let (schedule, time, last_scheduled): (String, String, Option<String>) = sqlx::query_as(
        "SELECT backup_schedule, backup_time,
                (SELECT created_at FROM backup
                 WHERE kind = 'scheduled' ORDER BY created_at DESC LIMIT 1)
         FROM config WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;

    let frequency = Frequency::parse(&schedule);
    if matches!(frequency, Frequency::Off) {
        return Ok(false);
    }
    let Some(target_time) = NaiveTime::parse_from_str(time.trim(), "%H:%M").ok() else {
        return Ok(false);
    };

    if !at_or_past_time_today(now, target_time) {
        return Ok(false);
    }
    if !meets_frequency_window(now, last_scheduled.as_deref(), frequency) {
        return Ok(false);
    }
    Ok(true)
}

fn at_or_past_time_today(now: DateTime<Utc>, target: NaiveTime) -> bool {
    let now_minutes = now.hour() * 60 + now.minute();
    let target_minutes = target.hour() * 60 + target.minute();
    // Fire any time within a 60-minute window after the target so a
    // missed tick (host suspended overnight, etc.) still backs up
    // within the same day.
    now_minutes >= target_minutes && now_minutes - target_minutes < 60
}

fn meets_frequency_window(
    now: DateTime<Utc>,
    last_scheduled: Option<&str>,
    frequency: Frequency,
) -> bool {
    let Some(last) = last_scheduled.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) else {
        return true;
    };
    let last_utc = last.with_timezone(&Utc);
    let elapsed = now.signed_duration_since(last_utc);
    match frequency {
        Frequency::Off => false,
        // Allow the next firing 23h after the previous (gives 1h
        // slack so the daily check at 03:00 the next day still
        // triggers even if the previous landed at 03:30).
        Frequency::Daily => elapsed >= Duration::hours(23),
        Frequency::Weekly => elapsed >= Duration::days(6),
        Frequency::Monthly => elapsed >= Duration::days(28),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn frequency_parses_known_presets_and_falls_back_to_off() {
        assert_eq!(Frequency::parse("daily"), Frequency::Daily);
        assert_eq!(Frequency::parse("WEEKLY"), Frequency::Weekly);
        assert_eq!(Frequency::parse(" Monthly "), Frequency::Monthly);
        assert_eq!(Frequency::parse("off"), Frequency::Off);
        assert_eq!(Frequency::parse("cron:0 3 * * *"), Frequency::Off);
        assert_eq!(Frequency::parse(""), Frequency::Off);
    }

    #[test]
    fn time_window_fires_within_60min_of_target_only() {
        let target = NaiveTime::from_hms_opt(3, 0, 0).unwrap();
        // Right at the target minute.
        assert!(at_or_past_time_today(ts("2026-04-26T03:00:00Z"), target));
        // 30 minutes past — still in window.
        assert!(at_or_past_time_today(ts("2026-04-26T03:30:00Z"), target));
        // 59 minutes — last in-window minute.
        assert!(at_or_past_time_today(ts("2026-04-26T03:59:00Z"), target));
        // Just before target — too early.
        assert!(!at_or_past_time_today(ts("2026-04-26T02:59:00Z"), target));
        // 60 minutes past — out of window.
        assert!(!at_or_past_time_today(ts("2026-04-26T04:00:00Z"), target));
    }

    #[test]
    fn frequency_window_debounces_same_day() {
        let now = ts("2026-04-26T03:00:00Z");
        // No previous run → fires.
        assert!(meets_frequency_window(now, None, Frequency::Daily));
        // 22h ago → too soon for daily.
        let recent = (now - Duration::hours(22)).to_rfc3339();
        assert!(!meets_frequency_window(
            now,
            Some(&recent),
            Frequency::Daily
        ));
        // 23h ago → just passes.
        let elapsed = (now - Duration::hours(23)).to_rfc3339();
        assert!(meets_frequency_window(
            now,
            Some(&elapsed),
            Frequency::Daily
        ));
    }

    #[test]
    fn weekly_and_monthly_windows() {
        let now = ts("2026-04-26T03:00:00Z");
        let five_days = (now - Duration::days(5)).to_rfc3339();
        assert!(!meets_frequency_window(
            now,
            Some(&five_days),
            Frequency::Weekly
        ));
        let six_days = (now - Duration::days(6)).to_rfc3339();
        assert!(meets_frequency_window(
            now,
            Some(&six_days),
            Frequency::Weekly
        ));

        let twenty_seven = (now - Duration::days(27)).to_rfc3339();
        assert!(!meets_frequency_window(
            now,
            Some(&twenty_seven),
            Frequency::Monthly
        ));
        let twenty_eight = (now - Duration::days(28)).to_rfc3339();
        assert!(meets_frequency_window(
            now,
            Some(&twenty_eight),
            Frequency::Monthly
        ));
    }
}
