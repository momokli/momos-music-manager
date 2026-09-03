//! Phase C — configurable periodic auto-apply (scheduler config + state).
//!
//! Three pieces live here:
//!
//! 1. **Interval configuration** with the same precedence rule as the
//!    toggle/channel: **Env > UI (DB) > TOML > Default**. The default is
//!    [`DEFAULT_AUTO_APPLY_INTERVAL_SECS`] (4 h). `0` disables the periodic
//!    auto-apply loop — the one-shot startup check still runs.
//!
//!    - `MOMOS_AUTOUPDATE_INTERVAL_SECS` (env)
//!    - `settings['autoupdate.interval_secs']` (UI, KV store)
//!    - `[autoupdate] interval_secs` (config.toml)
//!
//! 2. **Auto-apply state** (crash-loop breaker, persisted in the settings
//!    KV as `autoupdate.auto_apply_state`). The state machine prevents an
//!    endless restart loop when an auto-installed version never becomes
//!    healthy:
//!
//!    - after a successful auto-apply the scheduler records the attempted
//!      version and the process restarts itself;
//!    - the swap-based health machinery resolves the restart: healthy →
//!      *commit* (startup event, state cleared = success); repeated
//!      unhealthy starts → *auto-rollback* (startup event, breaker engaged
//!      for that version);
//!    - with the breaker engaged ([`MAX_AUTO_APPLY_CONSECUTIVE_FAILURES`]
//!      consecutive restart failures of the same version) the scheduler
//!      refuses to re-apply that version until a *different* version is
//!      published — no infinite apply → restart → crash → rollback loop.
//!      (macOS DMG installs have no swap marker; the same breaker state is
//!      written by the scheduler and engaged on rollback-like signals.)
//!
//!    The scheduler additionally never stacks a new apply while a swap
//!    marker is pending (update in flight) and does not re-apply a version
//!    that is already installed but still waiting for its restart.
//!
//! 3. The scheduler loop itself lives in `serve()` (main.rs) and the
//!    orchestration (`api::update::run_auto_apply_cycle`), which reads the
//!    effective settings and drives `verify::check`/`verify::apply`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};

use crate::config::ServiceCredentials;
use crate::db::settings::{self, KEY_AUTOUPDATE_INTERVAL_SECS};

/// Default period between two automatic check+apply cycles (seconds).
///
/// Sane default for a rolling dev install: the app checks at most every 4 h
/// for a new build of `main` and applies + restarts it (the `latest-main`
/// release is pushed on every main push anyway). Release-channel builds pull
/// stable semver releases with the same period. Set to `0` to disable the
/// periodic loop (the one-shot startup check still informs about updates).
pub const DEFAULT_AUTO_APPLY_INTERVAL_SECS: u64 = 4 * 60 * 60;

/// Consecutive failed auto-apply attempts (same version, restart never
/// became healthy) after which the scheduler stops re-applying that version.
///
/// Mirrors `swap::MAX_UNHEALTHY_STARTS` (2 unhealthy starts before
/// rollback): the breaker engages on the rollback event, i.e. only *after*
/// the swap-based health machinery had its chance.
pub const MAX_AUTO_APPLY_CONSECUTIVE_FAILURES: u32 = 2;

/// Persisted auto-apply state — one JSON document under
/// `autoupdate.auto_apply_state`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoApplyState {
    /// Version the last auto-apply attempted to install.
    pub attempted_version: String,
    /// Unix seconds of that apply.
    pub attempted_at_unix: i64,
    /// Consecutive restart failures of `attempted_version`.
    pub failures: u32,
    /// Unix seconds of the last observed failure.
    pub last_failure_at_unix: Option<i64>,
}

/// Outcome of one auto-apply cycle (logged by the scheduler loop in
/// `serve()`; only [`AutoApplyOutcome::Installed`] triggers a self-restart).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoApplyOutcome {
    /// Autoupdate disabled (effective `enabled` = false) — cycle no-op.
    Disabled,
    /// No newer version on the selected channel.
    UpToDate,
    /// A newer version exists but was not applied — either the breaker is
    /// engaged for this exact version (repeated failed restarts) or the
    /// version is already installed and still waiting for its restart.
    UpdateAvailableSkipped { version: String },
    /// Version applied — the caller should restart the process now.
    Installed {
        new_version: String,
        old_version: String,
    },
    /// Only the verified artifact was saved (macOS DMG self-install fell
    /// back to a plain download) — manual installation required.
    DownloadedOnly { version: String, path: std::path::PathBuf },
    /// A previous update is still uncommitted (swap marker present) — do
    /// not apply another one on top.
    InFlight { new_version: String },
    /// The update source serves the *other* channel than selected.
    ChannelMismatch { channel: &'static str },
    /// Check or apply failed (network/verification) — transient, retried
    /// next cycle.
    Failed { message: String },
}

