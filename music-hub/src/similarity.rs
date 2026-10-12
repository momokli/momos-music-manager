//! Tag-overlap similarity v2 (issues #221/#222).
//!
//! `tag_similarity` scores two tracks by their shared resolved tags, giving a
//! bonus to shared tags that have *different* types (cross-type matches count
//! more than same-type). Coefficients come from the engine settings.
//!
//! The Rumpelkiste exception (#222) is exposed via helpers: playlist edges whose
//! playlist feeds a `RUMPELKISTE` tag are excluded from playlist-based
//! comparison (use [`rumpelkiste_playlist_ids`] / [`rumpelkiste_tag_ids`]).

use std::collections::{HashMap, HashSet};

use sqlx::{QueryBuilder, Row, SqlitePool};

use crate::settings::Engine;

/// Playlist ids that feed at least one `RUMPELKISTE` tag.
pub async fn rumpelkiste_playlist_ids(pool: &SqlitePool) -> HashSet<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT ts.playlist_id
           FROM hub_tag_sources ts
           JOIN hub_tags t ON t.id = ts.tag_id
          WHERE t.type = 'RUMPELKISTE'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// Tag ids whose type is `RUMPELKISTE`.
pub async fn rumpelkiste_tag_ids(pool: &SqlitePool) -> HashSet<i64> {
    sqlx::query_scalar::<_, i64>("SELECT DISTINCT id FROM hub_tags WHERE type = 'RUMPELKISTE'")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// `tag_id -> type` for a track's resolved tags that carry a type.
async fn tag_types(pool: &SqlitePool, track_id: i64) -> HashMap<i64, String> {
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT rt.tag_id, t.type
           FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
          WHERE rt.track_id = ?1 AND t.type IS NOT NULL",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().collect()
}

/// `tag_id -> type` for a track's resolved tags, dropping any Rumpelkiste tags
/// (they must not create similarity — #222).
async fn tag_types_filtered(
    pool: &SqlitePool,
    track_id: i64,
    exclude: &HashSet<i64>,
) -> HashMap<i64, String> {
    tag_types(pool, track_id)
        .await
        .into_iter()
        .filter(|(t, _)| !exclude.contains(t))
        .collect()
}

/// Tag-overlap similarity: shared tags (base weight) plus a bonus for shared-tag
/// pairs that have different types. Rumpelkiste tags are ignored.
pub async fn tag_similarity(pool: &SqlitePool, a: i64, b: i64, e: &Engine) -> f64 {
    let rumpel = rumpelkiste_tag_ids(pool).await;
    let ta = tag_types_filtered(pool, a, &rumpel).await;
    let tb = tag_types_filtered(pool, b, &rumpel).await;
    let shared: Vec<i64> = ta.keys().filter(|k| tb.contains_key(k)).copied().collect();
    let base = shared.len() as f64;
    // Count unordered shared-tag pairs whose types differ.
    let mut cross = 0u64;
    for i in 0..shared.len() {
        for j in (i + 1)..shared.len() {
            let ti = ta.get(&shared[i]).map(String::as_str).unwrap_or("");
            let tj = ta.get(&shared[j]).map(String::as_str).unwrap_or("");
            if ti != tj {
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
    let ta = tag_types_filtered(pool, seed_id, &rumpel).await;
    if ta.is_empty() {
        return out;
    }

    // Candidate tags, chunked to respect the SQLite bind-variable limit.
    let mut tb_all: HashMap<i64, HashMap<i64, String>> = HashMap::new();
    for chunk in candidates.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT rt.track_id, rt.tag_id, t.type
               FROM hub_track_resolved_tags rt
               JOIN hub_tags t ON t.id = rt.tag_id
              WHERE t.type IS NOT NULL AND rt.track_id IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
        }
        qb.push(") GROUP BY rt.track_id, rt.tag_id, t.type");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tag: i64 = r.get(1);
            if rumpel.contains(&tag) {
                continue;
            }
            tb_all.entry(r.get(0)).or_default().insert(tag, r.get(2));
        }
    }

    for (tid, tb) in tb_all {
        let shared: Vec<i64> = ta.keys().filter(|k| tb.contains_key(k)).copied().collect();
        if shared.is_empty() {
            continue;
        }
        let base = shared.len() as f64;
        let mut cross = 0u64;
        for i in 0..shared.len() {
            for j in (i + 1)..shared.len() {
                let ti = ta.get(&shared[i]).map(String::as_str).unwrap_or("");
                let tj = tb.get(&shared[j]).map(String::as_str).unwrap_or("");
                if ti != tj {
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
