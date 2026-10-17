//! Tag layer + collectives.
//!
//! Tags are an explicit, **per-user** layer above playlists: a tag exists only
//! once a user creates it (usually by promoting a playlist), and can aggregate
//! several source playlists. Every tag has exactly one **type**
//! ([`crate::tagtypes`]) — the fixed classification that replaced the former
//! flexible group subsystem.
//!
//! **Collectives** are sets of users that collaborate on tags.

use anyhow::{Result, bail};
use sqlx::{QueryBuilder, Row, SqlitePool};
use std::collections::HashSet;

use crate::config::Config;

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
pub async fn tag_track(
    pool: &SqlitePool,
    cfg: &Config,
    user_id: i64,
    track_id: i64,
    tag_id: i64,
) -> Result<()> {
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
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO hub_track_tag_manual (track_id, tag_id, user_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(track_id)
    .bind(tag_id)
    .bind(user_id)
    .bind(&now)
    .execute(pool)
    .await?;
    // History (#296): open a membership interval if none is open.
    sqlx::query(
        "INSERT INTO hub_tag_membership_history (track_id, tag_id, added_at, removed_at)
         SELECT ?1, ?2, ?3, NULL
          WHERE NOT EXISTS (SELECT 1 FROM hub_tag_membership_history
                             WHERE track_id = ?1 AND tag_id = ?2 AND removed_at IS NULL)",
    )
    .bind(track_id)
    .bind(tag_id)
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE hub_tracks SET last_tagged_at = ?2 WHERE id = ?1")
        .bind(track_id)
        .bind(&now)
        .execute(pool)
        .await?;
    rebuild(pool).await?;
    // Bi-way: mirror the tag onto opted-in (owned) source playlists.
    let _ = sync_tag_to_playlists(pool, cfg, user_id, tag_id, track_id).await;
    Ok(())
}

/// Remove a direct (manual) track↔tag link. Idempotent.
pub async fn untag_track(
    pool: &SqlitePool,
    cfg: &Config,
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
    sqlx::query(
        "UPDATE hub_tag_membership_history SET removed_at = ?3
          WHERE track_id = ?1 AND tag_id = ?2 AND removed_at IS NULL",
    )
    .bind(track_id)
    .bind(tag_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    rebuild(pool).await?;
    // Bi-way: remove the track from opted-in (owned) source playlists on Spotify.
    let _ = unsync_tag_from_playlists(pool, cfg, user_id, tag_id, track_id).await;
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

/// A source playlist eligible for a bi-way push (#283).
struct PushSource {
    playlist_id: i64,
    remote_id: String,
    is_owned: bool,
    collaborative: bool,
    keep_on_remove: bool,
}

/// Source playlists of a tag owned by `user_id` (Spotify only).
async fn push_sources(pool: &SqlitePool, user_id: i64, tag_id: i64) -> Vec<PushSource> {
    let rows = sqlx::query(
        "SELECT ts.playlist_id AS hub_id,
                COALESCE(hp.playlist_id, '') AS remote_id,
                COALESCE(hp.is_owned, 0) AS is_owned,
                COALESCE(hp.collaborative, 0) AS collaborative,
                COALESCE(ts.keep_on_remove, 0) AS keep_on_remove
           FROM hub_tag_sources ts
           JOIN hub_playlists hp ON hp.id = ts.playlist_id
          WHERE ts.tag_id = ?1 AND hp.user_id = ?2 AND hp.service = 'spotify'",
    )
    .bind(tag_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| PushSource {
            playlist_id: r.get("hub_id"),
            remote_id: r.get("remote_id"),
            is_owned: r.get::<i64, _>("is_owned") != 0,
            collaborative: r.get::<i64, _>("collaborative") != 0,
            keep_on_remove: r.get::<i64, _>("keep_on_remove") != 0,
        })
        .collect()
}

/// Shorten an error message for storage in `last_push_error`.
fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Persist the outcome of the last push attempt for one playlist.
async fn record_push(
    pool: &SqlitePool,
    playlist_id: i64,
    status: &str,
    error: Option<&str>,
    action: &str,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let _ = sqlx::query(
        "UPDATE hub_playlists
            SET last_push_at = ?2, last_push_status = ?3,
                last_push_error = ?4, last_push_action = ?5
          WHERE id = ?1",
    )
    .bind(playlist_id)
    .bind(&now)
    .bind(status)
    .bind(error)
    .bind(action)
    .execute(pool)
    .await;
}

/// Bi-way sync (add): when a track is tagged, mirror it into the (owned)
/// source playlists of tags that opted in via `sync_playlist` — hub-side AND on
/// Spotify (`POST /playlists/{id}/items`, idempotent). Returns the playlist ids
/// that were considered.
pub async fn sync_tag_to_playlists(
    pool: &SqlitePool,
    cfg: &Config,
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
    let sources = push_sources(pool, user_id, tag_id).await;
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let now = chrono::Utc::now().to_rfc3339();
    let scope_ok = crate::curate::has_write_scope(pool, user_id).await;
    let uri = crate::curate::spotify_uris(pool, &[track_id])
        .await?
        .into_iter()
        .next();
    let mut updated = Vec::new();
    let mut token: Option<String> = None;
    for s in &sources {
        updated.push(s.playlist_id);
        // Hub-side membership (existing behaviour) — also tells us whether the
        // track was already known to be in the playlist (idempotency).
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO hub_playlist_tracks (playlist_id, track_id, added_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(s.playlist_id)
        .bind(track_id)
        .bind(&now)
        .execute(pool)
        .await?
        .rows_affected()
            > 0;
        if !(s.is_owned || s.collaborative) {
            record_push(pool, s.playlist_id, "skipped", None, "add").await;
            continue;
        }
        let Some(uri) = uri.as_deref() else {
            record_push(pool, s.playlist_id, "skipped", Some("kein Spotify-Track"), "add").await;
            continue;
        };
        if s.remote_id.is_empty() {
            record_push(
                pool,
                s.playlist_id,
                "error",
                Some("keine Spotify-Playlist-ID"),
                "add",
            )
            .await;
            continue;
        }
        if !scope_ok {
            record_push(pool, s.playlist_id, "scope_missing", None, "add").await;
            continue;
        }
        if !inserted {
            // Already mirrored before this call → already on Spotify.
            record_push(pool, s.playlist_id, "ok", None, "add").await;
            continue;
        }
        if token.is_none() {
            match crate::ingest::access_token(pool, cfg, user_id).await {
                Ok(t) => token = Some(t),
                Err(e) => {
                    record_push(pool, s.playlist_id, "error", Some(&short(&e.to_string())), "add")
                        .await;
                    continue;
                }
            }
        }
        let tok = token.as_deref().unwrap();
        let uris = vec![uri.to_string()];
        match crate::curate::spotify_add_uris(&cfg.spotify_api_base, tok, &s.remote_id, &uris).await
        {
            Ok(()) => record_push(pool, s.playlist_id, "ok", None, "add").await,
            Err(e) => {
                record_push(pool, s.playlist_id, "error", Some(&short(&e.to_string())), "add").await
            }
        }
    }
    Ok(updated)
}

/// Bi-way sync (remove): when a manual tag is removed, drop the track from the
/// (owned) source playlists on Spotify (`DELETE /playlists/{id}/items`). A source
/// with `keep_on_remove = 1` is *archived* instead and never removed from Spotify.
pub async fn unsync_tag_from_playlists(
    pool: &SqlitePool,
    cfg: &Config,
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
    let sources = push_sources(pool, user_id, tag_id).await;
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let scope_ok = crate::curate::has_write_scope(pool, user_id).await;
    let uri = crate::curate::spotify_uris(pool, &[track_id])
        .await?
        .into_iter()
        .next();
    let mut updated = Vec::new();
    let mut token: Option<String> = None;
    for s in &sources {
        updated.push(s.playlist_id);
        // keep_on_remove: keep the tag link (archive), do NOT touch Spotify.
        if s.keep_on_remove {
            let _ = archive_tag(pool, tag_id, track_id, Some(s.playlist_id)).await;
            record_push(pool, s.playlist_id, "skipped", Some("keep_on_remove"), "remove").await;
            continue;
        }
        let in_playlist = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
        )
        .bind(s.playlist_id)
        .bind(track_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some();
        if !in_playlist {
            record_push(pool, s.playlist_id, "skipped", Some("nicht in Playlist"), "remove").await;
            continue;
        }
        if !(s.is_owned || s.collaborative) {
            record_push(pool, s.playlist_id, "skipped", None, "remove").await;
            continue;
        }
        let Some(uri) = uri.as_deref() else {
            let _ = sqlx::query(
                "DELETE FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
            )
            .bind(s.playlist_id)
            .bind(track_id)
            .execute(pool)
            .await;
            record_push(pool, s.playlist_id, "skipped", Some("kein Spotify-Track"), "remove")
                .await;
            continue;
        };
        if s.remote_id.is_empty() {
            record_push(
                pool,
                s.playlist_id,
                "error",
                Some("keine Spotify-Playlist-ID"),
                "remove",
            )
            .await;
            continue;
        }
        if !scope_ok {
            record_push(pool, s.playlist_id, "scope_missing", None, "remove").await;
            continue;
        }
        if token.is_none() {
            match crate::ingest::access_token(pool, cfg, user_id).await {
                Ok(t) => token = Some(t),
                Err(e) => {
                    record_push(pool, s.playlist_id, "error", Some(&short(&e.to_string())), "remove")
                        .await;
                    continue;
                }
            }
        }
        let tok = token.as_deref().unwrap();
        let uris = vec![uri.to_string()];
        match crate::curate::spotify_remove_uris(&cfg.spotify_api_base, tok, &s.remote_id, &uris)
            .await
        {
            Ok(()) => {
                let _ = sqlx::query(
                    "DELETE FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
                )
                .bind(s.playlist_id)
                .bind(track_id)
                .execute(pool)
                .await;
                record_push(pool, s.playlist_id, "ok", None, "remove").await;
            }
            Err(e) => {
                record_push(pool, s.playlist_id, "error", Some(&short(&e.to_string())), "remove")
                    .await
            }
        }
    }
    Ok(updated)
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

/// One ranked tag-search hit (for the unified tag input, #449).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagHit {
    pub id: i64,
    pub name: String,
    /// Raw type (e.g. `MOOD`), if any.
    pub type_name: Option<String>,
    /// Display label (e.g. `Mood`), empty when untyped.
    pub type_label: String,
    /// Icon for the type, empty when untyped.
    pub icon: String,
}

/// Rank a tag name against a (lowercased) needle. Higher = better; `None` = no match.
/// Exact > prefix > word-prefix > substring > fuzzy (`table::matches_typo`).
fn tag_rank(name_lc: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    if name_lc == needle {
        return Some(100);
    }
    if name_lc.starts_with(needle) {
        return Some(80);
    }
    if name_lc
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| !w.is_empty() && w.starts_with(needle))
    {
        return Some(70);
    }
    if name_lc.contains(needle) {
        return Some(60);
    }
    if crate::table::matches_typo(name_lc, &[needle.to_string()]) {
        return Some(40);
    }
    None
}

/// Fuzzy search over the user's **own** tags (the ones they may add), best match
/// first. `tag_track` requires ownership, so other users' tags are not offered.
pub async fn search_user_tags(
    pool: &SqlitePool,
    user_id: i64,
    needle: &str,
    limit: usize,
) -> Vec<TagHit> {
    let rows = sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT id, name, type FROM hub_tags WHERE owner_user_id = ?1 ORDER BY name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let n = needle.trim().to_lowercase();
    let mut scored: Vec<(i32, TagHit)> = rows
        .into_iter()
        .filter_map(|(id, name, ty)| {
            let score = tag_rank(&name.to_lowercase(), &n)?;
            let parsed = ty.as_deref().and_then(crate::tagtypes::TagType::parse);
            Some((
                score,
                TagHit {
                    id,
                    name,
                    type_name: ty,
                    type_label: parsed.map(|t| t.label().to_string()).unwrap_or_default(),
                    icon: parsed.map(|t| t.icon().to_string()).unwrap_or_default(),
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
    });
    scored.into_iter().take(limit).map(|(_, h)| h).collect()
}

/// Resolve the tag to add from a form: an explicit `tag_id` (an existing tag the
/// user owns) or a `name` (+ optional `type`) to create. `None` = nothing to add.
pub async fn resolve_tag_for_add(
    pool: &SqlitePool,
    user_id: i64,
    tag_id: Option<i64>,
    name: Option<&str>,
    ty: Option<&str>,
) -> Option<i64> {
    if let Some(tid) = tag_id {
        let owned = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2",
        )
        .bind(tid)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        return owned.map(|_| tid);
    }
    let name = name?.trim();
    if name.is_empty() {
        return None;
    }
    let tid = ensure_tag(pool, user_id, name).await.ok()?;
    if let Some(ty) = ty.and_then(crate::tagtypes::TagType::parse) {
        let _ = crate::tagtypes::set(pool, user_id, tid, Some(ty)).await;
    }
    Some(tid)
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

/// True when the tag has the `GENRE` type.
pub async fn tag_is_genre(pool: &SqlitePool, tag_id: i64) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM hub_tags WHERE id = ?1 AND type = 'GENRE'")
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

/// Pickable collective icons (emoji — no FontAwesome dependency).
pub const ICONS: &[&str] = &[
    "🎧", "🎵", "🔥", "💜", "✨", "#️⃣", "🌙", "⚡", "🌊", "🕺", "🎚️", "🚀", "🧊", "🌶️", "🪩", "🎯",
    "🌅", "🌈", "🧭", "🛸",
];

#[derive(Debug, Clone)]
pub struct Collective {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub owner: String,
    pub role: String,
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
        members: r.get::<Option<i64>, _>("members").unwrap_or(0),
    }
}

pub async fn list_collectives_for(pool: &SqlitePool, user_id: i64) -> Vec<Collective> {
    let rows = sqlx::query(
        "SELECT c.id, c.name, c.icon, u.slug AS owner, m.role,
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
    Some(CollectiveDetail {
        id: row.get("id"),
        name: row.get("name"),
        icon: row.get("icon"),
        owner: row.get("owner"),
        role,
        is_owner,
        members,
    })
}

// ── tag detail ───────────────────────────────────────────────────────────────

pub struct TagDetail {
    pub id: i64,
    pub name: String,
    pub owner: String,
    /// The tag's type string (`MOOD`, `GENRE`, …, or empty when untyped).
    pub type_name: String,
    pub source_count: i64,
    pub tracks: Vec<(i64, String, String)>,
}

pub async fn tag_detail(pool: &SqlitePool, tag_id: i64) -> Option<TagDetail> {
    let row = sqlx::query(
        "SELECT t.id, t.name, u.slug AS owner, COALESCE(t.type,'') AS type_name,
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
        type_name: row
            .get::<Option<String>, _>("type_name")
            .unwrap_or_default(),
        source_count: row.get("source_count"),
        tracks,
    })
}

/// One other tag that co-occurs with a seed tag on the same tracks.
#[derive(Debug, Clone)]
pub struct TagCooccurrence {
    pub tag_id: i64,
    pub name: String,
    /// The other tag's type label (a type shared with the seed tag if one exists).
    pub type_label: String,
    /// True when the other tag has the same type as the seed tag.
    pub same_type: bool,
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
/// types. Lift = P(A,B) / (P(A)·P(B)); restricted to other tags carrying at
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

    // Type of the seed tag.
    let seed_type: String =
        sqlx::query_scalar::<_, Option<String>>("SELECT type FROM hub_tags WHERE id = ?1")
            .bind(tag_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten()
            .unwrap_or_default();

    // Types of the candidate tags (batched).
    let mut cand_types: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    {
        let mut qb = QueryBuilder::new("SELECT id, COALESCE(type,'') FROM hub_tags WHERE id IN (");
        {
            let mut sep = qb.separated(", ");
            for id in &ids {
                sep.push_bind(*id);
            }
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get(0);
            let ty: String = r.get(1);
            cand_types.insert(tid, ty);
        }
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

    let mut out: Vec<TagCooccurrence> = pairs
        .into_iter()
        .map(|(other_id, name, both, support, lift)| {
            let ctype = cand_types.get(&other_id).cloned().unwrap_or_default();
            let same_type = !ctype.is_empty() && ctype == seed_type;
            let type_label = crate::tagtypes::TagType::parse(&ctype)
                .map(|t| t.label().to_string())
                .unwrap_or_default();
            let union = (with_tag + support - both).max(1) as f64;
            TagCooccurrence {
                tag_id: other_id,
                name,
                type_label,
                same_type,
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
