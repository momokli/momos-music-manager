//! Touch-cascade + download-priority orders (#hub-crawl-everything, plan §2/§3).
//!
//! The hub **orders and reads**; music-api downloads and analyses. Two jobs:
//!
//! * **Download priority** — every touched track is ordered at music-api by ISRC
//!   with a priority derived from its references (liked / playlist count /
//!   now-playing). Re-ordering is a cheap no-op, and [`order_pending`] only
//!   re-orders on a real change (tracked in `hub_order_state`).
//! * **Touch-cascade** — a bounded, TTL'd Spotify crawl: touched artists →
//!   `GET /artists/{id}/albums` → `hub_albums`; each album →
//!   `GET /albums/{id}/tracks` → `hub_tracks` (which re-triggers enrichment).
//!
//! Everything is bounded: a per-pass request budget, per-artist album caps and
//! per-artist/album TTLs. It never runs unbounded and backs off on 429/quota.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

use crate::config::Config;
use crate::settings;

const CHUNK: usize = 400;
const ORDER_BATCH: usize = 500;
const PAGE_DELAY: Duration = Duration::from_millis(150);

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn in_clause(n: usize) -> String {
    std::iter::repeat("?").take(n).collect::<Vec<_>>().join(",")
}

async fn num_setting(pool: &SqlitePool, key: &str, default: f64) -> f64 {
    settings::get(pool, key)
        .await
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

async fn flag_setting(pool: &SqlitePool, key: &str, default: bool) -> bool {
    match settings::get(pool, key)
        .await
        .map(|v| v.trim().to_lowercase())
    {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
        None => default,
    }
}

/// Spotify request budget for one crawl pass.
pub async fn requests_per_pass(pool: &SqlitePool) -> usize {
    num_setting(pool, settings::CRAWL_REQUESTS_PER_PASS, 10.0)
        .await
        .max(1.0) as usize
}

// ── download priority ────────────────────────────────────────────────────────

/// Download priority for one track from its references (plan §2):
/// `(liked ? 1 : 0) + playlist_refs`. `0` when the track has no reference.
pub async fn priority_of(pool: &SqlitePool, track_id: i64) -> i32 {
    priorities(pool, &[track_id])
        .await
        .get(&track_id)
        .copied()
        .unwrap_or(0)
}

/// Batched [`priority_of`] over many tracks.
pub async fn priorities(pool: &SqlitePool, ids: &[i64]) -> HashMap<i64, i32> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT t.id AS id,
                    (SELECT COUNT(*) FROM hub_liked_tracks l WHERE l.track_id = t.id) AS likes,
                    (SELECT COUNT(*) FROM hub_playlist_tracks pt WHERE pt.track_id = t.id) AS pl_refs
               FROM hub_tracks t WHERE t.id IN ({})",
            in_clause(chunk.len())
        );
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for r in q.fetch_all(pool).await.unwrap_or_default() {
            let id: i64 = r.get("id");
            let likes: i64 = r.get("likes");
            let pl_refs: i64 = r.get("pl_refs");
            let liked = i64::from(likes > 0);
            out.insert(id, (liked + pl_refs) as i32);
        }
    }
    out
}

/// Order the given tracks at music-api, grouped by priority. Only tracks with an
/// ISRC are ordered. Records the ordered priority in `hub_order_state`.
/// Returns the number of ISRCs ordered.
pub async fn order_touched(pool: &SqlitePool, cfg: &Config, ids: &[i64]) -> Result<usize> {
    if cfg.music_api_token.is_none() || ids.is_empty() {
        return Ok(0);
    }
    if !flag_setting(pool, settings::CRAWL_ORDER_ON_TOUCH, true).await {
        return Ok(0);
    }
    let mut by_id: HashMap<i64, String> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT id, isrc FROM hub_tracks
              WHERE isrc IS NOT NULL AND TRIM(isrc) <> '' AND id IN ({})",
            in_clause(chunk.len())
        );
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for r in q.fetch_all(pool).await? {
            by_id.insert(r.get("id"), r.get::<String, _>("isrc"));
        }
    }
    if by_id.is_empty() {
        return Ok(0);
    }
    let prio = priorities(pool, ids).await;
    let ordered = order_grouped(pool, cfg, &by_id, &prio).await?;
    Ok(ordered)
}

