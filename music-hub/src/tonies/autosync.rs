//! Tonies Auto-Sync (#503): the debounce + run driver for `mode='auto'`
//! bindings.
//!
//! The worker marks an auto binding dirty whenever its source playlist's
//! snapshot changes (see `worker::playlist_jobs`). This module decides **when**
//! such a dirty binding may run: only after a quiet period (no further change)
//! and respecting a minimum spacing between two runs of the same binding.
//!
//! A run is the exact same exec as a manual sync ([`execute_binding`]) — Tonies
//! + music-api only. **This module never issues a Spotify request** (acceptance
//! T4); all its inputs come from the DB and the two non-Spotify mocks.
//!
//! Debounce knobs are read from the settings registry (`settings.rs`), never
//! hardcoded:
//! * `tonie_autosync_enabled`         (flag, default `1`)
//! * `tonie_autosync_quiet_secs`      (num,  default `300`)
//! * `tonie_autosync_min_interval_secs` (num, default `600`)

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::Binding;
use super::run::{Claim, claim_run, execute_binding, finish_run};
use crate::config::Config;
use crate::settings;

/// Upper bound on bindings considered in one pass (LIMIT-bounded query).
pub const DUE_LIMIT: i64 = 20;

/// The debounce window: how long a dirty binding must stay unchanged before it
/// may sync, and the minimum spacing between two runs of the same binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Debounce {
    pub quiet_secs: i64,
    pub min_interval_secs: i64,
}

impl Debounce {
    /// Read the knobs from the settings registry. Returns `None` when the
    /// master switch `tonie_autosync_enabled` is off, so callers can short-
    /// circuit without touching the DB further.
    pub async fn from_settings(pool: &SqlitePool) -> Option<Debounce> {
        if !flag_setting(pool, settings::TONIE_AUTOSYNC_ENABLED, true).await {
            return None;
        }
        let quiet_secs = num_setting(pool, settings::TONIE_AUTOSYNC_QUIET_SECS, 300.0)
            .await
            .max(0.0) as i64;
        let min_interval_secs = num_setting(pool, settings::TONIE_AUTOSYNC_MIN_INTERVAL_SECS, 600.0)
            .await
            .max(0.0) as i64;
        Some(Debounce {
            quiet_secs,
            min_interval_secs,
        })
    }
}

