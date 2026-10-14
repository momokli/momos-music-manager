//! Hard-wired tag **types** — the fixed classification layer that replaces the
//! former flexible group subsystem.
//!
//! A tag has exactly one type (nullable while migrating). Types are fixed in
//! code + a CHECK constraint on `hub_tags.type`; there is no user CRUD. Whether
//! a type *describes the song* is configurable per type via the
//! `engine_describes_*` settings (#348): `describes()` stays the built-in
//! default (MOOD / GENRE / VIBE / PHASE_1..5 / MERKMAL describe a track,
//! SETLIST / RUMPELKISTE only sort it), while `describes_map()` resolves the
//! live, admin-editable value with that default as fallback.
//!
//! A "phase" is encoded by PHASE_1..PHASE_5 (energy 1..5), replacing the old
//! ranked Phase group.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::settings;

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

    /// Built-in default for whether a tag of this type *describes* a track
    /// (vs. merely sorting it). Kept as the fallback for `describes_map()`
    /// when the corresponding `engine_describes_*` setting is unset.
    pub fn describes(self) -> bool {
        !matches!(self, TagType::Setlist | TagType::Rumpelkiste)
    }

    /// The `hub_settings` key holding this type's "describes" toggle (#348).
    /// `PHASE_1..5` share `engine_describes_phase` (analogous to the weight).
    pub fn describe_key(self) -> &'static str {
        match self {
            TagType::Mood => settings::ENGINE_DESCRIBES_MOOD,
            TagType::Genre => settings::ENGINE_DESCRIBES_GENRE,
            TagType::Vibe => settings::ENGINE_DESCRIBES_VIBE,
            TagType::Phase1
            | TagType::Phase2
            | TagType::Phase3
            | TagType::Phase4
            | TagType::Phase5 => settings::ENGINE_DESCRIBES_PHASE,
            TagType::Merkmal => settings::ENGINE_DESCRIBES_MERKMAL,
            TagType::Setlist => settings::ENGINE_DESCRIBES_SETLIST,
            TagType::Rumpelkiste => settings::ENGINE_DESCRIBES_RUMPELKISTE,
        }
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

/// The five canonical describing *families* (canonical order). `PHASE_1..5`
/// collapse into the single family `"PHASE"`.
pub const DESCRIBE_FAMILIES: &[&str] = &["MOOD", "GENRE", "VIBE", "PHASE", "MERKMAL"];

impl TagType {
    /// The describing *family* of this type, or `None` when the type does not
    /// describe a track (SETLIST / RUMPELKISTE). `PHASE_1..5` all map to
    /// `"PHASE"` so a single phase tag satisfies the whole phase family.
    pub fn describe_family(self) -> Option<&'static str> {
        Some(match self {
            TagType::Mood => "MOOD",
            TagType::Genre => "GENRE",
            TagType::Vibe => "VIBE",
            TagType::Phase1
            | TagType::Phase2
            | TagType::Phase3
            | TagType::Phase4
            | TagType::Phase5 => "PHASE",
            TagType::Merkmal => "MERKMAL",
            TagType::Setlist | TagType::Rumpelkiste => return None,
        })
    }
}

/// Parse a stored setting value as a boolean; unset/unknown -> `default`.
/// Accepts the same truthy spellings as the settings validator (`1/true/on/yes`).
fn parse_bool(raw: Option<&str>, default: bool) -> bool {
    match raw.map(|v| v.trim().to_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
        None => default,
    }
}

