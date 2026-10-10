//! Hub digging: seed-driven track discovery on the shared DB.
//!
//! Internal source only for now: tracks that co-occur with the seed in the same
//! playlists (across users), scored by how many playlists/users share them.
//! External sources (Last.fm, …) hang off the same view.

use anyhow::Result;
use sqlx::{QueryBuilder, Row, SqlitePool};
use std::collections::HashMap;

/// A suggested track related to the seed.
pub struct Suggestion {
    pub id: i64,
    pub title: String,
    pub artists: String,
    /// Number of the seed's playlists that also contain this track.
    pub shared_playlists: i64,
    /// Number of distinct hub users who have this track in those playlists.
    pub users: i64,
    /// Comma-separated user slugs.
    pub user_slugs: String,
}

/// Tracks that co-occur with `seed_track_id` in the same playlists.
///
/// Rumpelkiste playlists are excluded (#222): co-membership in a junk-drawer
/// playlist is not evidence of similarity.
pub async fn internal_suggestions(
    pool: &SqlitePool,
    seed_track_id: i64,
    limit: i64,
) -> Result<Vec<Suggestion>> {
    let rows = sqlx::query(
        "SELECT hpt2.track_id AS tid, t.title, t.artists,
                COUNT(DISTINCT hpt2.playlist_id) AS shared,
                COUNT(DISTINCT hp.user_id) AS users,
                GROUP_CONCAT(DISTINCT u.slug) AS user_slugs
           FROM hub_playlist_tracks seed
           JOIN hub_playlist_tracks hpt2
             ON hpt2.playlist_id = seed.playlist_id AND hpt2.track_id <> seed.track_id
           JOIN hub_playlists hp ON hp.id = hpt2.playlist_id
           JOIN hub_users u ON u.id = hp.user_id
           JOIN hub_tracks t ON t.id = hpt2.track_id
          WHERE seed.track_id = ?1
            AND seed.playlist_id NOT IN (
                SELECT ts.playlist_id FROM hub_tag_sources ts
                  JOIN hub_group_tags gt ON gt.tag_id = ts.tag_id
                  JOIN hub_tag_groups g ON g.id = gt.group_id
                 WHERE g.role = 'rumpelkiste')
          GROUP BY hpt2.track_id
          ORDER BY shared DESC, users DESC, t.artists, t.title
          LIMIT ?2",
    )
    .bind(seed_track_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    Ok(rows
        .iter()
        .map(|r| Suggestion {
            id: r.get("tid"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
            shared_playlists: r.get::<Option<i64>, _>("shared").unwrap_or(0),
            users: r.get::<Option<i64>, _>("users").unwrap_or(0),
            user_slugs: r.get::<Option<String>, _>("user_slugs").unwrap_or_default(),
        })
        .collect())
}

/// Minimal seed info for the header.
pub struct Seed {
    pub id: i64,
    pub title: String,
    pub artists: String,
}

pub async fn load_seed(pool: &SqlitePool, id: i64) -> Result<Option<Seed>> {
    let row = sqlx::query("SELECT id, title, artists FROM hub_tracks WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    Ok(row.map(|r| Seed {
        id: r.get("id"),
        title: r.get::<Option<String>, _>("title").unwrap_or_default(),
        artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
    }))
}

/// How a track shows up in *our* data.
#[derive(Debug, Default, Clone)]
pub struct Presence {
    pub users: i64,
    pub playlists: i64,
    pub likes: i64,
    pub slugs: String,
}

/// Enrich a candidate: which users have it, in how many playlists, likes.
pub async fn presence(pool: &SqlitePool, track_id: i64) -> Presence {
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(DISTINCT user_id) FROM hub_v_track_playlists WHERE track_id = ?1) AS users,
            (SELECT COUNT(DISTINCT playlist_id) FROM hub_v_track_playlists WHERE track_id = ?1) AS playlists,
            (SELECT COUNT(*) FROM hub_liked_tracks WHERE track_id = ?1) AS likes,
            (SELECT GROUP_CONCAT(DISTINCT u.slug) FROM hub_v_track_playlists p
               JOIN hub_users u ON u.id = p.user_id WHERE p.track_id = ?1) AS slugs",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some(r) => Presence {
            users: r.get::<Option<i64>, _>("users").unwrap_or(0),
            playlists: r.get::<Option<i64>, _>("playlists").unwrap_or(0),
            likes: r.get::<Option<i64>, _>("likes").unwrap_or(0),
            slugs: r.get::<Option<String>, _>("slugs").unwrap_or_default(),
        },
        None => Presence::default(),
    }
}

