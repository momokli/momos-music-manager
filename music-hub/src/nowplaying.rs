//! "Now playing" — the track a user is currently playing in Spotify (issue
//! #293/#294). Polled lazily by the shell widget; used both to show the track
//! and to tag it directly.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use crate::config::Config;

/// The Spotify scope required to read the currently-playing track.
pub const SCOPE: &str = "user-read-currently-playing";

/// Spotify accepts either of these for `GET /me/player/queue`.
pub const QUEUE_SCOPES: &[&str] = &["user-read-playback-state", "user-read-currently-playing"];

/// The scope required to append to the queue (`POST /me/player/queue`).
pub const PLAYBACK_WRITE_SCOPE: &str = "user-modify-playback-state";

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

/// The user's stored Spotify grant's scope string, if any.
async fn stored_scopes(pool: &SqlitePool, user_id: i64) -> Option<String> {
    sqlx::query_scalar(
        "SELECT scopes FROM hub_service_accounts WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Whether `scopes` grants `want` (space-separated per RFC 6749).
fn grants(scopes: Option<String>, want: &[&str]) -> bool {
    scopes
        .map(|s| {
            let set: Vec<&str> = s.split_whitespace().collect();
            want.iter().any(|w| set.contains(w))
        })
        .unwrap_or(false)
}

/// Whether the user's stored Spotify grant includes the now-playing scope.
pub async fn has_scope(pool: &SqlitePool, user_id: i64) -> bool {
    grants(stored_scopes(pool, user_id).await, &[SCOPE])
}

/// Whether the user's grant lets us read the playback queue.
pub async fn has_queue_scope(pool: &SqlitePool, user_id: i64) -> bool {
    grants(stored_scopes(pool, user_id).await, QUEUE_SCOPES)
}

/// Whether the user's grant lets us control playback (queue append, skip, …).
pub async fn has_playback_write_scope(pool: &SqlitePool, user_id: i64) -> bool {
    grants(stored_scopes(pool, user_id).await, &[PLAYBACK_WRITE_SCOPE])
}

fn artist_line(item: &serde_json::Value) -> String {
    item["artists"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// The smallest album cover Spotify returned (same pick as widgets elsewhere).
fn cover_url(item: &serde_json::Value) -> String {
    item["album"]["images"]
        .as_array()
        .and_then(|i| i.last())
        .and_then(|x| x["url"].as_str())
        .unwrap_or("")
        .to_string()
}

/// Resolve a Spotify track object to a hub track, **ingesting it on the fly**
/// (with artist/album/cover/isrc) when we do not have it yet (#319). Shared by
/// the now-playing poll and the queue read.
async fn hub_track_for(pool: &SqlitePool, item: &serde_json::Value) -> Option<i64> {
    let Some(sid) = item["id"].as_str() else {
        return None;
    };
    let known = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM hub_tracks WHERE service = 'spotify' AND service_track_id = ?1 LIMIT 1",
    )
    .bind(sid)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match known {
        Some(id) => Some(id),
        None => crate::ingest::ingest_spotify_item(pool, item)
            .await
            .ok()
            .flatten(),
    }
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
    let artists = artist_line(item);
    let hub_track_id = hub_track_for(pool, item).await;
    // Record a listening event (coalesced per session) when the track is playing.
    let playing = v["is_playing"].as_bool().unwrap_or(false);
    if playing {
        if let Some(tid) = hub_track_id {
            let new_session = record_listen(
                pool,
                user_id,
                tid,
                v["progress_ms"].as_i64().unwrap_or(0),
                item["duration_ms"].as_i64().unwrap_or(0),
            )
            .await
            .unwrap_or(false);
            // Fast-lane order at music-api once per session (not every poll),
            // plus seed the track's artists into the crawl frontier so the
            // cascade starts immediately.
            if new_session {
                let pool = pool.clone();
                let cfg = cfg.clone();
                tokio::spawn(async move {
                    let _ = crate::crawl::order_now_playing(&pool, &cfg, tid).await;
                    let _ = crate::crawl::seed_artist_frontier(&pool, &[tid]).await;
                });
            }
        }
    }
    Ok(Some(Now {
        playing,
        hub_track_id,
        title: item["name"].as_str().unwrap_or("").to_string(),
        artists,
        album: item["album"]["name"].as_str().unwrap_or("").to_string(),
        image: cover_url(item),
        progress_ms: v["progress_ms"].as_i64().unwrap_or(0),
        duration_ms: item["duration_ms"].as_i64().unwrap_or(0),
    }))
}

// ── playback queue ──────────────────────────────────────────────────────────

/// One entry of Spotify's "Next up" queue.
#[derive(Debug, Clone, Default)]
pub struct QueueItem {
    /// The corresponding hub track, when we have it (unknown items are ingested).
    pub hub_track_id: Option<i64>,
    pub title: String,
    pub artists: String,
    pub image: String,
    pub duration_ms: i64,
}

impl QueueItem {
    pub fn duration_disp(&self) -> String {
        fmt_ms(self.duration_ms)
    }
}

/// Read the user's playback queue.
///
/// NOTE: this is Spotify's *manually queued* "Next up" list — not the rest of
/// the current context (playlist/album), and it is practically capped. It can
/// be appended to but never reordered or emptied via the API.
pub async fn queue(pool: &SqlitePool, cfg: &Config, user_id: i64) -> Result<Vec<QueueItem>> {
    let token = crate::ingest::access_token(pool, cfg, user_id).await?;
    let url = format!("{}/me/player/queue", cfg.spotify_api_base);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Vec::new());
    }
    if !resp.status().is_success() {
        anyhow::bail!("spotify {url} -> {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await.unwrap_or_default();
    let mut out = Vec::new();
    for item in v["queue"].as_array().map(Vec::as_slice).unwrap_or_default() {
        out.push(QueueItem {
            hub_track_id: hub_track_for(pool, item).await,
            title: item["name"].as_str().unwrap_or("").to_string(),
            artists: artist_line(item),
            image: cover_url(item),
            duration_ms: item["duration_ms"].as_i64().unwrap_or(0),
        });
    }
    Ok(out)
}

/// The `spotify:track:…` uri for a hub track, if it has a Spotify id.
pub async fn spotify_uri(pool: &SqlitePool, track_id: i64) -> Option<String> {
    let sid: Option<String> = sqlx::query_scalar(
        "SELECT service_track_id FROM hub_tracks
          WHERE id = ?1 AND service = 'spotify'
            AND service_track_id IS NOT NULL AND TRIM(service_track_id) <> ''",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    sid.map(|s| format!("spotify:track:{s}"))
}

/// [`spotify_uri`] for many tracks at once: deduped, given order preserved.
/// Same source of truth (`hub_tracks.service_track_id`) so the single and bulk
/// paths can never disagree.
pub async fn spotify_uris(pool: &SqlitePool, track_ids: &[i64]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for chunk in track_ids.chunks(500) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT service_track_id FROM hub_tracks
              WHERE service = 'spotify' AND service_track_id IS NOT NULL
                AND TRIM(service_track_id) <> '' AND id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        sep.push_unseparated(")");
        for r in qb.build().fetch_all(pool).await? {
            let sid: String = r.get("service_track_id");
            if seen.insert(sid.clone()) {
                out.push(format!("spotify:track:{sid}"));
            }
        }
    }
    Ok(out)
}

/// How many uris we put into a single `PUT /me/player/play`.
///
/// Spotify does not document a limit for the `uris` array, so this is *our*
/// conservative cap — a 500-track tag must not turn into one huge request. The
/// caller reports the truncation instead of silently dropping tracks.
pub const PLAY_URIS_CAP: usize = 100;

/// Play `uris` as the current playback context (`PUT /me/player/play`).
///
/// Returns how many uris were actually sent (the caller reports truncation).
/// Premium + an active device required, exactly like [`transport`].
pub async fn play_uris(
    pool: &SqlitePool,
    cfg: &Config,
    user_id: i64,
    uris: &[String],
) -> Result<usize> {
    let capped = &uris[..uris.len().min(PLAY_URIS_CAP)];
    if capped.is_empty() {
        anyhow::bail!("Keine abspielbaren Spotify-Tracks gefunden.");
    }
    let token = crate::ingest::access_token(pool, cfg, user_id).await?;
    let url = format!("{}/me/player/play", cfg.spotify_api_base);
    let resp = reqwest::Client::new()
        .put(&url)
        .bearer_auth(&token)
        .json(&serde_json::json!({ "uris": capped }))
        .send()
        .await
        .with_context(|| format!("PUT {url}"))?;
    ok_or_player_error(resp).await?;
    Ok(capped.len())
}

/// Append `uri` to the user's playback queue (`POST /me/player/queue`).
///
/// Spotify requires **Premium** and an **active device** for this; both failures
/// come back as an actionable message via [`player_error`] so the UI can show
/// them verbatim instead of a generic 500.
pub async fn add_to_queue(pool: &SqlitePool, cfg: &Config, user_id: i64, uri: &str) -> Result<()> {
    let token = crate::ingest::access_token(pool, cfg, user_id).await?;
    let url = format!(
        "{}/me/player/queue?uri={}",
        cfg.spotify_api_base,
        urlencoding::encode(uri)
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&token)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    ok_or_player_error(resp).await
}

/// A stateless player transport command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Next,
    Previous,
    Play,
    Pause,
}

impl Transport {
    /// Spotify's Player API splits **actions** from **state changes**: skip is
    /// `POST`, play/pause are `PUT`. Getting this wrong 405s live, which no mock
    /// would catch — so the tests pin the method per command.
    fn method(self) -> reqwest::Method {
        match self {
            Transport::Next | Transport::Previous => reqwest::Method::POST,
            Transport::Play | Transport::Pause => reqwest::Method::PUT,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Transport::Next => "/me/player/next",
            Transport::Previous => "/me/player/previous",
            Transport::Play => "/me/player/play",
            Transport::Pause => "/me/player/pause",
        }
    }

    /// Parse the `{cmd}` path segment of the transport endpoint.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "next" => Some(Self::Next),
            "previous" => Some(Self::Previous),
            "play" => Some(Self::Play),
            "pause" => Some(Self::Pause),
            _ => None,
        }
    }

    /// The success message (also used as the button tooltip).
    pub fn label(self) -> &'static str {
        match self {
            Transport::Next => "Nächster Track",
            Transport::Previous => "Vorheriger Track",
            Transport::Play => "Weiter abspielen",
            Transport::Pause => "Pause",
        }
    }
}

