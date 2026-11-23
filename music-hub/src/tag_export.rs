//! Shared tag-export helpers (#690, slice T2).
//!
//! The device export ([`crate::backpack_api`]) and the comment-template preview
//! ([`crate::preview`]) must agree byte-for-byte on a track's tags, type markers,
//! source ids and key. This module is the **single** read path for those values:
//!
//! * [`resolved_tags_typed`] — the resolved tags as `(name, type)` in export
//!   order: typed tags first in [`TYPE_NAMES`] order (PHASE_1..5 in fixed
//!   order), each family name-sorted; untyped tags alphabetically last.
//! * [`type_marker`] — one single-letter marker per present type family in
//!   [`TYPE_NAMES`] order (`PHASE_1..5` collapse to a single `"P"`).
//! * [`display_key`] — the musical key for the Comment: Camelot when known,
//!   else the normalised musical key.
//! * [`source_ids`] — the track's external ids prefixed per service in fixed
//!   service order (`sp:`/`sc:`/`yt:`).

use anyhow::Result;
use sqlx::SqlitePool;

use crate::tagtypes::{TYPE_NAMES, TagType};

/// The `hub_tags.type` value of a tag, canonicalised through
/// [`TagType::parse`]; `None` for an untyped or unknown type string.
fn canonical_type(raw: Option<&str>) -> Option<&'static str> {
    let ty = TagType::parse(raw?)?;
    TYPE_NAMES.iter().copied().find(|n| *n == ty.as_str())
}

/// Export sort order: typed tags in [`TYPE_NAMES`] order (name-sorted within a
/// type), then untyped tags name-sorted.
fn sort_typed(mut rows: Vec<(String, Option<String>)>) -> Vec<(String, Option<String>)> {
    rows.sort_by(|(name_a, type_a), (name_b, type_b)| {
        let a = canonical_type(type_a.as_deref());
        let b = canonical_type(type_b.as_deref());
        match (a, b) {
            (Some(a), Some(b)) => {
                let ia = TYPE_NAMES
                    .iter()
                    .position(|n| *n == a)
                    .unwrap_or(usize::MAX);
                let ib = TYPE_NAMES
                    .iter()
                    .position(|n| *n == b)
                    .unwrap_or(usize::MAX);
                ia.cmp(&ib)
                    .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase()))
                    .then_with(|| name_a.cmp(name_b))
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => name_a
                .to_lowercase()
                .cmp(&name_b.to_lowercase())
                .then_with(|| name_a.cmp(name_b)),
        }
    });
    rows
}

/// The track's resolved tags as `(name, type)` in export order.
///
/// One SQL read over `hub_track_resolved_tags JOIN hub_tags`; an unknown track
/// (or one without tags) yields an empty vector.
pub async fn resolved_tags_typed(
    pool: &SqlitePool,
    track_id: i64,
) -> Result<Vec<(String, Option<String>)>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT h.name, h.type
           FROM hub_track_resolved_tags rt
           JOIN hub_tags h ON h.id = rt.tag_id
          WHERE rt.track_id = ?1",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;
    Ok(sort_typed(rows))
}

/// One single-letter marker per present type family, in [`TYPE_NAMES`] order.
/// `PHASE_1..5` collapse to one `"P"`; untyped tags contribute nothing.
pub fn type_marker(tags: &[(String, Option<String>)]) -> Vec<String> {
    let mut present: Vec<&'static str> = Vec::new();
    for (_, type_raw) in tags {
        if let Some(canonical) = canonical_type(type_raw.as_deref()) {
            if !present.contains(&canonical) {
                present.push(canonical);
            }
        }
    }
    present.sort_by_key(|n| TYPE_NAMES.iter().position(|t| t == n).unwrap_or(usize::MAX));

    let mut out: Vec<String> = Vec::new();
    for canonical in present {
        let abbrev = TagType::parse(canonical).map(TagType::abbrev).unwrap_or("");
        if abbrev.is_empty() || out.iter().any(|m| m == abbrev) {
            continue;
        }
        out.push(abbrev.to_string());
    }
    out
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
const SOURCE_SERVICES: &[(&str, &str)] = &[
    ("spotify", "sp:"),
    ("soundcloud", "sc:"),
    ("youtube", "yt:"),
];

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
        let rows = vec![
            untyped("zebra"),
            typed("Deep", "MOOD"),
            typed("Acid", "GENRE"),
            typed("bass", "MOOD"),
            untyped("Alpha"),
            typed("Peak", "PHASE_4"),
            typed("Drive", "PHASE_2"),
            typed("Hypno", "MERKMAL"),
        ];
        let sorted: Vec<String> = sort_typed(rows).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            sorted,
            vec![
                "bass", "Deep",  // MOOD, name-sorted
                "Acid",  // GENRE
                "Drive", // PHASE_2 (fixed phase order)
                "Peak",  // PHASE_4
                "Hypno", // MERKMAL
                "Alpha", "zebra", // untyped, name-sorted
            ]
        );
    }

    #[test]
    fn type_marker_one_per_family_in_canonical_order() {
        let tags = vec![
            typed("Peak", "PHASE_2"),
            typed("Hypno", "MERKMAL"),
            typed("Deep", "MOOD"),
            untyped("loose"),
        ];
        assert_eq!(type_marker(&tags), vec!["M", "P", "E"]);
    }

    #[test]
    fn type_marker_collapses_phases_and_ignores_untyped() {
        let tags = vec![typed("A", "PHASE_1"), typed("B", "PHASE_4")];
        assert_eq!(type_marker(&tags), vec!["P"]);
        assert!(type_marker(&[untyped("x")]).is_empty());
        assert!(type_marker(&[]).is_empty());
    }

    #[test]
    fn display_key_prefers_camelot_then_normalises_musical_key() {
        assert_eq!(
            display_key(Some("8A"), Some("G-Minor")).as_deref(),
            Some("8A")
        );
        assert_eq!(
            display_key(Some("  "), Some("G-Minor")).as_deref(),
            Some("G-Minor")
        );
        assert_eq!(
            display_key(None, Some("G-Minor")).as_deref(),
            Some("G-Minor")
        );
        assert_eq!(
            display_key(None, Some("G -- Minor")).as_deref(),
            Some("G-Minor")
        );
        assert_eq!(display_key(None, Some("11 m")).as_deref(), Some("11-m"));
        assert_eq!(display_key(None, Some("   ")), None);
        assert_eq!(display_key(None, None), None);
        assert_eq!(display_key(Some(""), Some("")), None);
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

        let tags = resolved_tags_typed(&pool, track).await.unwrap();
        assert_eq!(
            tags,
            vec![
                typed("Deep", "MOOD"),
                typed("Peak", "PHASE_2"),
                untyped("Zzz"),
            ]
        );
        assert_eq!(type_marker(&tags), vec!["M", "P"]);

        let ids = source_ids(&pool, track).await.unwrap();
        assert_eq!(ids, vec!["sp:sp-1", "sp:sp-2", "sc:sc-1", "yt:yt-1"]);
    }
}
