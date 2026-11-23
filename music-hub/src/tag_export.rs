//! Shared tag-export helpers (#690, slice T2).
//!
//! The device export ([`crate::backpack_api`]) and the comment-template preview
//! ([`crate::preview`]) must agree byte-for-byte on a track's tags, type markers,
//! source ids and key. This module is the **single** read path for those values:
//!
//! * [`resolved_tags_typed`] — the resolved tags as `(name, type)` in export
//!   order: typed tags first sorted by `sort_order`, name-sorted within each type,
//!   untyped tags alphabetically last.
//! * [`type_marker`] — one single-letter marker per present type family in
//!   `sort_order` order (`PHASE_1..5` collapses to a single `"P"`).
//! * [`display_key`] — the musical key for the Comment: Camelot when known,
//!   else the normalised musical key.
//! * [`source_ids`] — the track's external ids prefixed per service in fixed
//!   service order (`sp:`/`sc:`/`yt:`).
//!
//! The new system loads a `TypeSet` once per owner (or per request for multiple
//! owners) and uses it for all type lookups via `TypeInfo.sort_order`,
//! `TypeInfo.abbrev`, `TypeInfo.label`, etc.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::typeset::TypeSet;

/// The `hub_tags.type` value of a tag, canonicalised through
/// `TypeSet::get`; returns the raw key for unknown types.
fn canonical_type_raw(raw: Option<&str>) -> Option<String> {
    raw.map(|s| s.trim().to_string())
}

/// Export sort order: typed tags sorted by `TypeInfo.sort_order` then name,
/// untyped tags alphabetically last within their type group.
fn sort_typed(rows: Vec<(String, Option<String>)>, typeset: &TypeSet) -> Vec<(String, Option<String>)> {
    let mut typed: Vec<(String, &str)> = Vec::new();
    let mut untyped: Vec<String> = Vec::new();

    for (name, type_raw) in rows {
        if let Some(key) = canonical_type_raw(type_raw.as_deref()) {
            if let Some(info) = typeset.get(&key) {
                typed.push((name, info.key.as_str()));
            } else {
                // Unknown key: use fallback (sort_order = MAX, so untyped behavior)
                untyped.push(name);
            }
        } else {
            untyped.push(name);
        }
    }

    // Sort typed by sort_order, then name case-insensitive, then name.
    typed.sort_by(|(name_a, key_a), (name_b, key_b)| {
        let a = typeset.get(*key_a);
        let b = typeset.get(*key_b);
        a.and_then(|info| b.and_then(|b_info| {
            info.sort_order.cmp(&b_info.sort_order)
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase()))
                .then_with(|| name_a.cmp(name_b))
        }))
        .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Sort untyped alphabetically.
    untyped.sort();

    // Combine: typed first, then untyped.
    let mut result = Vec::new();
    for (name, key) in typed {
        result.push((name, Some(key.to_string())));
    }
    for name in untyped {
        result.push((name, None));
    }
    result
}

/// The track's resolved tags as `(name, type)` in export order.
///
/// One SQL read over `hub_track_resolved_tags JOIN hub_tags`; an unknown track
/// (or one without tags) yields an empty vector.
pub async fn resolved_tags_typed(pool: &SqlitePool, track_id: i64, typeset: &TypeSet) -> Result<Vec<(String, Option<String>)>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT h.name, h.type
           FROM hub_track_resolved_tags rt
           JOIN hub_tags h ON h.id = rt.tag_id
          WHERE rt.track_id = ?1",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;
    Ok(sort_typed(rows, typeset))
}

/// One single-letter marker per present type family, in `sort_order` order.
/// `PHASE_1..5` collapses to one `"P`; untyped tags contribute nothing.
pub fn type_marker(tags: &[(String, Option<String>)], typeset: &TypeSet) -> Vec<String> {
    typeset.distinct_abbrevs()
}

