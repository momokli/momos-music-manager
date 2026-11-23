/// Hard-wired tag type definitions used as the initial seed and the migration
/// from the old per‑tag group model to the new `hub_tag_types` table.
///
/// The types are frozen (no CRUD) and carried through `TYPE_NAMES` in `tagtypes.rs`.
/// Each type carries semantic metadata:
///   * key          – canonical DB string (matches `TYPE_NAMES`).
///   * label        – German display label for UI elements.
///   * abbrev       – single‑letter export marker (collapse `PHASE_1..5` into `"P"`).
///   * icon         – emoji used in hub tag UIs (see `tagtaskui.rs`).
///   * sort_order   – stable numeric sort key (lower = earlier in exports).
///   * describes    – whether tags of this type *describe* a track (vs. only sorting it).
///   * weight       – scoring weight (see `scoring.rs`).
///
/// These are loaded once per `hub_users.user_id` by `TypeSet::load()`.
pub const DEFAULT_TYPES: &[(&str, &str, &str, &str, i64, bool, f64)] = &[
    ("MOOD", "Mood", "M", "💜", 1, true, 1.0),
    ("GENRE", "Genre", "G", "🎼", 2, true, 0.9),
    ("VIBE", "Vibe", "V", "🌈", 3, true, 1.0),
    ("PHASE_1", "Phase 1", "P", "⚡1", 4, true, 0.7),
    ("PHASE_2", "Phase 2", "P", "⚡2", 5, true, 0.7),
    ("PHASE_3", "Phase 3", "P", "⚡3", 6, true, 0.7),
    ("PHASE_4", "Phase 4", "P", "⚡4", 7, true, 0.7),
    ("PHASE_5", "Phase 5", "P", "⚡5", 8, true, 0.7),
    ("MERKMAL", "Merkmal", "E", "#️⃣", 9, true, 0.8),
    ("SETLIST", "Setlist", "S", "📋", 10, false, 0.2),
    ("RUMPELKISTE", "Rumpelkiste", "R", "🧺", 11, false, 0.1),
];

use super::super::settings;
use anyhow::Result;
use sqlx::SqlitePool;

/// Insert the hard‑wired types into `hub_tag_types` for a given owner user.
/// This is a one‑shot backfill (used on signup, re‑seed or manual reset) that
/// inserts only types that are not already present for the owner.
pub async fn seed_types(pool: &SqlitePool, owner_user_id: i64) -> Result<()> {
    // Phase 1: collect the canonical key+owner combos we are about to insert.
    let keys: Vec<&str> = DEFAULT_TYPES.iter().map(|(k, _, _, _, _, _, _)| *k).collect();

    // Phase 2: run UPSERT per type; do‑nothing when a matching row already exists.
    for &(key, label, abbrev, icon, sort_order, describes, weight) in DEFAULT_TYPES.iter()
    {
        sqlx::query(
            "INSERT INTO hub_tag_types (owner_user_id, key, label, abbrev, icon, sort_order, describes, weight)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(owner_user_id, key) DO NOTHING",
        )
        .bind(owner_user_id)
        .bind(key)
        .bind(label)
        .bind(abbrev)
        .bind(icon)
        .bind(sort_order)
        .bind(describes)
        .bind(weight)
        .execute(pool)
        .await?;
    }
    Ok(())
}