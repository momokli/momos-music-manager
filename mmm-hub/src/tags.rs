//! Tag layer: playlists are *sources* that resolve to a normalized **tag**.
//!
//! Like Momo's Music Manager, a playlist is not itself a tag — it *feeds* one.
//! Several playlists (across users/services) can feed the same tag, and some
//! playlists are **meta** (e.g. a "liked songs" bucket) and are excluded.
//!
//! [`resolve`] rebuilds the materialized `hub_tags` / `hub_tag_sources` /
//! `hub_track_resolved_tags` from the current playlists. Idempotent.

use anyhow::Result;
use sqlx::{Row, SqlitePool};

/// Lowercase, keep alphanumerics, collapse everything else to single spaces.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Meta-playlists are not tags: they describe the library itself, not a theme.
pub fn is_meta_playlist(name: &str) -> bool {
    let n = normalize_name(name);
    n.is_empty()
        || n == "liked"
        || n == "likes"
        || n == "liked songs"
        || n.starts_with("liked ")
        || n.contains("my liked")
        || n.starts_with("all ")
}

#[derive(Debug, Default)]
pub struct ResolveSummary {
    pub tags: usize,
    pub sources: usize,
    pub resolved: usize,
}

/// Rebuild the tag layer from the current playlists. Idempotent (full rebuild).
pub async fn resolve(pool: &SqlitePool) -> Result<ResolveSummary> {
    // Full rebuild — cheap at our sizes and always consistent.
    sqlx::query("DELETE FROM hub_tag_sources").execute(pool).await?;
    sqlx::query("DELETE FROM hub_track_resolved_tags").execute(pool).await?;
    sqlx::query("DELETE FROM hub_tags").execute(pool).await?;

    let playlists = sqlx::query(
        "SELECT id, name, user_id, service FROM hub_playlists ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let now = chrono::Utc::now().to_rfc3339();
    let mut tag_ids: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut sources = 0usize;

    for p in &playlists {
        let name: String = p.get::<Option<String>, _>("name").unwrap_or_default();
        if is_meta_playlist(&name) {
            continue;
        }
        let slug = normalize_name(&name);
        let tag_id = match tag_ids.get(&slug) {
            Some(id) => *id,
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO hub_tags (slug, name, created_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(slug) DO UPDATE SET name = excluded.name RETURNING id",
                )
                .bind(&slug)
                .bind(&name)
                .bind(&now)
                .fetch_one(pool)
                .await?;
                tag_ids.insert(slug, id);
                id
            }
        };

        sqlx::query(
            "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(tag_id, playlist_id) DO NOTHING",
        )
        .bind(tag_id)
        .bind(p.get::<i64, _>("id"))
        .bind(p.get::<i64, _>("user_id"))
        .bind(p.get::<String, _>("service"))
        .execute(pool)
        .await?;
        sources += 1;
    }

    // tracks that are in a source playlist of a tag -> get that tag
    sqlx::query(
        "INSERT INTO hub_track_resolved_tags (track_id, tag_id)
         SELECT DISTINCT hpt.track_id, ts.tag_id
           FROM hub_playlist_tracks hpt
           JOIN hub_tag_sources ts ON ts.playlist_id = hpt.playlist_id
         ON CONFLICT(track_id, tag_id) DO NOTHING",
    )
    .execute(pool)
    .await?;

    let resolved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_resolved_tags")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    Ok(ResolveSummary {
        tags: tag_ids.len(),
        sources,
        resolved: resolved as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::{is_meta_playlist, normalize_name};

    #[test]
    fn normalisation() {
        assert_eq!(normalize_name("Deep  House!"), "deep house");
        assert_eq!(normalize_name("  Fusion / Bachstelzen "), "fusion bachstelzen");
        assert_eq!(normalize_name(""), "");
    }

    #[test]
    fn meta_detection() {
        assert!(is_meta_playlist("Liked Songs"));
        assert!(is_meta_playlist("Fusion 2025 | My liked Artists"));
        assert!(is_meta_playlist(""));
        assert!(!is_meta_playlist("Deep House"));
        assert!(!is_meta_playlist("Techno Collection"));
    }
}