/// Batched `presence` for many tracks at once — avoids the N+1 of one query per
/// candidate. Two chunked queries (playlists, likes) instead.
pub async fn presence_many(pool: &SqlitePool, ids: &[i64]) -> HashMap<i64, Presence> {
    let mut out: HashMap<i64, Presence> = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT p.track_id AS tid,
                    COUNT(DISTINCT p.user_id) AS users,
                    COUNT(DISTINCT p.playlist_id) AS playlists,
                    GROUP_CONCAT(DISTINCT u.slug) AS slugs
               FROM hub_v_track_playlists p
               JOIN hub_users u ON u.id = p.user_id
              WHERE p.track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") GROUP BY p.track_id");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            out.insert(
                tid,
                Presence {
                    users: r.get::<Option<i64>, _>("users").unwrap_or(0),
                    playlists: r.get::<Option<i64>, _>("playlists").unwrap_or(0),
                    likes: 0,
                    slugs: r.get::<Option<String>, _>("slugs").unwrap_or_default(),
                },
            );
        }

        let mut qb = QueryBuilder::new(
            "SELECT track_id AS tid, COUNT(*) AS n FROM hub_liked_tracks WHERE track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") GROUP BY track_id");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            out.entry(tid).or_default().likes = r.get::<i64, _>("n");
        }
    }
    out
}

/// A preloaded matcher: external candidates are resolved against in-memory maps
/// instead of one full-table scan per candidate (the old N×scan cost).
pub struct Matcher {
    by_spotify: HashMap<String, i64>,
    /// lower(trim(title)) -> [(track_id, lower(artists))]
    by_title: HashMap<String, Vec<(i64, String)>>,
}

impl Matcher {
    /// Load the matcher, pre-resolving the given Spotify ids in one query and the
    /// track titles/artists in a single pass.
    pub async fn load(pool: &SqlitePool, spotify_ids: &[String]) -> Matcher {
        let mut by_spotify: HashMap<String, i64> = HashMap::new();
        for chunk in spotify_ids.chunks(900) {
            let mut qb = QueryBuilder::new(
                "SELECT external_id, track_id FROM hub_track_external_ids
                  WHERE service = 'spotify' AND external_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id.clone());
            }
            qb.push(")");
            for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
                let sid: String = r.get("external_id");
                by_spotify.insert(sid, r.get("track_id"));
            }
        }

        let mut by_title: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        for r in sqlx::query("SELECT id, title, artists FROM hub_tracks")
            .fetch_all(pool)
            .await
            .unwrap_or_default()
        {
            let title = r
                .get::<Option<String>, _>("title")
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            if title.is_empty() {
                continue;
            }
            let artists = r
                .get::<Option<String>, _>("artists")
                .unwrap_or_default()
                .to_lowercase();
            by_title
                .entry(title)
                .or_default()
                .push((r.get("id"), artists));
        }

        Matcher {
            by_spotify,
            by_title,
        }
    }

    pub fn by_spotify(&self, sid: &str) -> Option<i64> {
        self.by_spotify.get(sid).copied()
    }

    /// Match by normalised title + first-artist substring (mirrors `match_track`).
    pub fn by_name(&self, artists: &str, title: &str) -> Option<i64> {
        let key = title.trim().to_lowercase();
        let list = self.by_title.get(&key)?;
        let first = artists
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if first.is_empty() {
            return list.first().map(|(id, _)| *id);
        }
        list.iter()
            .find(|(_, a)| a.contains(&first))
            .map(|(id, _)| *id)
    }
}

/// Resolve an external candidate (external suggestion) to a hub track id, by
/// Spotify id, then ISRC, then normalised title + artist.
pub async fn match_track(
    pool: &SqlitePool,
    spotify_id: Option<&str>,
    isrc: Option<&str>,
    artists: &str,
    title: &str,
) -> Option<i64> {
    if let Some(sid) = spotify_id.filter(|s| !s.is_empty()) {
        if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>(
            "SELECT track_id FROM hub_track_external_ids
              WHERE service = 'spotify' AND external_id = ?1 LIMIT 1",
        )
        .bind(sid)
        .fetch_optional(pool)
        .await
        {
            return Some(id);
        }
    }
    if let Some(code) = isrc.filter(|s| !s.is_empty()) {
        if let Ok(Some(id)) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM hub_tracks WHERE isrc = ?1 LIMIT 1")
                .bind(code)
                .fetch_optional(pool)
                .await
        {
            return Some(id);
        }
    }
    let nt = crate::tags::normalize_name(title);
    if !nt.is_empty() {
        let first_artist = artists.split(',').next().unwrap_or("").trim();
        if !first_artist.is_empty() {
            if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM hub_tracks
                  WHERE lower(trim(title)) = lower(trim(?1))
                    AND lower(artists) LIKE '%' || lower(?2) || '%'
                  LIMIT 1",
            )
            .bind(title)
            .bind(first_artist)
            .fetch_optional(pool)
            .await
            {
                return Some(id);
            }
        }
    }
    None
}
