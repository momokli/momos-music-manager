//! Backend worker: the only place that talks to Spotify for sync.
//!
//! Its jobs, driven purely by flags in the DB (the frontend only toggles those):
//!   * liked tracks per connected account   (`hub_service_accounts.likes_status`)
//!   * playlist items for `enabled_for_fetch` playlists (`items_available = 0`)
//!
//! It runs continuously while there is work, one request stream at a time, with a
//! small page delay and hard backoff on 429 / dev-mode quota.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::ingest;
use crate::settings;
use crate::spotify;

const IDLE_WAIT: Duration = Duration::from_secs(10);
const BUSY_WAIT: Duration = Duration::from_secs(1);
/// Hard cap on the in-memory event ring-buffer (issue #350).
pub const EVENT_CAPACITY: usize = 200;
const MAX_PLAYLISTS_PER_PASS: usize = 12;
const MAX_LIKES_ACCOUNTS_PER_PASS: usize = 2;
const PAGE_DELAY: Duration = Duration::from_millis(150);
/// Default cadence of the playlist listing poll (#506): 15 min.
const PLAYLIST_POLL_DEFAULT_SECS: u64 = 900;
/// Lower bound for the configurable cadence (#506).
const PLAYLIST_POLL_MIN_SECS: u64 = 60;

/// Cadence of the backpack content refresh pass (#682): 30 min, like the
/// curation/tonies loops.
const BACKPACK_REFRESH_SECS: u64 = 1800;

/// The eight Hub-Owned loop names, in report order (issues #353, #503, #506,
/// #682).
///
/// The crawl domain is served by two tasks (vault/order refresh + Spotify
/// touch-cascade); both report under `"crawl"` so the domain is one visible
/// loop, matching the plan. `"playlists"` is the listing poll that keeps
/// `hub_playlists.snapshot_id` fresh without a user clicking "Fetch".
/// `"backpacks"` materialises backpack item content for the view (#682).
pub const LOOP_NAMES: [&str; 8] = [
    "enrichment",
    "freqblog",
    "spotify",
    "crawl",
    "curation",
    "tonies",
    "playlists",
    "backpacks",
];

/// In-memory health registry for the Hub's background loops (issue #353).
///
/// Cheap (lock per `record`, no DB write): each loop reports once per iteration
/// whether it did work and the last error; read back via [`snapshot`].
///
/// [`snapshot`]: WorkerHealth::snapshot
#[derive(Clone)]
pub struct WorkerHealth(Arc<Mutex<WorkerHealthInner>>);

#[derive(Default)]
struct WorkerHealthInner {
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    loops: BTreeMap<&'static str, LoopState>,
    /// Bounded ring-buffer of the most recent iterations, oldest first.
    events: VecDeque<Event>,
}

#[derive(Clone, Default)]
struct LoopState {
    last_run_at: Option<String>,
    last_worked: bool,
    iterations: u64,
    /// Cumulative external requests attributed to this loop (#506).
    requests: u64,
    last_error: Option<String>,
    /// In-memory pause flag (issue #350): a paused loop skips its work and
    /// records nothing until resumed. Lost on restart (accepted).
    paused: bool,
}

/// One recorded loop iteration, surfaced in the ops log view (issue #350).
#[derive(Debug, Clone)]
pub struct Event {
    pub at: String,
    pub loop_name: &'static str,
    pub worked: bool,
    pub error: Option<String>,
}

/// Owned, JSON-ready snapshot of one loop's state.
#[derive(Debug, Clone, Default)]
pub struct LoopSnapshot {
    pub name: &'static str,
    pub last_run_at: Option<String>,
    pub last_worked: bool,
    pub iterations: u64,
    /// Cumulative external requests attributed to this loop (#506).
    pub requests: u64,
    pub last_error: Option<String>,
    pub paused: bool,
}

/// Owned snapshot of the whole registry.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub started_at: Option<String>,
    /// All registered loops, name-sorted (BTreeMap order).
    pub loops: Vec<LoopSnapshot>,
}

impl Default for WorkerHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerHealth {
    /// Fresh registry with all seven loops registered and `started_at = now`.
    pub fn new() -> Self {
        let mut inner = WorkerHealthInner {
            started_at: Some(chrono::Utc::now()),
            ..Default::default()
        };
        for name in LOOP_NAMES {
            inner.loops.entry(name).or_default();
        }
        Self(Arc::new(Mutex::new(inner)))
    }

