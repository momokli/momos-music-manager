//! Curated playlists (#316): user-built playlists whose track set is derived
//! from a tag or a smart "lens", and which can be pushed to Spotify.
//!
//! Resolution is always done here (server-side); the UI only displays the result
//! and toggles settings.

use std::collections::{HashMap, HashSet};

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
/// `manual` items are always included (and listed first); the tag/lens set is
/// appended, so hand-added tracks survive alongside a smart source.
pub async fn resolve_track_ids(pool: &SqlitePool, cp: &CuratedPlaylist) -> Result<Vec<i64>> {
    let limit = cp.fill_limit.clamp(1, 500);
    let manual: Vec<i64> = sqlx::query_scalar(
        "SELECT track_id FROM hub_curated_items
          WHERE playlist_id = ?1 ORDER BY position, added_at",
    )
    .bind(cp.id)
    .fetch_all(pool)
    .await?;

    let derived: Vec<i64> = match cp.fill_mode.as_str() {
        "tag" => {
            let Some(tag_id) = cp.fill_tag_id else {
                return Ok(manual);
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
        // manual mode: hand-picked items are the whole set.
        _ => return Ok(manual),
    };

    let mut out = manual;
    for id in derived {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Add a track to a curated playlist (idempotent), bumping `updated_at`.
pub async fn add_item(pool: &SqlitePool, playlist_id: i64, track_id: i64) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO hub_curated_items (playlist_id, track_id, position, added_at)
         VALUES (?1, ?2,
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM hub_curated_items WHERE playlist_id = ?1),
                 ?3)",
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE hub_curated_playlists SET updated_at = ?2 WHERE id = ?1")
        .bind(playlist_id)
        .bind(&now)
        .execute(pool)
        .await?;
    Ok(())
}

/// Explicit-position variant of [`add_item`] (setlist ordering). Idempotent.
pub async fn add_item_at(
    pool: &SqlitePool,
    playlist_id: i64,
    track_id: i64,
    position: i64,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO hub_curated_items (playlist_id, track_id, position, added_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(position)
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE hub_curated_playlists SET updated_at = ?2 WHERE id = ?1")
        .bind(playlist_id)
        .bind(&now)
        .execute(pool)
        .await?;
    Ok(())
}

/// How many of the playlist's seeds feed the embedding-neighbour scan (cosine is
/// O(n) per seed, so a large playlist must not blow up the request).
const SEED_SAMPLE_CAP: usize = 20;

/// A suggested track for a curated playlist (#299).
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub track_id: i64,
    pub title: String,
    pub artists: String,
    /// Linked-artist HTML (filled for the returned slice only).
    pub artists_html: String,
    pub score: f64,
    /// Human-readable reasons, `(label, count)`, most relevant first.
    pub why: Vec<(String, i64)>,
}

/// Batch-resolve Camelot keys for the given track ids from the unified audio view.
async fn camelot_map(pool: &SqlitePool, ids: &[i64]) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(900) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT track_id, camelot FROM v_track_audio
              WHERE camelot IS NOT NULL AND TRIM(camelot) <> '' AND track_id IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let key: String = r.get("camelot");
            if !key.trim().is_empty() {
                out.insert(r.get("track_id"), key);
            }
        }
    }
    out
}