/// Group `by_id` (track → isrc) by priority and issue one `/orders` per group.
/// Only successfully ordered tracks are recorded in `hub_order_state` (so a
/// failed call is retried next pass).
async fn order_grouped(
    pool: &SqlitePool,
    cfg: &Config,
    by_id: &HashMap<i64, String>,
    prio: &HashMap<i64, i32>,
) -> Result<usize> {
    let mut groups: HashMap<i32, Vec<(i64, String)>> = HashMap::new();
    for (id, isrc) in by_id {
        let p = prio.get(id).copied().unwrap_or(0);
        groups.entry(p).or_default().push((*id, isrc.clone()));
    }
    let mut ordered = 0usize;
    let mut ok: Vec<(i64, i32)> = Vec::new();
    for (p, entries) in groups {
        for batch in entries.chunks(ORDER_BATCH) {
            let isrcs: Vec<String> = batch.iter().map(|(_, i)| i.clone()).collect();
            match crate::music_api::order_with_priority(cfg, &isrcs, p).await {
                Ok(_) => {
                    ordered += batch.len();
                    ok.extend(batch.iter().map(|(id, _)| (*id, p)));
                }
                Err(e) => tracing::warn!("order (prio {p}) failed: {e}"),
            }
        }
    }
    record_ordered(pool, &ok).await?;
    Ok(ordered)
}

