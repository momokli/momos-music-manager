//! Tag layer + groups.
//!
//! Tags are an explicit, **per-user** layer above playlists: a tag exists only
//! once a user creates it (usually by promoting a playlist), and can aggregate
//! several source playlists.
//!
//! **Groups** (formerly "categories") are per-user collections that any tag can
//! be put into — a tag can be in several groups (many-to-many). Groups have
//! membership **roles**: `owner` (created it), `contributor` (may add/remove
//! its tags), `subscriber` (just follows it). Groups are for granular sorting
//! and filtering.

use anyhow::{Result, bail};
use sqlx::{QueryBuilder, Row, SqlitePool};
use std::collections::HashSet;

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
/// sources. Idempotent; does **not** create tags.
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
    // Direct (manual) track↔tag links, independent of any playlist.
    sqlx::query(
        "INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id)
         SELECT DISTINCT track_id, tag_id FROM hub_track_tag_manual",
    )
    .execute(pool)
    .await?;
    // Archived (kept) links: tracks that left a keep_on_remove source playlist.
    sqlx::query(
        "INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id)
         SELECT DISTINCT track_id, tag_id FROM hub_track_tag_archive",
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

/// Tag a track directly (manual link owned by `user_id`). Idempotent.
pub async fn tag_track(pool: &SqlitePool, user_id: i64, track_id: i64, tag_id: i64) -> Result<()> {
    let owned =
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
            .bind(tag_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if owned.is_none() {
        bail!("tag not owned by user");
    }
    sqlx::query(
        "INSERT OR IGNORE INTO hub_track_tag_manual (track_id, tag_id, user_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(track_id)
    .bind(tag_id)
    .bind(user_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    rebuild(pool).await?;
    // Bi-way: mirror the tag onto opted-in (owned) source playlists.
    let _ = sync_tag_to_playlists(pool, user_id, tag_id, track_id).await;
    Ok(())
}

/// Remove a direct (manual) track↔tag link. Idempotent.
pub async fn untag_track(
    pool: &SqlitePool,
    user_id: i64,
    track_id: i64,
    tag_id: i64,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM hub_track_tag_manual WHERE track_id = ?1 AND tag_id = ?2 AND user_id = ?3",
    )
    .bind(track_id)
    .bind(tag_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    rebuild(pool).await?;
    Ok(())
}

/// Whether the manual link exists (for the queue's tag-toggle display).
pub async fn is_tagged_manually(
    pool: &SqlitePool,
    user_id: i64,
    track_id: i64,
    tag_id: i64,
) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_track_tag_manual WHERE track_id = ?1 AND tag_id = ?2 AND user_id = ?3",
    )
    .bind(track_id)
    .bind(tag_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// Set whether a tag keeps (archives) tracks that leave the source playlist.
pub async fn set_source_keep_on_remove(
    pool: &SqlitePool,
    actor_user_id: i64,
    tag_id: i64,
    playlist_id: i64,
    keep: bool,
) -> Result<()> {
    let owned =
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
            .bind(tag_id)
            .bind(actor_user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if owned.is_none() {
        bail!("tag not owned by user");
    }
    sqlx::query(
        "UPDATE hub_tag_sources SET keep_on_remove = ?1 WHERE tag_id = ?2 AND playlist_id = ?3",
    )
    .bind(keep as i64)
    .bind(tag_id)
    .bind(playlist_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Opt a tag into bi-way playlist sync (push tagged tracks to a linked playlist).
pub async fn set_tag_sync(
    pool: &SqlitePool,
    owner_user_id: i64,
    tag_id: i64,
    sync: bool,
) -> Result<()> {
    sqlx::query("UPDATE hub_tags SET sync_playlist = ?1 WHERE id = ?2 AND owner_user_id = ?3")
        .bind(sync as i64)
        .bind(tag_id)
        .bind(owner_user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Archive a tag link so it survives removal from a `keep_on_remove` playlist.
pub async fn archive_tag(
    pool: &SqlitePool,
    tag_id: i64,
    track_id: i64,
    source_playlist_id: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO hub_track_tag_archive
             (tag_id, track_id, source_playlist_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(tag_id)
    .bind(track_id)
    .bind(source_playlist_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    rebuild(pool).await?;
    Ok(())
}

/// Bi-way sync: when a track is tagged, add it to the (owned) source playlists
/// of tags that opted in via `sync_playlist`. Hub-side membership only; returns
/// the playlist ids that were updated.
pub async fn sync_tag_to_playlists(
    pool: &SqlitePool,
    user_id: i64,
    tag_id: i64,
    track_id: i64,
) -> Result<Vec<i64>> {
    let sync: i64 =
        sqlx::query_scalar("SELECT COALESCE(sync_playlist,0) FROM hub_tags WHERE id = ?1")
            .bind(tag_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
    if sync == 0 {
        return Ok(Vec::new());
    }
    let playlists: Vec<i64> = sqlx::query_scalar(
        "SELECT ts.playlist_id FROM hub_tag_sources ts
           JOIN hub_playlists hp ON hp.id = ts.playlist_id
          WHERE ts.tag_id = ?1 AND hp.user_id = ?2",
    )
    .bind(tag_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    for pl in &playlists {
        sqlx::query(
            "INSERT OR IGNORE INTO hub_playlist_tracks (playlist_id, track_id, added_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(pl)
        .bind(track_id)
        .bind(&now)
        .execute(pool)
        .await?;
    }
    Ok(playlists)
}

/// Create (or update) a tag owned by a user, without a source playlist.
pub async fn ensure_tag(pool: &SqlitePool, owner_user_id: i64, name: &str) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        bail!("tag name required");
    }
    let slug = normalize_name(name);
    if slug.is_empty() {
        bail!("tag name has no usable characters");
    }
    let now = chrono::Utc::now().to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(owner_user_id, slug) DO UPDATE SET name = excluded.name
         RETURNING id",
    )
    .bind(owner_user_id)
    .bind(&slug)
    .bind(name)
    .bind(&now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn playlist_meta(pool: &SqlitePool, playlist_id: i64) -> Result<Option<(i64, String)>> {
    let row = sqlx::query("SELECT user_id, service FROM hub_playlists WHERE id = ?1")
        .bind(playlist_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    Ok(row.map(|r| (r.get::<i64, _>("user_id"), r.get::<String, _>("service"))))
}

/// Create (or return) the tag for a playlist, owned by `owner_user_id`.
pub async fn create_from_playlist(
    pool: &SqlitePool,
    owner_user_id: i64,
    playlist_id: i64,
) -> Result<i64> {
    let row = sqlx::query("SELECT name FROM hub_playlists WHERE id = ?1")
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
    let tag_id = ensure_tag(pool, owner_user_id, &name).await?;
    add_playlist_to_tag(pool, owner_user_id, tag_id, playlist_id).await?;
    Ok(tag_id)
}

/// Add a playlist as a source of a tag the user owns.
pub async fn add_playlist_to_tag(
    pool: &SqlitePool,
    owner_user_id: i64,
    tag_id: i64,
    playlist_id: i64,
) -> Result<()> {
    link_playlist_to_tag(pool, owner_user_id, tag_id, playlist_id).await?;
    rebuild(pool).await?;
    Ok(())
}

/// Link a playlist to a tag **without** rebuilding the materialized resolved
/// tags — for bulk callers (e.g. the MMM import) that rebuild once at the end.
pub async fn link_playlist_to_tag(
    pool: &SqlitePool,
    owner_user_id: i64,
    tag_id: i64,
    playlist_id: i64,
) -> Result<()> {
    let owned =
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
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

pub async fn delete_tag(pool: &SqlitePool, owner_user_id: i64, tag_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
        .bind(tag_id)
        .bind(owner_user_id)
        .execute(pool)
        .await?;
    rebuild(pool).await?;
    Ok(())
}

/// Rename a tag (owner only). Sources are linked by id, so they stay intact.
pub async fn rename_tag(
    pool: &SqlitePool,
    owner_user_id: i64,
    tag_id: i64,
    name: &str,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("tag name required");
    }
    let slug = normalize_name(name);
    if slug.is_empty() {
        bail!("tag name has no usable characters");
    }
    let clash = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_tags WHERE owner_user_id = ?1 AND slug = ?2 AND id <> ?3",
    )
    .bind(owner_user_id)
    .bind(&slug)
    .bind(tag_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if clash.is_some() {
        bail!("es gibt schon einen Tag mit diesem Namen");
    }
    let rows = sqlx::query(
        "UPDATE hub_tags SET name = ?1, slug = ?2 WHERE id = ?3 AND owner_user_id = ?4",
    )
    .bind(name)
    .bind(&slug)
    .bind(tag_id)
    .bind(owner_user_id)
    .execute(pool)
    .await?
    .rows_affected();
    if rows == 0 {
        bail!("tag not found or not yours");
    }
    Ok(())
}

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

/// Tags owned by a user: `(id, name)` for pickers.
pub async fn list_user_tags(pool: &SqlitePool, user_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT id, name FROM hub_tags WHERE owner_user_id = ?1 ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Ids of tags (any owner) whose name contains `needle`, **plus all of their
/// descendant tags** (transitively), so filtering by a broad/parent tag also
/// matches its children (e.g. "house" -> "Beatport Top 100 - Progressive House").
/// Tags that co-occur with a track's tags ("how others tagged similar tracks"),
/// excluding tags already on the track. Returns `(tag_id, name, owner, score)`
/// ordered by co-occurrence count descending.
///
/// Relationship-based recommendation: aggregates candidate tags from several
/// neighbourhoods of the track, weighted by how strong the relation is:
///   * tracks sharing a tag with the seed        (weight 2)
///   * tracks in the same playlists as the seed  (weight 1)
///   * tracks by the same artist                 (weight 1)
///   * tracks on the same album                  (weight 1)
/// Tags already on the track are excluded; tags owned by any hub user count.
pub async fn recommended_tags(
    pool: &SqlitePool,
    track_id: i64,
    limit: i64,
) -> Vec<(i64, String, String, i64)> {
    let e = crate::settings::engine(pool).await;
    sqlx::query_as::<_, (i64, String, String, i64)>(
        "WITH s AS (SELECT ?1 AS id),
              seed_tags AS (SELECT tag_id FROM hub_track_resolved_tags WHERE track_id = (SELECT id FROM s)),
              cand AS (
                 SELECT rt2.tag_id AS tag_id, ?2 AS w
                   FROM hub_track_resolved_tags rt1
                   JOIN hub_track_resolved_tags rt2 ON rt2.track_id = rt1.track_id
                  WHERE rt1.tag_id IN (SELECT tag_id FROM seed_tags)
                    AND rt2.track_id <> (SELECT id FROM s)
                 UNION ALL
                 SELECT rt.tag_id, ?3
                   FROM hub_playlist_tracks p1
                   JOIN hub_playlist_tracks p2 ON p2.playlist_id = p1.playlist_id
                   JOIN hub_track_resolved_tags rt ON rt.track_id = p2.track_id
                  WHERE p1.track_id = (SELECT id FROM s) AND p2.track_id <> (SELECT id FROM s)
                 UNION ALL
                 SELECT rt.tag_id, ?4
                   FROM hub_tracks t1
                   JOIN hub_tracks t2 ON t2.artists = t1.artists AND t2.id <> t1.id
                   JOIN hub_track_resolved_tags rt ON rt.track_id = t2.id
                  WHERE t1.id = (SELECT id FROM s)
                    AND t1.artists IS NOT NULL AND TRIM(t1.artists) <> ''
                 UNION ALL
                 SELECT rt.tag_id, ?5
                   FROM hub_tracks a1
                   JOIN hub_tracks a2 ON a2.album = a1.album AND a2.id <> a1.id
                   JOIN hub_track_resolved_tags rt ON rt.track_id = a2.id
                  WHERE a1.id = (SELECT id FROM s)
                    AND a1.album IS NOT NULL AND TRIM(a1.album) <> ''
              )
         SELECT x.tag_id, t.name, u.slug, SUM(x.w) AS score
           FROM cand x
           JOIN hub_tags t ON t.id = x.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE x.tag_id NOT IN (SELECT tag_id FROM seed_tags)
          GROUP BY x.tag_id, t.name, u.slug
          ORDER BY score DESC, t.name
          LIMIT ?6",
    )
    .bind(track_id)
    .bind(e.rec_tag.round() as i64)
    .bind(e.rec_playlist.round() as i64)
    .bind(e.rec_artist.round() as i64)
    .bind(e.rec_album.round() as i64)
    .bind(limit)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// `(id, name)` of every tag this track carries (any owner) — for the tag form.
pub async fn tags_on_track(pool: &SqlitePool, track_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT t.id, t.name FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
          WHERE rt.track_id = ?1 ORDER BY t.name",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

pub async fn matching_tag_ids(pool: &SqlitePool, needle: &str) -> HashSet<i64> {
    let like = format!("%{}%", needle.to_lowercase());
    let roots: Vec<i64> = sqlx::query_scalar("SELECT id FROM hub_tags WHERE lower(name) LIKE ?1")
        .bind(&like)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let mut seen: HashSet<i64> = roots.iter().copied().collect();
    let mut frontier = roots;
    while !frontier.is_empty() {
        let mut qb =
            QueryBuilder::new("SELECT tag_id FROM hub_tag_parents WHERE parent_tag_id IN (");
        let mut sep = qb.separated(", ");
        for id in &frontier {
            sep.push_bind(*id);
        }
        qb.push(")");
        let mut next: Vec<i64> = Vec::new();
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let id: i64 = r.get("tag_id");
            if seen.insert(id) {
                next.push(id);
            }
        }
        frontier = next;
    }
    seen
}

/// Direct parent tags of a tag: `(id, name)`.
pub async fn tag_parents_of(pool: &SqlitePool, tag_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT p.id, p.name FROM hub_tag_parents tp JOIN hub_tags p ON p.id = tp.parent_tag_id
          WHERE tp.tag_id = ?1 ORDER BY p.name COLLATE NOCASE",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Direct child tags of a tag: `(id, name)`.
pub async fn tag_children_of(pool: &SqlitePool, tag_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT c.id, c.name FROM hub_tag_parents tp JOIN hub_tags c ON c.id = tp.tag_id
          WHERE tp.parent_tag_id = ?1 ORDER BY c.name COLLATE NOCASE",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// A tag set expanded with all ancestors and descendants (transitively) — used
/// for "parent match": a shared ancestor/descendant counts as related.
pub async fn related_tag_ids(pool: &SqlitePool, ids: &[i64]) -> HashSet<i64> {
    let mut seen: HashSet<i64> = ids.iter().copied().collect();
    let mut frontier: Vec<i64> = ids.to_vec();
    while !frontier.is_empty() {
        let mut qb =
            QueryBuilder::new("SELECT parent_tag_id AS id FROM hub_tag_parents WHERE tag_id IN (");
        {
            let mut sep = qb.separated(", ");
            for id in &frontier {
                sep.push_bind(*id);
            }
        }
        qb.push(") UNION SELECT tag_id AS id FROM hub_tag_parents WHERE parent_tag_id IN (");
        {
            let mut sep = qb.separated(", ");
            for id in &frontier {
                sep.push_bind(*id);
            }
        }
        qb.push(")");
        let mut next: Vec<i64> = Vec::new();
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let id: i64 = r.get("id");
            if seen.insert(id) {
                next.push(id);
            }
        }
        frontier = next;
    }
    seen
}

/// Add a parent relationship (both tags must exist). Idempotent.
pub async fn add_tag_parent(pool: &SqlitePool, tag_id: i64, parent_tag_id: i64) -> Result<()> {
    if tag_id == parent_tag_id {
        bail!("a tag cannot be its own parent");
    }
    sqlx::query(
        "INSERT OR IGNORE INTO hub_tag_parents (tag_id, parent_tag_id, created_at)
         VALUES (?1, ?2, ?3)",
    )
    .bind(tag_id)
    .bind(parent_tag_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a parent relationship. Idempotent.
pub async fn remove_tag_parent(pool: &SqlitePool, tag_id: i64, parent_tag_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_tag_parents WHERE tag_id = ?1 AND parent_tag_id = ?2")
        .bind(tag_id)
        .bind(parent_tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Case-insensitive lookup of any tag by exact name (any owner).
pub async fn find_tag_id_by_name(pool: &SqlitePool, name: &str) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM hub_tags WHERE lower(name) = lower(?1) ORDER BY id LIMIT 1",
    )
    .bind(name.trim())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// True when the tag sits in at least one group with semantic role `genre`.
pub async fn tag_is_genre(pool: &SqlitePool, tag_id: i64) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
          WHERE gt.tag_id = ?1 AND g.role = 'genre'",
    )
    .bind(tag_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
        > 0
}

/// Distinct tag owners (user slugs) — for the filter picker.
pub async fn tag_owners(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT u.slug FROM hub_tags t JOIN hub_users u ON u.id = t.owner_user_id
          ORDER BY u.slug",
    )
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

// ── groups ───────────────────────────────────────────────────────────────────

pub const ROLE_OWNER: &str = "owner";
pub const ROLE_CONTRIBUTOR: &str = "contributor";
pub const ROLE_SUBSCRIBER: &str = "subscriber";

/// Pickable group icons (emoji — no FontAwesome dependency).
pub const ICONS: &[&str] = &[
    "🎧", "🎵", "🔥", "💜", "✨", "#️⃣", "🌙", "⚡", "🌊", "🕺", "🎚️", "🚀", "🧊", "🌶️", "🪩", "🎯",
    "🌅", "🌈", "🧭", "🛸",
];

#[derive(Debug, Clone)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub owner: String,
    /// The caller's effective role in the group, or "" if none.
    pub role: String,
    /// True when the role comes from a parent (contributor) group.
    pub inherited: bool,
    pub tag_count: i64,
    pub members: i64,
    /// Importance weight (used by the scoring engine).
    pub weight: f64,
    /// `class` (classifying) or `sort` (sorting) group kind.
    pub kind: String,
    /// Semantic role (`""`, `rumpelkiste`, `setlist`, `genre`, `phase`).
    pub semantic_role: String,
}

#[derive(Debug)]
pub struct GroupDetail {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub owner: String,
    /// The caller's effective role (own or via the collective).
    pub role: String,
    /// True when `role` comes from a collective membership.
    pub role_inherited: bool,
    pub collective_id: i64,
    pub collective_name: String,
    pub collective_icon: String,
    pub weight: f64,
    pub ranked: bool,
    /// `class` (classifying) or `sort` (sorting) group kind.
    pub kind: String,
    /// Semantic role (`""`, `rumpelkiste`, `setlist`, `genre`, `phase`).
    pub semantic_role: String,
    pub members: Vec<(String, String)>,
    /// `(tag_id, tag name, tag owner slug, rank)`.
    pub tags: Vec<(i64, String, String, Option<i64>)>,
}

#[derive(Debug, Clone)]
pub struct Collective {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub owner: String,
    pub role: String,
    pub groups: i64,
    pub members: i64,
}

#[derive(Debug)]
pub struct CollectiveDetail {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub owner: String,
    pub role: String,
    pub is_owner: bool,
    pub members: Vec<(String, String)>,
    /// `(group_id, group name, group icon, tag count, weight)`.
    pub groups: Vec<(i64, String, String, i64, f64)>,
}

/// Infer `(kind, semantic_role)` from a well-known group name (case-insensitive).
pub fn infer_group_kind_role(name: &str) -> (&'static str, &'static str) {
    match name.trim().to_lowercase().as_str() {
        "rumpelkiste" | "rumpel" => ("sort", "rumpelkiste"),
        "setlist" | "setlists" => ("sort", "setlist"),
        "genre" | "genres" => ("class", "genre"),
        "phase" | "phase/energy" | "phase / energy" | "energy" => ("class", "phase"),
        _ => ("class", ""),
    }
}

/// Create a group owned by `owner_user_id` (idempotent on the slug); the owner
/// becomes its `owner` member.
pub async fn create_group(
    pool: &SqlitePool,
    owner_user_id: i64,
    name: &str,
    icon: &str,
) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        bail!("group name required");
    }
    let slug = normalize_name(name);
    if slug.is_empty() {
        bail!("group name has no usable characters");
    }
    let (kind, role) = infer_group_kind_role(name);
    let now = chrono::Utc::now().to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tag_groups (owner_user_id, name, slug, icon, sort_order, created_at, kind, role)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)
         ON CONFLICT(owner_user_id, slug) DO UPDATE SET name = excluded.name, icon = excluded.icon
         RETURNING id",
    )
    .bind(owner_user_id)
    .bind(name)
    .bind(&slug)
    .bind(icon)
    .bind(&now)
    .bind(kind)
    .bind(role)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT INTO hub_group_members (group_id, user_id, role, created_at)
         VALUES (?1, ?2, 'owner', ?3)
         ON CONFLICT(group_id, user_id) DO UPDATE SET role = 'owner'",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn delete_group(pool: &SqlitePool, actor_user_id: i64, group_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_tag_groups WHERE id = ?1 AND owner_user_id = ?2")
        .bind(group_id)
        .bind(actor_user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Rename a group and/or set its icon (owner only).
pub async fn update_group(
    pool: &SqlitePool,
    owner_user_id: i64,
    group_id: i64,
    name: &str,
    icon: &str,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("group name required");
    }
    let slug = normalize_name(name);
    if slug.is_empty() {
        bail!("group name has no usable characters");
    }
    let clash = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_tag_groups WHERE owner_user_id = ?1 AND slug = ?2 AND id <> ?3",
    )
    .bind(owner_user_id)
    .bind(&slug)
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if clash.is_some() {
        bail!("es gibt schon eine Gruppe mit diesem Namen");
    }
    let rows = sqlx::query(
        "UPDATE hub_tag_groups SET name = ?1, slug = ?2, icon = ?3\n          WHERE id = ?4 AND owner_user_id = ?5",
    )
    .bind(name)
    .bind(&slug)
    .bind(icon)
    .bind(group_id)
    .bind(owner_user_id)
    .execute(pool)
    .await?
    .rows_affected();
    if rows == 0 {
        bail!("group not found or not yours");
    }
    Ok(())
}

/// The user's role *in this group only* (no inheritance).
async fn own_role_of(pool: &SqlitePool, user_id: i64, group_id: i64) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT role FROM hub_group_members WHERE group_id = ?1 AND user_id = ?2",
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// The user's **effective** role in a group: their own role on the group, or —
/// if the group belongs to a collective the user is a member of — `contributor`.
pub async fn effective_role_of(pool: &SqlitePool, user_id: i64, group_id: i64) -> Option<String> {
    if let Some(r) = own_role_of(pool, user_id, group_id).await {
        return Some(r);
    }
    let in_collective = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_tag_groups g\n           JOIN hub_collective_members cm ON cm.collective_id = g.collective_id\n          WHERE g.id = ?1 AND cm.user_id = ?2 LIMIT 1",
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    in_collective.map(|_| ROLE_CONTRIBUTOR.to_string())
}

pub async fn can_contribute(pool: &SqlitePool, user_id: i64, group_id: i64) -> bool {
    matches!(
        effective_role_of(pool, user_id, group_id).await.as_deref(),
        Some(ROLE_OWNER) | Some(ROLE_CONTRIBUTOR)
    )
}

/// Groups the user can see as *theirs* — own membership plus everything
/// inherited from a parent (contributor) group.
pub async fn list_groups_for(pool: &SqlitePool, user_id: i64) -> Vec<Group> {
    let rows = sqlx::query(
        "SELECT g.id, g.name, g.icon, u.slug AS owner, g.weight,
                COALESCE(g.kind,'class') AS kind, COALESCE(g.role,'') AS semantic_role,
                (SELECT COUNT(*) FROM hub_group_tags gt WHERE gt.group_id = g.id) AS tag_count,
                (SELECT COUNT(*) FROM hub_group_members mm WHERE mm.group_id = g.id) AS members
           FROM hub_tag_groups g
           JOIN hub_users u ON u.id = g.owner_user_id
          ORDER BY g.name",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut out = Vec::new();
    for r in &rows {
        let id: i64 = r.get("id");
        if let Some(role) = effective_role_of(pool, user_id, id).await {
            let inherited = own_role_of(pool, user_id, id).await.is_none();
            out.push(Group {
                id,
                name: r.get("name"),
                icon: r.get::<Option<String>, _>("icon").unwrap_or_default(),
                owner: r.get("owner"),
                role,
                inherited,
                tag_count: r.get::<Option<i64>, _>("tag_count").unwrap_or(0),
                members: r.get::<Option<i64>, _>("members").unwrap_or(0),
                weight: r.get::<Option<f64>, _>("weight").unwrap_or(0.0),
                kind: r
                    .get::<Option<String>, _>("kind")
                    .unwrap_or_else(|| "class".into()),
                semantic_role: r
                    .get::<Option<String>, _>("semantic_role")
                    .unwrap_or_default(),
            });
        }
    }
    out
}

/// Groups the user is *not* a member of (to discover / subscribe to).
pub async fn list_discover_groups(pool: &SqlitePool, user_id: i64) -> Vec<Group> {
    let rows = sqlx::query(
        "SELECT g.id, g.name, g.icon, u.slug AS owner, g.weight,
                COALESCE(g.kind,'class') AS kind, COALESCE(g.role,'') AS semantic_role,
                (SELECT COUNT(*) FROM hub_group_tags gt WHERE gt.group_id = g.id) AS tag_count,
                (SELECT COUNT(*) FROM hub_group_members mm WHERE mm.group_id = g.id) AS members
           FROM hub_tag_groups g
           JOIN hub_users u ON u.id = g.owner_user_id
          WHERE NOT EXISTS (SELECT 1 FROM hub_group_members m
                             WHERE m.group_id = g.id AND m.user_id = ?1)
          ORDER BY u.slug, g.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| Group {
            id: r.get("id"),
            name: r.get("name"),
            icon: r.get::<Option<String>, _>("icon").unwrap_or_default(),
            owner: r.get("owner"),
            role: String::new(),
            inherited: false,
            tag_count: r.get::<Option<i64>, _>("tag_count").unwrap_or(0),
            members: r.get::<Option<i64>, _>("members").unwrap_or(0),
            weight: r.get::<Option<f64>, _>("weight").unwrap_or(0.0),
            kind: r
                .get::<Option<String>, _>("kind")
                .unwrap_or_else(|| "class".into()),
            semantic_role: r
                .get::<Option<String>, _>("semantic_role")
                .unwrap_or_default(),
        })
        .collect()
}

/// Subscribe (join as `subscriber`). No-op if already a member.
pub async fn subscribe(pool: &SqlitePool, user_id: i64, group_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_group_members (group_id, user_id, role, created_at)
         VALUES (?1, ?2, 'subscriber', ?3)
         ON CONFLICT(group_id, user_id) DO NOTHING",
    )
    .bind(group_id)
    .bind(user_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Leave a group (only if a plain subscriber — owners/contributors can't drop out).
pub async fn unsubscribe(pool: &SqlitePool, user_id: i64, group_id: i64) -> Result<()> {
    sqlx::query(
        "DELETE FROM hub_group_members
          WHERE group_id = ?1 AND user_id = ?2 AND role = 'subscriber'",
    )
    .bind(group_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Owner adds/promotes/demotes a member (role = contributor|subscriber).
pub async fn set_member_role(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    target_slug: &str,
    role: &str,
) -> Result<()> {
    if role != ROLE_CONTRIBUTOR && role != ROLE_SUBSCRIBER {
        bail!("invalid role");
    }
    let owner = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM hub_tag_groups WHERE id = ?1 AND owner_user_id = ?2",
    )
    .bind(group_id)
    .bind(actor_user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if owner.is_none() {
        bail!("only the group owner can manage members");
    }
    let target: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE LIMIT 1")
            .bind(target_slug.trim())
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let Some(target) = target else {
        bail!("user '{target_slug}' not found");
    };
    sqlx::query(
        "INSERT INTO hub_group_members (group_id, user_id, role, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(group_id, user_id) DO UPDATE SET role = excluded.role
           WHERE hub_group_members.role <> 'owner'",
    )
    .bind(group_id)
    .bind(target)
    .bind(role)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a member (owner only; can't remove the owner).
pub async fn remove_member(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    target_slug: &str,
) -> Result<()> {
    let owner = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM hub_tag_groups WHERE id = ?1 AND owner_user_id = ?2",
    )
    .bind(group_id)
    .bind(actor_user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if owner.is_none() {
        bail!("only the group owner can manage members");
    }
    sqlx::query(
        "DELETE FROM hub_group_members
          WHERE group_id = ?1 AND role <> 'owner'
            AND user_id = (SELECT id FROM hub_users WHERE slug = ?2 COLLATE NOCASE LIMIT 1)",
    )
    .bind(group_id)
    .bind(target_slug.trim())
    .execute(pool)
    .await?;
    Ok(())
}

/// Groups the user may add tags to (owner or contributor): `(id, name, icon)`.
pub async fn groups_i_contribute(pool: &SqlitePool, user_id: i64) -> Vec<(i64, String, String)> {
    sqlx::query_as::<_, (i64, String, String)>(
        "SELECT g.id, g.name, COALESCE(g.icon,'')
           FROM hub_tag_groups g
           JOIN hub_group_members m ON m.group_id = g.id
          WHERE m.user_id = ?1 AND m.role IN ('owner','contributor')
          ORDER BY g.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Put a tag into a group (actor must be owner/contributor of the group).
pub async fn add_tag_to_group(
    pool: &SqlitePool,
    actor_user_id: i64,
    tag_id: i64,
    group_id: i64,
) -> Result<()> {
    if !can_contribute(pool, actor_user_id, group_id).await {
        bail!("you can't contribute to this group");
    }
    sqlx::query(
        "INSERT INTO hub_group_tags (tag_id, group_id) VALUES (?1, ?2)
         ON CONFLICT(tag_id, group_id) DO NOTHING",
    )
    .bind(tag_id)
    .bind(group_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a tag from a group (actor must be owner/contributor of the group).
pub async fn remove_tag_from_group(
    pool: &SqlitePool,
    actor_user_id: i64,
    tag_id: i64,
    group_id: i64,
) -> Result<()> {
    if !can_contribute(pool, actor_user_id, group_id).await {
        bail!("you can't contribute to this group");
    }
    sqlx::query("DELETE FROM hub_group_tags WHERE tag_id = ?1 AND group_id = ?2")
        .bind(tag_id)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Groups a tag belongs to: `(id, name, icon)`.
pub async fn groups_for_tag(
    pool: &SqlitePool,
    tag_id: i64,
) -> Vec<(i64, String, String, Option<i64>, bool)> {
    sqlx::query_as::<_, (i64, String, String, Option<i64>, bool)>(
        "SELECT g.id, g.name, COALESCE(g.icon,''), gt.rank, COALESCE(g.ranked,0) AS ranked
           FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
          WHERE gt.tag_id = ?1 ORDER BY g.name",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

pub async fn group_detail(pool: &SqlitePool, user_id: i64, group_id: i64) -> Option<GroupDetail> {
    let row = sqlx::query(
        "SELECT g.id, g.name, COALESCE(g.icon,'') AS icon, u.slug AS owner,
                COALESCE(g.collective_id, 0) AS collective_id, g.weight,
                COALESCE(g.ranked, 0) AS ranked,
                COALESCE(g.kind,'class') AS kind, COALESCE(g.role,'') AS semantic_role
           FROM hub_tag_groups g JOIN hub_users u ON u.id = g.owner_user_id
          WHERE g.id = ?1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    let role = effective_role_of(pool, user_id, group_id)
        .await
        .unwrap_or_default();
    let role_inherited = !role.is_empty() && own_role_of(pool, user_id, group_id).await.is_none();
    let collective_id: i64 = row.get("collective_id");
    let (collective_name, collective_icon) = if collective_id != 0 {
        sqlx::query_as::<_, (String, String)>(
            "SELECT name, COALESCE(icon,'') FROM hub_collectives WHERE id = ?1",
        )
        .bind(collective_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
    } else {
        (String::new(), String::new())
    };
    let members = sqlx::query_as::<_, (String, String)>(
        "SELECT u.slug, m.role FROM hub_group_members m JOIN hub_users u ON u.id = m.user_id
          WHERE m.group_id = ?1 ORDER BY (m.role <> 'owner'), m.role, u.slug",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let tags = sqlx::query_as::<_, (i64, String, String, Option<i64>)>(
        "SELECT t.id, t.name, u.slug, gt.rank FROM hub_group_tags gt
           JOIN hub_tags t ON t.id = gt.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE gt.group_id = ?1 ORDER BY (gt.rank IS NULL), gt.rank, t.name",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    Some(GroupDetail {
        id: row.get("id"),
        name: row.get("name"),
        icon: row.get("icon"),
        owner: row.get("owner"),
        role,
        role_inherited,
        collective_id,
        collective_name,
        collective_icon,
        weight: row.get::<Option<f64>, _>("weight").unwrap_or(0.0),
        ranked: row.get::<Option<i64>, _>("ranked").unwrap_or(0) == 1,
        kind: row
            .get::<Option<String>, _>("kind")
            .unwrap_or_else(|| "class".into()),
        semantic_role: row
            .get::<Option<String>, _>("semantic_role")
            .unwrap_or_default(),
        members,
        tags,
    })
}

/// Semantic group roles recognized by the engine (`""` = none).
pub const GROUP_ROLES: &[&str] = &["", "rumpelkiste", "setlist", "genre", "phase"];

/// Update a group's kind (`class`|`sort`) and semantic role.
pub async fn set_group_kind_role(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    kind: &str,
    role: &str,
) -> Result<()> {
    if !can_contribute(pool, actor_user_id, group_id).await {
        bail!("you can't change this group");
    }
    let kind = if kind == "sort" { "sort" } else { "class" };
    let role = if GROUP_ROLES.contains(&role) {
        role
    } else {
        ""
    };
    sqlx::query("UPDATE hub_tag_groups SET kind = ?1, role = ?2 WHERE id = ?3")
        .bind(kind)
        .bind(role)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark/unmark a group as "ranked" (its tags can be ordered 1..5).
pub async fn set_group_ranked(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    ranked: bool,
) -> Result<()> {
    if !can_contribute(pool, actor_user_id, group_id).await {
        bail!("you can't change this group");
    }
    sqlx::query("UPDATE hub_tag_groups SET ranked = ?1 WHERE id = ?2")
        .bind(ranked as i64)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set (or clear, with `None`) a tag's rank within a ranked group (0..5).
pub async fn set_group_tag_rank(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    tag_id: i64,
    rank: Option<i64>,
) -> Result<()> {
    if !can_contribute(pool, actor_user_id, group_id).await {
        bail!("you can't change this group");
    }
    let rank = rank.filter(|r| (0..=5).contains(r));
    sqlx::query("UPDATE hub_group_tags SET rank = ?1 WHERE group_id = ?2 AND tag_id = ?3")
        .bind(rank)
        .bind(group_id)
        .bind(tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set a group's importance weight. Any contributor (owner or collective
/// member) may do this, so a collective can rank its groups.
pub async fn set_group_weight(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    weight: f64,
) -> Result<()> {
    if !can_contribute(pool, actor_user_id, group_id).await {
        bail!("you can't change this group's weight");
    }
    let weight = weight.clamp(0.0, 100.0);
    sqlx::query("UPDATE hub_tag_groups SET weight = ?1 WHERE id = ?2")
        .bind(weight)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set (or clear) the collective a group belongs to. The group's owner may do
/// this, and must be a member of the chosen collective.
pub async fn set_group_collective(
    pool: &SqlitePool,
    actor_user_id: i64,
    group_id: i64,
    collective_id: Option<i64>,
) -> Result<()> {
    let owner = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM hub_tag_groups WHERE id = ?1 AND owner_user_id = ?2",
    )
    .bind(group_id)
    .bind(actor_user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if owner.is_none() {
        bail!("only the group owner can set its collective");
    }
    if let Some(cid) = collective_id {
        let member = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM hub_collective_members WHERE collective_id = ?1 AND user_id = ?2",
        )
        .bind(cid)
        .bind(actor_user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if member.is_none() {
            bail!("you are not a member of that collective");
        }
    }
    sqlx::query("UPDATE hub_tag_groups SET collective_id = ?1 WHERE id = ?2")
        .bind(collective_id)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── collectives ──────────────────────────────────────────────────────────────

pub const COLLECTIVE_OWNER: &str = "owner";
pub const COLLECTIVE_MEMBER: &str = "member";

/// Create a collective owned by `owner_user_id` (the owner is a member).
pub async fn create_collective(
    pool: &SqlitePool,
    owner_user_id: i64,
    name: &str,
    icon: &str,
) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        bail!("collective name required");
    }
    let slug = normalize_name(name);
    if slug.is_empty() {
        bail!("collective name has no usable characters");
    }
    let now = chrono::Utc::now().to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_collectives (slug, name, icon, owner_user_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(slug) DO UPDATE SET name = excluded.name, icon = excluded.icon
         RETURNING id",
    )
    .bind(&slug)
    .bind(name)
    .bind(icon)
    .bind(owner_user_id)
    .bind(&now)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT INTO hub_collective_members (collective_id, user_id, role, created_at)
         VALUES (?1, ?2, 'owner', ?3)
         ON CONFLICT(collective_id, user_id) DO UPDATE SET role = 'owner'",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn collective_role_of(pool: &SqlitePool, user_id: i64, id: i64) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT role FROM hub_collective_members WHERE collective_id = ?1 AND user_id = ?2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

fn collective_from_row(r: &sqlx::sqlite::SqliteRow, role: String) -> Collective {
    Collective {
        id: r.get("id"),
        name: r.get("name"),
        icon: r.get::<Option<String>, _>("icon").unwrap_or_default(),
        owner: r.get("owner"),
        role,
        groups: r.get::<Option<i64>, _>("groups").unwrap_or(0),
        members: r.get::<Option<i64>, _>("members").unwrap_or(0),
    }
}

pub async fn list_collectives_for(pool: &SqlitePool, user_id: i64) -> Vec<Collective> {
    let rows = sqlx::query(
        "SELECT c.id, c.name, c.icon, u.slug AS owner, m.role,
                (SELECT COUNT(*) FROM hub_tag_groups g WHERE g.collective_id = c.id) AS groups,
                (SELECT COUNT(*) FROM hub_collective_members mm WHERE mm.collective_id = c.id) AS members
           FROM hub_collectives c
           JOIN hub_collective_members m ON m.collective_id = c.id AND m.user_id = ?1
           JOIN hub_users u ON u.id = c.owner_user_id
          ORDER BY c.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| collective_from_row(r, r.get::<String, _>("role")))
        .collect()
}

pub async fn list_discover_collectives(pool: &SqlitePool, user_id: i64) -> Vec<Collective> {
    let rows = sqlx::query(
        "SELECT c.id, c.name, c.icon, u.slug AS owner,
                (SELECT COUNT(*) FROM hub_tag_groups g WHERE g.collective_id = c.id) AS groups,
                (SELECT COUNT(*) FROM hub_collective_members mm WHERE mm.collective_id = c.id) AS members
           FROM hub_collectives c
           JOIN hub_users u ON u.id = c.owner_user_id
          WHERE NOT EXISTS (SELECT 1 FROM hub_collective_members m
                             WHERE m.collective_id = c.id AND m.user_id = ?1)
          ORDER BY u.slug, c.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| collective_from_row(r, String::new()))
        .collect()
}

pub async fn join_collective(pool: &SqlitePool, user_id: i64, id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_collective_members (collective_id, user_id, role, created_at)
         VALUES (?1, ?2, 'member', ?3)
         ON CONFLICT(collective_id, user_id) DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn leave_collective(pool: &SqlitePool, user_id: i64, id: i64) -> Result<()> {
    sqlx::query(
        "DELETE FROM hub_collective_members
          WHERE collective_id = ?1 AND user_id = ?2 AND role = 'member'",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Collective owner sets/removes a member (role = owner|member).
pub async fn set_collective_member(
    pool: &SqlitePool,
    actor_user_id: i64,
    id: i64,
    target_slug: &str,
    role: &str,
) -> Result<()> {
    if role != COLLECTIVE_OWNER && role != COLLECTIVE_MEMBER {
        bail!("invalid role");
    }
    if collective_role_of(pool, actor_user_id, id).await.as_deref() != Some(COLLECTIVE_OWNER) {
        bail!("only the collective owner can manage members");
    }
    let target: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE LIMIT 1")
            .bind(target_slug.trim())
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let Some(target) = target else {
        bail!("user '{target_slug}' not found");
    };
    sqlx::query(
        "INSERT INTO hub_collective_members (collective_id, user_id, role, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(collective_id, user_id) DO UPDATE SET role = excluded.role
           WHERE hub_collective_members.role <> 'owner'",
    )
    .bind(id)
    .bind(target)
    .bind(role)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_collective_member(
    pool: &SqlitePool,
    actor_user_id: i64,
    id: i64,
    target_slug: &str,
) -> Result<()> {
    if collective_role_of(pool, actor_user_id, id).await.as_deref() != Some(COLLECTIVE_OWNER) {
        bail!("only the collective owner can manage members");
    }
    sqlx::query(
        "DELETE FROM hub_collective_members
          WHERE collective_id = ?1 AND role <> 'owner'
            AND user_id = (SELECT id FROM hub_users WHERE slug = ?2 COLLATE NOCASE LIMIT 1)",
    )
    .bind(id)
    .bind(target_slug.trim())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_collective(pool: &SqlitePool, actor_user_id: i64, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_collectives WHERE id = ?1 AND owner_user_id = ?2")
        .bind(id)
        .bind(actor_user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Rename a collective and/or set its icon (owner only).
pub async fn update_collective(
    pool: &SqlitePool,
    owner_user_id: i64,
    id: i64,
    name: &str,
    icon: &str,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("collective name required");
    }
    let slug = normalize_name(name);
    if slug.is_empty() {
        bail!("collective name has no usable characters");
    }
    let clash =
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM hub_collectives WHERE slug = ?1 AND id <> ?2")
            .bind(&slug)
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if clash.is_some() {
        bail!("es gibt schon ein Collective mit diesem Namen");
    }
    let rows = sqlx::query(
        "UPDATE hub_collectives SET name = ?1, slug = ?2, icon = ?3\n          WHERE id = ?4 AND owner_user_id = ?5",
    )
    .bind(name)
    .bind(&slug)
    .bind(icon)
    .bind(id)
    .bind(owner_user_id)
    .execute(pool)
    .await?
    .rows_affected();
    if rows == 0 {
        bail!("collective not found or not yours");
    }
    Ok(())
}

/// Collectives the user is a member of: `(id, name, icon)` for pickers.
pub async fn collectives_i_belong(pool: &SqlitePool, user_id: i64) -> Vec<(i64, String, String)> {
    sqlx::query_as::<_, (i64, String, String)>(
        "SELECT c.id, c.name, COALESCE(c.icon,'') FROM hub_collectives c
           JOIN hub_collective_members m ON m.collective_id = c.id
          WHERE m.user_id = ?1 ORDER BY c.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

pub async fn collective_detail(
    pool: &SqlitePool,
    user_id: i64,
    id: i64,
) -> Option<CollectiveDetail> {
    let row = sqlx::query(
        "SELECT c.id, c.name, COALESCE(c.icon,'') AS icon, u.slug AS owner
           FROM hub_collectives c JOIN hub_users u ON u.id = c.owner_user_id
          WHERE c.id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    let role = collective_role_of(pool, user_id, id)
        .await
        .unwrap_or_default();
    let is_owner = role == COLLECTIVE_OWNER;
    let members = sqlx::query_as::<_, (String, String)>(
        "SELECT u.slug, m.role FROM hub_collective_members m JOIN hub_users u ON u.id = m.user_id
          WHERE m.collective_id = ?1 ORDER BY (m.role <> 'owner'), m.role, u.slug",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let groups = sqlx::query_as::<_, (i64, String, String, i64, f64)>(
        "SELECT g.id, g.name, COALESCE(g.icon,''),
                (SELECT COUNT(*) FROM hub_group_tags gt WHERE gt.group_id = g.id), g.weight
           FROM hub_tag_groups g WHERE g.collective_id = ?1 ORDER BY g.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    Some(CollectiveDetail {
        id: row.get("id"),
        name: row.get("name"),
        icon: row.get("icon"),
        owner: row.get("owner"),
        role,
        is_owner,
        members,
        groups,
    })
}

// ── tag detail ───────────────────────────────────────────────────────────────

pub struct TagDetail {
    pub id: i64,
    pub name: String,
    pub owner: String,
    /// `(group_id, group name, group icon, rank, group_ranked)`.
    pub groups: Vec<(i64, String, String, Option<i64>, bool)>,
    pub source_count: i64,
    pub tracks: Vec<(i64, String, String)>,
}

pub async fn tag_detail(pool: &SqlitePool, tag_id: i64) -> Option<TagDetail> {
    let row = sqlx::query(
        "SELECT t.id, t.name, u.slug AS owner,
                (SELECT COUNT(*) FROM hub_tag_sources s WHERE s.tag_id = t.id) AS source_count
           FROM hub_tags t JOIN hub_users u ON u.id = t.owner_user_id
          WHERE t.id = ?1",
    )
    .bind(tag_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    let tracks = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT tr.id, COALESCE(tr.title,''), COALESCE(tr.artists,'')
           FROM hub_track_resolved_tags r JOIN hub_tracks tr ON tr.id = r.track_id
          WHERE r.tag_id = ?1 ORDER BY tr.artists, tr.title LIMIT 1000",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    Some(TagDetail {
        id: row.get("id"),
        name: row.get("name"),
        owner: row.get("owner"),
        groups: groups_for_tag(pool, tag_id).await,
        source_count: row.get("source_count"),
        tracks,
    })
}

/// One other tag that co-occurs with a seed tag on the same tracks.
#[derive(Debug, Clone)]
pub struct TagCooccurrence {
    pub tag_id: i64,
    pub name: String,
    /// A group of the other tag (a group shared with the seed tag if one exists).
    pub group: String,
    /// True when the other tag shares at least one group with the seed tag.
    pub same_group: bool,
    /// Lift = P(A,B) / (P(A)·P(B)).
    pub lift: f64,
    /// Jaccard = |A∩B| / |A∪B| over the tracks carrying either tag.
    pub jaccard: f64,
    /// Tracks carrying both tags.
    pub both: i64,
    /// Tracks carrying the other tag (its total support).
    pub support: i64,
    /// One shared track id (digging seed link); `0` when none.
    pub sample_track_id: i64,
}

/// Top artists for a tag, by number of tracks tagged with it.
///
/// Returns `(artist, tagged_tracks_by_artist, tracks_with_tag, tracks_by_artist)`,
/// descending by the artist's tagged-track count (max 10 rows). Artist identity
/// is the raw `hub_tracks.artists` string (blank values collapse to `—`).
pub async fn tag_top_artists(pool: &SqlitePool, tag_id: i64) -> Vec<(String, i64, i64, i64)> {
    sqlx::query_as::<_, (String, i64, i64, i64)>(
        "WITH tagged AS (
             SELECT r.track_id AS track_id,
                    COALESCE(NULLIF(TRIM(t.artists), ''), '—') AS artist
               FROM hub_track_resolved_tags r
               JOIN hub_tracks t ON t.id = r.track_id
              WHERE r.tag_id = ?1
         ),
         per_artist AS (
             SELECT artist, COUNT(DISTINCT track_id) AS c
               FROM tagged GROUP BY artist
         ),
         artist_total AS (
             SELECT COALESCE(NULLIF(TRIM(artists), ''), '—') AS artist, COUNT(*) AS c
               FROM hub_tracks GROUP BY 1
         ),
         total AS (SELECT COUNT(DISTINCT track_id) AS c FROM tagged)
         SELECT pa.artist,
                pa.c,
                (SELECT c FROM total) AS tracks_with_tag,
                COALESCE(at.c, pa.c) AS tracks_by_artist
           FROM per_artist pa
           LEFT JOIN artist_total at ON at.artist = pa.artist
          ORDER BY pa.c DESC, pa.artist ASC
          LIMIT 10",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Tags that co-occur with `tag_id` on the same tracks, within and across
/// groups. Lift = P(A,B) / (P(A)·P(B)); restricted to other tags carrying at
/// least `min_support` tracks. At most 40 rows are returned (the caller trims
/// further); they are ranked by the configured co-occurrence metric
/// (`engine_cooc_metric`, default `lift`).
pub async fn tag_cooccurrence(
    pool: &SqlitePool,
    tag_id: i64,
    min_support: i64,
) -> Vec<TagCooccurrence> {
    let with_tag: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_resolved_tags WHERE tag_id = ?1")
            .bind(tag_id)
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    if with_tag == 0 {
        return Vec::new();
    }

    let pairs = sqlx::query_as::<_, (i64, String, i64, i64, f64)>(
        "WITH a AS (
             SELECT track_id FROM hub_track_resolved_tags WHERE tag_id = ?1
         ),
         total AS (SELECT COUNT(*) AS n FROM hub_tracks),
         sizes AS (
             SELECT tag_id, COUNT(*) AS c FROM hub_track_resolved_tags GROUP BY tag_id
         ),
         pairs AS (
             SELECT r.tag_id AS other_id, COUNT(DISTINCT r.track_id) AS both
               FROM hub_track_resolved_tags r
               JOIN a ON a.track_id = r.track_id
              WHERE r.tag_id <> ?1
              GROUP BY r.tag_id
         )
         SELECT p.other_id,
                t.name,
                p.both,
                s.c AS support,
                CAST(p.both AS REAL) * (SELECT n FROM total)
                  / ((SELECT COUNT(*) FROM a) * s.c) AS lift
           FROM pairs p
           JOIN hub_tags t ON t.id = p.other_id
           JOIN sizes s ON s.tag_id = p.other_id
          WHERE s.c >= ?2
          ORDER BY lift DESC, p.both DESC, t.name ASC
          LIMIT 40",
    )
    .bind(tag_id)
    .bind(min_support)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if pairs.is_empty() {
        return Vec::new();
    }

    let ids: Vec<i64> = pairs.iter().map(|p| p.0).collect();

    // Groups of the seed tag.
    let seed_groups: std::collections::HashSet<i64> =
        sqlx::query_scalar("SELECT group_id FROM hub_group_tags WHERE tag_id = ?1")
            .bind(tag_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

    // Groups of the candidate tags (batched).
    let mut qb = QueryBuilder::new("SELECT tag_id, group_id FROM hub_group_tags WHERE tag_id IN (");
    {
        let mut sep = qb.separated(", ");
        for id in &ids {
            sep.push_bind(*id);
        }
    }
    qb.push(")");
    let mut cand_groups: std::collections::HashMap<i64, Vec<i64>> =
        std::collections::HashMap::new();
    for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
        let tid: i64 = r.get("tag_id");
        let gid: i64 = r.get("group_id");
        cand_groups.entry(tid).or_default().push(gid);
    }

    // Group names.
    let mut gnames: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for r in sqlx::query("SELECT id, name FROM hub_tag_groups")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
    {
        gnames.insert(r.get("id"), r.get("name"));
    }

    // One shared track per candidate (batched).
    let mut qb = QueryBuilder::new(
        "SELECT r.tag_id AS other, MIN(r.track_id) AS sample
           FROM hub_track_resolved_tags r
           JOIN hub_track_resolved_tags a
             ON a.track_id = r.track_id AND a.tag_id = ",
    );
    qb.push_bind(tag_id);
    qb.push(" WHERE r.tag_id IN (");
    {
        let mut sep = qb.separated(", ");
        for id in &ids {
            sep.push_bind(*id);
        }
    }
    qb.push(") GROUP BY r.tag_id");
    let mut samples: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
        samples.insert(r.get("other"), r.get("sample"));
    }

    let empty: Vec<i64> = Vec::new();
    let mut out: Vec<TagCooccurrence> = pairs
        .into_iter()
        .map(|(other_id, name, both, support, lift)| {
            let cgroups = cand_groups.get(&other_id).unwrap_or(&empty);
            let shared: Vec<i64> = cgroups
                .iter()
                .copied()
                .filter(|g| seed_groups.contains(g))
                .collect();
            let (same_group, group) = if let Some(&g) = shared.iter().min() {
                (true, gnames.get(&g).cloned().unwrap_or_default())
            } else if let Some(&g) = cgroups.iter().min() {
                (false, gnames.get(&g).cloned().unwrap_or_default())
            } else {
                (false, String::new())
            };
            let union = (with_tag + support - both).max(1) as f64;
            TagCooccurrence {
                tag_id: other_id,
                name,
                group,
                same_group,
                lift,
                jaccard: both as f64 / union,
                both,
                support,
                sample_track_id: samples.get(&other_id).copied().unwrap_or(0),
            }
        })
        .collect();

    // Rank by the configured metric (#217).
    let metric = crate::settings::get(pool, crate::settings::ENGINE_COOC_METRIC)
        .await
        .map(|v| v.trim().to_lowercase())
        .unwrap_or_else(|| "lift".to_string());
    if metric == "jaccard" {
        out.sort_by(|a, b| {
            b.jaccard
                .partial_cmp(&a.jaccard)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.both.cmp(&a.both))
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{is_meta_playlist, normalize_name};

    #[test]
    fn normalisation() {
        assert_eq!(normalize_name("Deep  House!"), "deep house");
        assert_eq!(
            normalize_name("  Fusion / Bachstelzen "),
            "fusion bachstelzen"
        );
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
