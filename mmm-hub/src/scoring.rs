//! Ripeness score — "how well is this track's data populated?" (issues #214/#215).
//!
//! `RIPENESS = tag_weight*TAG_SCORE + meta_weight*META_SCORE + trak_weight*TRAKTOR_SCORE`
//! with HUMAN-TAGS weighing more than meta (tag_weight > meta_weight by default).
//!
//! Tag points are position-based and hang on the **group multiset**, not the
//! tag: deleting any tag from a group removes the cheapest tier, never the
//! "100 points" tier. See [`group_points`].

use sqlx::SqlitePool;

use crate::settings::Engine;

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
                (SELECT bpm FROM v_track_audio WHERE track_id = t.id) AS bpm,
                (SELECT camelot FROM v_track_audio WHERE track_id = t.id) AS camelot,
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