    /// Set the in-memory pause flag for a loop (issue #350).
    ///
    /// Unknown names are harmless: the entry is created lazily, like
    /// [`record`](WorkerHealth::record). A running pass is never cancelled;
    /// the flag takes effect at the next loop iteration.
    pub fn set_paused(&self, name: &'static str, paused: bool) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.loops.entry(name).or_default().paused = paused;
    }

    /// Whether a loop is currently paused (unknown names read as not paused).
    pub fn is_paused(&self, name: &'static str) -> bool {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.loops.get(name).map(|s| s.paused).unwrap_or(false)
    }

    /// Record one loop iteration. `err = None` clears a previous error.
    pub fn record(&self, name: &'static str, worked: bool, err: Option<String>) {
        self.record_with_requests(name, worked, err, 0);
    }

    /// Record one loop iteration plus external requests made during it.
    ///
    /// `requests` is added to the loop's cumulative counter so the status
    /// surface can show the request budget (#506).
    pub fn record_with_requests(
        &self,
        name: &'static str,
        worked: bool,
        err: Option<String>,
        requests: u64,
    ) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if inner.started_at.is_none() {
            inner.started_at = Some(chrono::Utc::now());
        }
        let at = chrono::Utc::now().to_rfc3339();
        let st = inner.loops.entry(name).or_default();
        st.last_run_at = Some(at.clone());
        st.last_worked = worked;
        st.iterations += 1;
        st.requests += requests;
        st.last_error = err.clone();
        // One event per iteration; drop the oldest when at capacity.
        if inner.events.len() >= EVENT_CAPACITY {
            inner.events.pop_front();
        }
        inner.events.push_back(Event {
            at,
            loop_name: name,
            worked,
            error: err,
        });
    }

    /// A cloned, newest-first view of the recent loop events (issue #350).
    pub fn events(&self) -> Vec<Event> {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.events.iter().rev().cloned().collect()
    }

    /// Owned snapshot of every registered loop.
    pub fn snapshot(&self) -> Snapshot {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let loops = inner
            .loops
            .iter()
            .map(|(name, st)| LoopSnapshot {
                name,
                last_run_at: st.last_run_at.clone(),
                last_worked: st.last_worked,
                iterations: st.iterations,
                requests: st.requests,
                last_error: st.last_error.clone(),
                paused: st.paused,
            })
            .collect();
        Snapshot {
            started_at: inner.started_at.map(|d| d.to_rfc3339()),
            loops,
        }
    }

    /// Seconds since the registry was created (0 when unknown).
    pub fn uptime_secs(&self) -> i64 {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .started_at
            .map(|d| {
                chrono::Utc::now()
                    .signed_duration_since(d)
                    .num_seconds()
                    .max(0)
            })
            .unwrap_or(0)
    }
}

