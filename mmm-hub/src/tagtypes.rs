//! Hard-wired tag **types** — the fixed classification layer that replaces the
//! former flexible group subsystem.
//!
//! A tag has exactly one type (nullable while migrating). Types are fixed in
//! code + a CHECK constraint on `hub_tags.type`; there is no user CRUD. The
//! "describes the song" distinction is inherent: MOOD / GENRE / VIBE /
//! PHASE_1..5 / MERKMAL describe a track, SETLIST / RUMPELKISTE only sort it.
//!
//! A "phase" is encoded by PHASE_1..PHASE_5 (energy 1..5), replacing the old
//! ranked Phase group.

use anyhow::Result;
use sqlx::SqlitePool;

/// All valid type strings, in canonical (display) order.
pub const TYPE_NAMES: &[&str] = &[
    "MOOD",
    "GENRE",
    "VIBE",
    "PHASE_1",
    "PHASE_2",
    "PHASE_3",
    "PHASE_4",
    "PHASE_5",
    "MERKMAL",
    "SETLIST",
    "RUMPELKISTE",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagType {
    Mood,
    Genre,
    Vibe,
    Phase1,
    Phase2,
    Phase3,
    Phase4,
    Phase5,
    Merkmal,
    Setlist,
    Rumpelkiste,
}

impl TagType {
    /// Canonical DB/string form.
    pub fn as_str(self) -> &'static str {
        match self {
            TagType::Mood => "MOOD",
            TagType::Genre => "GENRE",
            TagType::Vibe => "VIBE",
            TagType::Phase1 => "PHASE_1",
            TagType::Phase2 => "PHASE_2",
            TagType::Phase3 => "PHASE_3",
            TagType::Phase4 => "PHASE_4",
            TagType::Phase5 => "PHASE_5",
            TagType::Merkmal => "MERKMAL",
            TagType::Setlist => "SETLIST",
            TagType::Rumpelkiste => "RUMPELKISTE",
        }
    }

    /// Parse a DB/string value (case-insensitive). `None` for empty/unknown.
    pub fn parse(s: &str) -> Option<TagType> {
        let up = s.trim().to_ascii_uppercase();
        Some(match up.as_str() {
            "MOOD" => TagType::Mood,
            "GENRE" => TagType::Genre,
            "VIBE" => TagType::Vibe,
            "PHASE_1" => TagType::Phase1,
            "PHASE_2" => TagType::Phase2,
            "PHASE_3" => TagType::Phase3,
            "PHASE_4" => TagType::Phase4,
            "PHASE_5" => TagType::Phase5,
            "MERKMAL" => TagType::Merkmal,
            "SETLIST" => TagType::Setlist,
            "RUMPELKISTE" => TagType::Rumpelkiste,
            _ => return None,
        })
    }

    /// German display label (used in tables/forms).
    pub fn label(self) -> &'static str {
        match self {
            TagType::Mood => "Mood",
            TagType::Genre => "Genre",
            TagType::Vibe => "Vibe",
            TagType::Phase1 => "Phase 1",
            TagType::Phase2 => "Phase 2",
            TagType::Phase3 => "Phase 3",
            TagType::Phase4 => "Phase 4",
            TagType::Phase5 => "Phase 5",
            TagType::Merkmal => "Merkmal",
            TagType::Setlist => "Setlist",
            TagType::Rumpelkiste => "Rumpelkiste",
        }
    }

    /// Emoji marker (no FontAwesome dependency), matching the hub's chip style.
    pub fn icon(self) -> &'static str {
        match self {
            TagType::Mood => "💜",
            TagType::Genre => "🎼",
            TagType::Vibe => "🌈",
            TagType::Phase1 => "⚡1",
            TagType::Phase2 => "⚡2",
            TagType::Phase3 => "⚡3",
            TagType::Phase4 => "⚡4",
            TagType::Phase5 => "⚡5",
            TagType::Merkmal => "#️⃣",
            TagType::Setlist => "📋",
            TagType::Rumpelkiste => "🧺",
        }
    }

    /// True when a tag of this type *describes* a track (vs. merely sorting it).
    pub fn describes(self) -> bool {
        !matches!(self, TagType::Setlist | TagType::Rumpelkiste)
    }

    /// Engine weight: how much a match via a tag of this type counts.
    pub fn weight(self) -> f64 {
        match self {
            TagType::Mood => 1.0,
            TagType::Vibe => 1.0,
            TagType::Genre => 0.9,
            TagType::Merkmal => 0.8,
            TagType::Phase1
            | TagType::Phase2
            | TagType::Phase3
            | TagType::Phase4
            | TagType::Phase5 => 0.7,
            TagType::Setlist => 0.2,
            TagType::Rumpelkiste => 0.1,
        }
    }

    /// Energy rank for PHASE_n types (1..5), else `None`.
    pub fn phase_num(self) -> Option<i64> {
        match self {
            TagType::Phase1 => Some(1),
            TagType::Phase2 => Some(2),
            TagType::Phase3 => Some(3),
            TagType::Phase4 => Some(4),
            TagType::Phase5 => Some(5),
            _ => None,
        }
    }
}

/// All types as a display-ready list of `(value, label, icon)`.
pub fn options() -> Vec<(&'static str, &'static str, &'static str)> {
    TYPE_NAMES
        .iter()
        .filter_map(|n| TagType::parse(n))
        .map(|t| (t.as_str(), t.label(), t.icon()))
        .collect()
}

/// A tag's type from the DB (or `None` when untyped).
pub async fn get(pool: &SqlitePool, tag_id: i64) -> Option<TagType> {
    sqlx::query_scalar::<_, Option<String>>("SELECT type FROM hub_tags WHERE id = ?1")
        .bind(tag_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .and_then(|s| TagType::parse(&s))
}

/// Set (or clear, with `None`) a tag's type. Only the tag's owner may change it.
pub async fn set(
    pool: &SqlitePool,
    actor_user_id: i64,
    tag_id: i64,
    ty: Option<TagType>,
) -> Result<()> {
    let rows = sqlx::query("UPDATE hub_tags SET type = ?1 WHERE id = ?2 AND owner_user_id = ?3")
        .bind(ty.map(|t| t.as_str()))
        .bind(tag_id)
        .bind(actor_user_id)
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        anyhow::bail!("tag not found or not yours");
    }
    Ok(())
}