/// Send a transport command (Premium + an active device required).
pub async fn transport(
    pool: &SqlitePool,
    cfg: &Config,
    user_id: i64,
    cmd: Transport,
) -> Result<()> {
    let token = crate::ingest::access_token(pool, cfg, user_id).await?;
    let url = format!("{}{}", cfg.spotify_api_base, cmd.path());
    let resp = reqwest::Client::new()
        .request(cmd.method(), &url)
        .bearer_auth(&token)
        .send()
        .await
        .with_context(|| format!("{} {url}", cmd.method()))?;
    ok_or_player_error(resp).await
}

/// `2xx` → `Ok`, anything else → an actionable message via [`player_error`].
async fn ok_or_player_error(resp: reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    anyhow::bail!(
        "{}",
        player_error(
            status.as_u16(),
            body["error"]["reason"].as_str().unwrap_or(""),
            body["error"]["message"].as_str().unwrap_or(""),
        )
    )
}

/// Turn a Spotify player failure into an actionable German message.
pub fn player_error(status: u16, reason: &str, message: &str) -> String {
    match (status, reason) {
        (404, "NO_ACTIVE_DEVICE") => {
            "Kein aktives Spotify-Gerät — starte die Wiedergabe und versuch es erneut.".to_string()
        }
        (403, "PREMIUM_REQUIRED") | (403, "PREMIUM_REQUIRED_EXTENSION") => {
            "Das braucht Spotify Premium.".to_string()
        }
        (403, "INSUFFICIENT_SCOPE") => {
            "Spotify hat den Zugriff abgelehnt — bitte neu verbinden (Scope user-modify-playback-state)."
                .to_string()
        }
        (403, "RESTRICTION_VIOLATED") => {
            "Spotify erlaubt das hier nicht (z. B. Werbung oder ein gesperrter Track).".to_string()
        }
        (403, _) => "Spotify hat den Zugriff abgelehnt — bitte neu verbinden.".to_string(),
        (429, _) => "Spotify-Rate-Limit erreicht — kurz warten und erneut versuchen.".to_string(),
        _ => {
            let detail = if reason.is_empty() { message } else { reason };
            format!("Spotify-Fehler {status}: {detail}")
        }
    }
}

/// Record a listening event, coalescing consecutive polls of the same track
/// into a single row (a "session", extended while it keeps playing) so the
/// 20 s polling does not flood the table (#319). Returns `true` when a new
/// session row was inserted (used to fire the now-playing fast-lane order once).
pub async fn record_listen(
    pool: &SqlitePool,
    user_id: i64,
    track_id: i64,
    progress_ms: i64,
    duration_ms: i64,
) -> Result<bool> {
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
            return Ok(false);
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
    Ok(true)
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
