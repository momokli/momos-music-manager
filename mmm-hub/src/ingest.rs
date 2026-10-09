//! Spotify ingest: likes + owned/collaborative playlists into the shared DB.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::spotify::{self, Tokens};

#[derive(Debug, Default)]
pub struct Summary {
    pub playlists: usize,
    pub followed: usize,
    pub owned_with_items: usize,
    pub liked_tracks: usize,
    pub memberships: usize,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn epoch_to_iso(epoch: i64) -> String {
    DateTime::from_timestamp(epoch, 0)
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

fn iso_to_epoch(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

// ── Users / accounts ─────────────────────────────────────────────────────────

pub async fn ensure_user(pool: &SqlitePool, slug: &str) -> Result<i64> {
    // Resolve case-insensitively so `momo`/`Momo` don't create duplicate rows.
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_users (slug, created_at) VALUES (?1, ?2) RETURNING id",
    )
    .bind(slug)
    .bind(now_iso())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Store a fresh authorization (sets `authorized_at` + `connected_at`).
pub async fn store_initial_tokens(
    pool: &SqlitePool,
    user_id: i64,
    tokens: &Tokens,
    remote_user_id: Option<&str>,
    display_name: Option<&str>,
) -> Result<()> {
    let now = now_iso();
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, remote_user_id, display_name, access_token, refresh_token,
              token_expiry, scopes, authorized_at, connected_at, updated_at)
         VALUES (?1, 'spotify', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)
         ON CONFLICT(user_id, service) DO UPDATE SET
             access_token  = excluded.access_token,
             refresh_token = COALESCE(excluded.refresh_token, hub_service_accounts.refresh_token),
             token_expiry  = excluded.token_expiry,
             scopes        = excluded.scopes,
             remote_user_id = COALESCE(excluded.remote_user_id, hub_service_accounts.remote_user_id),
             display_name  = COALESCE(excluded.display_name, hub_service_accounts.display_name),
             authorized_at = excluded.authorized_at,
             connected_at  = excluded.connected_at,
             updated_at    = excluded.updated_at",
    )
    .bind(user_id)
    .bind(remote_user_id)
    .bind(display_name)
    .bind(&tokens.access_token)
    .bind(&tokens.refresh_token)
    .bind(epoch_to_iso(tokens.expires_at))
    .bind(spotify::SCOPES)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Store only a refreshed access token (keeps `authorized_at`).
pub async fn store_refreshed_tokens(
    pool: &SqlitePool,
    user_id: i64,
    tokens: &Tokens,
) -> Result<()> {
    sqlx::query(
        "UPDATE hub_service_accounts
            SET access_token = ?2,
                refresh_token = COALESCE(?3, refresh_token),
                token_expiry = ?4,
                connected_at = ?5,
                updated_at = ?5
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .bind(&tokens.access_token)
    .bind(&tokens.refresh_token)
    .bind(epoch_to_iso(tokens.expires_at))
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(())
}

struct Account {
    access_token: String,
    refresh_token: Option<String>,
    expiry_epoch: Option<i64>,
}

async fn load_account(pool: &SqlitePool, user_id: i64) -> Result<Option<Account>> {
    let row = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
        "SELECT access_token, refresh_token, token_expiry
           FROM hub_service_accounts WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(access, refresh, expiry)| Account {
        access_token: access.unwrap_or_default(),
        refresh_token: refresh,
        expiry_epoch: expiry.as_deref().and_then(iso_to_epoch),
    }))
}

/// Ensure a usable access token (refreshes when expired/expiring).
pub(crate) async fn access_token(pool: &SqlitePool, cfg: &Config, user_id: i64) -> Result<String> {
    let account = load_account(pool, user_id).await?.with_context(|| {
        "no linked Spotify account — connect Spotify in the web UI first".to_string()
    })?;
    let needs_refresh = account
        .expiry_epoch
        .map(|e| e <= Utc::now().timestamp() + 60)
        .unwrap_or(true);
    if !needs_refresh {
        return Ok(account.access_token);
    }
    let (client_id, client_secret) = cfg.spotify_creds()?;
    let refresh_token = account
        .refresh_token
        .clone()
        .context("no refresh token stored — reconnect Spotify in the web UI")?;
    let tokens = spotify::refresh(client_id, client_secret, &refresh_token)
        .await
        .context("refresh Spotify token")?;
    store_refreshed_tokens(pool, user_id, &tokens).await?;
    Ok(tokens.access_token)
}

/// `GET /me`, persisting the remote id + display name.
async fn remote_profile(
    pool: &SqlitePool,
    base: &str,
    token: &str,
    user_id: i64,
) -> Result<(String, Option<String>)> {
    let (status, me) = spotify::api_get(base, token, "/me").await?;
    if status == 403 {
        bail!(
            "Spotify verweigert den Zugriff (403). Dieser Account ist nicht in der Allowlist \
             der Spotify-App — im Spotify Developer Dashboard unter 'Users Management' \
             hinzufuegen (die App des Owners muss Spotify Premium haben, dev mode = max. 5 User)."
        );
    }
    if status != 200 {
        bail!("GET /me returned {status}: {me}");
    }
    let remote_id = me["id"].as_str().context("GET /me had no id")?.to_string();
    let display_name = me["display_name"].as_str().map(str::to_string);
    sqlx::query(
        "UPDATE hub_service_accounts SET remote_user_id = ?2, display_name = ?3, updated_at = ?4
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .bind(&remote_id)
    .bind(&display_name)
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok((remote_id, display_name))
}

/// Upsert a playlist's metadata only (no items). Returns the local playlist id.
async fn upsert_playlist_meta(
    pool: &SqlitePool,
    user_id: i64,
    pl: &Value,
    owned: bool,
) -> Result<Option<i64>> {
    let Some(playlist_id) = pl["id"].as_str() else {
        return Ok(None);
    };
    let name = pl["name"].as_str().unwrap_or("(unnamed)");
    let description = pl["description"].as_str();
    let track_count = pl["items"]["total"].as_i64();
    let snapshot = pl["snapshot_id"].as_str();
    // The Spotify account that owns the playlist (≠ the hub user).
    let owner_id = pl["owner"]["id"].as_str();
    let owner_name = pl["owner"]["display_name"].as_str().or(owner_id);
    let collaborative = pl["collaborative"].as_bool().unwrap_or(false);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists
             (user_id, service, playlist_id, name, description, is_liked, track_count,
              snapshot_id, items_available, fetched_at, is_owned, owner_id, owner_name, collaborative)
         VALUES (?1, 'spotify', ?2, ?3, ?4, 0, ?5, ?6, 0, NULL, ?7, ?8, ?9, ?10)
         ON CONFLICT(user_id, service, playlist_id) DO UPDATE SET
             name = excluded.name,
             description = excluded.description,
             track_count = excluded.track_count,
             snapshot_id = excluded.snapshot_id,
             is_owned = excluded.is_owned,
             owner_id = excluded.owner_id,
             owner_name = excluded.owner_name,
             collaborative = excluded.collaborative
         RETURNING id",
    )
    .bind(user_id)
    .bind(playlist_id)
    .bind(name)
    .bind(description)
    .bind(track_count)
    .bind(snapshot)
    .bind(owned as i64)
    .bind(owner_id)
    .bind(owner_name)
    .bind(collaborative as i64)
    .fetch_one(pool)
    .await?;
    Ok(Some(id))
}

/// Replace a playlist's membership with `items` (tracks upserted by service id).
pub(crate) async fn store_playlist_items(
    pool: &SqlitePool,
    local_playlist_id: i64,
    items: &[Value],
) -> Result<usize> {
    sqlx::query("DELETE FROM hub_playlist_tracks WHERE playlist_id = ?1")
        .bind(local_playlist_id)
        .execute(pool)
        .await?;
    let mut count = 0usize;
    let mut position = 0i64;
    for entry in items {
        let Some(t) = track_object(entry) else {
            continue;
        };
        let Some(track_id) = upsert_track(pool, t).await? else {
            continue;
        };
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(playlist_id, track_id) DO UPDATE SET
                 position = excluded.position, added_at = excluded.added_at",
        )
        .bind(local_playlist_id)
        .bind(track_id)
        .bind(position)
        .bind(entry["added_at"].as_str())
        .execute(pool)
        .await?;
        count += 1;
        position += 1;
    }
    Ok(count)
}