/// Resolve the current "describes" state for every tag type from
/// `hub_settings`, falling back to the built-in `TagType::describes()` when a
/// key is unset. Loaded once per request by callers (artists view, tagdone).
pub async fn describes_map(pool: &SqlitePool) -> HashMap<TagType, bool> {
    let all = settings::load_all(pool).await;
    TYPE_NAMES
        .iter()
        .filter_map(|n| TagType::parse(n))
        .map(|t| {
            let raw = all.get(t.describe_key()).map(String::as_str);
            (t, parse_bool(raw, t.describes()))
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_key_covers_all_types() {
        for name in TYPE_NAMES {
            let ty = TagType::parse(name).unwrap();
            let key = ty.describe_key();
            assert!(
                key.starts_with("engine_describes_"),
                "{name} key {key} not namespaced"
            );
            assert!(
                settings::spec(key).is_some(),
                "{name} key {key} not registered"
            );
        }
        // PHASE_1..5 share a single key.
        for p in [
            TagType::Phase1,
            TagType::Phase2,
            TagType::Phase3,
            TagType::Phase4,
            TagType::Phase5,
        ] {
            assert_eq!(p.describe_key(), settings::ENGINE_DESCRIBES_PHASE);
        }
    }

    #[test]
    fn describe_key_unique_per_semantic_type() {
        let mut keys: Vec<&str> = TYPE_NAMES
            .iter()
            .filter_map(|n| TagType::parse(n))
            .map(TagType::describe_key)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 7, "expected 7 distinct describe keys");
    }

    #[test]
    fn parse_bool_handles_truthy_and_fallback() {
        assert!(parse_bool(Some("1"), false));
        assert!(parse_bool(Some("TRUE"), false));
        assert!(parse_bool(Some(" on "), false));
        assert!(parse_bool(Some("yes"), false));
        assert!(!parse_bool(Some("0"), true));
        assert!(!parse_bool(Some("false"), true));
        assert!(!parse_bool(Some("nope"), true), "unknown -> false when parsed");
        assert!(parse_bool(None, true), "unset -> default");
        assert!(!parse_bool(None, false), "unset -> default");
    }

    #[test]
    fn describes_map_defaults_to_builtin() {
        // Unset path: parse_bool(None, d) == d, i.e. the built-in default.
        for name in TYPE_NAMES {
            let ty = TagType::parse(name).unwrap();
            assert!(!ty.describe_key().is_empty());
            assert_eq!(parse_bool(None, ty.describes()), ty.describes());
        }
        assert!(parse_bool(None, TagType::Mood.describes()));
        assert!(!parse_bool(None, TagType::Setlist.describes()));
        assert!(!parse_bool(None, TagType::Rumpelkiste.describes()));
    }

    #[tokio::test]
    async fn describes_map_reads_settings_with_builtin_fallback() {
        // Empty store -> every type equals its built-in default.
        let pool = crate::db::connect("sqlite::memory:").await.unwrap();
        let defaults = describes_map(&pool).await;
        assert_eq!(defaults.len(), TYPE_NAMES.len());
        assert_eq!(defaults[&TagType::Mood], true);
        assert_eq!(defaults[&TagType::Setlist], false);
        assert_eq!(defaults[&TagType::Rumpelkiste], false);
        for name in TYPE_NAMES {
            let ty = TagType::parse(name).unwrap();
            assert_eq!(defaults[&ty], ty.describes(), "{name} default mismatch");
        }

        // Toggle two types; the rest must stay on their built-in default.
        settings::set(&pool, settings::ENGINE_DESCRIBES_SETLIST, "1")
            .await
            .unwrap();
        settings::set(&pool, settings::ENGINE_DESCRIBES_MOOD, "0")
            .await
            .unwrap();
        let map = describes_map(&pool).await;
        assert_eq!(map[&TagType::Setlist], true);
        assert_eq!(map[&TagType::Mood], false);
        // PHASE_1..5 share one key.
        assert_eq!(map[&TagType::Phase3], true);
        assert_eq!(map[&TagType::Genre], true);
    }

    #[test]
    fn describe_family_covers_all_types() {
        for name in TYPE_NAMES {
            let ty = TagType::parse(name).unwrap();
            let fam = ty.describe_family();
            match ty {
                TagType::Setlist | TagType::Rumpelkiste => {
                    assert_eq!(fam, None, "{name} must not describe");
                }
                _ => {
                    let fam = fam.unwrap_or_else(|| panic!("{name} has no family"));
                    assert!(
                        DESCRIBE_FAMILIES.contains(&fam),
                        "{name} family {fam} not canonical"
                    );
                }
            }
        }
        // PHASE_1..5 all collapse to one family.
        for p in [
            TagType::Phase1,
            TagType::Phase2,
            TagType::Phase3,
            TagType::Phase4,
            TagType::Phase5,
        ] {
            assert_eq!(p.describe_family(), Some("PHASE"));
        }
        assert_eq!(TagType::Mood.describe_family(), Some("MOOD"));
        assert_eq!(TagType::Genre.describe_family(), Some("GENRE"));
        assert_eq!(TagType::Vibe.describe_family(), Some("VIBE"));
        assert_eq!(TagType::Merkmal.describe_family(), Some("MERKMAL"));
        assert!(!DESCRIBE_FAMILIES.contains(&"SETLIST"));
    }
}