/// Start all background loops and return the shared [`WorkerHealth`] registry
/// so the HTTP surface can report their state (issue #353).
pub fn spawn(pool: SqlitePool, cfg: Arc<Config>) -> WorkerHealth {
    let health = WorkerHealth::new();
    // Both Spotify-touching loops (item sync + listing poll) serialize their
    // passes so one tick never fetches playlists twice (#506).
    let spotify_tick: Arc<tokio::sync::Mutex<()>> = Arc::new(tokio::sync::Mutex::new(()));

    // Enrichment loop: audio features + genres. Local/external APIs that don't
    // touch Spotify — kept separate so Spotify quota backoffs can't starve them.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("enrichment") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                // On-demand ReccoBeats requests first, then the generic backlog.
                let p = crate::features::priority_once(&pool, &cfg, 50).await;
                let f = crate::features::sync_once(&pool, &cfg, 50).await;
                let g = crate::genres::sync_once(&pool, &cfg, 50).await;
                let worked = matches!(p, Ok(n) if n > 0)
                    || matches!(f, Ok(n) if n > 0)
                    || matches!(g, Ok(n) if n > 0);
                let err = p
                    .err()
                    .or_else(|| f.err())
                    .or_else(|| g.err())
                    .map(|e| e.to_string());
                health.record("enrichment", worked, err);
                tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
            }
        });
    }

    // FreqBlog fallback loop (slow, paid): only on-demand requests that
    // ReccoBeats already missed. Separate so the ~20s lookups can't block the
    // ReccoBeats/genre loops.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("freqblog") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let n = crate::features::freqblog_priority_once(&pool, &cfg, 5).await;
                let worked = matches!(n, Ok(v) if v > 0);
                health.record("freqblog", worked, n.err().map(|e| e.to_string()));
                tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
            }
        });
    }

    // Spotify loop: likes + playlist items (may back off on quota).
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        let spotify_tick = spotify_tick.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("spotify") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let guard = spotify_tick.lock().await;
                let r = run_once(&pool, &cfg).await;
                drop(guard);
                if let Err(e) = &r {
                    tracing::warn!("worker pass failed: {e}");
                }
                let worked = matches!(r, Ok(true));
                health.record("spotify", worked, r.err().map(|e| e.to_string()));
                tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
            }
        });
    }

    // music-api loop: pull the vault's resolved BPM/key into the read-model
    // (`hub_track_audio`) and drain the download-priority orders. Both are
    // bounded + idempotent. Part of the `crawl` domain (issue #353).
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("crawl") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let a = crate::analysis::refresh_from_vault(&pool, &cfg, 200).await;
                let o = crate::crawl::order_pending(&pool, &cfg, 500).await;
                let worked = matches!(a, Ok(n) if n > 0) || matches!(o, Ok(n) if n > 0);
                let err = a.err().or_else(|| o.err()).map(|e| e.to_string());
                health.record("crawl", worked, err);
                tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
            }
        });
    }

    // Spotify crawl loop: bounded touch-cascade (artist -> albums -> album
    // tracks). Own task so its quota backoffs can't starve the other loops.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("crawl") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let budget = crate::crawl::requests_per_pass(&pool).await;
                let n = crate::crawl::touch_cascade(&pool, &cfg, budget).await;
                let worked = matches!(n, Ok(v) if v > 0);
                health.record("crawl", worked, n.err().map(|e| e.to_string()));
                tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
            }
        });
    }

    // Curation loop: push daily curated playlists to Spotify (#316). Checks
    // every 30 min; `daily_due` gates each playlist to at most once per 20 h.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("curation") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let cutoff = (chrono::Utc::now() - chrono::Duration::hours(20)).to_rfc3339();
                let due = match crate::curate::daily_due(&pool, &cutoff).await {
                    Ok(due) => due,
                    Err(e) => {
                        tracing::warn!("curation loop: {e}");
                        health.record("curation", false, Some(e.to_string()));
                        tokio::time::sleep(Duration::from_secs(1800)).await;
                        continue;
                    }
                };
                let worked = !due.is_empty();
                let mut err: Option<String> = None;
                for cp in due {
                    match crate::curate::push(&pool, &cfg, &cp).await {
                        Ok(r) => {
                            tracing::info!("daily push '{}' -> {} tracks", cp.name, r.pushed)
                        }
                        Err(e) => {
                            tracing::warn!("daily push '{}' failed: {e}", cp.name);
                            let msg = e.to_string();
                            let _ = sqlx::query(
                                "UPDATE hub_curated_playlists
                                    SET last_push_status = 'error', last_push_error = ?2
                                  WHERE id = ?1",
                            )
                            .bind(cp.id)
                            .bind(&msg)
                            .execute(&pool)
                            .await;
                            if err.is_none() {
                                err = Some(msg);
                            }
                        }
                    }
                }
                health.record("curation", worked, err);
                tokio::time::sleep(Duration::from_secs(1800)).await;
            }
        });
    }

    // Tonies auto-sync loop (#503): run due `mode='auto'` bindings after their
    // debounce window. The debounce lives in the query, not the sleep; this loop
    // never touches Spotify.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("tonies") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let r = crate::tonies::autosync::run_due(&pool, &cfg, 10).await;
                if let Err(e) = &r {
                    tracing::warn!("tonies autosync pass failed: {e}");
                }
                let worked = matches!(r, Ok(n) if n > 0);
                health.record("tonies", worked, r.err().map(|e| e.to_string()));
                tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
            }
        });
    }

    // Playlist listing loop (#506): Spotify has no playlist-change webhook, so
    // this poll refreshes `hub_playlists.snapshot_id` (metadata only). The
    // next `spotify` pass then fetches items for changed playlists.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        let spotify_tick = spotify_tick.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("playlists") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let interval = playlist_poll_interval(&pool).await;
                if !playlist_poll_enabled(&pool).await {
                    tokio::time::sleep(interval).await;
                    continue;
                }
                let guard = spotify_tick.lock().await;
                let r = playlist_listing_pass(&pool, &cfg).await;
                drop(guard);
                match &r {
                    Ok(PlaylistPollOutcome::Ok(stats)) => {
                        tracing::info!(
                            "playlists poll: {} users, {} requests, {} playlists updated",
                            stats.users,
                            stats.requests,
                            stats.updated
                        );
                        health.record_with_requests(
                            "playlists",
                            stats.updated > 0,
                            None,
                            stats.requests,
                        );
                    }
                    Ok(PlaylistPollOutcome::RateLimited { retry_after, stats }) => {
                        tracing::warn!(
                            "playlists poll: rate limited after {} requests",
                            stats.requests
                        );
                        health.record_with_requests(
                            "playlists",
                            stats.updated > 0,
                            Some("Spotify rate limit (429)".to_string()),
                            stats.requests,
                        );
                        backoff(*retry_after).await;
                    }
                    Ok(PlaylistPollOutcome::Quota { stats }) => {
                        tracing::warn!(
                            "playlists poll: dev-mode quota exceeded after {} requests",
                            stats.requests
                        );
                        health.record_with_requests(
                            "playlists",
                            stats.updated > 0,
                            Some("Spotify dev-mode quota exceeded".to_string()),
                            stats.requests,
                        );
                        quota_backoff().await;
                    }
                    Err(e) => {
                        tracing::warn!("playlists poll failed: {e}");
                        health.record("playlists", false, Some(e.to_string()));
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    // Backpack content refresh (#682): materialise each item's resolved tracks
    // so `/backpacks` can show counts/availability without live resolution or
    // any music-api call. Idempotent: unchanged sources write nothing.
    {
        let pool = pool.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                if health.is_paused("backpacks") {
                    tokio::time::sleep(IDLE_WAIT).await;
                    continue;
                }
                let r = crate::backpacks::refresh_all(&pool).await;
                if let Err(e) = &r {
                    tracing::warn!("backpack refresh pass failed: {e}");
                }
                let worked = matches!(r, Ok(n) if n > 0);
                health.record("backpacks", worked, r.err().map(|e| e.to_string()));
                tokio::time::sleep(Duration::from_secs(BACKPACK_REFRESH_SECS)).await;
            }
        });
    }

    health
}