/// The API exposes items only for owned/collaborative playlists.
fn is_owned(pl: &Value, remote_id: &str) -> bool {
    pl["collaborative"].as_bool().unwrap_or(false) || pl["owner"]["id"].as_str() == Some(remote_id)
}

#[derive(Debug, Default)]
pub struct PlaylistFetch {
    pub total: usize,
    pub owned: usize,
    pub followed: usize,
}

/// Fetch ALL playlists (owned **and** followed) as metadata only — no tracks.
pub async fn fetch_playlists(pool: &SqlitePool, cfg: &Config, slug: &str) -> Result<PlaylistFetch> {
    let user_id = ensure_user(pool, slug).await?;
    let token = access_token(pool, cfg, user_id).await?;
    let (remote_id, _) = remote_profile(pool, &cfg.spotify_api_base, &token, user_id).await?;

    let playlists = spotify::get_all_items(&cfg.spotify_api_base, &token, "/me/playlists?limit=50").await?;
    let mut out = PlaylistFetch::default();
    for pl in &playlists {
        let owned = is_owned(pl, &remote_id);
        if upsert_playlist_meta(pool, user_id, pl, owned).await?.is_some() {
            out.total += 1;
            if owned {
                out.owned += 1;
            } else {
                out.followed += 1;
            }
        }
    }
    Ok(out)
}