/// Upsert the ordered priority for each `(track_id, priority)` (idempotency ledger).
async fn record_ordered(pool: &SqlitePool, ok: &[(i64, i32)]) -> Result<()> {
    let now = now_iso();
    for (id, p) in ok {
        sqlx::query(
            "INSERT INTO hub_order_state (track_id, priority, ordered_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(track_id) DO UPDATE SET priority = excluded.priority, ordered_at = excluded.ordered_at",
        )
        .bind(id)
        .bind(p)
        .bind(&now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Drain tracks that have references but are not yet ordered (or whose priority
/// changed). Bounded by `limit`; idempotent. Returns the number ordered.
pub async fn order_pending(pool: &SqlitePool, cfg: &Config, limit: usize) -> Result<usize> {
    if cfg.music_api_token.is_none() || limit == 0 {
        return Ok(0);
    }
    if !flag_setting(pool, settings::CRAWL_ORDER_ON_TOUCH, true).await {
        return Ok(0);
    }
    let rows = sqlx::query(
        "SELECT t.id AS id, t.isrc AS isrc,
                (CASE WHEN EXISTS (SELECT 1 FROM hub_liked_tracks l WHERE l.track_id = t.id)
                      THEN 1 ELSE 0 END)
                + (SELECT COUNT(*) FROM hub_playlist_tracks pt WHERE pt.track_id = t.id) AS prio
           FROM hub_tracks t
           LEFT JOIN hub_order_state o ON o.track_id = t.id
          WHERE t.isrc IS NOT NULL AND TRIM(t.isrc) <> ''
            AND (EXISTS (SELECT 1 FROM hub_liked_tracks l WHERE l.track_id = t.id)
                 OR EXISTS (SELECT 1 FROM hub_playlist_tracks pt WHERE pt.track_id = t.id))
            AND (o.track_id IS NULL OR o.priority <> (
                 (CASE WHEN EXISTS (SELECT 1 FROM hub_liked_tracks l WHERE l.track_id = t.id)
                       THEN 1 ELSE 0 END)
                 + (SELECT COUNT(*) FROM hub_playlist_tracks pt WHERE pt.track_id = t.id)))
          ORDER BY t.first_seen_at DESC, t.id DESC
          LIMIT ?1",
    )
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let mut by_id: HashMap<i64, String> = HashMap::new();
    let mut prio: HashMap<i64, i32> = HashMap::new();
    for r in &rows {
        let id: i64 = r.get("id");
        by_id.insert(id, r.get::<String, _>("isrc"));
        prio.insert(id, r.get::<i64, _>("prio") as i32);
    }
    order_grouped(pool, cfg, &by_id, &prio).await
}

/// Fast-lane order for the currently-playing track: base priority + boost.
pub async fn order_now_playing(pool: &SqlitePool, cfg: &Config, track_id: i64) -> Result<()> {
    if cfg.music_api_token.is_none() {
        return Ok(());
    }
    if !flag_setting(pool, settings::CRAWL_ORDER_ON_TOUCH, true).await {
        return Ok(());
    }
    let isrc: Option<String> = sqlx::query_scalar(
        "SELECT isrc FROM hub_tracks WHERE id = ?1 AND isrc IS NOT NULL AND TRIM(isrc) <> ''",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some(isrc) = isrc else {
        return Ok(());
    };
    let base = priority_of(pool, track_id).await;
    let boost = num_setting(pool, settings::CRAWL_NOWPLAYING_BOOST, 100.0).await as i32;
    let p = base + boost;
    let _ = crate::music_api::order_with_priority(cfg, &[isrc], p).await;
    Ok(())
}

// ── touch-cascade (Spotify, bounded) ─────────────────────────────────────────

/// Run one bounded crawl pass: touched artists → albums, then pending albums →
/// tracks. Returns the number of Spotify requests spent.
pub async fn touch_cascade(pool: &SqlitePool, cfg: &Config, budget: usize) -> Result<usize> {
    if budget == 0 || !flag_setting(pool, settings::CRAWL_ENABLED, true).await {
        return Ok(0);
    }
    let Some(token) = any_spotify_token(pool, cfg).await else {
        return Ok(0);
    };
    let mut used = crawl_artists(pool, cfg, &token, budget).await?;
    if used < budget {
        used += crawl_albums(pool, cfg, &token, budget - used).await?;
    }
    Ok(used)
}

/// Any connected Spotify account's access token (public catalogue reads).
async fn any_spotify_token(pool: &SqlitePool, cfg: &Config) -> Option<String> {
    let uid: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM hub_service_accounts
          WHERE service = 'spotify' AND access_token IS NOT NULL
          ORDER BY user_id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match uid {
        Some(u) => crate::ingest::access_token(pool, cfg, u).await.ok(),
        None => None,
    }
}

async fn ttl_cutoff(pool: &SqlitePool, key: &str, default_days: f64) -> String {
    let days = num_setting(pool, key, default_days).await.max(0.0);
    (chrono::Utc::now() - chrono::Duration::seconds((days * 86_400.0) as i64)).to_rfc3339()
}

/// Crawl up to `budget` touched artists: `GET /artists/{id}/albums` → upsert
/// albums, then enqueue each album for the track crawl.
async fn crawl_artists(
    pool: &SqlitePool,
    cfg: &Config,
    token: &str,
    budget: usize,
) -> Result<usize> {
    let ttl = ttl_cutoff(pool, settings::CRAWL_ARTIST_TTL_DAYS, 90.0).await;
    let max_albums = num_setting(pool, settings::CRAWL_MAX_ALBUMS_PER_ARTIST, 40.0)
        .await
        .max(1.0) as usize;
    let rows = sqlx::query(
        "SELECT e.external_id AS sid
           FROM hub_artists a
           JOIN hub_artist_external_ids e ON e.artist_id = a.id AND e.service = 'spotify'
           LEFT JOIN hub_crawl_state c ON c.kind = 'artist' AND c.ref = e.external_id
          WHERE c.ref IS NULL
             OR c.state = 'pending'
             OR (c.state = 'error' AND c.attempts < 5)
             OR (c.state = 'done' AND (c.crawled_at IS NULL OR c.crawled_at < ?1))
          ORDER BY a.updated_at DESC, a.id DESC
          LIMIT ?2",
    )
    .bind(&ttl)
    .bind(budget as i64)
    .fetch_all(pool)
    .await?;

    let mut used = 0usize;
    for r in rows {
        if used >= budget {
            break;
        }
        let sid: String = r.get("sid");
        let url = format!(
            "{}/artists/{}/albums?include_groups=album,single&limit=50",
            cfg.spotify_api_base, sid
        );
        match get_paged(token, &url, max_albums).await {
            Ok(albums) => {
                for alb in &albums {
                    crate::artists::upsert_album(pool, alb).await?;
                    if let Some(aid) = alb["id"].as_str().filter(|s| !s.is_empty()) {
                        mark_crawl(pool, "album", aid, "pending", None).await?;
                    }
                }
                mark_crawl(pool, "artist", &sid, "done", None).await?;
                used += 1;
            }
            Err(e) => {
                mark_crawl(pool, "artist", &sid, "error", Some(&e.to_string())).await?;
                tracing::warn!("artist crawl {sid} failed: {e}");
                if is_rate_limit(&e) {
                    break;
                }
            }
        }
    }
    Ok(used)
}

/// Crawl up to `budget` pending albums: `GET /albums/{id}/tracks` → upsert
/// tracks (which re-triggers enrichment).
async fn crawl_albums(
    pool: &SqlitePool,
    cfg: &Config,
    token: &str,
    budget: usize,
) -> Result<usize> {
    let ttl = ttl_cutoff(pool, settings::CRAWL_ALBUM_TTL_DAYS, 90.0).await;
    let rows = sqlx::query(
        "SELECT e.external_id AS sid, e.album_id AS album_id
           FROM hub_album_external_ids e
           LEFT JOIN hub_crawl_state c ON c.kind = 'album' AND c.ref = e.external_id
          WHERE e.service = 'spotify'
            AND (c.ref IS NULL OR c.state = 'pending'
                 OR (c.state = 'error' AND c.attempts < 5)
                 OR (c.state = 'done' AND (c.crawled_at IS NULL OR c.crawled_at < ?1)))
          ORDER BY e.fetched_at DESC
          LIMIT ?2",
    )
    .bind(&ttl)
    .bind(budget as i64)
    .fetch_all(pool)
    .await?;

    let mut used = 0usize;
    for r in rows {
        if used >= budget {
            break;
        }
        let sid: String = r.get("sid");
        let album_id: i64 = r.get("album_id");
        let url = format!("{}/albums/{}/tracks?limit=50", cfg.spotify_api_base, sid);
        match get_paged(token, &url, 500).await {
            Ok(tracks) => {
                for t in &tracks {
                    upsert_album_track(pool, t, album_id).await?;
                }
                mark_crawl(pool, "album", &sid, "done", None).await?;
                used += 1;
            }
            Err(e) => {
                mark_crawl(pool, "album", &sid, "error", Some(&e.to_string())).await?;
                tracing::warn!("album crawl {sid} failed: {e}");
                if is_rate_limit(&e) {
                    break;
                }
            }
        }
    }
    Ok(used)
}

/// Follow `next` links up to `max_items`, mapping 429/403 to errors (with a
/// backoff sleep on 429). Never unbounded.
async fn get_paged(token: &str, first_url: &str, max_items: usize) -> Result<Vec<Value>> {
    let mut url = first_url.to_string();
    let mut out: Vec<Value> = Vec::new();
    loop {
        let page = crate::spotify::get_page(token, &url)
            .await
            .with_context(|| format!("GET {url}"))?;
        match page.status {
            200 => {}
            429 => {
                let secs = page.retry_after.unwrap_or(30).max(1);
                tracing::warn!("Spotify rate limit (429); backing off {secs}s");
                tokio::time::sleep(Duration::from_secs(secs)).await;
                anyhow::bail!("spotify 429");
            }
            403 => anyhow::bail!("spotify 403"),
            s => anyhow::bail!("spotify HTTP {s}"),
        }
        if let Some(items) = page.body["items"].as_array() {
            out.extend(items.iter().cloned());
        }
        if out.len() >= max_items {
            out.truncate(max_items);
            break;
        }
        match page.body["next"].as_str() {
            Some(next) if !next.is_empty() => url = next.to_string(),
            _ => break,
        }
        tokio::time::sleep(PAGE_DELAY).await;
    }
    Ok(out)
}

fn is_rate_limit(e: &anyhow::Error) -> bool {
    e.to_string().contains("429")
}

/// Upsert a track from a simplified album-track object without clobbering the
/// richer fields a full ingest may already have set (album/isrc/image).
async fn upsert_album_track(pool: &SqlitePool, t: &Value, album_id: i64) -> Result<Option<i64>> {
    let Some(sid) = t["id"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if t["type"].as_str().is_some_and(|ty| ty != "track") {
        return Ok(None);
    }
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
    let duration_ms = t["duration_ms"].as_i64();
    let explicit = t["explicit"].as_bool().map(|b| b as i64);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks
             (service, service_track_id, title, artists, duration_ms, explicit, album_id, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(service, service_track_id) DO UPDATE SET
             title       = excluded.title,
             artists     = excluded.artists,
             duration_ms = COALESCE(excluded.duration_ms, hub_tracks.duration_ms),
             explicit    = COALESCE(excluded.explicit, hub_tracks.explicit),
             album_id    = COALESCE(hub_tracks.album_id, excluded.album_id)
         RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .bind(&artists)
    .bind(duration_ms)
    .bind(explicit)
    .bind(album_id)
    .bind(now_iso())
    .fetch_one(pool)
    .await?;
    crate::artists::upsert_track_artists(pool, id, &t["artists"]).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO hub_track_external_ids
             (track_id, service, external_id, url, source, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, 'crawl', ?4)",
    )
    .bind(id)
    .bind(sid)
    .bind(format!("https://open.spotify.com/track/{sid}"))
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(Some(id))
}

/// Upsert the crawl frontier state for one artist/album.
async fn mark_crawl(
    pool: &SqlitePool,
    kind: &str,
    reference: &str,
    state: &str,
    error: Option<&str>,
) -> Result<()> {
    let now = now_iso();
    sqlx::query(
        "INSERT INTO hub_crawl_state
             (kind, ref, state, attempts, last_error, crawled_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?5, ?5)
         ON CONFLICT(kind, ref) DO UPDATE SET
             state      = excluded.state,
             attempts   = hub_crawl_state.attempts + 1,
             last_error = excluded.last_error,
             crawled_at = CASE WHEN excluded.state = 'done'
                               THEN excluded.crawled_at ELSE hub_crawl_state.crawled_at END,
             updated_at = excluded.updated_at",
    )
    .bind(kind)
    .bind(reference)
    .bind(state)
    .bind(error)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_clause_builds_placeholders() {
        assert_eq!(in_clause(1), "?");
        assert_eq!(in_clause(3), "?,?,?");
    }
}