/// Adopting a `Suggestion` means [`add_item`] on its `track_id`.
///
/// Score per candidate is additive over the seed set (aggregated with **max**, so
/// a bigger playlist does not win by size alone):
/// `score = w_sim·max_tag_overlap + w_occ·shared_playlists
///        + w_audio·max_cosine + w_key·camelot_bonus + w_ripe·ripeness`.
/// Weights come from the live engine config (`settings::engine`). Candidates are
/// every `hub_tracks` row minus the seeds, minus tracks whose only tags are
/// SETLIST/RUMPELKISTE, minus tracks the viewer already curated elsewhere.
pub async fn suggestions(
    pool: &SqlitePool,
    cp: &CuratedPlaylist,
    limit: usize,
    viewer_id: i64,
) -> Result<Vec<Suggestion>> {
    let e = crate::settings::engine(pool).await;
    let seeds = resolve_track_ids(pool, cp).await?;
    if seeds.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let seed_set: HashSet<i64> = seeds.iter().copied().collect();
    // Cosine is O(n) per seed → cap the seed sample (deterministic: resolve order).
    let seed_sample: Vec<i64> = seeds.iter().copied().take(SEED_SAMPLE_CAP).collect();

    // Tracks the viewer already has in one of *their* curated playlists.
    let mut owned: HashSet<i64> = HashSet::new();
    for r in sqlx::query(
        "SELECT DISTINCT i.track_id FROM hub_curated_items i
           JOIN hub_curated_playlists p ON p.id = i.playlist_id
          WHERE p.user_id = ?1",
    )
    .bind(viewer_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    {
        owned.insert(r.get(0));
    }

    // Candidate universe: keep untagged tracks, or tracks with >=1 non-junk tag.
    let cand_rows = sqlx::query(
        "SELECT t.id, COALESCE(t.title, '') AS title, COALESCE(t.artists, '') AS artists
           FROM hub_tracks t
          WHERE NOT EXISTS (SELECT 1 FROM hub_track_resolved_tags rt WHERE rt.track_id = t.id)
             OR EXISTS (SELECT 1 FROM hub_track_resolved_tags rt
                          JOIN hub_tags tg ON tg.id = rt.tag_id
                         WHERE rt.track_id = t.id
                           AND (tg.type IS NULL OR tg.type NOT IN ('SETLIST', 'RUMPELKISTE')))
          ORDER BY t.id",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let candidates: Vec<(i64, String, String)> = cand_rows
        .iter()
        .map(|r| {
            (
                r.get::<i64, _>("id"),
                r.get::<String, _>("title"),
                r.get::<String, _>("artists"),
            )
        })
        .collect();
    let candidate_ids: Vec<i64> = candidates.iter().map(|c| c.0).collect();

    // Tag-overlap similarity to the seeds (batched, max-aggregated).
    let mut sim: HashMap<i64, f64> = HashMap::new();
    for &seed in &seed_sample {
        for (cid, v) in crate::similarity::similarity_to_seed(pool, seed, &candidate_ids, &e, &[])
            .await
        {
            let slot = sim.entry(cid).or_insert(0.0);
            if v > *slot {
                *slot = v;
            }
        }
    }

    // Playlist co-occurrence with the seeds (Rumpelkiste-aware).
    let mut occ: HashMap<i64, f64> = HashMap::new();
    for &seed in &seed_sample {
        for s in crate::digging::internal_suggestions(pool, seed, 500)
            .await
            .unwrap_or_default()
        {
            if seed_set.contains(&s.id) {
                continue;
            }
            let v = s.shared_playlists as f64;
            let slot = occ.entry(s.id).or_insert(0.0);
            if v > *slot {
                *slot = v;
            }
        }
    }

    // EffNet embedding neighbourhood (`similar::neighbors`).
    let mut cos: HashMap<i64, f64> = HashMap::new();
    for &seed in &seed_sample {
        for n in crate::similar::neighbors(pool, seed, 200)
            .await
            .unwrap_or_default()
        {
            if seed_set.contains(&n.track_id) {
                continue;
            }
            let v = n.score as f64;
            let slot = cos.entry(n.track_id).or_insert(0.0);
            if v > *slot {
                *slot = v;
            }
        }
    }

    // Harmonic (Camelot) compatibility: any compatible seed key → bonus.
    let mut key_ids: Vec<i64> = seed_sample.clone();
    key_ids.extend_from_slice(&candidate_ids);
    let keys = camelot_map(pool, &key_ids).await;
    let seed_keys: Vec<String> = seed_sample
        .iter()
        .filter_map(|s| keys.get(s).cloned())
        .collect();
    let mut key_hit: HashMap<i64, f64> = HashMap::new();
    if !seed_keys.is_empty() {
        for (cid, _, _) in &candidates {
            if let Some(ck) = keys.get(cid) {
                if seed_keys
                    .iter()
                    .any(|sk| crate::features::camelot_compatible(sk, ck))
                {
                    key_hit.insert(*cid, 1.0);
                }
            }
        }
    }

    // Data-quality tiebreak.
    let ripe = crate::scoring::ripeness_many(pool, &candidate_ids, &e, &[]).await;

    // Weights (reuse the live engine knobs; no hard-coding).
    let w_sim = e.sim_factor;
    let w_occ = e.shared_factor;
    let w_audio = e.rec_cluster;
    let w_key = e.tag_overlap_base;
    let w_ripe = e.ripeness_factor;

    let mut out: Vec<Suggestion> = Vec::new();
    for (cid, title, artists) in &candidates {
        if seed_set.contains(cid) || owned.contains(cid) {
            continue;
        }
        let s_sim = sim.get(cid).copied().unwrap_or(0.0);
        let s_occ = occ.get(cid).copied().unwrap_or(0.0);
        let s_cos = cos.get(cid).copied().unwrap_or(0.0);
        let s_key = key_hit.get(cid).copied().unwrap_or(0.0);
        let s_ripe = ripe.get(cid).copied().unwrap_or(0.0);
        let score =
            w_sim * s_sim + w_occ * s_occ + w_audio * s_cos + w_key * s_key + w_ripe * s_ripe;
        if score <= 0.0 {
            continue;
        }
        let mut why = Vec::new();
        if s_sim > 0.0 {
            why.push(("Tags geteilt".to_string(), s_sim as i64));
        }
        if s_occ > 0.0 {
            why.push(("Playlists geteilt".to_string(), s_occ as i64));
        }
        if s_cos > 0.0 {
            why.push(("Klang-Nachbarschaft".to_string(), 1));
        }
        if s_key > 0.0 {
            why.push(("Harmonisch (Key)".to_string(), 1));
        }
        out.push(Suggestion {
            track_id: *cid,
            title: title.clone(),
            artists: artists.clone(),
            artists_html: String::new(),
            score,
            why,
        });
    }

    // Deterministic ordering: score desc, then artists, title, id.
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.artists.cmp(&b.artists))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.track_id.cmp(&b.track_id))
    });
    out.truncate(limit);

    let ids: Vec<i64> = out.iter().map(|s| s.track_id).collect();
    let mut html = crate::artists::artists_html_for_tracks(pool, &ids).await;
    for s in out.iter_mut() {
        s.artists_html = html.remove(&s.track_id).unwrap_or_default();
    }
    Ok(out)
}

