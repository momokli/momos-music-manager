//! App-wide settings as a simple KV store (SQLite table `settings`).
//!
//! The table is created by migration 024 and is deliberately generic: any
//! feature can store small string values here. The autoupdater uses the
//! `autoupdate.` key namespace (toggle persistence + last-check cache +
//! auto-apply interval + auto-apply crash-loop breaker, see
//! `autoupdate::update_auto`).
//!
//! Precedence for the *effective* autoupdate value is handled in
//! [`crate::api::update::effective_autoupdate_enabled`].

use sqlx::{Pool, Sqlite};

/// `settings`-KV keys used by the autoupdater (namespace `autoupdate.`).
pub const KEY_AUTOUPDATE_ENABLED: &str = "autoupdate.enabled";
/// Update channel (`"rolling"` | `"release"`) chosen in the UI.
pub const KEY_AUTOUPDATE_CHANNEL: &str = "autoupdate.channel";
/// Unix seconds (as INTEGER string) of the last completed check.
pub const KEY_AUTOUPDATE_LAST_CHECK_AT: &str = "autoupdate.last_check_at";
/// `"ok"` or `"error"` — outcome of the last check.
pub const KEY_AUTOUPDATE_LAST_CHECK_STATUS: &str = "autoupdate.last_check_status";
/// JSON of the last check result (see `api::update::LastCheckResult`).
pub const KEY_AUTOUPDATE_LAST_CHECK_RESULT: &str = "autoupdate.last_check_result";
/// Human-readable error of the last failed check (empty when absent).
pub const KEY_AUTOUPDATE_LAST_CHECK_ERROR: &str = "autoupdate.last_check_error";
/// Auto-apply interval in seconds (as INTEGER string) chosen in the UI —
/// precedence env > UI > TOML > default (see
/// `autoupdate::update_auto::effective_auto_apply_interval`). `0` disables
/// the periodic auto-apply loop (startup check still runs).
pub const KEY_AUTOUPDATE_INTERVAL_SECS: &str = "autoupdate.interval_secs";
/// JSON of the last auto-apply attempt (crash-loop breaker, see
/// `autoupdate::update_auto::AutoApplyState`).
pub const KEY_AUTOUPDATE_AUTO_APPLY_STATE: &str = "autoupdate.auto_apply_state";

/// Read a setting; `Ok(None)` when the key does not exist.
pub async fn get_setting(
    pool: &Pool<Sqlite>,
    key: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Write a setting (UPSERT — overwrites any existing value).
pub async fn set_setting(pool: &Pool<Sqlite>, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, unixepoch()) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = unixepoch()",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a setting (no-op when the key does not exist).
pub async fn delete_setting(pool: &Pool<Sqlite>, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Read a boolean setting. `Ok(None)` when the key does not exist; an
/// unparseable value is an error (never silently coerced).
pub async fn get_bool(pool: &Pool<Sqlite>, key: &str) -> Result<Option<bool>, sqlx::Error> {
    match get_setting(pool, key).await? {
        Some(value) => parse_bool_value(&value).map(Some),
        None => Ok(None),
    }
}

/// Write a boolean setting as `"true"` / `"false"`.
pub async fn set_bool(pool: &Pool<Sqlite>, key: &str, value: bool) -> Result<(), sqlx::Error> {
    set_setting(pool, key, if value { "true" } else { "false" }).await
}

/// Parse a stored boolean value (`"true"` / `"false"` only).
pub fn parse_bool_value(value: &str) -> Result<bool, sqlx::Error> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "invalid boolean setting value `{other}` (expected \"true\" or \"false\")"
            ),
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn set_get_roundtrip() {
        let pool = test_pool().await;
        set_setting(&pool, "autoupdate.enabled", "true").await.unwrap();
        assert_eq!(
            get_setting(&pool, "autoupdate.enabled").await.unwrap(),
            Some("true".to_string())
        );
    }

    #[tokio::test]
    async fn missing_key_returns_none() {
        let pool = test_pool().await;
        assert_eq!(
            get_setting(&pool, "does.not.exist").await.unwrap(),
            None
        );
        assert_eq!(get_bool(&pool, "does.not.exist").await.unwrap(), None);
    }

    #[tokio::test]
    async fn upsert_overwrites_previous_value() {
        let pool = test_pool().await;
        set_setting(&pool, "autoupdate.enabled", "true").await.unwrap();
        set_setting(&pool, "autoupdate.enabled", "false").await.unwrap();
        assert_eq!(
            get_setting(&pool, "autoupdate.enabled").await.unwrap(),
            Some("false".to_string())
        );
        // Still exactly one row.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn set_bool_roundtrip() {
        let pool = test_pool().await;
        set_bool(&pool, "autoupdate.enabled", true).await.unwrap();
        assert_eq!(get_bool(&pool, "autoupdate.enabled").await.unwrap(), Some(true));
        set_bool(&pool, "autoupdate.enabled", false).await.unwrap();
        assert_eq!(
            get_bool(&pool, "autoupdate.enabled").await.unwrap(),
            Some(false)
        );
    }

    #[test]
    fn parse_bool_accepts_true_and_false() {
        assert_eq!(parse_bool_value("true").unwrap(), true);
        assert_eq!(parse_bool_value("false").unwrap(), false);
    }

    #[test]
    fn parse_bool_rejects_garbage() {
        assert!(parse_bool_value("1").is_err());
        assert!(parse_bool_value("yes").is_err());
        assert!(parse_bool_value("").is_err());
        assert!(parse_bool_value("True").is_err());
    }

    #[tokio::test]
    async fn delete_removes_only_the_requested_key() {
        let pool = test_pool().await;
        set_setting(&pool, "autoupdate.enabled", "true").await.unwrap();
        set_setting(&pool, "autoupdate.channel", "rolling").await.unwrap();
        delete_setting(&pool, "autoupdate.channel").await.unwrap();
        assert_eq!(
            get_setting(&pool, "autoupdate.channel").await.unwrap(),
            None
        );
        // The other key stays untouched.
        assert_eq!(
            get_setting(&pool, "autoupdate.enabled").await.unwrap(),
            Some("true".to_string())
        );
    }

    #[tokio::test]
    async fn delete_missing_key_is_a_noop() {
        let pool = test_pool().await;
        delete_setting(&pool, "does.not.exist").await.unwrap();
    }
}
