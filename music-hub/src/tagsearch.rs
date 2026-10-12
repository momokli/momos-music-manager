//! Track discovery by tags (issues #295, #297):
//!
//! * [`weighted_search`] — score every track against a set of query tags
//!   (explicit names with strengths, and/or derived from a seed set of tracks),
//!   sorted by fit.
//! * [`revisit`] — tracks whose last manual tagging predates the most tags the
//!   user has created since, i.e. candidates for re-tagging.

use std::collections::{HashMap, HashSet};

use sqlx::{QueryBuilder, Row, SqlitePool};

/// Points contributed by a matched query tag of strength `s`:
/// `2 - 2^-(s-1)` → 1.0, 1.5, 1.75, … approaching 2.
pub fn strength_points(s: f64) -> f64 {
    let s = s.max(1.0);
    2.0 - 2f64.powf(-(s - 1.0))
}

/// One weighted-search hit.
pub struct Hit {
    pub track_id: i64,
    pub title: String,
    pub artists: String,
    pub score: f64,
    /// Distinct query tags the track carries.
    pub matched: i64,
}

/// Weighted tag search. `explicit` are `(name, strength)` pairs; `seed` is an
/// optional set of track ids whose tags contribute strength = their frequency in
/// the seed. Returns tracks with `score > 0`, sorted by score desc.
pub async fn weighted_search(
    pool: &SqlitePool,
    explicit: &[(String, f64)],
    seed: &[i64],
    limit: i64,
) -> Vec<Hit> {
    // name(lower) -> strength
    let mut strengths: HashMap<String, f64> = HashMap::new();
    for (name, s) in explicit {
        let k = name.trim().to_lowercase();
        if !k.is_empty() {
            strengths.insert(k, s.max(1.0));
        }
    }
    // Seed tracks: frequency adds to the strength map.
    if !seed.is_empty() {
        for chunk in seed.chunks(900) {
            let mut qb = QueryBuilder::new(
                "SELECT lower(t.name) AS nm, COUNT(*) AS c
                   FROM hub_track_resolved_tags rt JOIN hub_tags t ON t.id = rt.tag_id
                  WHERE rt.track_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
            qb.push(") GROUP BY lower(t.name)");
            for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
                let nm: String = r.get("nm");
                let c: i64 = r.get("c");
                *strengths.entry(nm).or_insert(0.0) += c as f64;
            }
        }
    }
    if strengths.is_empty() {
        return Vec::new();
    }

    // Match resolved tags against the query names; dedupe per (track, name) so a
    // track isn't double-counted for the same name owned by several users.
    let names: Vec<String> = strengths.keys().cloned().collect();
    let mut seen: HashSet<(i64, String)> = HashSet::new();
    let mut scores: HashMap<i64, (f64, i64)> = HashMap::new();
    for chunk in names.chunks(400) {
        let mut qb = QueryBuilder::new(
            "SELECT rt.track_id AS tid, lower(t.name) AS nm
               FROM hub_track_resolved_tags rt JOIN hub_tags t ON t.id = rt.tag_id
              WHERE lower(t.name) IN (",
        );
        let mut sep = qb.separated(", ");
        for n in chunk {
            sep.push_bind(n.clone());
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            let nm: String = r.get("nm");
            if !seen.insert((tid, nm.clone())) {
                continue;
            }
            let pts = strength_points(strengths.get(&nm).copied().unwrap_or(1.0));
            let e = scores.entry(tid).or_insert((0.0, 0));
            e.0 += pts;
            e.1 += 1;
        }
    }

    // Top-N by score.
    let mut ranked: Vec<(i64, f64, i64)> = scores
        .into_iter()
        .map(|(tid, (s, m))| (tid, s, m))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(limit.max(0) as usize);
    if ranked.is_empty() {
        return Vec::new();
    }

    // Titles/artists for the ranked ids.
    let ids: Vec<i64> = ranked.iter().map(|(id, _, _)| *id).collect();
    let mut meta: HashMap<i64, (String, String)> = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT id, COALESCE(title,'') AS title, COALESCE(artists,'') AS artists
               FROM hub_tracks WHERE id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            meta.insert(r.get("id"), (r.get("title"), r.get("artists")));
        }
    }

    ranked
        .into_iter()
        .map(|(tid, score, matched)| {
            let (title, artists) = meta.remove(&tid).unwrap_or_default();
            Hit {
                track_id: tid,
                title,
                artists,
                score,
                matched,
            }
        })
        .collect()
}

/// One revisit candidate.
pub struct Revisit {
    pub track_id: i64,
    pub title: String,
    pub artists: String,
    pub last_tagged: String,
    pub missed: i64,
}

/// Tracks whose last manual tagging predates the most of the user's own tags
/// created since (excluding sorting-only types). Sorted by `missed` desc.
pub async fn revisit(pool: &SqlitePool, user_id: i64, limit: i64) -> Vec<Revisit> {
    let row_sql = "SELECT t.tid AS tid, t.lt AS lt,
                (SELECT COUNT(*) FROM hub_tags g
                  WHERE g.owner_user_id = ?1
                    AND COALESCE(g.type,'') NOT IN ('SETLIST','RUMPELKISTE')
                    AND g.created_at > t.lt) AS missed
           FROM (SELECT track_id AS tid, MAX(COALESCE(created_at,'')) AS lt
                   FROM hub_track_tag_manual WHERE user_id = ?1 GROUP BY track_id) t
          WHERE missed > 0
          ORDER BY missed DESC
          LIMIT ?2";
    let rows = sqlx::query(row_sql)
        .bind(user_id)
        .bind(limit)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    if rows.is_empty() {
        return Vec::new();
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("tid")).collect();
    let mut meta: HashMap<i64, (String, String)> = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT id, COALESCE(title,'') AS title, COALESCE(artists,'') AS artists
               FROM hub_tracks WHERE id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            meta.insert(r.get("id"), (r.get("title"), r.get("artists")));
        }
    }
    rows.into_iter()
        .map(|r| {
            let tid: i64 = r.get("tid");
            let (title, artists) = meta.remove(&tid).unwrap_or_default();
            Revisit {
                track_id: tid,
                title,
                artists,
                last_tagged: r.get::<Option<String>, _>("lt").unwrap_or_default(),
                missed: r.get("missed"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::strength_points;

    #[test]
    fn strength_points_curve() {
        assert!((strength_points(1.0) - 1.0).abs() < 1e-9);
        assert!((strength_points(2.0) - 1.5).abs() < 1e-9);
        assert!((strength_points(3.0) - 1.75).abs() < 1e-9);
        assert!((strength_points(4.0) - 1.875).abs() < 1e-9);
        // Clamped below 1 -> 1 point, and approaches 2.
        assert!((strength_points(0.0) - 1.0).abs() < 1e-9);
        assert!(strength_points(50.0) < 2.0 && strength_points(50.0) > 1.99);
    }
}