async fn run_once(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    // Likes first (cheap, one endpoint), then playlist items.
    let likes = sync_likes_jobs(pool, cfg).await?;
    let playlists = playlist_jobs(pool, cfg).await?;
    Ok(likes || playlists)
}

// ── playlist listing poll (#506) ────────────────────────────────────────────

/// Counters for one listing poll pass, surfaced in the log and health.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaylistPollStats {
    /// Users that had a usable token and were polled.
    pub users: usize,
    /// Spotify API requests made (profile + listing pages).
    pub requests: u64,
    /// Playlists whose metadata was upserted.
    pub updated: usize,
}

/// Result of one listing poll pass.
#[derive(Debug)]
pub enum PlaylistPollOutcome {
    Ok(PlaylistPollStats),
    RateLimited {
        retry_after: Option<u64>,
        stats: PlaylistPollStats,
    },
    Quota {
        stats: PlaylistPollStats,
    },
}

/// One metadata-only `/me/playlists` listing poll over every user with a
/// Spotify account (#506).
///
/// User without a usable token are skipped with a log line and produce no
/// request. A 429 or dev-mode quota aborts the pass with the matching outcome
/// so the caller can back off instead of retrying in a storm.
pub async fn playlist_listing_pass(pool: &SqlitePool, cfg: &Config) -> Result<PlaylistPollOutcome> {
    let accounts = sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT u.id, u.slug, a.access_token
           FROM hub_users u
           JOIN hub_service_accounts a
             ON a.user_id = u.id AND a.service = 'spotify'
          ORDER BY u.id",
    )
    .fetch_all(pool)
    .await?;

    let mut stats = PlaylistPollStats::default();
    for (user_id, slug, stored_token) in accounts {
        // No token at all: log only, never touch the network (#506).
        if stored_token
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            tracing::info!("playlists poll: skipping {slug} (no Spotify token)");
            continue;
        }
        // Refresh the token if it is expired/expiring.
        let token = match ingest::access_token(pool, cfg, user_id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::info!("playlists poll: skipping {slug} (unusable token: {e})");
                continue;
            }
        };
        stats.users += 1;

        // 1 request: `/me` (also persists remote id + display name).
        stats.requests += 1;
        let remote_id =
            match ingest::remote_profile(pool, &cfg.spotify_api_base, &token, user_id).await {
                Ok((id, _)) => id,
                Err(e) => {
                    tracing::warn!("playlists poll: profile fetch for {slug} failed: {e}");
                    continue;
                }
            };

        let url = spotify::api_url(&cfg.spotify_api_base, "/me/playlists?limit=50");
        let (outcome, pages) = match paged_counted(&token, &url).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("playlists poll: listing for {slug} failed: {e}");
                continue;
            }
        };
        stats.requests += pages as u64;
        match outcome {
            Outcome::Ok(items) => {
                for pl in &items {
                    let owned = ingest::is_owned(pl, &remote_id);
                    if ingest::upsert_playlist_meta(pool, user_id, pl, owned)
                        .await?
                        .is_some()
                    {
                        stats.updated += 1;
                    }
                }
            }
            Outcome::RateLimited { retry_after } => {
                return Ok(PlaylistPollOutcome::RateLimited { retry_after, stats });
            }
            Outcome::Quota => return Ok(PlaylistPollOutcome::Quota { stats }),
            Outcome::Forbidden => {
                tracing::warn!("playlists poll: {slug} not in Spotify app allowlist (403)");
            }
            Outcome::Error(m) => {
                tracing::warn!("playlists poll: listing for {slug} returned {m}");
            }
        }
    }
    Ok(PlaylistPollOutcome::Ok(stats))
}

