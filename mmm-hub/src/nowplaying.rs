//! "Now playing" — the track a user is currently playing in Spotify (issue
//! #293/#294). Polled lazily by the shell widget; used both to show the track
//! and to tag it directly.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use crate::config::Config;

/// The Spotify scope required to read the currently-playing track.
pub const SCOPE: &str = "user-read-currently-playing";

/// A snapshot of the user's current playback.
#[derive(Debug, Clone, Default)]
pub struct Now {
    pub playing: bool,
    /// The corresponding hub track, when we have it.
    pub hub_track_id: Option<i64>,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub image: String,
    pub progress_ms: i64,
    pub duration_ms: i64,
}

impl Now {
    pub fn progress_pct(&self) -> i64 {
        if self.duration_ms > 0 {
            (self.progress_ms * 100 / self.duration_ms).clamp(0, 100)
        } else {
            0
        }
    }
    pub fn duration_disp(&self) -> String {
        fmt_ms(self.duration_ms)
    }
    pub fn progress_disp(&self) -> String {
        fmt_ms(self.progress_ms)
    }
}

fn fmt_ms(ms: i64) -> String {
    let s = (ms.max(0)) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Whether the user's stored Spotify grant includes the now-playing scope.
pub async fn has_scope(pool: &SqlitePool, user_id: i64) -> bool {
    let scopes: Option<String> = sqlx::query_scalar(
        "SELECT scopes FROM hub_service_accounts WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    scopes
        .map(|s| s.split_whitespace().any(|x| x == SCOPE))
        .unwrap_or(false)
}

/// Fetch the user's currently-playing track (`None` when nothing is playing).
pub async fn current(pool: &SqlitePool, cfg: &Config, user_id: i64) -> Result<Option<Now>> {
    let token = crate::ingest::access_token(pool, cfg, user_id).await?;
    let url = format!("{}/me/player/currently-playing", cfg.spotify_api_base);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !resp.status().is_success() {
        anyhow::bail!("spotify {url} -> {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await.unwrap_or_default();
    let item = &v["item"];
    let track_id = item["id"].as_str().map(str::to_string);
    let artists = item["artists"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let hub_track_id = match &track_id {
        Some(id) => sqlx::query_scalar::<_, i64>(
            "SELECT id FROM hub_tracks WHERE service = 'spotify' AND service_track_id = ?1 LIMIT 1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten(),
        None => None,
    };
    // Unknown track: ingest it (with artist/album/cover/isrc) on the fly (#319).
    let hub_track_id = match hub_track_id {
        Some(id) => Some(id),
        None if track_id.is_some() => crate::ingest::ingest_spotify_item(pool, item)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    // Record a listening event (coalesced per session) when the track is playing.
    let playing = v["is_playing"].as_bool().unwrap_or(false);
    if playing {
        if let Some(tid) = hub_track_id {
            let _ = record_listen(
                pool,
                user_id,
                tid,
                v["progress_ms"].as_i64().unwrap_or(0),
                item["duration_ms"].as_i64().unwrap_or(0),
            )
            .await;
        }
    }
    Ok(Some(Now {
        playing,
        hub_track_id,
        title: item["name"].as_str().unwrap_or("").to_string(),
        artists,
        album: item["album"]["name"].as_str().unwrap_or("").to_string(),
        image: item["album"]["images"]
            .as_array()
            .and_then(|i| i.last())
            .and_then(|x| x["url"].as_str())
            .unwrap_or("")
            .to_string(),
        progress_ms: v["progress_ms"].as_i64().unwrap_or(0),
        duration_ms: item["duration_ms"].as_i64().unwrap_or(0),
    }))
}

/// Record a listening event, coalescing consecutive polls of the same track
/// into a single row (a "session", extended while it keeps playing) so the
/// 20 s polling does not flood the table (#319).
pub async fn record_listen(
    pool: &SqlitePool,
    user_id: i64,
    track_id: i64,
    progress_ms: i64,
    duration_ms: i64,
) -> Result<()> {
    let now = chrono::Utc::now();
    let now_iso = now.to_rfc3339();
    let last: Option<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, track_id, played_at FROM hub_listening_history
          WHERE user_id = ?1 ORDER BY played_at DESC, id DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if let Some((id, tid, ts)) = last {
        if tid == track_id && within_session(&ts, &now) {
            sqlx::query(
                "UPDATE hub_listening_history SET played_at = ?2, progress_ms = ?3 WHERE id = ?1",
            )
            .bind(id)
            .bind(&now_iso)
            .bind(progress_ms)
            .execute(pool)
            .await?;
            return Ok(());
        }
    }
    sqlx::query(
        "INSERT INTO hub_listening_history
             (user_id, track_id, played_at, progress_ms, duration_ms, source)
         VALUES (?1, ?2, ?3, ?4, ?5, 'spotify')",
    )
    .bind(user_id)
    .bind(track_id)
    .bind(&now_iso)
    .bind(progress_ms)
    .bind(duration_ms)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether a stored `played_at` is close enough in time to extend the same
/// listening session (within 15 minutes).
fn within_session(ts: &str, now: &chrono::DateTime<chrono::Utc>) -> bool {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| {
            now.signed_duration_since(t.with_timezone(&chrono::Utc))
                .num_minutes()
                .unsigned_abs()
                < 15
        })
        .unwrap_or(false)
}
