//! Ripeness score — "how well is this track's data populated?" (issues #214/#215).
//!
//! `RIPENESS = tag_weight*TAG_SCORE + meta_weight*META_SCORE + trak_weight*TRAKTOR_SCORE`
//! with HUMAN-TAGS weighing more than meta (tag_weight > meta_weight by default).
//!
//! Tag points are position-based and hang on the **group multiset**, not the
//! tag: deleting any tag from a group removes the cheapest tier, never the
//! "100 points" tier. See [`group_points`].

use sqlx::{QueryBuilder, Row, SqlitePool};

use crate::settings::Engine;

use std::collections::HashMap;

/// Points contributed by a group holding `k` tags. Position `i` (0-based) uses
/// `points[i]`; beyond the vector the last value repeats.
pub fn group_points(points: &[f64], k: usize) -> f64 {
    let Some(last) = points.last().copied() else {
        return 0.0;
    };
    (0..k).map(|i| points.get(i).copied().unwrap_or(last)).sum()
}

#[derive(Debug, Default, Clone)]
pub struct Ripeness {
    pub tag_score: f64,
    pub meta_score: f64,
    pub traktor_score: f64,
    pub total: f64,
    /// `(group id, group name, tag count)` per classifying group.
    pub tags_per_group: Vec<(i64, String, usize)>,
    /// `(field, present)` for the meta-fields checklist.
    pub meta_present: Vec<(String, bool)>,
}