async fn playlist_poll_enabled(pool: &SqlitePool) -> bool {
    match settings::get(pool, settings::PLAYLIST_POLL_ENABLED)
        .await
        .map(|v| v.trim().to_lowercase())
    {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
        None => true,
    }
}

async fn playlist_poll_interval(pool: &SqlitePool) -> Duration {
    let secs = settings::get(pool, settings::PLAYLIST_POLL_INTERVAL_SECS)
        .await
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(PLAYLIST_POLL_DEFAULT_SECS)
        .max(PLAYLIST_POLL_MIN_SECS);
    Duration::from_secs(secs)
}

// ── liked tracks ────────────────────────────────────────────────────────────

async fn sync_likes_jobs(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    let accounts = sqlx::query_as::<_, (i64, String)>(
        "SELECT a.user_id, u.slug
           FROM hub_service_accounts a
           JOIN hub_users u ON u.id = a.user_id
          WHERE a.service = 'spotify'
            AND a.access_token IS NOT NULL
            AND (a.likes_status IS NULL OR a.likes_status = 'queued')
          ORDER BY a.user_id
          LIMIT ?1",
    )
    .bind(MAX_LIKES_ACCOUNTS_PER_PASS as i64)
    .fetch_all(pool)
    .await?;

    let mut worked = false;
    for (user_id, slug) in accounts {
        worked = true;
        let token = match ingest::access_token(pool, cfg, user_id).await {
            Ok(t) => t,
            Err(e) => {
                set_likes_error(pool, user_id, &e.to_string()).await?;
                continue;
            }
        };
        match paged(
            &token,
            &spotify::api_url(&cfg.spotify_api_base, "/me/tracks?limit=50"),
        )
        .await?
        {
            Outcome::Ok(items) => {
                let n = ingest::store_likes(pool, cfg, user_id, &items).await?;
                sqlx::query(
                    "UPDATE hub_service_accounts
                        SET likes_synced_at = ?2, likes_status = 'done', likes_error = NULL
                      WHERE user_id = ?1 AND service = 'spotify'",
                )
                .bind(user_id)
                .bind(now_iso())
                .execute(pool)
                .await?;
                tracing::info!("synced {n} liked tracks for {slug}");
            }
            Outcome::Forbidden => {
                set_likes_error(
                    pool,
                    user_id,
                    "Spotify 403 — Account nicht in der App-Allowlist",
                )
                .await?;
            }
            Outcome::RateLimited { retry_after } => {
                backoff(retry_after).await;
                return Ok(worked);
            }
            Outcome::Quota => {
                quota_backoff().await;
                return Ok(worked);
            }
            Outcome::Error(m) => set_likes_error(pool, user_id, &m).await?,
        }
    }
    Ok(worked)
}