// ── Interval precedence (Env > UI > TOML > default) ──────────────────────

/// Effective auto-apply interval per the precedence rule
/// **Env > UI (DB) > TOML > Default [`DEFAULT_AUTO_APPLY_INTERVAL_SECS]**.
///
/// - parseable `MOMOS_AUTOUPDATE_INTERVAL_SECS` wins (`"env"`);
/// - otherwise a persisted UI value `settings['autoupdate.interval_secs']`
///   (`"ui"`; an unparseable stored value is an error, never silently
///   coerced);
/// - otherwise the config value — TOML `[autoupdate] interval_secs` or the
///   built-in default (`"toml"`/`"default"`). Unparseable env values fall
///   through, unchanged from `ServiceCredentials::load()`.
pub async fn effective_auto_apply_interval(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<(u64, &'static str), sqlx::Error> {
    effective_auto_apply_interval_with_env(
        config,
        db,
        std::env::var("MOMOS_AUTOUPDATE_INTERVAL_SECS").ok(),
    )
    .await
}

/// Testable core of [`effective_auto_apply_interval`] — the env value is
/// injected so the precedence matrix can be unit-tested without mutating the
/// process-global environment.
pub async fn effective_auto_apply_interval_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    env_value: Option<String>,
) -> Result<(u64, &'static str), sqlx::Error> {
    if let Some(v) = env_value.and_then(|v| v.parse::<u64>().ok()) {
        return Ok((v, "env"));
    }
    if let Some(raw) = settings::get_setting(db, KEY_AUTOUPDATE_INTERVAL_SECS).await? {
        let interval = raw.parse::<u64>().map_err(|_| {
            sqlx::Error::Decode(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "invalid stored auto-apply interval `{raw}` (expected an integer number of seconds, 0 disables the periodic loop)"
                ),
            )))
        })?;
        return Ok((interval, "ui"));
    }
    Ok((
        config.autoupdate_interval_secs,
        config.autoupdate_interval_source(),
    ))
}

// ── Auto-apply state (crash-loop breaker) ─────────────────────────────────

/// Read the persisted auto-apply state; `Ok(None)` when absent.
pub async fn read_auto_apply_state(
    db: &Pool<Sqlite>,
) -> Result<Option<AutoApplyState>, sqlx::Error> {
    match settings::get_setting(db, settings::KEY_AUTOUPDATE_AUTO_APPLY_STATE).await? {
        Some(text) => match serde_json::from_str(&text) {
            Ok(state) => Ok(Some(state)),
            Err(e) => Err(sqlx::Error::Decode(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid stored auto-apply state: {e}"),
            )))),
        },
        None => Ok(None),
    }
}

/// Persist (overwrite) the auto-apply state.
pub async fn write_auto_apply_state(
    db: &Pool<Sqlite>,
    state: &AutoApplyState,
) -> Result<(), sqlx::Error> {
    let text = serde_json::to_string(state).expect("auto-apply state serializes");
    settings::set_setting(db, settings::KEY_AUTOUPDATE_AUTO_APPLY_STATE, &text).await
}

/// Remove the persisted auto-apply state (update stuck / success observed).
pub async fn clear_auto_apply_state(db: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    settings::delete_setting(db, settings::KEY_AUTOUPDATE_AUTO_APPLY_STATE).await
}

/// Build the state for a fresh apply attempt.
pub fn state_for_attempt(attempted_version: &str) -> AutoApplyState {
    AutoApplyState {
        attempted_version: attempted_version.to_string(),
        attempted_at_unix: now_unix(),
        failures: 0,
        last_failure_at_unix: None,
    }
}