/// Replace a user's liked tracks with `entries` (saved-track objects).
pub(crate) async fn store_likes(
    pool: &SqlitePool,
    user_id: i64,
    entries: &[Value],
) -> Result<usize> {
    sqlx::query("DELETE FROM hub_liked_tracks WHERE user_id = ?1")
        .bind(user_id)
        .execute(pool)
        .await?;
    let mut count = 0usize;
    for entry in entries {
        let Some(t) = track_object(entry) else {
            continue;
        };
        let Some(track_id) = upsert_track(pool, t).await? else {
            continue;
        };
        sqlx::query(
            "INSERT INTO hub_liked_tracks (user_id, track_id, liked_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id, track_id) DO UPDATE SET liked_at = excluded.liked_at",
        )
        .bind(user_id)
        .bind(track_id)
        .bind(entry["added_at"].as_str())
        .execute(pool)
        .await?;
        count += 1;
    }
    Ok(count)
}

// ── Ingest ───────────────────────────────────────────────────────────────────

pub async fn ingest_user(pool: &SqlitePool, cfg: &Config, slug: &str) -> Result<Summary> {
    let user_id = ensure_user(pool, slug).await?;
    let token = access_token(pool, cfg, user_id).await?;
    let (remote_id, _display_name) = remote_profile(pool, &cfg.spotify_api_base, &token, user_id).await?;

    let mut summary = Summary::default();

    // ── Likes ────────────────────────────────────────────────────────────────
    let likes = spotify::get_all_items(&cfg.spotify_api_base, &token, "/me/tracks?limit=50").await?;
    summary.liked_tracks = store_likes(pool, user_id, &likes).await?;

    // ── Playlists + their items ──────────────────────────────────────────────
    let playlists = spotify::get_all_items(&cfg.spotify_api_base, &token, "/me/playlists?limit=50").await?;
    summary.playlists = playlists.len();

    for pl in &playlists {
        let owned = is_owned(pl, &remote_id);
        let Some(playlist_id) = pl["id"].as_str() else {
            continue;
        };
        let Some(local_playlist_id) = upsert_playlist_meta(pool, user_id, pl, owned).await? else {
            continue;
        };

        if !owned {
            summary.followed += 1;
            continue; // metadata only — the API returns no items for followed playlists
        }

        // Owned/collaborative → fetch items. 403 (or absent items) → leave as metadata.
        let path = format!("/playlists/{playlist_id}/items?limit=50");
        let (status, first) = spotify::api_get(&cfg.spotify_api_base, &token, &path).await?;
        if status != 200 {
            tracing::warn!("playlist {playlist_id} items returned {status}; storing metadata only");
            continue;
        }

        let mut items = first["items"].as_array().cloned().unwrap_or_default();
        let mut next = first["next"].as_str().map(str::to_string);
        while let Some(url) = next {
            let (status, page) = spotify::get_json(&token, &url).await?;
            if status != 200 {
                break;
            }
            if let Some(arr) = page["items"].as_array() {
                items.extend(arr.iter().cloned());
            }
            next = page["next"].as_str().map(str::to_string);
        }

        summary.memberships += store_playlist_items(pool, local_playlist_id, &items).await?;

        sqlx::query("UPDATE hub_playlists SET items_available = 1, fetched_at = ?2 WHERE id = ?1")
            .bind(local_playlist_id)
            .bind(now_iso())
            .execute(pool)
            .await?;
        summary.owned_with_items += 1;
    }

    Ok(summary)
}