async fn set_likes_error(pool: &SqlitePool, user_id: i64, msg: &str) -> Result<()> {
    sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_status = 'error', likes_error = ?2
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .bind(msg)
    .execute(pool)
    .await?;
    tracing::warn!("likes sync for user {user_id} failed: {msg}");
    Ok(())
}

// ── on-demand playlist refresh ─────────────────────────────────────────────

/// Refresh one playlist's items right now (the `SYNC` button on `/tonies`).
///
/// Same fetch+store path as [`playlist_jobs`], but for exactly one playlist
/// and without the backoff dance: the user is waiting, so a 429/403 is
/// reported as an error instead of being retried silently.
pub async fn refresh_playlist_items(
    pool: &SqlitePool,
    cfg: &Config,
    user_id: i64,
    local_playlist_id: i64,
) -> Result<usize> {
    let remote: Option<String> = sqlx::query_scalar(
        "SELECT playlist_id FROM hub_playlists
          WHERE id = ?1 AND user_id = ?2 AND service = 'spotify'",
    )
    .bind(local_playlist_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let remote = remote.ok_or_else(|| anyhow::anyhow!("Playlist nicht gefunden"))?;

    let token = ingest::access_token(pool, cfg, user_id).await?;
    let url = spotify::api_url(
        &cfg.spotify_api_base,
        &format!("/playlists/{remote}/items?limit=50"),
    );
    let items = match paged(&token, &url).await? {
        Outcome::Ok(items) => items,
        Outcome::Forbidden => anyhow::bail!("Spotify 403 — kein Zugriff auf diese Playlist"),
        Outcome::RateLimited { .. } => anyhow::bail!("Spotify 429 — gleich nochmal versuchen"),
        Outcome::Quota => anyhow::bail!("Spotify-Tagesquote erreicht"),
        Outcome::Error(m) => anyhow::bail!("Spotify-Fehler: {m}"),
    };
    let n = ingest::store_playlist_items(pool, cfg, local_playlist_id, &items).await?;
    let now = now_iso();
    sqlx::query(
        "UPDATE hub_playlists
            SET items_available = 1, fetched_at = ?2,
                last_synced_snapshot = snapshot_id, fetch_status = 'done',
                fetch_error = NULL
          WHERE id = ?1",
    )
    .bind(local_playlist_id)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(n)
}

// ── playlist items ──────────────────────────────────────────────────────────

pub async fn playlist_jobs(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    // Fetch never-fetched playlists, plus those whose remote snapshot changed
    // since the last item fetch (incremental update — minimizes API calls).
    let jobs = sqlx::query_as::<_, (i64, i64, String, String)>(
        "SELECT p.id, p.user_id, p.playlist_id, u.slug
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
          WHERE p.service = 'spotify'
            AND p.enabled_for_fetch = 1
            AND (p.items_available = 0
                 OR (p.snapshot_id IS NOT NULL
                     AND p.snapshot_id <> IFNULL(p.last_synced_snapshot, '')))
          ORDER BY p.id
          LIMIT ?1",
    )
    .bind(MAX_PLAYLISTS_PER_PASS as i64)
    .fetch_all(pool)
    .await?;

    let mut tokens: HashMap<i64, String> = HashMap::new();
    let mut worked = false;

    for (local_id, user_id, playlist_id, slug) in jobs {
        worked = true;
        let token = if let Some(t) = tokens.get(&user_id) {
            t.clone()
        } else {
            match ingest::access_token(pool, cfg, user_id).await {
                Ok(t) => {
                    tokens.insert(user_id, t.clone());
                    t
                }
                Err(e) => {
                    fail(pool, local_id, &format!("auth: {e}")).await?;
                    continue;
                }
            }
        };

        let url = spotify::api_url(
            &cfg.spotify_api_base,
            &format!("/playlists/{playlist_id}/items?limit=50"),
        );
        match paged(&token, &url).await? {
            Outcome::Ok(items) => {
                let n = ingest::store_playlist_items(pool, cfg, local_id, &items).await?;
                let now = now_iso();
                sqlx::query(
                    "UPDATE hub_playlists
                        SET items_available = 1, fetched_at = ?2,
                            last_synced_snapshot = snapshot_id,
                            fetch_status = 'done', fetch_error = NULL
                      WHERE id = ?1",
                )
                .bind(local_id)
                .bind(&now)
                .execute(pool)
                .await?;
                // Auto-Sync hook (#503): a changed snapshot marks every
                // `mode='auto'` binding fed by this playlist dirty. No extra
                // Spotify call — the change is already known here.
                sqlx::query(
                    "UPDATE hub_tonie_bindings
                        SET dirty_since = ?2, updated_at = ?2
                      WHERE mode = 'auto' AND source_kind = 'playlist' AND source_id = ?1",
                )
                .bind(local_id)
                .bind(&now)
                .execute(pool)
                .await?;
                tracing::info!("fetched {n} tracks for playlist {local_id} ({slug})");
            }
            Outcome::Forbidden => {
                fail(pool, local_id, "kein Zugriff (Spotify 403)").await?;
            }
            Outcome::RateLimited { retry_after } => {
                backoff(retry_after).await;
                return Ok(worked);
            }
            Outcome::Quota => {
                quota_backoff().await;
                return Ok(worked);
            }
            Outcome::Error(m) => fail(pool, local_id, &m).await?,
        }
    }
    Ok(worked)
}

async fn fail(pool: &SqlitePool, id: i64, msg: &str) -> Result<()> {
    sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 0, fetch_status = 'error', fetch_error = ?2
          WHERE id = ?1",
    )
    .bind(id)
    .bind(msg)
    .execute(pool)
    .await?;
    tracing::warn!("playlist {id} fetch failed: {msg}");
    Ok(())
}

