//! Tonie source bindings (#500): which playlist feeds a Kreativ-Tonie, at
//! which format and in which mode.
//!
//! Every query is scoped to `user_id`, so a binding is never readable or
//! writable across users. The store performs no network calls — a mutation is
//! a pure DB write, which keeps the tests CI-network-free.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::now_iso;

/// The formats a Tonie binding may use. `mp3-128` is the default.
pub const FORMAT_MP3_128: &str = "mp3-128";
/// Higher-quality MP3 target.
pub const FORMAT_MP3_320: &str = "mp3-320";

/// Manual mode: the user triggers a sync explicitly.
pub const MODE_MANUAL: &str = "manual";
/// Auto mode: the (future) sync run may refresh the Tonie on its own.
pub const MODE_AUTO: &str = "auto";

/// Whether `f` is a supported format (`mp3-128` | `mp3-320`).
pub fn valid_format(f: &str) -> bool {
    matches!(f, FORMAT_MP3_128 | FORMAT_MP3_320)
}

/// Whether `m` is a supported mode (`manual` | `auto`).
pub fn valid_mode(m: &str) -> bool {
    matches!(m, MODE_MANUAL | MODE_AUTO)
}

/// The source kinds a Tonie binding may use (#504). `playlist` is the default.
pub const SOURCE_PLAYLIST: &str = "playlist";
/// A whole backpack (all of its resolved tracks) feeds the Tonie.
pub const SOURCE_BACKPACK: &str = "backpack";
/// A tag (its resolved tracks) feeds the Tonie.
pub const SOURCE_TAG: &str = "tag";

/// Whether `k` is a supported source kind (`playlist` | `backpack` | `tag`).
pub fn valid_source_kind(k: &str) -> bool {
    matches!(k, SOURCE_PLAYLIST | SOURCE_BACKPACK | SOURCE_TAG)
}

/// German display label for a source kind; unknown kinds fall back to the raw
/// token so a stray DB value never renders as an empty label.
pub fn source_kind_label(kind: &str) -> &str {
    match kind {
        SOURCE_PLAYLIST => "Playlist",
        SOURCE_BACKPACK => "Backpack",
        SOURCE_TAG => "Tag",
        other => other,
    }
}

/// One `hub_tonie_bindings` row (user-scoped; never carries another user's id).
#[derive(Debug, Clone, Default)]
pub struct Binding {
    pub id: i64,
    pub tonie_id: String,
    pub tonie_name: Option<String>,
    pub source_kind: String,
    pub source_id: Option<i64>,
    pub format: String,
    pub cap_seconds: i64,
    pub mode: String,
    pub dirty_since: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_sync_status: Option<String>,
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
        dirty_since: r.get("dirty_since"),
        last_sync_at: r.get("last_sync_at"),
        last_sync_status: r.get("last_sync_status"),
    }
}

const SELECT_COLS: &str = "id, tonie_id, tonie_name, source_kind, source_id, format, \
     cap_seconds, mode, dirty_since, last_sync_at, last_sync_status";

/// All bindings owned by `user_id`, ordered by Tonie id.
pub async fn list_bindings(pool: &SqlitePool, user_id: i64) -> Result<Vec<Binding>> {
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM hub_tonie_bindings \
          WHERE user_id = ?1 ORDER BY tonie_id COLLATE NOCASE"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("Tonies-Bindings konnten nicht gelesen werden")?;
    Ok(rows.into_iter().map(row_to_binding).collect())
}

/// The binding for one Tonie of `user_id`, or `None` when absent.
pub async fn get_binding(
    pool: &SqlitePool,
    user_id: i64,
    tonie_id: &str,
) -> Result<Option<Binding>> {
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM hub_tonie_bindings \
          WHERE user_id = ?1 AND tonie_id = ?2"
    ))
    .bind(user_id)
    .bind(tonie_id)
    .fetch_optional(pool)
    .await
    .context("Tonies-Binding konnte nicht gelesen werden")?;
    Ok(row.map(row_to_binding))
}

/// Insert or update the binding of `user_id` for `tonie_id`. Re-sets
/// `updated_at` and `dirty_since` so a (future) sync run can spot the change.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_binding(
    pool: &SqlitePool,
    user_id: i64,
    tonie_id: &str,
    tonie_name: Option<&str>,
    source_kind: &str,
    source_id: Option<i64>,
    format: &str,
    mode: &str,
) -> Result<()> {
    let ts = now_iso();
    sqlx::query(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, tonie_name, source_kind, source_id, format, mode,
              dirty_since, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)
         ON CONFLICT(user_id, tonie_id) DO UPDATE SET
             tonie_name  = excluded.tonie_name,
             source_kind = excluded.source_kind,
             source_id   = excluded.source_id,
             format      = excluded.format,
             mode        = excluded.mode,
             dirty_since = excluded.dirty_since,
             updated_at  = excluded.updated_at",
    )
    .bind(user_id)
    .bind(tonie_id)
    .bind(tonie_name)
    .bind(source_kind)
    .bind(source_id)
    .bind(format)
    .bind(mode)
    .bind(&ts)
    .execute(pool)
    .await
    .context("Tonies-Binding konnte nicht gespeichert werden")?;
    Ok(())
}

/// Remove `user_id`'s binding for `tonie_id`. No-op when it does not exist.
pub async fn delete_binding(pool: &SqlitePool, user_id: i64, tonie_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM hub_tonie_bindings WHERE user_id = ?1 AND tonie_id = ?2")
        .bind(user_id)
        .bind(tonie_id)
        .execute(pool)
        .await
        .context("Tonies-Binding konnte nicht entfernt werden")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_validation() {
        assert!(valid_format("mp3-128"));
        assert!(valid_format("mp3-320"));
        assert!(!valid_format("flac"));
        assert!(!valid_format(""));
        assert!(!valid_format("MP3-128"));
    }

    #[test]
    fn mode_validation() {
        assert!(valid_mode("manual"));
        assert!(valid_mode("auto"));
        assert!(!valid_mode("always"));
        assert!(!valid_mode(""));
        assert!(!valid_mode("Auto"));
    }

    #[test]
    fn source_kind_validation() {
        assert!(valid_source_kind("playlist"));
        assert!(valid_source_kind("backpack"));
        assert!(valid_source_kind("tag"));
        assert!(!valid_source_kind("lens"));
        assert!(!valid_source_kind(""));
        assert!(!valid_source_kind("Playlist"));
    }

    #[test]
    fn source_kind_labels() {
        assert_eq!(source_kind_label(SOURCE_PLAYLIST), "Playlist");
        assert_eq!(source_kind_label(SOURCE_BACKPACK), "Backpack");
        assert_eq!(source_kind_label(SOURCE_TAG), "Tag");
        assert_eq!(source_kind_label("weird"), "weird");
    }
}
