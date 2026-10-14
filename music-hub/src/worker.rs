//! Backend worker: the only place that talks to Spotify for sync.
//!
//! Its jobs, driven purely by flags in the DB (the frontend only toggles those):
//!   * liked tracks per connected account   (`hub_service_accounts.likes_status`)
//!   * playlist items for `enabled_for_fetch` playlists (`items_available = 0`)
//!
//! It runs continuously while there is work, one request stream at a time, with a
//! small page delay and hard backoff on 429 / dev-mode quota.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::ingest;
use crate::spotify;

const IDLE_WAIT: Duration = Duration::from_secs(10);
const BUSY_WAIT: Duration = Duration::from_secs(1);
const MAX_PLAYLISTS_PER_PASS: usize = 12;
const MAX_LIKES_ACCOUNTS_PER_PASS: usize = 2;
const PAGE_DELAY: Duration = Duration::from_millis(150);

/// The five Hub-Owned loop names, in report order (issue #353).
///
/// The crawl domain is served by two tasks (vault/order refresh + Spotify
/// touch-cascade); both report under `"crawl"` so the domain is one visible
/// loop, matching the plan.
pub const LOOP_NAMES: [&str; 5] = ["enrichment", "freqblog", "spotify", "crawl", "curation"];

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
}

#[derive(Clone, Default)]
struct LoopState {
    last_run_at: Option<String>,
    last_worked: bool,
    iterations: u64,
    last_error: Option<String>,
}

/// Owned, JSON-ready snapshot of one loop's state.
#[derive(Debug, Clone, Default)]
pub struct LoopSnapshot {
    pub name: &'static str,
    pub last_run_at: Option<String>,
    pub last_worked: bool,
    pub iterations: u64,
    pub last_error: Option<String>,
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
    /// Fresh registry with all five loops registered and `started_at = now`.
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

    /// Record one loop iteration. `err = None` clears a previous error.
    pub fn record(&self, name: &'static str, worked: bool, err: Option<String>) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if inner.started_at.is_none() {
            inner.started_at = Some(chrono::Utc::now());
        }
        let st = inner.loops.entry(name).or_default();
        st.last_run_at = Some(chrono::Utc::now().to_rfc3339());
        st.last_worked = worked;
        st.iterations += 1;
        st.last_error = err;
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
                last_error: st.last_error.clone(),
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

    // Enrichment loop: audio features + genres. Local/external APIs that don't
    // touch Spotify — kept separate so Spotify quota backoffs can't starve them.
    {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let health = health.clone();
        tokio::spawn(async move {
            loop {
                // On-demand ReccoBeats requests first, then the generic backlog.
                let p = crate::features::priority_once(&pool, &cfg, 50).await;
                let f = crate::features::sync_once(&pool, &cfg, 50).await;
                let g = crate::genres::sync_once(&pool, &cfg, 50).await;
                let worked = matches!(p, Ok(n) if n > 0)
                    || matches!(f, Ok(n) if n > 0)
                    || matches!(g, Ok(n) if n > 0);
                let err = p.err().or_else(|| f.err()).or_else(|| g.err()).map(|e| e.to_string());
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
        tokio::spawn(async move {
            loop {
                let r = run_once(&pool, &cfg).await;
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
        let health = health.clone();
        tokio::spawn(async move {
            loop {
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

    health
}

async fn run_once(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    // Likes first (cheap, one endpoint), then playlist items.
    let likes = sync_likes_jobs(pool, cfg).await?;
    let playlists = playlist_jobs(pool, cfg).await?;
    Ok(likes || playlists)
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

// ── playlist items ──────────────────────────────────────────────────────────

async fn playlist_jobs(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
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
                sqlx::query(
                    "UPDATE hub_playlists
                        SET items_available = 1, fetched_at = ?2,
                            last_synced_snapshot = snapshot_id,
                            fetch_status = 'done', fetch_error = NULL
                      WHERE id = ?1",
                )
                .bind(local_id)
                .bind(now_iso())
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
    let mut url = first_url.to_string();
    let mut items = Vec::new();
    loop {
        let page = spotify::get_page(token, &url).await?;
        match page.status {
            200 => {}
            429 if page.quota_exceeded => return Ok(Outcome::Quota),
            429 => {
                return Ok(Outcome::RateLimited {
                    retry_after: page.retry_after,
                });
            }
            403 => return Ok(Outcome::Forbidden),
            s => return Ok(Outcome::Error(format!("HTTP {s}"))),
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
    Ok(Outcome::Ok(items))
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
    fn worker_health_uptime_is_non_negative() {
        let h = WorkerHealth::new();
        assert!(h.uptime_secs() >= 0);
        assert!(h.snapshot().started_at.is_some());
    }
}