// ── paging + backoff ────────────────────────────────────────────────────────

enum Outcome {
    Ok(Vec<Value>),
    Forbidden,
    RateLimited { retry_after: Option<u64> },
    Quota,
    Error(String),
}

/// Follow `next` links, mapping 429/403 to outcomes instead of erroring.
async fn paged(token: &str, first_url: &str) -> Result<Outcome> {
    Ok(paged_counted(token, first_url).await?.0)
}

/// Like [`paged`], but also reports how many HTTP requests were made.
async fn paged_counted(token: &str, first_url: &str) -> Result<(Outcome, usize)> {
    let mut url = first_url.to_string();
    let mut items = Vec::new();
    let mut requests = 0usize;
    loop {
        requests += 1;
        let page = spotify::get_page(token, &url).await?;
        match page.status {
            200 => {}
            429 if page.quota_exceeded => return Ok((Outcome::Quota, requests)),
            429 => {
                return Ok((
                    Outcome::RateLimited {
                        retry_after: page.retry_after,
                    },
                    requests,
                ));
            }
            403 => return Ok((Outcome::Forbidden, requests)),
            s => return Ok((Outcome::Error(format!("HTTP {s}")), requests)),
        }
        if let Some(arr) = page.body["items"].as_array() {
            items.extend(arr.iter().cloned());
        }
        match page.body["next"].as_str() {
            Some(next) if !next.is_empty() => url = next.to_string(),
            _ => break,
        }
        tokio::time::sleep(PAGE_DELAY).await;
    }
    Ok((Outcome::Ok(items), requests))
}

