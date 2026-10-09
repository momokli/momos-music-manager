//! Hub digging: seed-driven track discovery on the shared DB.
//!
//! Internal source only for now: tracks that co-occur with the seed in the same
//! playlists (across users), scored by how many playlists/users share them.
//! External sources (Last.fm, …) hang off the same view.

use anyhow::Result;
use sqlx::{Row, SqlitePool};

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