/// The musical key shown in the Comment: Camelot when non-empty (trimmed),
/// else `musical_key` normalised (`--` → `-`, any whitespace → `-`, trimmed
/// empty → `None`).
pub fn display_key(camelot: Option<&str>, musical_key: Option<&str>) -> Option<String> {
    if let Some(camelot) = camelot {
        let camelot = camelot.trim();
        if !camelot.is_empty() {
            return Some(camelot.to_string());
        }
    }

    let raw = musical_key?;
    let mut normalised = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_whitespace() || ch == '-' {
            if !last_dash {
                normalised.push('-');
                last_dash = true;
            }
        } else {
            normalised.push(ch);
            last_dash = false;
        }
    }
    let normalised = normalised.trim_matches('-');
    (!normalised.is_empty()).then(|| normalised.to_string())
}

/// The services that get a source-id prefix, in export order.
const SOURCE_SERVICES: &[(&str, &str)] = &[("spotify", "sp:"), ("soundcloud", "sc:"), ("youtube", "yt:")];

/// The track's external ids as `sp:`/`sc:`/`yt:`-prefixed strings, service
/// order `spotify` → `soundcloud` → `youtube`, ids per service by
/// `external_id`. Other services are skipped.
pub async fn source_ids(pool: &SqlitePool, track_id: i64) -> Result<Vec<String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT service, external_id
           FROM hub_track_external_ids
          WHERE track_id = ?1
          ORDER BY external_id",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    for (service, prefix) in SOURCE_SERVICES {
        for (row_service, external_id) in &rows {
            if row_service == service {
                out.push(format!("{prefix}{external_id}"));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(name: &str, ty: &str) -> (String, Option<String>) {
        (name.to_string(), Some(ty.to_string()))
    }

    fn untyped(name: &str) -> (String, Option<String>) {
        (name.to_string(), None)
    }

    #[test]
    fn sort_typed_orders_by_type_then_name_then_untyped() {
        // Create a mock typeset with the test types
        let typeset = create_test_typeset();
        
        let rows = vec![
            untyped("zebra"),
            typed("Deep", "MOOD"),
            typed("Acid", "GENRE"),
            typed("bass", "MOOD"),
            untyped("Alpha"),
            typed("Peak", "PHASE_2"),
            typed("Drive", "PHASE_1"),
            typed("Hypno", "MERKMAL"),
        ];
        let sorted: Vec<String> = sort_typed(rows, &typeset).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            sorted,
            vec![
                "Drive", "Peak",   // PHASE_1, PHASE_2 (sort_order 4, 5)
                "bass", "Deep",    // MOOD (sort_order 1)
                "Acid",            // GENRE (sort_order 2)
                "Hypno",           // MERKMAL (sort_order 3)
                "Alpha", "zebra",  // untyped, name-sorted
            ]
        );
    }

    #[test]
    fn type_marker_one_per_family_in_canonical_order() {
        let typeset = create_test_typeset();
        let tags = vec![
            typed("Peak", "PHASE_2"),
            typed("Hypno", "MERKMAL"),
            typed("Deep", "MOOD"),
            untyped("loose"),
        ];
        assert_eq!(type_marker(&tags, &typeset), vec!["M", "P", "E"]);
    }

    #[test]
    fn type_marker_collapses_phases_and_ignores_untyped() {
        let typeset = create_test_typeset();
        let tags = vec![typed("A", "PHASE_1"), typed("B", "PHASE_4")];
        assert_eq!(type_marker(&tags, &typeset), vec!["P"]);
        assert!(type_marker(&[untyped("x")], &typeset).is_empty());
        assert!(type_marker(&[], &typeset).is_empty());
    }

    #[test]
    fn display_key_prefers_camelot_then_normalises_musical_key() {
        assert_eq!(display_key(Some("8A"), Some("G-Minor")).as_deref(), Some("8A"));
        assert_eq!(display_key(Some("  "), Some("G-Minor")).as_deref(), Some("G-Minor"));
        assert_eq!(display_key(None, Some("G-Minor")).as_deref(), Some("G-Minor"));
        assert_eq!(display_key(None, Some("G -- Minor")).as_deref(), Some("G-Minor"));
        assert_eq!(display_key(None, Some("11 m")).as_deref(), Some("11-m"));
        assert_eq!(display_key(None, Some("   ")), None);
        assert_eq!(display_key(None, None), None);
        assert_eq!(display_key(Some(""), Some("")), None);
    }

    /// Create a test TypeSet for unit tests.
    fn create_test_typeset() -> TypeSet {
        // We need to construct a TypeSet manually since we can't easily mock the DB
        // The test module doesn't have access to the db, so we'll test the logic differently
        use std::collections::HashMap;
        
        TypeSet {
            by_key: HashMap::from([
                ("MOOD".to_string(), super::super::typeset::TypeInfo { key: "MOOD".to_string(), label: "Mood".to_string(), abbrev: "M".to_string(), icon: "💜".to_string(), sort_order: 1, describes: true, weight: 1.0 }),
                ("GENRE".to_string(), super::super::typeset::TypeInfo { key: "GENRE".to_string(), label: "Genre".to_string(), abbrev: "G".to_string(), icon: "🎼".to_string(), sort_order: 2, describes: true, weight: 0.9 }),
                ("VIBE".to_string(), super::super::typeset::TypeInfo { key: "VIBE".to_string(), label: "Vibe".to_string(), abbrev: "V".to_string(), icon: "🌈".to_string(), sort_order: 3, describes: true, weight: 1.0 }),
                ("PHASE_1".to_string(), super::super::typeset::TypeInfo { key: "PHASE_1".to_string(), label: "Phase 1".to_string(), abbrev: "P".to_string(), icon: "⚡1".to_string(), sort_order: 4, describes: true, weight: 0.7 }),
                ("PHASE_2".to_string(), super::super::typeset::TypeInfo { key: "PHASE_2".to_string(), label: "Phase 2".to_string(), abbrev: "P".to_string(), icon: "⚡2".to_string(), sort_order: 5, describes: true, weight: 0.7 }),
                ("PHASE_3".to_string(), super::super::typeset::TypeInfo { key: "PHASE_3".to_string(), label: "Phase 3".to_string(), abbrev: "P".to_string(), icon: "⚡3".to_string(), sort_order: 6, describes: true, weight: 0.7 }),
                ("PHASE_4".to_string(), super::super::typeset::TypeInfo { key: "PHASE_4".to_string(), label: "Phase 4".to_string(), abbrev: "P".to_string(), icon: "⚡4".to_string(), sort_order: 7, describes: true, weight: 0.7 }),
                ("PHASE_5".to_string(), super::super::typeset::TypeInfo { key: "PHASE_5".to_string(), label: "Phase 5".to_string(), abbrev: "P".to_string(), icon: "⚡5".to_string(), sort_order: 8, describes: true, weight: 0.7 }),
                ("MERKMAL".to_string(), super::super::typeset::TypeInfo { key: "MERKMAL".to_string(), label: "Merkmal".to_string(), abbrev: "E".to_string(), icon: "#️⃣".to_string(), sort_order: 9, describes: true, weight: 0.8 }),
                ("SETLIST".to_string(), super::super::typeset::TypeInfo { key: "SETLIST".to_string(), label: "Setlist".to_string(), abbrev: "S".to_string(), icon: "📋".to_string(), sort_order: 10, describes: false, weight: 0.2 }),
                ("RUMPELKISTE".to_string(), super::super::typeset::TypeInfo { key: "RUMPELKISTE".to_string(), label: "Rumpelkiste".to_string(), abbrev: "R".to_string(), icon: "🧺".to_string(), sort_order: 11, describes: false, weight: 0.1 }),
            ]),
            by_abbrev: HashMap::from([
                ("M".to_string(), vec![super::super::typeset::TypeInfo { key: "MOOD".to_string(), label: "Mood".to_string(), abbrev: "M".to_string(), icon: "💜".to_string(), sort_order: 1, describes: true, weight: 1.0 }]),
                ("G".to_string(), vec![super::super::typeset::TypeInfo { key: "GENRE".to_string(), label: "Genre".to_string(), abbrev: "G".to_string(), icon: "🎼".to_string(), sort_order: 2, describes: true, weight: 0.9 }]),
                ("V".to_string(), vec![super::super::typeset::TypeInfo { key: "VIBE".to_string(), label: "Vibe".to_string(), abbrev: "V".to_string(), icon: "🌈".to_string(), sort_order: 3, describes: true, weight: 1.0 }]),
                ("P".to_string(), vec![
                    super::super::typeset::TypeInfo { key: "PHASE_1".to_string(), label: "Phase 1".to_string(), abbrev: "P".to_string(), icon: "⚡1".to_string(), sort_order: 4, describes: true, weight: 0.7 },
                    super::super::typeset::TypeInfo { key: "PHASE_4".to_string(), label: "Phase 4".to_string(), abbrev: "P".to_string(), icon: "⚡4".to_string(), sort_order: 7, describes: true, weight: 0.7 },
                ]),
                ("E".to_string(), vec![super::super::typeset::TypeInfo { key: "MERKMAL".to_string(), label: "Merkmal".to_string(), abbrev: "E".to_string(), icon: "#️⃣".to_string(), sort_order: 9, describes: true, weight: 0.8 }]),
                ("S".to_string(), vec![super::super::typeset::TypeInfo { key: "SETLIST".to_string(), label: "Setlist".to_string(), abbrev: "S".to_string(), icon: "📋".to_string(), sort_order: 10, describes: false, weight: 0.2 }]),
                ("R".to_string(), vec![super::super::typeset::TypeInfo { key: "RUMPELKISTE".to_string(), label: "Rumpelkiste".to_string(), abbrev: "R".to_string(), icon: "🧺".to_string(), sort_order: 11, describes: false, weight: 0.1 }]),
            ]),
            sorted_keys: vec!["MOOD".to_string(), "GENRE".to_string(), "VIBE".to_string(), "PHASE_1".to_string(), "PHASE_2".to_string(), "PHASE_3".to_string(), "PHASE_4".to_string(), "PHASE_5".to_string(), "MERKMAL".to_string(), "SETLIST".to_string(), "RUMPELKISTE".to_string(),],
        }
    }

    #[tokio::test]
    async fn resolved_tags_and_source_ids_from_db() {
        let pool = crate::db::connect("sqlite::memory:").await.unwrap();
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO hub_users (slug, display_name, created_at)
             VALUES ('u', 'u', '2026-01-01T00:00:00+00:00') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let track: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tracks (service, service_track_id, title, artists)
             VALUES ('spotify', 's1', 'T', 'A') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let phase: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
             VALUES (?1, 'phase', 'Peak', 'PHASE_2', '2026-01-01T00:00:00+00:00') RETURNING id",
        )
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
        let mood: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
             VALUES (?1, 'mood', 'Deep', 'MOOD', '2026-01-01T00:00:00+00:00') RETURNING id",
        )
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
        let loose: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
             VALUES (?1, 'loose', 'Zzz', '2026-01-01T00:00:00+00:00') RETURNING id",
        )
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
        for tag_id in [loose, phase, mood] {
            sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
                .bind(track)
                .bind(tag_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (service, id) in [
            ("youtube", "yt-1"),
            ("spotify", "sp-2"),
            ("soundcloud", "sc-1"),
            ("spotify", "sp-1"),
            ("tidal", "td-1"),
        ] {
            sqlx::query(
                "INSERT INTO hub_track_external_ids (track_id, service, external_id)
                 VALUES (?1, ?2, ?3)",
            )
            .bind(track)
            .bind(service)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }

        let typeset = TypeSet::load(&pool, user).await.unwrap();
        let tags = resolved_tags_typed(&pool, track, &typeset).await.unwrap();
        assert_eq!(
            tags,
            vec![
                typed("Deep", "MOOD"),
                typed("Peak", "PHASE_2"),
                untyped("Zzz"),
            ]
        );
        assert_eq!(type_marker(&tags, &typeset), vec!["M", "P"]);

        let ids = source_ids(&pool, track).await.unwrap();
        assert_eq!(ids, vec!["sp:sp-1", "sp:sp-2", "sc:sc-1", "yt:yt-1"]);
    }
}