async fn backoff(retry_after: Option<u64>) {
    let secs = retry_after.unwrap_or(30).max(1);
    tracing::warn!("Spotify rate limit (429); backing off {secs}s");
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

async fn quota_backoff() {
    tracing::warn!("Spotify dev-mode quota exceeded; backing off 10m");
    tokio::time::sleep(Duration::from_secs(600)).await;
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod health_tests {
    use super::{LOOP_NAMES, WorkerHealth};

    #[test]
    fn worker_health_records_and_resets_error() {
        let h = WorkerHealth::new();
        h.record("x", false, Some("boom".to_string()));
        let snap = h.snapshot();
        let x = snap
            .loops
            .iter()
            .find(|l| l.name == "x")
            .expect("x registered by record");
        assert_eq!(x.last_error.as_deref(), Some("boom"));
        assert!(!x.last_worked);
        assert_eq!(x.iterations, 1);
        assert!(x.last_run_at.is_some(), "last_run_at stamped");

        // A successful pass clears the error and bumps iterations.
        h.record("x", true, None);
        let snap = h.snapshot();
        let x = snap.loops.iter().find(|l| l.name == "x").unwrap();
        assert_eq!(x.last_error, None);
        assert!(x.last_worked);
        assert_eq!(x.iterations, 2);
    }

    #[test]
    fn worker_health_snapshot_is_name_stable() {
        let h = WorkerHealth::default();
        let snap = h.snapshot();
        assert_eq!(snap.loops.len(), LOOP_NAMES.len());
        for name in LOOP_NAMES {
            assert!(
                snap.loops.iter().any(|l| l.name == name),
                "{name} missing from snapshot"
            );
        }
        // BTreeMap yields a name-sorted list.
        let names: Vec<&str> = snap.loops.iter().map(|l| l.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(snap.loops[0].iterations, 0);
    }

    #[test]
    fn loop_registry_has_eight_including_playlists_and_backpacks() {
        assert_eq!(LOOP_NAMES.len(), 8);
        assert!(LOOP_NAMES.contains(&"playlists"));
        assert!(LOOP_NAMES.contains(&"backpacks"));
        let h = WorkerHealth::new();
        let snap = h.snapshot();
        assert_eq!(snap.loops.len(), 8);
        assert!(snap.loops.iter().any(|l| l.name == "playlists"));
        assert!(snap.loops.iter().any(|l| l.name == "backpacks"));
        // Pausable like every other registered loop (#350).
        assert!(!h.is_paused("playlists"));
        h.set_paused("playlists", true);
        assert!(h.is_paused("playlists"));
    }

    #[test]
    fn record_accumulates_request_budget() {
        let h = WorkerHealth::new();
        h.record_with_requests("playlists", true, None, 6);
        h.record_with_requests("playlists", false, None, 4);
        let snap = h.snapshot();
        let p = snap.loops.iter().find(|l| l.name == "playlists").unwrap();
        assert_eq!(p.requests, 10, "request counter is cumulative");
        assert_eq!(p.iterations, 2);
        // Plain record leaves the counter untouched.
        h.record("playlists", true, None);
        let snap = h.snapshot();
        let p = snap.loops.iter().find(|l| l.name == "playlists").unwrap();
        assert_eq!(p.requests, 10);
    }

    #[test]
    fn worker_health_uptime_is_non_negative() {
        let h = WorkerHealth::new();
        assert!(h.uptime_secs() >= 0);
        assert!(h.snapshot().started_at.is_some());
    }

    #[test]
    fn pause_flag_toggles_per_loop() {
        let h = WorkerHealth::new();
        // Default: every registered loop runs.
        for name in LOOP_NAMES {
            assert!(!h.is_paused(name), "{name} starts running");
        }
        h.set_paused("spotify", true);
        assert!(h.is_paused("spotify"));
        assert!(!h.is_paused("crawl"), "only the named loop pauses");
        // The flag is visible in the snapshot.
        let snap = h.snapshot();
        let spotify = snap.loops.iter().find(|l| l.name == "spotify").unwrap();
        assert!(spotify.paused);
        h.set_paused("spotify", false);
        assert!(!h.is_paused("spotify"));
        // Unknown names are harmless.
        h.set_paused("nope", true);
        assert!(h.is_paused("nope"));
    }

    #[test]
    fn events_are_newest_first_and_bounded() {
        let h = WorkerHealth::new();
        assert!(h.events().is_empty());
        h.record("enrichment", true, None);
        h.record("crawl", false, Some("boom".to_string()));
        let ev = h.events();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].loop_name, "crawl", "newest first");
        assert_eq!(ev[1].loop_name, "enrichment");
        assert_eq!(ev[0].error.as_deref(), Some("boom"));
        assert!(ev[0].at.contains('T'), "RFC3339 timestamp");

        // Ring-buffer stays hard-capped at EVENT_CAPACITY.
        for i in 0..(super::EVENT_CAPACITY + 50) {
            h.record("curation", i % 2 == 0, None);
        }
        let ev = h.events();
        assert_eq!(ev.len(), super::EVENT_CAPACITY, "capacity is constant");
    }
}