async fn num_setting(pool: &SqlitePool, key: &str, default: f64) -> f64 {
    settings::get(pool, key)
        .await
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

async fn flag_setting(pool: &SqlitePool, key: &str, default: bool) -> bool {
    match settings::get(pool, key)
        .await
        .map(|v| v.trim().to_lowercase())
    {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
        None => default,
    }
}

fn parse_ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Pure decision: may a dirty binding sync right now?
///
/// True iff a dirty mark is present, it is **older than** the quiet window
/// (`dirty_since <= now - quiet_secs`), and either no run happened yet or the
/// last run is at least `min_interval_secs` in the past. A missing/
/// unparseable timestamp never counts as due.
pub fn is_due(
    dirty_since: Option<&str>,
    last_sync_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    d: &Debounce,
) -> bool {
    let Some(dirty) = dirty_since.and_then(parse_ts) else {
        return false;
    };
    if dirty > now - chrono::Duration::seconds(d.quiet_secs.max(0)) {
        return false;
    }
    match last_sync_at.and_then(parse_ts) {
        Some(last) => last <= now - chrono::Duration::seconds(d.min_interval_secs.max(0)),
        None => true,
    }
}

fn row_to_binding(r: sqlx::sqlite::SqliteRow) -> Binding {
    Binding {
        id: r.get("id"),
        tonie_id: r.get("tonie_id"),
        tonie_name: r.get("tonie_name"),
        source_kind: r.get("source_kind"),
        source_id: r.get("source_id"),
        format: r.get("format"),
        cap_seconds: r.get("cap_seconds"),
        mode: r.get("mode"),
        order_by: r.get("order_by"),
        album_mode: r.get("album_mode"),
        dirty_since: r.get("dirty_since"),
        last_sync_at: r.get("last_sync_at"),
        last_sync_status: r.get("last_sync_status"),
    }
}

/// Auto bindings that are due at `now`: dirty, past the quiet window, and past
/// the minimum spacing. The debounce lives in the query, not in the caller's
/// sleep. LIMIT-bounded (oldest dirty first).
pub async fn due_bindings(
    pool: &SqlitePool,
    now: chrono::DateTime<chrono::Utc>,
    d: &Debounce,
) -> Result<Vec<Binding>> {
    let quiet_cutoff = (now - chrono::Duration::seconds(d.quiet_secs.max(0))).to_rfc3339();
    let interval_cutoff = (now - chrono::Duration::seconds(d.min_interval_secs.max(0))).to_rfc3339();
    let rows = sqlx::query(
        "SELECT id, tonie_id, tonie_name, source_kind, source_id, format,
                cap_seconds, mode, order_by, album_mode, dirty_since, last_sync_at, last_sync_status
           FROM hub_tonie_bindings
          WHERE mode = 'auto'
            AND dirty_since IS NOT NULL
            AND dirty_since <= ?1
            AND (last_sync_at IS NULL OR last_sync_at <= ?2)
          ORDER BY dirty_since ASC
          LIMIT ?3",
    )
    .bind(&quiet_cutoff)
    .bind(&interval_cutoff)
    .bind(DUE_LIMIT)
    .fetch_all(pool)
    .await
    .context("faellige Tonie-Bindings konnten nicht gelesen werden")?;
    Ok(rows.into_iter().map(row_to_binding).collect())
}

/// Run every binding that is due right now, up to `limit`.
///
/// For each binding: `claim_run` (skip `Busy`/`RateLimited`) → `execute_binding`
/// → `finish_run`. Returns the number of runs actually started. Reads no
/// Spotify data and issues no Spotify request.
pub async fn run_due(pool: &SqlitePool, cfg: &Config, limit: i64) -> Result<usize> {
    let Some(d) = Debounce::from_settings(pool).await else {
        return Ok(0);
    };
    let now = chrono::Utc::now();
    let mut bindings = due_bindings(pool, now, &d).await?;
    if limit >= 0 {
        bindings.truncate(limit as usize);
    }

    let mut ran = 0usize;
    for binding in bindings {
        let uid: i64 = sqlx::query_scalar("SELECT user_id FROM hub_tonie_bindings WHERE id = ?1")
            .bind(binding.id)
            .fetch_one(pool)
            .await
            .context("Tonie-Binding-Besitzer konnte nicht gelesen werden")?;

        let run_id = match claim_run(pool, binding.id).await? {
            Claim::Started(id) => id,
            // Another run is live / too recent — leave the dirty mark for later.
            Claim::Busy | Claim::RateLimited => continue,
        };
        let outcome = execute_binding(pool, cfg, uid, &binding).await;
        finish_run(pool, run_id, &outcome).await?;
        ran += 1;
    }
    Ok(ran)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(quiet: i64, min_interval: i64) -> Debounce {
        Debounce {
            quiet_secs: quiet,
            min_interval_secs: min_interval,
        }
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    #[test]
    fn is_due_requires_a_dirty_mark() {
        let now = now();
        assert!(!is_due(None, None, now, &d(300, 600)));
    }

    #[test]
    fn is_due_respects_the_quiet_window() {
        let now = now();
        let inside = (now - chrono::Duration::seconds(10)).to_rfc3339();
        let outside = (now - chrono::Duration::seconds(301)).to_rfc3339();
        assert!(!is_due(Some(&inside), None, now, &d(300, 600)));
        assert!(is_due(Some(&outside), None, now, &d(300, 600)));
    }

    #[test]
    fn is_due_respects_min_interval() {
        let now = now();
        let dirty = (now - chrono::Duration::seconds(400)).to_rfc3339();
        let recent = (now - chrono::Duration::seconds(100)).to_rfc3339();
        let old = (now - chrono::Duration::seconds(601)).to_rfc3339();
        assert!(!is_due(Some(&dirty), Some(&recent), now, &d(300, 600)));
        assert!(is_due(Some(&dirty), Some(&old), now, &d(300, 600)));
    }

    #[test]
    fn unparseable_timestamps_are_not_due() {
        let now = now();
        assert!(!is_due(Some("not-a-timestamp"), None, now, &d(300, 600)));
    }
}