/// Record that the startup auto-rollback restored the previous binary —
/// the attempted version definitively failed its health check. Engages the
/// breaker immediately (failures = [`MAX_AUTO_APPLY_CONSECUTIVE_FAILURES`]).
pub fn record_rollback(
    state: Option<AutoApplyState>,
    rolled_back_version: &str,
) -> AutoApplyState {
    let mut state = state.filter(|s| s.attempted_version == rolled_back_version)
        .unwrap_or_else(|| state_for_attempt(rolled_back_version));
    state.failures = MAX_AUTO_APPLY_CONSECUTIVE_FAILURES;
    state.last_failure_at_unix = Some(now_unix());
    state
}

/// Persist a rollback event (startup auto-rollback): read-modify-write of
/// [`record_rollback`].
pub async fn note_rollback(
    db: &Pool<Sqlite>,
    rolled_back_version: &str,
) -> Result<(), sqlx::Error> {
    let state = read_auto_apply_state(db).await?;
    write_auto_apply_state(db, &record_rollback(state, rolled_back_version)).await
}

/// Why an available version is not applied by the scheduler right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Not skipped — apply it.
    None,
    /// The version is already installed and waiting for its restart
    /// (previous apply recorded, no failure so far, process still runs the
    /// old version).
    WaitingForActivation,
    /// The breaker is engaged: the same version failed to become healthy
    /// [`MAX_AUTO_APPLY_CONSECUTIVE_FAILURES`] times after auto-apply.
    BreakerEngaged,
}

