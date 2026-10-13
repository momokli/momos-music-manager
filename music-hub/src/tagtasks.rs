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

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Utc};
use sqlx::SqlitePool;

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

/// One candidate/line in a tagging task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTaskItem {
    pub track_id: i64,
    /// 1-based position in the task (`plays DESC, track_id ASC`).
    pub rank: i64,
    /// Per-user plays: Traktor `play_count` + listening-history rows.
    pub plays: i64,
    /// Missing describing families in canonical (`families`) order.
    pub missing: Vec<String>,
}

/// Compute the ordered, uncomplete-track candidates for `user_id`.
///
/// Candidate set = every track, minus those that already cover *all* of
/// `families` (auto-complete) and minus those the user manually marked done.
/// Ordering is `plays DESC, track_id ASC`; `plays` accumulates the user's
/// Traktor `play_count` and their `hub_listening_history` rows. Tracks with no
/// play data are kept at `plays = 0`. At most `limit` items are returned.
pub async fn candidates(
    pool: &SqlitePool,
    user_id: i64,
    families: &[String],
    limit: i64,
) -> Result<Vec<TagTaskItem>> {
    // Batch query: per-user plays + the canonical covering families per track,
    // excluding manually-done tracks. Rows are pre-ordered most-played first.
    let rows = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "WITH user_plays AS (
             SELECT t.id AS track_id,
                    COALESCE(tr.play_count, 0) + COALESCE(h.cnt, 0) AS plays
               FROM hub_tracks t
               LEFT JOIN hub_traktor_tracks tr
                      ON tr.track_id = t.id AND tr.user_id = ?1
               LEFT JOIN (
                     SELECT track_id, COUNT(*) AS cnt
                       FROM hub_listening_history
                      WHERE user_id = ?1
                      GROUP BY track_id
                    ) h ON h.track_id = t.id
         ),
         covered AS (
             SELECT rt.track_id AS track_id,
                    CASE WHEN tg.type LIKE 'PHASE_%' THEN 'PHASE' ELSE tg.type END AS family
               FROM hub_track_resolved_tags rt
               JOIN hub_tags tg ON tg.id = rt.tag_id
              WHERE tg.type IS NOT NULL
              GROUP BY rt.track_id, family
         )
         SELECT p.track_id, p.plays, GROUP_CONCAT(c.family)
           FROM user_plays p
           LEFT JOIN covered c ON c.track_id = p.track_id
          WHERE NOT EXISTS (
                    SELECT 1 FROM hub_track_tag_done d
                     WHERE d.user_id = ?1 AND d.track_id = p.track_id)
          GROUP BY p.track_id, p.plays
          ORDER BY p.plays DESC, p.track_id ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut items: Vec<TagTaskItem> = Vec::new();
    for (track_id, plays, covered_csv) in rows {
        let covered: Vec<String> = covered_csv
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .collect();
        let missing: Vec<String> = families
            .iter()
            .map(|f| f.trim().to_ascii_uppercase())
            .filter(|f| !f.is_empty() && !covered.iter().any(|c| c == f))
            .collect();
        // Auto-complete tracks are not part of the task.
        if missing.is_empty() {
            continue;
        }
        items.push(TagTaskItem {
            track_id,
            rank: items.len() as i64 + 1,
            plays,
            missing,
        });
        if limit > 0 && items.len() as i64 >= limit {
            break;
        }
    }
    Ok(items)
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
