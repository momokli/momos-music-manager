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

use sqlx::{QueryBuilder, Row, SqlitePool};

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

/// `tag_id -> group_id` (lowest group) for a track's resolved tags, dropping any
/// Rumpelkiste tags (they must not create similarity — #222).
async fn tag_groups_filtered(
    pool: &SqlitePool,
    track_id: i64,
    exclude: &HashSet<i64>,
) -> HashMap<i64, i64> {
    tag_groups(pool, track_id)
        .await
        .into_iter()
        .filter(|(t, _)| !exclude.contains(t))
        .collect()
}

/// Tag-overlap similarity: shared tags (base weight) plus a bonus for shared-tag
/// pairs that sit in different groups. Rumpelkiste tags are ignored.
pub async fn tag_similarity(pool: &SqlitePool, a: i64, b: i64, e: &Engine) -> f64 {
    let rumpel = rumpelkiste_tag_ids(pool).await;
    let ga = tag_groups_filtered(pool, a, &rumpel).await;
    let gb = tag_groups_filtered(pool, b, &rumpel).await;
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

/// Similarity of `seed_id` against many candidates in one pass (no N+1).
/// Returns `candidate_id -> score` only for candidates that share >= 1 tag
/// with the seed. Used by the Digging ranking (#223).
pub async fn similarity_to_seed(
    pool: &SqlitePool,
    seed_id: i64,
    candidates: &[i64],
    e: &Engine,
) -> HashMap<i64, f64> {
    let mut out: HashMap<i64, f64> = HashMap::new();
    if candidates.is_empty() {
        return out;
    }
    let rumpel = rumpelkiste_tag_ids(pool).await;
    let ga = tag_groups_filtered(pool, seed_id, &rumpel).await;
    if ga.is_empty() {
        return out;
    }

    // Candidate tags, chunked to respect the SQLite bind-variable limit.
    let mut gb_all: HashMap<i64, HashMap<i64, i64>> = HashMap::new();
    for chunk in candidates.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT rt.track_id, rt.tag_id, MIN(gt.group_id)
               FROM hub_track_resolved_tags rt
               JOIN hub_group_tags gt ON gt.tag_id = rt.tag_id
              WHERE rt.track_id IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
        }
        qb.push(") GROUP BY rt.track_id, rt.tag_id");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tag: i64 = r.get(1);
            if rumpel.contains(&tag) {
                continue;
            }
            gb_all.entry(r.get(0)).or_default().insert(tag, r.get(2));
        }
    }

    for (tid, gb) in gb_all {
        let shared: Vec<i64> = ga.keys().filter(|k| gb.contains_key(k)).copied().collect();
        if shared.is_empty() {
            continue;
        }
        let base = shared.len() as f64;
        let mut cross = 0u64;
        for i in 0..shared.len() {
            for j in (i + 1)..shared.len() {
                let gi = ga.get(&shared[i]).copied().unwrap_or(-1);
                let gj = gb.get(&shared[j]).copied().unwrap_or(-1);
                if gi != gj {
                    cross += 1;
                }
            }
        }
        out.insert(
            tid,
            base * e.tag_overlap_base + cross as f64 * e.cross_group_bonus,
        );
    }
    out
}