/// Decide whether `available_version` may be applied, based on the state of
/// the last auto-apply attempt. A *different* version is never blocked.
pub fn skip_reason(
    state: Option<&AutoApplyState>,
    available_version: &str,
) -> SkipReason {
    match state {
        Some(s) if s.attempted_version == available_version => {
            if s.failures >= MAX_AUTO_APPLY_CONSECUTIVE_FAILURES {
                SkipReason::BreakerEngaged
            } else {
                SkipReason::WaitingForActivation
            }
        }
        _ => SkipReason::None,
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceCredentials;
    use sqlx::SqlitePool;

    async fn test_pool() -> Pool<Sqlite> {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn config_with(interval: u64, toml: Option<u64>) -> ServiceCredentials {
        let mut c = ServiceCredentials::defaults_for_test();
        c.autoupdate_interval_secs = interval;
        c.autoupdate_interval_toml = toml;
        c
    }

    // ── Interval precedence: env × ui × toml × default ─────────────────

    #[tokio::test]
    async fn interval_default_without_anything() {
        let pool = test_pool().await;
        // interval_secs already = built-in default in defaults_for_test.
        let config = config_with(DEFAULT_AUTO_APPLY_INTERVAL_SECS, None);
        assert_eq!(
            effective_auto_apply_interval_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (DEFAULT_AUTO_APPLY_INTERVAL_SECS, "default")
        );
    }

    #[tokio::test]
    async fn interval_toml_wins_over_default() {
        let pool = test_pool().await;
        let config = config_with(7200, Some(7200));
        assert_eq!(
            effective_auto_apply_interval_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (7200, "toml")
        );
    }

    #[tokio::test]
    async fn interval_ui_wins_over_toml() {
        let pool = test_pool().await;
        settings::set_setting(&pool, KEY_AUTOUPDATE_INTERVAL_SECS, "86400")
            .await
            .unwrap();
        let config = config_with(7200, Some(7200));
        assert_eq!(
            effective_auto_apply_interval_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (86400, "ui")
        );
    }

    #[tokio::test]
    async fn interval_env_wins_over_everything() {
        let pool = test_pool().await;
        settings::set_setting(&pool, KEY_AUTOUPDATE_INTERVAL_SECS, "86400")
            .await
            .unwrap();
        let config = config_with(7200, Some(7200));
        assert_eq!(
            effective_auto_apply_interval_with_env(&config, &pool, Some("3600".into()))
                .await
                .unwrap(),
            (3600, "env")
        );
    }

    #[tokio::test]
    async fn interval_zero_disables_periodic_loop() {
        let pool = test_pool().await;
        let config = config_with(0, None);
        assert_eq!(
            effective_auto_apply_interval_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (0, "default")
        );
    }

    #[tokio::test]
    async fn interval_unparseable_env_falls_through_to_default() {
        let pool = test_pool().await;
        let config = config_with(DEFAULT_AUTO_APPLY_INTERVAL_SECS, None);
        assert_eq!(
            effective_auto_apply_interval_with_env(&config, &pool, Some("soon".into()))
                .await
                .unwrap(),
            (DEFAULT_AUTO_APPLY_INTERVAL_SECS, "default")
        );
    }

    #[tokio::test]
    async fn interval_invalid_ui_value_is_an_error() {
        let pool = test_pool().await;
        settings::set_setting(&pool, KEY_AUTOUPDATE_INTERVAL_SECS, "often")
            .await
            .unwrap();
        let config = config_with(DEFAULT_AUTO_APPLY_INTERVAL_SECS, None);
        assert!(effective_auto_apply_interval_with_env(&config, &pool, None)
            .await
            .is_err());
    }

    // ── Breaker state machine ──────────────────────────────────────────

    fn state(version: &str, failures: u32) -> AutoApplyState {
        AutoApplyState {
            attempted_version: version.to_string(),
            attempted_at_unix: 1_000,
            failures,
            last_failure_at_unix: Some(2_000),
        }
    }

    #[test]
    fn state_for_attempt_starts_clean() {
        let s = state_for_attempt("2.0.0");
        assert_eq!(s.attempted_version, "2.0.0");
        assert_eq!(s.failures, 0);
        assert!(s.last_failure_at_unix.is_none());
    }

    #[test]
    fn record_rollback_engages_breaker_for_that_version() {
        let s = record_rollback(Some(state("2.0.0", 1)), "2.0.0");
        assert_eq!(s.attempted_version, "2.0.0");
        assert_eq!(s.failures, MAX_AUTO_APPLY_CONSECUTIVE_FAILURES);
        assert!(s.last_failure_at_unix.is_some());
    }

    #[test]
    fn record_rollback_without_state_starts_fresh() {
        let s = record_rollback(None, "2.0.0");
        assert_eq!(s.attempted_version, "2.0.0");
        assert_eq!(s.failures, MAX_AUTO_APPLY_CONSECUTIVE_FAILURES);
    }

    #[test]
    fn record_rollback_of_other_version_replaces_state() {
        let s = record_rollback(Some(state("2.0.0", 0)), "2.1.0");
        assert_eq!(s.attempted_version, "2.1.0");
        assert_eq!(s.failures, MAX_AUTO_APPLY_CONSECUTIVE_FAILURES);
    }

    #[test]
    fn skip_reason_none_without_state_or_for_new_version() {
        assert_eq!(skip_reason(None, "2.0.0"), SkipReason::None);
        let s = state("2.0.0", MAX_AUTO_APPLY_CONSECUTIVE_FAILURES);
        // A different (newer) version is never blocked.
        assert_eq!(skip_reason(Some(&s), "2.1.0"), SkipReason::None);
    }

    #[test]
    fn skip_reason_waiting_for_activation_before_failures() {
        let s = state("2.0.0", 0);
        assert_eq!(
            skip_reason(Some(&s), "2.0.0"),
            SkipReason::WaitingForActivation
        );
    }

    #[test]
    fn skip_reason_breaker_after_max_failures() {
        let s = state("2.0.0", MAX_AUTO_APPLY_CONSECUTIVE_FAILURES);
        assert_eq!(
            skip_reason(Some(&s), "2.0.0"),
            SkipReason::BreakerEngaged
        );
        let s1 = state("2.0.0", MAX_AUTO_APPLY_CONSECUTIVE_FAILURES - 1);
        assert_eq!(
            skip_reason(Some(&s1), "2.0.0"),
            SkipReason::WaitingForActivation
        );
    }

    #[tokio::test]
    async fn state_roundtrip_and_clear() {
        let pool = test_pool().await;
        assert!(read_auto_apply_state(&pool).await.unwrap().is_none());
        let s = state("2.0.0", 1);
        write_auto_apply_state(&pool, &s).await.unwrap();
        assert_eq!(read_auto_apply_state(&pool).await.unwrap(), Some(s.clone()));
        clear_auto_apply_state(&pool).await.unwrap();
        assert!(read_auto_apply_state(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn note_rollback_persists_engaged_breaker() {
        let pool = test_pool().await;
        note_rollback(&pool, "2.0.0").await.unwrap();
        let s = read_auto_apply_state(&pool).await.unwrap().unwrap();
        assert_eq!(s.attempted_version, "2.0.0");
        assert_eq!(s.failures, MAX_AUTO_APPLY_CONSECUTIVE_FAILURES);
    }
}
