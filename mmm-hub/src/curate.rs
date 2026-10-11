//! Curated playlists (#316): user-built playlists whose track set is derived
//! from a tag or a smart "lens", and which can be pushed to Spotify.
//!
//! Resolution is always done here (server-side); the UI only displays the result
//! and toggles settings.

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Row, SqlitePool};

use crate::config::Config;

/// Smart lenses: server-side rules that pick tracks dynamically.
pub const LENSES: &[(&str, &str)] = &[
    ("newest", "Neueste im Hub"),
    ("liked", "Meine Likes"),
    ("unheard", "Am wenigsten gespielt"),
];

pub fn lens_label(key: &str) -> &'static str {
    LENSES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| *l)
        .unwrap_or("Unbekannt")
}

/// A curated playlist row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CuratedPlaylist {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: String,
    pub is_public: i64,
    pub spotify_playlist_id: Option<String>,
    pub fill_mode: String,
    pub fill_tag_id: Option<i64>,
    pub fill_lens: Option<String>,
    pub fill_limit: i64,
    pub daily: i64,
    pub last_push_at: Option<String>,
    pub last_push_status: Option<String>,
    pub last_push_error: Option<String>,
}

impl CuratedPlaylist {
    pub fn is_public(&self) -> bool {
        self.is_public != 0
    }
    pub fn daily(&self) -> bool {
        self.daily != 0
    }
    pub fn pushed_to_spotify(&self) -> bool {
        self.spotify_playlist_id
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<CuratedPlaylist>> {
    Ok(sqlx::query_as::<_, CuratedPlaylist>(
        "SELECT id, user_id, name, description, is_public, spotify_playlist_id,
                fill_mode, fill_tag_id, fill_lens, fill_limit, daily,
                last_push_at, last_push_status, last_push_error
           FROM hub_curated_playlists WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<CuratedPlaylist>> {
    Ok(sqlx::query_as::<_, CuratedPlaylist>(
        "SELECT id, user_id, name, description, is_public, spotify_playlist_id,
                fill_mode, fill_tag_id, fill_lens, fill_limit, daily,
                last_push_at, last_push_status, last_push_error
           FROM hub_curated_playlists WHERE user_id = ?1 ORDER BY name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Whether the user's Spotify grant includes a playlist write scope.
pub async fn has_write_scope(pool: &SqlitePool, user_id: i64) -> bool {
    let scopes: Option<String> = sqlx::query_scalar(
        "SELECT scopes FROM hub_service_accounts WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    scopes
        .map(|s| {
            s.split_whitespace()
                .any(|x| x == "playlist-modify-private" || x == "playlist-modify-public")
        })
        .unwrap_or(false)
}

/// Resolve a curated playlist to an ordered list of hub track ids.
pub async fn resolve_track_ids(pool: &SqlitePool, cp: &CuratedPlaylist) -> Result<Vec<i64>> {
    let limit = cp.fill_limit.clamp(1, 500);
    let ids: Vec<i64> = match cp.fill_mode.as_str() {
        "tag" => {
            let Some(tag_id) = cp.fill_tag_id else {
                return Ok(Vec::new());
            };
            sqlx::query_scalar(
                "SELECT rt.track_id
                   FROM hub_track_resolved_tags rt
                   JOIN hub_tracks t ON t.id = rt.track_id
                  WHERE rt.tag_id = ?1
                  GROUP BY rt.track_id
                  ORDER BY t.first_seen_at DESC, t.id DESC
                  LIMIT ?2",
            )
            .bind(tag_id)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        "lens" => match cp.fill_lens.as_deref().unwrap_or("") {
            "liked" => {
                sqlx::query_scalar(
                    "SELECT track_id FROM hub_liked_tracks
                      WHERE user_id = ?1 ORDER BY liked_at DESC LIMIT ?2",
                )
                .bind(cp.user_id)
                .bind(limit)
                .fetch_all(pool)
                .await?
            }
            "unheard" => {
                // Least-played tracks (Traktor play count), newest first as tiebreak.
                sqlx::query_scalar(
                    "SELECT t.id
                       FROM hub_tracks t
                       LEFT JOIN hub_v_track_traktor v ON v.track_id = t.id
                      ORDER BY COALESCE(v.play_count, 0) ASC, t.first_seen_at DESC
                      LIMIT ?1",
                )
                .bind(limit)
                .fetch_all(pool)
                .await?
            }
            // "newest" and unknown keys fall back to newest-in-hub.
            _ => {
                sqlx::query_scalar(
                    "SELECT id FROM hub_tracks
                      ORDER BY first_seen_at DESC, id DESC LIMIT ?1",
                )
                .bind(limit)
                .fetch_all(pool)
                .await?
            }
        },
        // manual (and unknown modes): hand-picked items.
        _ => {
            sqlx::query_scalar(
                "SELECT track_id FROM hub_curated_items
                  WHERE playlist_id = ?1 ORDER BY position, added_at",
            )
            .bind(cp.id)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(ids)
}

/// Map hub track ids to Spotify track ids (order preserved, duplicates dropped).
async fn spotify_uris(pool: &SqlitePool, track_ids: &[i64]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for chunk in track_ids.chunks(500) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT external_id FROM hub_track_external_ids
              WHERE service = 'spotify' AND track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        sep.push_unseparated(")");
        for r in qb.build().fetch_all(pool).await? {
            let id: String = r.get("external_id");
            if !id.is_empty() && seen.insert(id.clone()) {
                out.push(format!("spotify:track:{id}"));
            }
        }
    }
    Ok(out)
}

/// The result of a push, for user feedback.
pub struct PushResult {
    pub spotify_playlist_id: String,
    pub pushed: usize,
    pub total: usize,
}

/// Push a curated playlist to Spotify, creating the remote playlist on first
/// push and replacing its items afterwards.
pub async fn push(pool: &SqlitePool, cfg: &Config, cp: &CuratedPlaylist) -> Result<PushResult> {
    let token = crate::ingest::access_token(pool, cfg, cp.user_id).await?;
    let track_ids = resolve_track_ids(pool, cp).await?;
    let uris = spotify_uris(pool, &track_ids).await?;

    let base = &cfg.spotify_api_base;
    let playlist_id = match cp.spotify_playlist_id.as_deref() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => create_playlist(base, &token, cp).await?,
    };

    replace_items(base, &token, &playlist_id, &uris).await?;

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE hub_curated_playlists
            SET spotify_playlist_id = ?2, last_push_at = ?3,
                last_push_status = 'ok', last_push_error = NULL, updated_at = ?3
          WHERE id = ?1",
    )
    .bind(cp.id)
    .bind(&playlist_id)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(PushResult {
        spotify_playlist_id: playlist_id,
        pushed: uris.len(),
        total: track_ids.len(),
    })
}

async fn create_playlist(base: &str, token: &str, cp: &CuratedPlaylist) -> Result<String> {
    let (status, me) = crate::spotify::api_get(base, token, "/me").await?;
    if status != 200 {
        bail!("Spotify /me -> {status}: {me}");
    }
    let user_id = me["id"].as_str().context("Spotify /me missing id")?;
    let body = json!({
        "name": cp.name,
        "description": cp.description,
        "public": cp.is_public(),
    });
    let url = crate::spotify::api_url(base, &format!("/users/{user_id}/playlists"));
    let (status, resp) = crate::spotify::post_json(token, &url, &body).await?;
    if !(200..300).contains(&status) {
        bail!(
            "Spotify create playlist -> {status}: {}",
            spotify_err(&resp)
        );
    }
    resp["id"]
        .as_str()
        .map(str::to_string)
        .context("Spotify create playlist response missing id")
}

async fn replace_items(base: &str, token: &str, playlist_id: &str, uris: &[String]) -> Result<()> {
    let url = crate::spotify::api_url(base, &format!("/playlists/{playlist_id}/items"));
    if uris.len() <= 100 {
        let (status, resp) =
            crate::spotify::put_json(token, &url, &json!({ "uris": uris })).await?;
        if !(200..300).contains(&status) {
            bail!("Spotify replace items -> {status}: {}", spotify_err(&resp));
        }
        return Ok(());
    }
    // Clear, then append in batches of 100.
    let (status, resp) = crate::spotify::put_json(token, &url, &json!({ "uris": [] })).await?;
    if !(200..300).contains(&status) {
        bail!("Spotify clear items -> {status}: {}", spotify_err(&resp));
    }
    for chunk in uris.chunks(100) {
        let (status, resp) =
            crate::spotify::post_json(token, &url, &json!({ "uris": chunk })).await?;
        if !(200..300).contains(&status) {
            bail!("Spotify add items -> {status}: {}", spotify_err(&resp));
        }
    }
    Ok(())
}

fn spotify_err(body: &serde_json::Value) -> String {
    body["error"]["message"]
        .as_str()
        .or_else(|| body["error_description"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}

/// Curated playlists due for their daily auto-push (issue #316).
pub async fn daily_due(pool: &SqlitePool, cutoff_iso: &str) -> Result<Vec<CuratedPlaylist>> {
    Ok(sqlx::query_as::<_, CuratedPlaylist>(
        "SELECT id, user_id, name, description, is_public, spotify_playlist_id,
                fill_mode, fill_tag_id, fill_lens, fill_limit, daily,
                last_push_at, last_push_status, last_push_error
           FROM hub_curated_playlists
          WHERE daily = 1 AND spotify_playlist_id IS NOT NULL
            AND (last_push_at IS NULL OR last_push_at < ?1)
          ORDER BY id",
    )
    .bind(cutoff_iso)
    .fetch_all(pool)
    .await?)
}