/// Create a `manual` curated playlist and return its id.
pub async fn create_manual(pool: &SqlitePool, user_id: i64, name: &str) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_curated_playlists
            (user_id, name, description, is_public, fill_mode, fill_limit, daily, created_at, updated_at)
         VALUES (?1, ?2, '', 0, 'manual', 50, 0, ?3, ?3)
         RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(&now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Map hub track ids to Spotify track URIs (order preserved, duplicates dropped).
pub(crate) async fn spotify_uris(pool: &SqlitePool, track_ids: &[i64]) -> Result<Vec<String>> {
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

/// A failed Spotify playlist write. Carries the HTTP status so callers can tell a
/// terminal `429`/quota wall apart from a generic failure (→ push status `quota`).
#[derive(Debug)]
pub(crate) struct SpotifyWriteError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for SpotifyWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SpotifyWriteError {}

fn write_failed(status: u16, message: String) -> anyhow::Error {
    anyhow::Error::new(SpotifyWriteError { status, message })
}

/// Add URIs to a Spotify playlist in batches of 100 (Spotify's per-request cap).
pub(crate) async fn spotify_add_uris(
    base: &str,
    token: &str,
    remote_id: &str,
    uris: &[String],
) -> Result<()> {
    let url = crate::spotify::api_url(base, &format!("/playlists/{remote_id}/items"));
    for chunk in uris.chunks(100) {
        let (status, resp) =
            crate::spotify::post_json(token, &url, &json!({ "uris": chunk })).await?;
        if !(200..300).contains(&status) {
            return Err(write_failed(
                status,
                format!("Spotify add items -> {status}: {}", spotify_err(&resp)),
            ));
        }
    }
    Ok(())
}

/// Remove URIs from a Spotify playlist in batches of 100. The post-Feb-2026
/// endpoint takes `{"tracks":[{"uri":…}]}` in the body.
pub(crate) async fn spotify_remove_uris(
    base: &str,
    token: &str,
    remote_id: &str,
    uris: &[String],
) -> Result<()> {
    let url = crate::spotify::api_url(base, &format!("/playlists/{remote_id}/items"));
    for chunk in uris.chunks(100) {
        let tracks: Vec<serde_json::Value> =
            chunk.iter().map(|u| json!({ "uri": u })).collect();
        let (status, resp) =
            crate::spotify::delete_json(token, &url, &json!({ "tracks": tracks })).await?;
        if !(200..300).contains(&status) {
            return Err(write_failed(
                status,
                format!("Spotify remove items -> {status}: {}", spotify_err(&resp)),
            ));
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

#[cfg(test)]
mod tests {
    use super::spotify_add_uris;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::Value;

    /// 101 URIs must be sent as two batches (100 + 1).
    #[tokio::test]
    async fn add_uris_chunks_at_100() {
        let seen: Arc<Mutex<Vec<usize>>> = Arc::default();
        async fn add(
            State(seen): State<Arc<Mutex<Vec<usize>>>>,
            Path(_id): Path<String>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            seen.lock()
                .unwrap()
                .push(body["uris"].as_array().unwrap().len());
            Json(serde_json::json!({}))
        }
        let app = Router::new()
            .route("/playlists/{id}/items", post(add))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let uris: Vec<String> = (0..101).map(|i| format!("spotify:track:{i}")).collect();
        spotify_add_uris(&format!("http://{addr}"), "tok", "rem", &uris)
            .await
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![100, 1]);
    }
}
