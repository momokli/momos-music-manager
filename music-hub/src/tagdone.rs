//! Track "tagged-complete" flag (#345, T5-1 / F1).
//!
//! A track counts as fully tagged when **either** derived signal holds:
//!
//! * `auto`   — every configured describing tag-type *family* has at least one
//!   resolved tag on the track (`hub_track_resolved_tags`, all sources).
//!   Families whose type is toggled off via `engine_describes_*` (#348) are
//!   skipped so a disabled type never becomes a dead requirement.
//! * `manual` — the user explicitly marked it done (`hub_track_tag_done`).
//!
//! `complete = auto || manual`; a manual mark overrides a missing family.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::settings::{self, ENGINE_TAG_DONE_TYPES};
use crate::tagtypes::{DESCRIBE_FAMILIES, TagType};

/// The evaluated completion state for one (user, track) pair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagDoneStatus {
    /// `auto || manual`.
    pub complete: bool,
    /// All configured families are covered by resolved tags.
    pub auto: bool,
    /// The user manually marked the track done.
    pub manual: bool,
    /// Configured families without any resolved tag (canonical order).
    pub missing: Vec<String>,
}

/// The representative tag type for a describing *family* name (canonical,
/// uppercase). `PHASE` maps to `Phase1` (the family is shared by all phases).
/// `None` for unknown family names.
pub fn family_primary_type(family: &str) -> Option<TagType> {
    Some(match family.trim().to_ascii_uppercase().as_str() {
        "MOOD" => TagType::Mood,
        "GENRE" => TagType::Genre,
        "VIBE" => TagType::Vibe,
        "PHASE" => TagType::Phase1,
        "MERKMAL" => TagType::Merkmal,
        _ => return None,
    })
}

/// Read the configured describing families from settings. Falls back to the
/// built-in `DESCRIBE_FAMILIES` when the setting is unset or empty.
///
/// Since #348 a family whose representative type is toggled off via
/// `engine_describes_*` is dropped, so a disabled type never becomes a dead
/// "required" family. Unknown family tokens are kept as-is (no type to look up).
pub async fn configured_families(pool: &SqlitePool) -> Vec<String> {
    let raw = settings::get(pool, ENGINE_TAG_DONE_TYPES).await;
    let mut parsed: Vec<String> = Vec::new();
    for p in raw.unwrap_or_default().split(',') {
        let p = p.trim().to_ascii_uppercase();
        // Dedupe (`MOOD,MOOD`) while keeping first-seen order.
        if !p.is_empty() && !parsed.iter().any(|q| q == &p) {
            parsed.push(p);
        }
    }
    if parsed.is_empty() {
        parsed = DESCRIBE_FAMILIES.iter().map(|s| s.to_string()).collect();
    }
    // Keep only families whose representative type currently describes a track.
    let describes = crate::tagtypes::describes_map(pool).await;
    parsed.retain(|fam| {
        family_primary_type(fam)
            .map(|t| describes.get(&t).copied().unwrap_or_else(|| t.describes()))
            .unwrap_or(true)
    });
    parsed
}

/// The describing *family* for a raw tag-type string (canonical form; `PHASE_1..5`
/// all collapse to `"PHASE"`). `None` when the type is unknown or does not
/// describe a track (SETLIST / RUMPELKISTE).
pub fn family_of(type_name: &str) -> Option<String> {
    TagType::parse(type_name)
        .and_then(|t| t.describe_family())
        .map(str::to_string)
}

/// Is the track manually marked done for this user?
pub async fn is_done(pool: &SqlitePool, user_id: i64, track_id: i64) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM hub_track_tag_done WHERE user_id = ?1 AND track_id = ?2",
    )
    .bind(user_id)
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(0)
        > 0
}

/// Mark the track done for this user (idempotent upsert, `marked_at` = now).
pub async fn set_done(pool: &SqlitePool, user_id: i64, track_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_tag_done (user_id, track_id, marked_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, track_id) DO UPDATE SET marked_at = excluded.marked_at",
    )
    .bind(user_id)
    .bind(track_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove the manual "done" mark for this user (no-op when absent).
pub async fn clear_done(pool: &SqlitePool, user_id: i64, track_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_track_tag_done WHERE user_id = ?1 AND track_id = ?2")
        .bind(user_id)
        .bind(track_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The set of describing *families* covered by the track's resolved tags.
async fn covered_families(pool: &SqlitePool, track_id: i64) -> Vec<String> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT t.type, COUNT(*)
           FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
          WHERE rt.track_id = ?1 AND t.type IS NOT NULL
          GROUP BY t.type",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut families: Vec<String> = Vec::new();
    for (type_name, _cnt) in rows {
        if let Some(fam) = family_of(&type_name) {
            if !families.iter().any(|f| f == &fam) {
                families.push(fam);
            }
        }
    }
    families
}

/// Evaluate the completion state for `(user, track)` against `types`
/// (canonical describing family names). `auto` is true when every listed
/// family is covered by a resolved tag; `manual` when the user marked it done.
pub async fn status(
    pool: &SqlitePool,
    user_id: i64,
    track_id: i64,
    types: &[&str],
) -> TagDoneStatus {
    let covered = covered_families(pool, track_id).await;
    let missing: Vec<String> = types
        .iter()
        .map(|t| t.trim().to_ascii_uppercase())
        .filter(|t| !t.is_empty())
        .filter(|t| !covered.iter().any(|c| c == t))
        .collect();
    let auto = missing.is_empty();
    let manual = is_done(pool, user_id, track_id).await;
    TagDoneStatus {
        complete: auto || manual,
        auto,
        manual,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_primary_type_maps_canonical_families() {
        assert_eq!(family_primary_type("MOOD"), Some(TagType::Mood));
        assert_eq!(family_primary_type("genre"), Some(TagType::Genre));
        assert_eq!(family_primary_type("Vibe"), Some(TagType::Vibe));
        assert_eq!(family_primary_type("PHASE"), Some(TagType::Phase1));
        assert_eq!(family_primary_type("MERKMAL"), Some(TagType::Merkmal));
        assert_eq!(family_primary_type("SETLIST"), None);
        assert_eq!(family_primary_type("bogus"), None);
    }

    #[tokio::test]
    async fn configured_families_default_to_builtin() {
        let pool = crate::db::connect("sqlite::memory:").await.unwrap();
        let fams = configured_families(&pool).await;
        assert_eq!(
            fams,
            DESCRIBE_FAMILIES.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn configured_families_drop_disabled_types() {
        let pool = crate::db::connect("sqlite::memory:").await.unwrap();
        // Turn MOOD and setlist describing off; MOOD must drop from the list.
        settings::set(&pool, settings::ENGINE_DESCRIBES_MOOD, "0")
            .await
            .unwrap();
        let fams = configured_families(&pool).await;
        assert!(!fams.iter().any(|f| f == "MOOD"), "MOOD should be dropped");
        assert!(fams.iter().any(|f| f == "GENRE"));
        assert!(fams.iter().any(|f| f == "PHASE"));
        // Setlist is not a describe family; unrelated toggles leave it absent.
        assert!(!fams.iter().any(|f| f == "SETLIST"));
    }
}
