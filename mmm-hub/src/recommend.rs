//! Tag recommendation engine.
//!
//! Given a track, propose tags from several *relationship neighbourhoods* and,
//! crucially, explain **why** each candidate is recommended. Weights are read
//! live from the engine settings (`engine_rec_*`) so they can be tuned in the UI.
//!
//! Neighbourhoods:
//!   * `tag`      — tracks sharing a resolved tag with the seed
//!   * `playlist` — tracks in the same playlists as the seed
//!   * `artist`   — other tracks by the same artist
//!   * `album`    — other tracks on the same album

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::settings::Engine;

/// One contributing signal for a recommended tag.
#[derive(Debug, Clone)]
pub struct Reason {
    /// Human label (German): Tag | Playlist | Artist | Album.
    pub label: &'static str,
    /// How many neighbours contributed via this signal.
    pub count: i64,
    pub weight: f64,
}

/// A recommended tag with its score and the reasons behind it.
#[derive(Debug, Clone)]
pub struct Recommendation {
    pub tag_id: i64,
    pub name: String,
    pub owner: String,
    pub score: i64,
    pub reasons: Vec<Reason>,
}

impl Recommendation {
    /// Compact explanation, e.g. `3× Tag · 2× Playlist · Artist`.
    pub fn why(&self) -> String {
        self.reasons
            .iter()
            .map(|r| {
                if r.count > 1 {
                    format!("{}× {}", r.count, r.label)
                } else {
                    r.label.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn weight(e: &Engine, kind: &str) -> f64 {
    match kind {
        "tag" => e.rec_tag,
        "playlist" => e.rec_playlist,
        "artist" => e.rec_artist,
        "album" => e.rec_album,
        _ => 0.0,
    }
}

fn label(kind: &str) -> &'static str {
    match kind {
        "tag" => "Tag",
        "playlist" => "Playlist",
        "artist" => "Artist",
        "album" => "Album",
        _ => "?",
    }
}

/// Recommend tags for `track_id`, ranked by weighted score, with reasons.
pub async fn recommend_tags(pool: &SqlitePool, track_id: i64, limit: usize) -> Vec<Recommendation> {
    let e = crate::settings::engine(pool).await;
    let rows = sqlx::query_as::<_, (i64, String, String, String, i64)>(
        "WITH s AS (SELECT ?1 AS id),
              seed_tags AS (SELECT tag_id FROM hub_track_resolved_tags WHERE track_id = (SELECT id FROM s)),
              cand AS (
                 SELECT rt2.tag_id AS tag_id, 'tag' AS kind
                   FROM hub_track_resolved_tags rt1
                   JOIN hub_track_resolved_tags rt2 ON rt2.track_id = rt1.track_id
                  WHERE rt1.tag_id IN (SELECT tag_id FROM seed_tags)
                    AND rt2.track_id <> (SELECT id FROM s)
                 UNION ALL
                 SELECT rt.tag_id, 'playlist'
                   FROM hub_playlist_tracks p1
                   JOIN hub_playlist_tracks p2 ON p2.playlist_id = p1.playlist_id
                   JOIN hub_track_resolved_tags rt ON rt.track_id = p2.track_id
                  WHERE p1.track_id = (SELECT id FROM s) AND p2.track_id <> (SELECT id FROM s)
                 UNION ALL
                 SELECT rt.tag_id, 'artist'
                   FROM hub_tracks t1
                   JOIN hub_tracks t2 ON t2.artists = t1.artists AND t2.id <> t1.id
                   JOIN hub_track_resolved_tags rt ON rt.track_id = t2.id
                  WHERE t1.id = (SELECT id FROM s)
                    AND t1.artists IS NOT NULL AND TRIM(t1.artists) <> ''
                 UNION ALL
                 SELECT rt.tag_id, 'album'
                   FROM hub_tracks a1
                   JOIN hub_tracks a2 ON a2.album = a1.album AND a2.id <> a1.id
                   JOIN hub_track_resolved_tags rt ON rt.track_id = a2.id
                  WHERE a1.id = (SELECT id FROM s)
                    AND a1.album IS NOT NULL AND TRIM(a1.album) <> ''
              )
         SELECT x.tag_id, t.name, u.slug, x.kind, COUNT(*) AS cnt
           FROM cand x
           JOIN hub_tags t ON t.id = x.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE x.tag_id NOT IN (SELECT tag_id FROM seed_tags)
            AND x.tag_id NOT IN (
                 SELECT gt.tag_id FROM hub_group_tags gt
                   JOIN hub_tag_groups g ON g.id = gt.group_id
                  WHERE g.role IN ('setlist', 'rumpelkiste'))
          GROUP BY x.tag_id, t.name, u.slug, x.kind",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut map: HashMap<i64, Recommendation> = HashMap::new();
    for (tag_id, name, owner, kind, cnt) in rows {
        let r = map.entry(tag_id).or_insert_with(|| Recommendation {
            tag_id,
            name,
            owner,
            score: 0,
            reasons: Vec::new(),
        });
        let w = weight(&e, &kind);
        r.score += (w.round() as i64) * cnt;
        r.reasons.push(Reason {
            label: label(&kind),
            count: cnt,
            weight: w,
        });
    }

    let mut out: Vec<Recommendation> = map.into_values().collect();
    for r in &mut out {
        // Strongest signals first.
        r.reasons.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.count.cmp(&a.count))
        });
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    out.truncate(limit);
    out
}
