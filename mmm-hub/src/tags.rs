//! Tag layer — an explicit, **per-user** layer above playlists and tracks.
//!
//! Unlike before, a tag is **not** auto-derived from every playlist. It exists
//! only once a user creates it, typically by promoting one of their playlists
//! to a tag (1:1, same name, linked by playlist id). Tracks in a tag's source
//! playlist are the tag's tracks; [`rebuild`] materializes that mapping into
//! `hub_track_resolved_tags`.

use anyhow::{Result, bail};
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

/// Meta-playlists can't become tags: they describe the library itself.
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

/// Rebuild the materialized `hub_track_resolved_tags` from the current tag
/// sources. Idempotent; does **not** create tags. Returns current counts.
pub async fn rebuild(pool: &SqlitePool) -> Result<ResolveSummary> {
    sqlx::query("DELETE FROM hub_track_resolved_tags")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO hub_track_resolved_tags (track_id, tag_id)
         SELECT DISTINCT hpt.track_id, ts.tag_id
           FROM hub_playlist_tracks hpt
           JOIN hub_tag_sources ts ON ts.playlist_id = hpt.playlist_id
         ON CONFLICT(track_id, tag_id) DO NOTHING",
    )
    .execute(pool)
    .await?;

    let tags: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tags")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tag_sources")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let resolved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_resolved_tags")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    Ok(ResolveSummary {
        tags: tags as usize,
        sources: sources as usize,
        resolved: resolved as usize,
    })
}

/// Create (or return) the tag for a playlist, owned by `owner_user_id`, linking
/// tag ↔ playlist 1:1. Rebuilds the track mapping. Returns the tag id.
pub async fn create_from_playlist(
    pool: &SqlitePool,
    owner_user_id: i64,
    playlist_id: i64,
) -> Result<i64> {
    let row = sqlx::query("SELECT name, user_id, service FROM hub_playlists WHERE id = ?1")
        .bind(playlist_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(r) = row else {
        bail!("playlist {playlist_id} not found");
    };
    let name = r.get::<Option<String>, _>("name").unwrap_or_default();
    if is_meta_playlist(&name) {
        bail!("meta-playlists can't become tags");
    }
    let slug = normalize_name(&name);
    if slug.is_empty() {
        bail!("playlist has no usable name");
    }
    let pl_user = r.get::<i64, _>("user_id");
    let service = r.get::<String, _>("service");
    let now = chrono::Utc::now().to_rfc3339();

    let tag_id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(owner_user_id, slug) DO UPDATE SET name = excluded.name
         RETURNING id",
    )
    .bind(owner_user_id)
    .bind(&slug)
    .bind(&name)
    .bind(&now)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(tag_id, playlist_id) DO NOTHING",
    )
    .bind(tag_id)
    .bind(playlist_id)
    .bind(pl_user)
    .bind(&service)
    .bind(&now)
    .execute(pool)
    .await?;

    rebuild(pool).await?;
    Ok(tag_id)
}

/// Delete a tag owned by `owner_user_id` (and its source links). Rebuilds.
pub async fn delete_tag(pool: &SqlitePool, owner_user_id: i64, tag_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
        .bind(tag_id)
        .bind(owner_user_id)
        .execute(pool)
        .await?;
    rebuild(pool).await?;
    Ok(())
}

/// The tag created for a playlist by a user, if any (for UI state).
pub async fn tag_for_playlist(
    pool: &SqlitePool,
    owner_user_id: i64,
    playlist_id: i64,
) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT t.id FROM hub_tags t
           JOIN hub_tag_sources s ON s.tag_id = t.id
          WHERE t.owner_user_id = ?1 AND s.playlist_id = ?2 LIMIT 1",
    )
    .bind(owner_user_id)
    .bind(playlist_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Load `(playlist owner user_id, service)` for a playlist.
async fn playlist_meta(pool: &SqlitePool, playlist_id: i64) -> Result<Option<(i64, String)>> {
    let row = sqlx::query("SELECT user_id, service FROM hub_playlists WHERE id = ?1")
        .bind(playlist_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    Ok(row.map(|r| (r.get::<i64, _>("user_id"), r.get::<String, _>("service"))))
}

/// Add a playlist as an additional source of an existing tag the user owns.
/// This is how a tag is *curated* from several playlists (own and/or followed).
pub async fn add_playlist_to_tag(
    pool: &SqlitePool,
    owner_user_id: i64,
    tag_id: i64,
    playlist_id: i64,
) -> Result<()> {
    let owned = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2",
    )
    .bind(tag_id)
    .bind(owner_user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if owned.is_none() {
        bail!("tag {tag_id} does not belong to you");
    }
    let Some((pl_user, service)) = playlist_meta(pool, playlist_id).await? else {
        bail!("playlist {playlist_id} not found");
    };
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(tag_id, playlist_id) DO NOTHING",
    )
    .bind(tag_id)
    .bind(playlist_id)
    .bind(pl_user)
    .bind(&service)
    .bind(&now)
    .execute(pool)
    .await?;
    rebuild(pool).await?;
    Ok(())
}

/// Remove a playlist as a source of a tag the user owns.
pub async fn remove_playlist_from_tag(
    pool: &SqlitePool,
    owner_user_id: i64,
    tag_id: i64,
    playlist_id: i64,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM hub_tag_sources
          WHERE tag_id = ?1 AND playlist_id = ?2
            AND tag_id IN (SELECT id FROM hub_tags WHERE owner_user_id = ?3)",
    )
    .bind(tag_id)
    .bind(playlist_id)
    .bind(owner_user_id)
    .execute(pool)
    .await?;
    rebuild(pool).await?;
    Ok(())
}

/// Tags owned by a user: `(id, name)`, for pickers.
pub async fn list_user_tags(pool: &SqlitePool, user_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT id, name FROM hub_tags WHERE owner_user_id = ?1 ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Tags a playlist currently feeds: `(tag_id, tag name, owner slug)`.
pub async fn tags_feeding_playlist(
    pool: &SqlitePool,
    playlist_id: i64,
) -> Vec<(i64, String, String)> {
    sqlx::query_as::<_, (i64, String, String)>(
        "SELECT t.id, t.name, u.slug FROM hub_tag_sources s
           JOIN hub_tags t ON t.id = s.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE s.playlist_id = ?1 ORDER BY u.slug, t.name",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
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