/// Compute the ripeness score for a track on the fly.
pub async fn ripeness(pool: &SqlitePool, track_id: i64, e: &Engine) -> Ripeness {
    let mut out = Ripeness::default();

    // Human tags, grouped.
    let rows = sqlx::query_as::<_, (i64, String, i64)>(
        "SELECT g.id, COALESCE(g.name,''), COUNT(*)
           FROM hub_track_resolved_tags rt
           JOIN hub_group_tags gt ON gt.tag_id = rt.tag_id
           JOIN hub_tag_groups g ON g.id = gt.group_id
          WHERE rt.track_id = ?1
          GROUP BY g.id, g.name",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (id, name, cnt) in rows {
        let k = cnt.max(0) as usize;
        out.tag_score += group_points(&e.tag_points, k);
        out.tags_per_group.push((id, name, k));
    }

    // Track meta fields.
    let meta = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        ),
    >(
        "SELECT t.title, t.artists, t.album, t.duration_ms, t.image_url,
                CAST((SELECT bpm FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS bpm,
                CAST((SELECT camelot FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS camelot,
                (SELECT COUNT(*) FROM hub_track_genres WHERE track_id = t.id) AS genres
           FROM hub_tracks t WHERE t.id = ?1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let present = |s: &Option<String>| s.as_deref().map(|v| !v.trim().is_empty()).unwrap_or(false);
    if let Some((title, artists, album, _dur, image, bpm, camelot, genres)) = meta {
        let checks = [
            ("Titel", present(&title)),
            ("Artist", present(&artists)),
            ("Album", present(&album)),
            ("Cover", present(&image)),
            ("BPM", present(&bpm)),
            ("Key", present(&camelot)),
            ("Genre", genres > 0),
        ];
        out.meta_score = checks.iter().filter(|(_, ok)| *ok).count() as f64;
        out.meta_present = checks.iter().map(|(n, ok)| (n.to_string(), *ok)).collect();
    }

    // Traktor signal.
    if let Some((pc, last_played, rating, session_occ, playlist_occ)) =
        sqlx::query_as::<_, (i64, Option<String>, Option<i64>, i64, i64)>(
            "SELECT play_count, last_played, rating, session_occurrence, playlist_occurrence
               FROM hub_v_track_traktor WHERE track_id = ?1",
        )
        .bind(track_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    {
        let cap = e.trak_playcount_cap.max(1.0);
        let pc_norm = (pc as f64 / cap).min(1.0);
        let rating_norm = rating.map(|r| r as f64 / 5.0).unwrap_or(0.0);
        let recency = if last_played
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            1.0
        } else {
            0.0
        };
        let occurrence = if session_occ > 0 || playlist_occ > 0 {
            1.0
        } else {
            0.0
        };
        out.traktor_score = pc_norm + rating_norm + recency + occurrence;
    }

    out.total = e.tag_weight * out.tag_score
        + e.meta_weight * out.meta_score
        + e.trak_weight * out.traktor_score;
    out
}

/// Ripeness totals (`RIPENESS`) for a specific set of tracks, batched — used by
/// the Digging/Overlap ranking (#223). Missing tracks simply have no entry.
pub async fn ripeness_many(pool: &SqlitePool, ids: &[i64], e: &Engine) -> HashMap<i64, f64> {
    let mut out: HashMap<i64, f64> = HashMap::new();
    if ids.is_empty() {
        return out;
    }

    // Position-weighted tag score per track.
    let mut tag_score: HashMap<i64, f64> = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT rt.track_id, gt.group_id, COUNT(*)
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
        qb.push(") GROUP BY rt.track_id, gt.group_id");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let cnt: i64 = r.get(2);
            *tag_score.entry(r.get(0)).or_insert(0.0) +=
                group_points(&e.tag_points, cnt.max(0) as usize);
        }
    }

    // Traktor signal per track.
    let cap = e.trak_playcount_cap.max(1.0);
    let mut trak: HashMap<i64, f64> = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT track_id, play_count, rating, last_played,
                    session_occurrence, playlist_occurrence
               FROM hub_v_track_traktor WHERE track_id IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let pc: i64 = r.get(1);
            let rating: Option<i64> = r.get(2);
            let last: Option<String> = r.get(3);
            let sess: i64 = r.get(4);
            let pl: i64 = r.get(5);
            let pc_norm = (pc as f64 / cap).min(1.0);
            let rating_norm = rating.map(|v| v as f64 / 5.0).unwrap_or(0.0);
            let recency = if last.map(|s| !s.trim().is_empty()).unwrap_or(false) {
                1.0
            } else {
                0.0
            };
            let occ = if sess > 0 || pl > 0 { 1.0 } else { 0.0 };
            trak.insert(r.get(0), pc_norm + rating_norm + recency + occ);
        }
    }

    // Meta-field presence per track.
    let mut meta_score: HashMap<i64, f64> = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT t.id, COALESCE(t.title,'') AS title, COALESCE(t.artists,'') AS artists,
                    COALESCE(t.album,'') AS album, t.image_url,
                    CAST((SELECT bpm FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS bpm,
                    CAST((SELECT camelot FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS camelot,
                    (SELECT COUNT(*) FROM hub_track_genres WHERE track_id = t.id) AS genres
               FROM hub_tracks t WHERE t.id IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let has = |v: &str| !v.trim().is_empty();
            let n = [
                has(&r.get::<String, _>("title")),
                has(&r.get::<String, _>("artists")),
                has(&r.get::<String, _>("album")),
                r.get::<Option<String>, _>("image_url")
                    .map(|s| has(&s))
                    .unwrap_or(false),
                r.get::<Option<String>, _>("bpm")
                    .map(|s| has(&s))
                    .unwrap_or(false),
                r.get::<Option<String>, _>("camelot")
                    .map(|s| has(&s))
                    .unwrap_or(false),
                r.get::<i64, _>("genres") > 0,
            ]
            .iter()
            .filter(|b| **b)
            .count() as f64;
            meta_score.insert(r.get(0), n);
        }
    }

    for id in ids {
        let t = tag_score.get(id).copied().unwrap_or(0.0);
        let m = meta_score.get(id).copied().unwrap_or(0.0);
        let k = trak.get(id).copied().unwrap_or(0.0);
        out.insert(
            *id,
            e.tag_weight * t + e.meta_weight * m + e.trak_weight * k,
        );
    }
    out
}

/// One row of the tag queue.
#[derive(Debug, Clone)]
pub struct QueueRow {
    pub track_id: i64,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub total: f64,
    pub tag_score: f64,
    pub meta_score: f64,
    pub traktor_score: f64,
    pub tag_count: i64,
}

/// Build the tag queue: every track scored by ripeness, ascending (least-ripe
/// first = most in need of tagging). `max_total` hides tracks already "ripe
/// enough" (the configurable threshold). `only_untagged` keeps only tracks with
/// no tags at all.
pub async fn queue(
    pool: &SqlitePool,
    e: &Engine,
    max_total: Option<f64>,
    only_untagged: bool,
    sort: &str,
    limit: usize,
) -> Vec<QueueRow> {
    // Tag counts per (track, group) -> position-weighted tag score per track.
    let tag_rows = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT rt.track_id, gt.group_id, COUNT(*)
           FROM hub_track_resolved_tags rt
           JOIN hub_group_tags gt ON gt.tag_id = rt.tag_id
          GROUP BY rt.track_id, gt.group_id",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut tag_score: HashMap<i64, (f64, i64)> = HashMap::new();
    for (tid, _gid, cnt) in tag_rows {
        let slot = tag_score.entry(tid).or_insert((0.0, 0));
        slot.0 += group_points(&e.tag_points, cnt.max(0) as usize);
        slot.1 += cnt;
    }

    // Traktor signal per track.
    let trak_rows = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<String>, i64, i64)>(
        "SELECT track_id, play_count, rating, last_played, session_occurrence, playlist_occurrence
           FROM hub_v_track_traktor",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let cap = e.trak_playcount_cap.max(1.0);
    let mut trak: HashMap<i64, f64> = HashMap::new();
    for (tid, pc, rating, last, sess, pl) in trak_rows {
        let pc_norm = (pc as f64 / cap).min(1.0);
        let rating_norm = rating.map(|r| r as f64 / 5.0).unwrap_or(0.0);
        let recency = if last.map(|s| !s.trim().is_empty()).unwrap_or(false) {
            1.0
        } else {
            0.0
        };
        let occ = if sess > 0 || pl > 0 { 1.0 } else { 0.0 };
        trak.insert(tid, pc_norm + rating_norm + recency + occ);
    }

    // Track meta.
    let meta_rows = sqlx::query(
        "SELECT t.id, COALESCE(t.title,'') AS title, COALESCE(t.artists,'') AS artists,
                COALESCE(t.album,'') AS album, t.image_url,
                CAST((SELECT bpm FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS bpm,
                CAST((SELECT camelot FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS camelot,
                (SELECT COUNT(*) FROM hub_track_genres WHERE track_id = t.id) AS genres
           FROM hub_tracks t",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut out: Vec<QueueRow> = Vec::with_capacity(meta_rows.len());
    for r in &meta_rows {
        let id: i64 = r.get("id");
        let (t_score, t_count) = tag_score.get(&id).copied().unwrap_or((0.0, 0));
        if only_untagged && t_count > 0 {
            continue;
        }
        let has = |v: &str| !v.trim().is_empty();
        let meta_score = [
            has(&r.get::<String, _>("title")),
            has(&r.get::<String, _>("artists")),
            has(&r.get::<String, _>("album")),
            r.get::<Option<String>, _>("image_url")
                .map(|s| has(&s))
                .unwrap_or(false),
            r.get::<Option<String>, _>("bpm")
                .map(|s| has(&s))
                .unwrap_or(false),
            r.get::<Option<String>, _>("camelot")
                .map(|s| has(&s))
                .unwrap_or(false),
            r.get::<i64, _>("genres") > 0,
        ]
        .iter()
        .filter(|b| **b)
        .count() as f64;
        let traktor_score = trak.get(&id).copied().unwrap_or(0.0);
        let total =
            e.tag_weight * t_score + e.meta_weight * meta_score + e.trak_weight * traktor_score;
        if let Some(max) = max_total {
            if total > max {
                continue;
            }
        }
        out.push(QueueRow {
            track_id: id,
            title: r.get("title"),
            artists: r.get("artists"),
            album: r.get("album"),
            total,
            tag_score: t_score,
            meta_score,
            traktor_score,
            tag_count: t_count,
        });
    }
    // Sort by the requested order (default: least ripe first).
    let by_total = |a: &QueueRow, b: &QueueRow| {
        a.total
            .partial_cmp(&b.total)
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    match sort {
        "ripeness-desc" => out.sort_by(|a, b| by_total(b, a)),
        "title" => out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        "artist" => out.sort_by(|a, b| {
            a.artists
                .to_lowercase()
                .cmp(&b.artists.to_lowercase())
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        "tags" => out.sort_by(|a, b| a.tag_count.cmp(&b.tag_count).then_with(|| by_total(a, b))),
        "tags-desc" => {
            out.sort_by(|a, b| b.tag_count.cmp(&a.tag_count).then_with(|| by_total(a, b)))
        }
        "meta" => out.sort_by(|a, b| {
            a.meta_score
                .partial_cmp(&b.meta_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| by_total(a, b))
        }),
        "traktor" => out.sort_by(|a, b| {
            b.traktor_score
                .partial_cmp(&a.traktor_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| by_total(a, b))
        }),
        _ => out.sort_by(by_total),
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::group_points;

    const POINTS: &[f64] = &[100.0, 50.0, 25.0, 10.0, 5.0, 1.0];

    #[test]
    fn group_points_positions() {
        assert_eq!(group_points(POINTS, 0), 0.0);
        assert_eq!(group_points(POINTS, 1), 100.0);
        assert_eq!(group_points(POINTS, 2), 150.0);
        assert_eq!(group_points(POINTS, 3), 175.0);
        assert_eq!(group_points(POINTS, 6), 191.0);
    }

    #[test]
    fn group_points_repeats_last_beyond_vector() {
        // 8th tag onwards still scores 1 each.
        assert_eq!(group_points(POINTS, 8), 193.0);
    }

    #[test]
    fn deleting_a_tag_removes_the_cheapest_tier() {
        // 2 tags in a group = 150; removing *either* leaves 100 (a -50 drop),
        // regardless of which tag was originally the "100 point" one.
        let two = group_points(POINTS, 2);
        let one = group_points(POINTS, 1);
        assert_eq!(two - one, 50.0);
    }
}
