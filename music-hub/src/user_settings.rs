//! Per-user Hub settings (issue #267).
//!
//! Per-user configuration lives in the additive `hub_user_settings` table
//! (migration `053`): a simple `(user_id, key, value, updated_at)` key/value
//! store. A user without a row reads as the registry default — no row is
//! materialised. Values are validated against the typed registry before they
//! are written (see [`crate::settings::USER_SETTINGS`]).
//!
//! Currently the only user-level setting is the engine [`TAG_SCOPE`]
//! (`own` | `collective`).

use std::collections::HashMap;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::settings;

/// Registry key of the per-user tag scope (`own` | `collective`).
pub const TAG_SCOPE: &str = settings::TAG_SCOPE;
/// Default tag scope when the user has no stored value.
pub const DEFAULT_TAG_SCOPE: &str = "own";

/// The stored value for `(user_id, key)`, or `None` when unset.
pub async fn get(pool: &SqlitePool, user_id: i64, key: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT value FROM hub_user_settings WHERE user_id = ?1 AND key = ?2",
    )
    .bind(user_id)
    .bind(key)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Upsert a value for `(user_id, key)`. Assumes an already-validated value.
pub async fn set(pool: &SqlitePool, user_id: i64, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_user_settings (user_id, key, value, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, key) DO UPDATE SET
             value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(key)
    .bind(value)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// All stored settings for a user as a `key -> value` map.
pub async fn all(pool: &SqlitePool, user_id: i64) -> HashMap<String, String> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT key, value FROM hub_user_settings WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// Validate + persist a raw value for `key`. Unknown key or invalid value
/// yields an error and writes nothing.
pub async fn set_validated(
    pool: &SqlitePool,
    user_id: i64,
    key: &str,
    raw: &str,
) -> Result<String, String> {
    let spec = settings::user_spec(key).ok_or_else(|| format!("unbekanntes Setting: {key}"))?;
    let value = settings::validate(spec, raw)?;
    set(pool, user_id, key, &value)
        .await
        .map_err(|e| format!("Speichern fehlgeschlagen: {e}"))?;
    Ok(value)
}
