//! Daily/weekly tagging task (#346).
//!
//! Surfaces the list of **uncomplete** tracks for a user, **most-played first**,
//! refreshed on a `daily` or `weekly` cadence. The task is persisted per
//! `(user, cadence, period_start)` so it stays stable within a period and is
//! only regenerated at the period boundary.
//!
//! Completion is defined by `crate::tagdone`: a track is complete when every
//! configured describing family has a resolved tag (`auto`) or the user marked
//! it done (`manual`). Only tracks that are neither appear in the task.

use chrono::{DateTime, Datelike, Duration, Utc};

/// Refresh cadence of a tagging task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Daily,
    Weekly,
}

impl Cadence {
    /// Parse the wire/db form (case-insensitive): `"daily"` / `"weekly"`.
    pub fn parse(s: &str) -> Option<Cadence> {
        match s.trim().to_ascii_lowercase().as_str() {
            "daily" => Some(Cadence::Daily),
            "weekly" => Some(Cadence::Weekly),
            _ => None,
        }
    }

    /// Canonical wire/db string.
    pub fn as_str(self) -> &'static str {
        match self {
            Cadence::Daily => "daily",
            Cadence::Weekly => "weekly",
        }
    }
}

/// The period start (`YYYY-MM-DD`) for `now` under `cadence`.
///
/// * `Daily`  — the UTC calendar day.
/// * `Weekly` — the Monday of the ISO week (Monday-based), so the period runs
///   Monday through Sunday.
pub fn period_start(cadence: Cadence, now: DateTime<Utc>) -> String {
    let date = now.date_naive();
    let start = match cadence {
        Cadence::Daily => date,
        Cadence::Weekly => {
            let days_from_monday = date.weekday().num_days_from_monday() as i64;
            date - Duration::days(days_from_monday)
        }
    };
    start.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, hh, mm, 0).unwrap()
    }

    #[test]
    fn cadence_parse_roundtrip() {
        for c in [Cadence::Daily, Cadence::Weekly] {
            assert_eq!(Cadence::parse(c.as_str()), Some(c));
        }
        assert_eq!(Cadence::parse("DAILY"), Some(Cadence::Daily));
        assert_eq!(Cadence::parse(" Weekly "), Some(Cadence::Weekly));
        assert_eq!(Cadence::parse("hourly"), None);
        assert_eq!(Cadence::parse(""), None);
    }

    #[test]
    fn daily_is_stable_within_a_day_and_rolls_at_midnight() {
        let a = period_start(Cadence::Daily, ts(2026, 5, 12, 0, 1));
        let b = period_start(Cadence::Daily, ts(2026, 5, 12, 23, 59));
        assert_eq!(a, "2026-05-12");
        assert_eq!(a, b, "daily period must be stable within a day");
        let c = period_start(Cadence::Daily, ts(2026, 5, 13, 0, 0));
        assert_eq!(c, "2026-05-13");
        assert_ne!(a, c, "daily must roll at the day boundary");
    }

    #[test]
    fn weekly_is_stable_monday_to_sunday() {
        // 2026-05-11 is a Monday, 2026-05-17 the following Sunday.
        let mon = period_start(Cadence::Weekly, ts(2026, 5, 11, 0, 0));
        let mid = period_start(Cadence::Weekly, ts(2026, 5, 14, 12, 0));
        let sun = period_start(Cadence::Weekly, ts(2026, 5, 17, 23, 59));
        assert_eq!(mon, "2026-05-11");
        assert_eq!(mon, mid, "weekly must be stable Mo–So");
        assert_eq!(mon, sun, "weekly must be stable Mo–So");
        let next_mon = period_start(Cadence::Weekly, ts(2026, 5, 18, 0, 0));
        assert_eq!(next_mon, "2026-05-18");
        assert_ne!(mon, next_mon, "weekly must roll on Monday");
    }

    #[test]
    fn weekly_handles_year_boundary() {
        // 2026-12-28 is a Monday; 2027-01-03 (Sunday) belongs to that week.
        let a = period_start(Cadence::Weekly, ts(2026, 12, 31, 10, 0));
        assert_eq!(a, "2026-12-28");
        let b = period_start(Cadence::Weekly, ts(2027, 1, 3, 10, 0));
        assert_eq!(a, b, "the week spans the year boundary");
        let c = period_start(Cadence::Weekly, ts(2027, 1, 4, 0, 0));
        assert_eq!(c, "2027-01-04");
    }
}