/// Insert deterministic demo data so the overlap views can be exercised without
/// any Spotify credentials. Idempotent.
pub async fn seed_demo(pool: &SqlitePool) -> Result<()> {
    let now = now_iso();
    let users = [("momo", "Momo"), ("simon", "Simon"), ("jonas", "Jonas")];
    let mut ids = std::collections::HashMap::new();
    for (slug, name) in users {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO hub_users (slug, display_name, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(slug) DO UPDATE SET display_name = excluded.display_name RETURNING id",
        )
        .bind(slug)
        .bind(name)
        .bind(&now)
        .fetch_one(pool)
        .await?;
        ids.insert(slug, id);
    }

    let tracks = [
        ("demo-t1", "Shared Anthem", "Aaa"),
        ("demo-t2", "Momo Only", "Bbb"),
        ("demo-t3", "Simon Only", "Ccc"),
    ];
    let mut track_ids = std::collections::HashMap::new();
    for (sid, title, artists) in tracks {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
             VALUES ('spotify', ?1, ?2, ?3, ?4)
             ON CONFLICT(service, service_track_id) DO UPDATE SET title = excluded.title RETURNING id",
        )
        .bind(sid)
        .bind(title)
        .bind(artists)
        .bind(&now)
        .fetch_one(pool)
        .await?;
        track_ids.insert(sid, id);
    }

    // Likes: t1 by all three, t2 by momo, t3 by simon.
    let likes = [("momo", "demo-t1"), ("simon", "demo-t1"), ("jonas", "demo-t1"), ("momo", "demo-t2"), ("simon", "demo-t3")];
    for (slug, sid) in likes {
        sqlx::query(
            "INSERT INTO hub_liked_tracks (user_id, track_id, liked_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id, track_id) DO UPDATE SET liked_at = excluded.liked_at",
        )
        .bind(ids[slug])
        .bind(track_ids[sid])
        .bind(&now)
        .execute(pool)
        .await?;
    }

    // Playlists: momo + simon each own a "Warehouse" playlist.
    for (slug, pid, name, members) in [
        ("momo", "demo-pl-momo", "Warehouse", vec!["demo-t1", "demo-t2"]),
        ("simon", "demo-pl-simon", "Warehouse", vec!["demo-t1", "demo-t3"]),
    ] {
        let pl_id: i64 = sqlx::query_scalar(
            "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_liked, items_available, fetched_at)
             VALUES (?1, 'spotify', ?2, ?3, 0, 1, ?4)
             ON CONFLICT(user_id, service, playlist_id) DO UPDATE SET name = excluded.name RETURNING id",
        )
        .bind(ids[slug])
        .bind(pid)
        .bind(name)
        .bind(&now)
        .fetch_one(pool)
        .await?;

        for (pos, sid) in members.iter().enumerate() {
            sqlx::query(
                "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(playlist_id, track_id) DO UPDATE SET position = excluded.position",
            )
            .bind(pl_id)
            .bind(track_ids[*sid])
            .bind(pos as i64)
            .bind(&now)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}
fn track_object(entry: &Value) -> Option<&Value> {
    let t = entry.get("item").or_else(|| entry.get("track"))?;
    if t.is_null() || t["type"].as_str() != Some("track") || t["is_local"].as_bool() == Some(true) {
        return None;
    }
    t["id"].as_str()?;
    Some(t)
}

/// Upsert a track object into `hub_tracks`, returning its local id.
async fn upsert_track(pool: &SqlitePool, t: &Value) -> Result<Option<i64>> {
    let Some(service_track_id) = t["id"].as_str() else {
        return Ok(None);
    };
    let isrc = t["external_ids"]["isrc"].as_str();
    let title = t["name"].as_str().unwrap_or("");
    let artists = t["artists"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let album = t["album"]["name"].as_str();
    let duration_ms = t["duration_ms"].as_i64();
    let explicit = t["explicit"].as_bool().map(|b| b as i64);
    let image_url = t["album"]["images"]
        .as_array()
        .and_then(|imgs| imgs.iter().find_map(|i| i["url"].as_str()));

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks
             (service, service_track_id, isrc, title, artists, album, duration_ms, explicit,
              image_url, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(service, service_track_id) DO UPDATE SET
             isrc        = COALESCE(excluded.isrc, hub_tracks.isrc),
             title       = excluded.title,
             artists     = excluded.artists,
             album       = excluded.album,
             duration_ms = excluded.duration_ms,
             explicit    = excluded.explicit,
             image_url   = COALESCE(excluded.image_url, hub_tracks.image_url)
         RETURNING id",
    )
    .bind(service_track_id)
    .bind(isrc)
    .bind(title)
    .bind(artists)
    .bind(album)
    .bind(duration_ms)
    .bind(explicit)
    .bind(image_url)
    .bind(now_iso())
    .fetch_one(pool)
    .await?;
    Ok(Some(id))
}
