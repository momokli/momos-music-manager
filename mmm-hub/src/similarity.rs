//! Tag-overlap similarity v2 (issues #221/#222).
//!
//! `tag_similarity` scores two tracks by their shared resolved tags, giving a
//! bonus to shared tags that live in *different* groups (cross-group matches
//! count more than same-group). Coefficients come from the engine settings.
//!
//! The Rumpelkiste exception (#222) is exposed via helpers: playlist edges whose
//! playlist feeds a `role = 'rumpelkiste'` tag are excluded from playlist-based
//! comparison (use [`rumpelkiste_playlist_ids`] / [`rumpelkiste_tag_ids`]).

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::settings::Engine;

/// Playlist ids that feed at least one tag in a `rumpelkiste` group.
pub async fn rumpelkiste_playlist_ids(pool: &SqlitePool) -> HashSet<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT ts.playlist_id
           FROM hub_tag_sources ts
           JOIN hub_group_tags gt ON gt.tag_id = ts.tag_id
           JOIN hub_tag_groups g ON g.id = gt.group_id
          WHERE g.role = 'rumpelkiste'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// Tag ids that live in a `rumpelkiste` group.
pub async fn rumpelkiste_tag_ids(pool: &SqlitePool) -> HashSet<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT gt.tag_id
           FROM hub_group_tags gt
           JOIN hub_tag_groups g ON g.id = gt.group_id
          WHERE g.role = 'rumpelkiste'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// `tag_id -> group_id` (lowest group) for a track's resolved tags.
async fn tag_groups(pool: &SqlitePool, track_id: i64) -> HashMap<i64, i64> {
    let rows = sqlx::query_as::<_, (i64, i64)>(
        "SELECT rt.tag_id, MIN(gt.group_id) AS gid
           FROM hub_track_resolved_tags rt
           JOIN hub_group_tags gt ON gt.tag_id = rt.tag_id
          WHERE rt.track_id = ?1
          GROUP BY rt.tag_id",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().collect()
}

/// Tag-overlap similarity: shared tags (base weight) plus a bonus for shared-tag
/// pairs that sit in different groups.
pub async fn tag_similarity(pool: &SqlitePool, a: i64, b: i64, e: &Engine) -> f64 {
    let ga = tag_groups(pool, a).await;
    let gb = tag_groups(pool, b).await;
    let shared: Vec<i64> = ga.keys().filter(|k| gb.contains_key(k)).copied().collect();
    let base = shared.len() as f64;
    // Count unordered shared-tag pairs whose groups differ.
    let mut cross = 0u64;
    for i in 0..shared.len() {
        for j in (i + 1)..shared.len() {
            let gi = ga.get(&shared[i]).copied().unwrap_or(-1);
            let gj = ga.get(&shared[j]).copied().unwrap_or(-1);
            if gi != gj {
                cross += 1;
            }
        }
    }
    base * e.tag_overlap_base + (cross as f64) * e.cross_group_bonus
}
