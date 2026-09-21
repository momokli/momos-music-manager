//! The single "Backpack" concept.
//!
//! Backpack (the *set*) is the DISTINCT union of:
//!   (a) every track of the playlists marked in MMM (`playlist_subscriptions`,
//!       `is_active = 1`), and
//!   (b) every track whose tags have `backpack = 1` (inherited via the
//!       playlist→tag chain, see `plans/done/backpack-system.md`).
//!
//! The Backpack has two guaranteed effects for every member track:
//!   1. **Auto-download** — best locally available version, priority
//!      `stem.m4a` > `flac` > `mp3` (WAV sources excluded), see
//!      [`crate::db::get_backpack_pull_candidates`].
//!   2. **Prune-safe** — never a prune candidate, see
//!      [`crate::db::get_prune_candidates`].
//!
//! The Backpack *playlist* is pure transport: the whole set is materialised into
//! ONE Spotify playlist named `Backpack`, and only that ONE playlist URL is
//! submitted to deemix. This replaces the old "N single-playlist submits".

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::db::settings::{
    KEY_BACKPACK_DIRTY_AT, KEY_BACKPACK_LAST_PUSH_AT, KEY_BACKPACK_LAST_PUSH_ERROR,
    KEY_BACKPACK_LAST_PUSH_STATUS, KEY_BACKPACK_PLAYLIST_ID, KEY_BACKPACK_PLAYLIST_URL,
    KEY_BACKPACK_SIGNATURE, delete_setting, get_setting, set_setting,
};

/// Current Unix time in seconds (0 only if the clock is before the epoch).
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Name of the single materialised Spotify playlist.
pub const BACKPACK_PLAYLIST_NAME: &str = "Backpack";

/// Sentinel URL used for the consolidated `deemix_downloads` row *before* the
/// real Backpack playlist has been materialised (see migration 025). Replaced
/// with the real URL on first materialisation.
pub const BACKPACK_SENTINEL_URL: &str = "https://open.spotify.com/playlist/backpack";

// ── Aggregation ───────────────────────────────────────────────────────────

/// Return the deduplicated, stably-ordered set of `service_tracks.id` that form
/// the Backpack (union of (a) subscribed playlists and (b) backpack tags).
pub async fn get_backpack_track_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>> {
    // (a) tracks in active (subscribed) playlists.
    let subscribed: Vec<i64> = sqlx::query_scalar(
        r#"SELECT DISTINCT spt.track_id
           FROM playlist_subscriptions ps
           JOIN service_playlists sp
             ON sp.service = ps.service AND sp.playlist_id = ps.playlist_id
           JOIN service_playlist_tracks spt
             ON spt.playlist_id = sp.id AND spt.deleted_at IS NULL
           WHERE ps.is_active = 1"#,
    )
    .fetch_all(pool)
    .await?;

    // (b) tracks whose resolved tags have backpack = 1 (inheritance).
    let tagged: Vec<i64> = sqlx::query_scalar(
        r#"SELECT DISTINCT vtt.track_id
           FROM v_track_tags vtt
           JOIN tags t ON t.id = vtt.tag_id
           WHERE t.backpack = 1"#,
    )
    .fetch_all(pool)
    .await?;

    // Deduplicate and order stably (ascending id).
    let ids: BTreeSet<i64> = subscribed.into_iter().chain(tagged).collect();
    Ok(ids.into_iter().collect())
}

/// Resolve the Backpack track set to stable, deduplicated Spotify track URIs
/// (`spotify:track:<service_id>`), ordered by track id.
pub async fn resolve_backpack_track_uris(pool: &Pool<Sqlite>) -> Result<Vec<String>> {
    let ids = get_backpack_track_ids(pool).await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT 'spotify:track:' || service_id
         FROM service_tracks
         WHERE service = 'spotify' AND id IN ({})
         ORDER BY id",
        placeholders
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql);
    for id in &ids {
        query = query.bind(id);
    }
    Ok(query.fetch_all(pool).await?)
}

/// Return the set of local `files.id` linked to Backpack tracks (via
/// `v_file_track_link`). This is the set the two effects operate on
/// (auto-download candidates + prune protection).
pub async fn get_backpack_file_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>> {
    let ids = get_backpack_track_ids(pool).await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT DISTINCT v.file_id FROM v_file_track_link v WHERE v.track_id IN ({}) ORDER BY v.file_id",
        placeholders
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in &ids {
        query = query.bind(id);
    }
    Ok(query.fetch_all(pool).await?)
}

/// Stable SHA-256 signature of a Backpack track set, used to detect changes and
/// skip redundant Spotify/deemix API calls when the set is unchanged.
pub fn backpack_signature(uris: &[String]) -> String {
    let mut hasher = Sha256::new();
    for uri in uris {
        hasher.update(uri.as_bytes());
        hasher.update([0u8]);
    }
    format!("{:x}", hasher.finalize())
}

// ── Transport traits (mockable seam) ──────────────────────────────────────

/// Minimal Spotify write surface needed to materialise the Backpack playlist.
#[async_trait]
pub trait BackpackSpotifyOps: Send + Sync {
    async fn current_user_id(&self) -> Result<String>;
    async fn create_playlist(
        &self,
        user_id: &str,
        name: &str,
        public: bool,
        description: Option<&str>,
    ) -> Result<(String, String)>;
    async fn add_tracks_to_playlist(&self, playlist_id: &str, uris: &[String]) -> Result<()>;
    /// Mirror `uris` into the playlist (replace, not append).
    async fn replace_tracks(&self, playlist_id: &str, uris: &[String]) -> Result<()>;
    /// Read the URIs currently in the playlist (for verification).
    async fn playlist_uris(&self, playlist_id: &str) -> Result<Vec<String>>;
}

/// Minimal deemix queue surface needed to submit the Backpack transport URL.
#[async_trait]
pub trait BackpackDeemixOps: Send + Sync {
    async fn ensure_queued(&self, url: &str) -> Result<()>;
}

// ── Materialisation ───────────────────────────────────────────────────────

/// Result of a Backpack materialisation run.
#[derive(Debug, Clone, Default)]
pub struct MaterializeOutcome {
    /// Number of Spotify tracks in the Backpack set.
    pub track_count: usize,
    /// True when the Spotify playlist was newly created this run.
    pub created: bool,
    /// True when the set changed and tracks were (re-)added this run.
    pub updated: bool,
    /// The materialised Spotify playlist URL (when known).
    pub spotify_url: Option<String>,
    /// True when deemix was asked to queue the single Backpack URL.
    pub deemix_submitted: bool,
    /// True when the run was a dry run (no writes, no submits).
    pub dry_run: bool,
    /// True when the remote playlist did not match the intended set after the
    /// replace. In that case `backpack.signature` is **not** updated, so the
    /// next run retries instead of declaring a wrong state "in sync".
    pub verification_failed: bool,
}

/// Options controlling a Backpack materialisation run.
#[derive(Debug, Clone)]
pub struct MaterializeOptions {
    /// Ignore the stored signature and always build (used by the manual push
    /// button so the user gets visible feedback).
    pub force: bool,
    /// Submit the single Backpack URL to deemix after (re)building the
    /// playlist. `false` builds the playlist only.
    pub submit_to_deemix: bool,
    /// Build nothing — only report what *would* happen.
    pub dry_run: bool,
}

impl Default for MaterializeOptions {
    fn default() -> Self {
        Self {
            force: false,
            submit_to_deemix: true,
            dry_run: false,
        }
    }
}

/// Materialise the Backpack set into ONE Spotify playlist and submit that ONE
/// URL to deemix.
///
/// Idempotent: if the Backpack set signature is unchanged and the playlist
/// already exists, no Spotify/deemix API calls are made.
pub async fn materialize_backpack_playlist<S, D>(
    pool: &Pool<Sqlite>,
    spotify: &S,
    deemix: Option<&D>,
) -> Result<MaterializeOutcome>
where
    S: BackpackSpotifyOps,
    D: BackpackDeemixOps,
{
    materialize_backpack_playlist_with(pool, spotify, deemix, MaterializeOptions::default()).await
}

/// Like [`materialize_backpack_playlist`] but with explicit options (force /
/// dry-run / deemix toggle).
///
/// Semantics: the Spotify playlist is **derived** state, so it is *replaced*
/// (mirror), never appended. After a replace the remote playlist is re-read and
/// compared; only on equality is `backpack.signature` advanced.
pub async fn materialize_backpack_playlist_with<S, D>(
    pool: &Pool<Sqlite>,
    spotify: &S,
    deemix: Option<&D>,
    opts: MaterializeOptions,
) -> Result<MaterializeOutcome>
where
    S: BackpackSpotifyOps,
    D: BackpackDeemixOps,
{
    let uris = resolve_backpack_track_uris(pool).await?;
    let signature = backpack_signature(&uris);

    let stored_id = get_setting(pool, KEY_BACKPACK_PLAYLIST_ID).await?;
    let stored_signature = get_setting(pool, KEY_BACKPACK_SIGNATURE).await?;
    let stored_url = get_setting(pool, KEY_BACKPACK_PLAYLIST_URL).await?;

    let unchanged =
        !opts.force && stored_id.is_some() && stored_signature.as_deref() == Some(signature.as_str());
    if unchanged {
        debug!("Backpack unchanged ({signature}), skipping materialisation");
        return Ok(MaterializeOutcome {
            track_count: uris.len(),
            created: false,
            updated: false,
            spotify_url: stored_url,
            deemix_submitted: false,
            dry_run: opts.dry_run,
            verification_failed: false,
        });
    }

    // Dry run: report what would happen without touching Spotify/deemix.
    if opts.dry_run {
        debug!("Backpack dry run ({signature}), no writes");
        return Ok(MaterializeOutcome {
            track_count: uris.len(),
            created: false,
            updated: false,
            spotify_url: stored_url,
            deemix_submitted: false,
            dry_run: true,
            verification_failed: false,
        });
    }

    // Ensure the playlist exists (create once, then reuse the persisted id).
    // An empty set still needs a playlist to *clear* when one already exists.
    let mut created = stored_id.is_none();
    let mut playlist_id = stored_id.clone();
    let mut spotify_url = stored_url;

    if playlist_id.is_none() && !uris.is_empty() {
        let (id, url) = create_backpack_playlist(pool, spotify).await?;
        playlist_id = Some(id);
        spotify_url = Some(url);
        created = true;
    }

    let Some(mut playlist_id) = playlist_id else {
        // Nothing stored and nothing to store (empty set) — nothing to do.
        debug!("Backpack set is empty and no playlist exists; nothing to materialise");
        clear_backpack_dirty(pool).await?;
        return Ok(MaterializeOutcome {
            track_count: 0,
            created: false,
            updated: false,
            spotify_url: None,
            deemix_submitted: false,
            dry_run: false,
            verification_failed: false,
        });
    };

    let mut spotify_url = spotify_url
        .unwrap_or_else(|| format!("https://open.spotify.com/playlist/{playlist_id}"));

    // Mirror the (deduplicated, stably ordered) track set into the playlist.
    let mirrored = match spotify.replace_tracks(&playlist_id, &uris).await {
        Ok(()) => true,
        Err(e) if is_playlist_gone(&e) => {
            // The playlist was deleted manually or belongs to a different
            // account now — re-create it and retry once. Never wedge.
            warn!(
                "Backpack playlist '{playlist_id}' is not authorable ({e:#}); re-creating it"
            );
            delete_setting(pool, KEY_BACKPACK_PLAYLIST_ID).await?;
            delete_setting(pool, KEY_BACKPACK_PLAYLIST_URL).await?;
            let (id, url) = create_backpack_playlist(pool, spotify).await?;
            playlist_id = id;
            spotify_url = url;
            created = true;
            spotify.replace_tracks(&playlist_id, &uris).await?;
            true
        }
        Err(e) => return Err(e),
    };
    debug_assert!(mirrored);

    info!(
        "Backpack materialised: {} track(s) into '{}' ({})",
        uris.len(),
        BACKPACK_PLAYLIST_NAME,
        spotify_url
    );

    // Verify: re-read the remote playlist and compare the URI multiset with the
    // intended one. Only on equality is the signature advanced.
    let intended: BTreeSet<&String> = uris.iter().collect();
    let verification_failed = match spotify.playlist_uris(&playlist_id).await {
        Ok(remote) => {
            let remote_set: BTreeSet<&String> = remote.iter().collect();
            if remote_set == intended && remote.len() == uris.len() {
                false
            } else {
                warn!(
                    "Backpack verification mismatch: intended {} track(s), remote has {} (playlist {})",
                    uris.len(),
                    remote.len(),
                    playlist_id,
                );
                true
            }
        }
        Err(e) => {
            warn!("Backpack verification read failed: {e:#}");
            true
        }
    };

    if !verification_failed {
        set_setting(pool, KEY_BACKPACK_SIGNATURE, &signature).await?;
    }
    clear_backpack_dirty(pool).await?;
    set_setting(pool, KEY_BACKPACK_LAST_PUSH_AT, &now_secs().to_string()).await?;

    // Transport = 1: submit only this one URL to deemix.
    let deemix_submitted = if !opts.submit_to_deemix {
        debug!("Backpack: deemix submit disabled for this run");
        false
    } else {
        match deemix {
            Some(client) => {
                client.ensure_queued(&spotify_url).await?;
                record_backpack_submit(pool, &spotify_url).await?;
                true
            }
            None => {
                debug!("deemix not configured; skipping Backpack submit");
                false
            }
        }
    };

    Ok(MaterializeOutcome {
        track_count: uris.len(),
        created,
        updated: true,
        spotify_url: Some(spotify_url),
        deemix_submitted,
        dry_run: false,
        verification_failed,
    })
}

/// Create the single Backpack playlist and persist its id/url.
async fn create_backpack_playlist<S: BackpackSpotifyOps>(
    pool: &Pool<Sqlite>,
    spotify: &S,
) -> Result<(String, String)> {
    let user_id = spotify.current_user_id().await?;
    let (id, url) = spotify
        .create_playlist(
            &user_id,
            BACKPACK_PLAYLIST_NAME,
            false,
            Some("Backpack transport — generated by Momo's Music Manager"),
        )
        .await?;
    set_setting(pool, KEY_BACKPACK_PLAYLIST_ID, &id).await?;
    set_setting(pool, KEY_BACKPACK_PLAYLIST_URL, &url).await?;
    Ok((id, url))
}

/// True when the Spotify error means the stored playlist is no longer
/// authorable by this account (deleted manually, account switched).
fn is_playlist_gone(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("404") || msg.contains("403")
}

// ── Dirty marker + coordinator ────────────────────────────────────────────

/// Mark the Backpack as out of sync (called from every membership mutation).
pub async fn mark_backpack_dirty(pool: &Pool<Sqlite>) -> Result<()> {
    set_setting(pool, KEY_BACKPACK_DIRTY_AT, &now_secs().to_string())
        .await
        .context("Failed to set backpack dirty marker")
}

/// Clear the dirty marker (after a successful materialisation).
async fn clear_backpack_dirty(pool: &Pool<Sqlite>) -> Result<()> {
    delete_setting(pool, KEY_BACKPACK_DIRTY_AT).await?;
    Ok(())
}

/// Snapshot of the Backpack state for `GET /api/backpack`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackpackStatus {
    pub track_count: usize,
    pub file_count: usize,
    pub playlist_url: Option<String>,
    pub signature: Option<String>,
    pub dirty: bool,
    pub dirty_at: Option<i64>,
    pub last_push_at: Option<i64>,
    pub last_push_status: Option<String>,
    pub last_push_error: Option<String>,
}

/// Read the current Backpack status (aggregation + persisted transport state).
pub async fn backpack_status(pool: &Pool<Sqlite>) -> Result<BackpackStatus> {
    let track_count = get_backpack_track_ids(pool).await?.len();
    let file_count = get_backpack_file_ids(pool).await?.len();
    let parse_i64 = |v: Option<String>| v.and_then(|s| s.parse::<i64>().ok());
    Ok(BackpackStatus {
        track_count,
        file_count,
        playlist_url: get_setting(pool, KEY_BACKPACK_PLAYLIST_URL).await?,
        signature: get_setting(pool, KEY_BACKPACK_SIGNATURE).await?,
        dirty: get_setting(pool, KEY_BACKPACK_DIRTY_AT).await?.is_some(),
        dirty_at: parse_i64(get_setting(pool, KEY_BACKPACK_DIRTY_AT).await?),
        last_push_at: parse_i64(get_setting(pool, KEY_BACKPACK_LAST_PUSH_AT).await?),
        last_push_status: get_setting(pool, KEY_BACKPACK_LAST_PUSH_STATUS).await?,
        last_push_error: get_setting(pool, KEY_BACKPACK_LAST_PUSH_ERROR).await?,
    })
}

/// Debounce window for automatic materialisation after a membership mutation.
pub const BACKPACK_DEBOUNCE: Duration = Duration::from_secs(30);
/// Safety-net reconciliation interval for the Backpack loop.
pub const BACKPACK_RECONCILE_INTERVAL: Duration = Duration::from_secs(600);

/// Coordinates Backpack materialisation.
///
/// Mutations only set a dirty marker; the [`start_backpack_coordinator`] loop
/// materialises after a debounce. `request_push()` materialises on the next
/// tick regardless of the signature (used by the manual push endpoint).
pub struct BackpackSyncCoordinator {
    push_requested: std::sync::atomic::AtomicBool,
}

impl BackpackSyncCoordinator {
    pub fn new() -> Self {
        Self {
            push_requested: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Request an on-demand (forced) push on the next coordinator tick.
    pub fn request_push(&self) {
        self.push_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Consumed by the coordinator loop; kept for symmetry with `take`.
    }

    /// Consume a pending on-demand request (returns `true` at most once).
    pub fn take_push_request(&self) -> bool {
        self.push_requested
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    fn has_push_request(&self) -> bool {
        self.push_requested.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for BackpackSyncCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the Backpack coordinator loop forever (until cancelled).
///
/// Every tick: if a manual push was requested, materialise **forced** (always
/// builds, always submits to deemix). Otherwise, if the dirty marker is older
/// than [`BACKPACK_DEBOUNCE`], materialise (signature-gated). Every
/// [`BACKPACK_RECONCILE_INTERVAL`] the materialisation also runs as a safety
/// net even without a dirty marker.
pub async fn start_backpack_coordinator(
    db: Pool<Sqlite>,
    credentials: crate::config::ServiceCredentials,
    coordinator: Arc<BackpackSyncCoordinator>,
    cancel_token: CancellationToken,
) {
    use crate::spotify::client::SpotifyClient;

    let mut last_reconcile = SystemTime::now();
    // Small initial delay so DB migrations/startup work settles first.
    tokio::time::sleep(Duration::from_secs(10)).await;

    while !cancel_token.is_cancelled() {
        let forced = coordinator.take_push_request();
        let dirty_at = get_setting(&db, KEY_BACKPACK_DIRTY_AT)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok());

        let elapsed_since_dirty = dirty_at.map(|t| now_secs() - t);
        let debounce_elapsed = elapsed_since_dirty
            .map(|e| e >= BACKPACK_DEBOUNCE.as_secs() as i64)
            .unwrap_or(false);
        let reconcile_due = last_reconcile.elapsed().unwrap_or_default()
            >= BACKPACK_RECONCILE_INTERVAL;

        if forced || debounce_elapsed || reconcile_due {
            last_reconcile = SystemTime::now();
            match SpotifyClient::from_stored_tokens(db.clone(), &credentials).await {
                Ok(client) => {
                    let deemix = crate::deemix::DeemixClient::from_db(db.clone()).await;
                    let opts = MaterializeOptions {
                        force: forced,
                        submit_to_deemix: forced,
                        dry_run: false,
                    };
                    match materialize_backpack_playlist_with(
                        &db,
                        &client,
                        deemix.as_ref(),
                        opts,
                    )
                    .await
                    {
                        Ok(outcome) if outcome.updated => {
                            info!(
                                "Backpack coordinator: materialised {} track(s) (forced={}, verification_failed={})",
                                outcome.track_count,
                                forced,
                                outcome.verification_failed,
                            );
                            record_push_status(&db, "ok", None).await;
                        }
                        Ok(_) => {
                            if forced {
                                record_push_status(&db, "ok", None).await;
                            }
                        }
                        Err(e) => {
                            warn!("Backpack coordinator: push failed: {e:#}");
                            record_push_status(&db, "error", Some(&format!("{e:#}"))).await;
                        }
                    }
                }
                Err(e) => {
                    debug!("Backpack coordinator: Spotify not available, skipping tick: {e:#}");
                }
            }
        }

        // Poll resolution: fine-grained enough for a snappy debounce.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    info!("Backpack coordinator: stopped");
}

/// True when the coordinator currently has an unconsumed on-demand request.
pub fn has_pending_push(coordinator: &BackpackSyncCoordinator) -> bool {
    coordinator.has_push_request()
}

/// Persist the outcome of a push attempt for the status endpoint.
pub async fn record_push_status(pool: &Pool<Sqlite>, status: &str, error: Option<&str>) {
    let _ = set_setting(pool, KEY_BACKPACK_LAST_PUSH_STATUS, status).await;
    match error {
        Some(e) => {
            let _ = set_setting(pool, KEY_BACKPACK_LAST_PUSH_ERROR, e).await;
        }
        None => {
            let _ = delete_setting(pool, KEY_BACKPACK_LAST_PUSH_ERROR).await;
        }
    }
}

/// Record the single Backpack transport row in `deemix_downloads` and remove any
/// other `is_backpack = 1` row (the migration-025 sentinel and stale rows).
pub async fn record_backpack_submit(pool: &Pool<Sqlite>, url: &str) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    sqlx::query(
        r#"INSERT INTO deemix_downloads
               (spotify_playlist_url, playlist_name, status, is_backpack, created_at, updated_at)
           VALUES (?, 'Backpack', 'queued', 1, ?, ?)
           ON CONFLICT(spotify_playlist_url) DO UPDATE SET
               playlist_name = 'Backpack',
               status = 'queued',
               is_backpack = 1,
               error_message = NULL,
               updated_at = excluded.updated_at"#,
    )
    .bind(url)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .context("Failed to upsert Backpack deemix_downloads row")?;

    sqlx::query(
        "DELETE FROM deemix_downloads WHERE is_backpack = 1 AND spotify_playlist_url != ?",
    )
    .bind(url)
    .execute(pool)
    .await?;

    Ok(())
}

// ── Transport impls for the concrete clients ──────────────────────────────

#[async_trait]
impl BackpackSpotifyOps for crate::spotify::client::SpotifyClient {
    async fn current_user_id(&self) -> Result<String> {
        crate::spotify::client::SpotifyClient::get_current_user_id(self).await
    }

    async fn create_playlist(
        &self,
        user_id: &str,
        name: &str,
        public: bool,
        description: Option<&str>,
    ) -> Result<(String, String)> {
        crate::spotify::client::SpotifyClient::create_playlist(
            self, user_id, name, public, description,
        )
        .await
    }

    async fn add_tracks_to_playlist(&self, playlist_id: &str, uris: &[String]) -> Result<()> {
        crate::spotify::client::SpotifyClient::add_tracks_to_playlist(self, playlist_id, uris)
            .await
    }

    async fn replace_tracks(&self, playlist_id: &str, uris: &[String]) -> Result<()> {
        crate::spotify::client::SpotifyClient::replace_playlist_items(self, playlist_id, uris).await
    }

    async fn playlist_uris(&self, playlist_id: &str) -> Result<Vec<String>> {
        crate::spotify::client::SpotifyClient::get_playlist_track_uris(self, playlist_id).await
    }
}

#[async_trait]
impl BackpackDeemixOps for crate::deemix::DeemixClient {
    async fn ensure_queued(&self, url: &str) -> Result<()> {
        crate::deemix::DeemixClient::ensure_queued(self, url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Minimal schema covering the tables/queries the Backpack aggregation reads.
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        sqlx::query(
            "CREATE TABLE tags (
                id INTEGER PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                category_id INTEGER NOT NULL,
                backpack INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE tag_categories (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                prefix TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE service_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                service_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                isrc TEXT,
                UNIQUE(service, service_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE service_playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                UNIQUE(service, playlist_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE service_playlist_tracks (
                playlist_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                position INTEGER,
                added_at INTEGER DEFAULT 0,
                deleted_at INTEGER,
                PRIMARY KEY (playlist_id, track_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE playlist_subscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                service_playlist_id INTEGER,
                is_active INTEGER NOT NULL DEFAULT 1,
                UNIQUE(service, playlist_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE VIEW v_track_tags AS
             SELECT DISTINCT spt.track_id,
                    t.id AS tag_id, t.name AS tag_name,
                    tc.id AS category_id, tc.name AS category_name,
                    tc.prefix, tc.is_default
             FROM service_playlist_tracks spt
             JOIN service_playlists sp ON sp.id = spt.playlist_id
             JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
             JOIN tag_categories tc ON tc.id = t.category_id",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                file_type TEXT NOT NULL,
                file_size INTEGER NOT NULL DEFAULT 0,
                isrc TEXT,
                spotify_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE VIEW v_file_track_link AS
             SELECT f.id AS file_id, st.id AS track_id
             FROM files f
             JOIN service_tracks st ON (
                 st.isrc = f.isrc
                 OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    async fn seed(pool: &SqlitePool) {
        // Tags: 'Groovy' (backpack=0), 'Deep' (backpack=1).
        sqlx::query(
            "INSERT INTO tags (id, name, category_id, backpack) VALUES
                (1, 'Groovy', 1, 0),
                (2, 'Deep', 1, 1)",
        )
        .execute(pool)
        .await
        .unwrap();

        sqlx::query("INSERT INTO tag_categories (id, name, prefix, is_default) VALUES (1, 'Mood', 'M', 0)")
            .execute(pool)
            .await
            .unwrap();

        // Tracks 1..5.
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist) VALUES
                (1, 'spotify', 'aaa', 'T1', 'A'),
                (2, 'spotify', 'bbb', 'T2', 'A'),
                (3, 'spotify', 'ccc', 'T3', 'B'),
                (4, 'spotify', 'ddd', 'T4', 'B'),
                (5, 'spotify', 'eee', 'T5', 'C')",
        )
        .execute(pool)
        .await
        .unwrap();

        // Playlists: 'Groovy' (id 1) and 'Deep Mix' (id 2, no tag match).
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (1, 'spotify', 'pl-groovy', 'Groovy'),
                (2, 'spotify', 'pl-other', 'Deep Mix')",
        )
        .execute(pool)
        .await
        .unwrap();

        // Track 1 in 'Groovy', track 2 in 'Deep Mix'.
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (1, 1, 0),
                (2, 2, 0)",
        )
        .execute(pool)
        .await
        .unwrap();

        // Subscription on 'Deep Mix' (track 2) — source (a).
        sqlx::query(
            "INSERT INTO playlist_subscriptions (service, playlist_id, is_active) VALUES
                ('spotify', 'pl-other', 1)",
        )
        .execute(pool)
        .await
        .unwrap();

        // Files: file 1 → track 1 (via spotify_id), file 2 → track 3 (no backpack
        // membership), file 3 → track 2 (subscribed).
        sqlx::query(
            "INSERT INTO files (id, file_path, file_type, file_size, spotify_id) VALUES
                (1, '/t1.flac', 'flac', 1, 'aaa'),
                (2, '/t3.flac', 'flac', 1, 'ccc'),
                (3, '/t2.flac', 'flac', 1, 'bbb')",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn backpack_track_ids_union_and_dedupe() {
        let pool = test_db().await;
        seed(&pool).await;

        // Source (a): subscribed 'Deep Mix' → track 2.
        // Source (b): tag 'Deep' backpack=1 → no playlist matches 'Deep' (playlist
        // is 'Deep Mix'), so only track 2 from (a). Now add a matching playlist/tag.
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES (3, 4, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        // track 2 (subscribed) ∪ track 4 (backpack tag) → sorted [2, 4].
        assert_eq!(ids, vec![2, 4]);
    }

    #[tokio::test]
    async fn backpack_track_ids_dedupes_overlapping_sources() {
        let pool = test_db().await;
        seed(&pool).await;

        // Make track 2 belong to BOTH sources: subscribe the playlist AND give it
        // a backpack tag via a playlist named 'Deep' that also contains track 2.
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES (3, 2, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        assert_eq!(ids, vec![2], "overlapping sources must dedupe to a single id");
    }

    #[tokio::test]
    async fn resolve_uris_stable_and_deduped() {
        let pool = test_db().await;
        seed(&pool).await;

        let uris = resolve_backpack_track_uris(&pool).await.unwrap();
        assert_eq!(uris, vec!["spotify:track:bbb".to_string()]);
    }

    #[tokio::test]
    async fn backpack_file_ids_maps_tracks_to_files() {
        let pool = test_db().await;
        seed(&pool).await;

        let files = get_backpack_file_ids(&pool).await.unwrap();
        // Only file 3 is linked to a backpack track (track 2).
        assert_eq!(files, vec![3]);
    }

    #[tokio::test]
    async fn signature_is_stable_and_order_sensitive() {
        let a = backpack_signature(&["spotify:track:aaa".into(), "spotify:track:bbb".into()]);
        let b = backpack_signature(&["spotify:track:aaa".into(), "spotify:track:bbb".into()]);
        let c = backpack_signature(&["spotify:track:bbb".into(), "spotify:track:aaa".into()]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // ── Materialisation (mocked transport) ────────────────────────────────

    struct MockSpotify {
        creates: std::sync::Mutex<usize>,
        adds: std::sync::Mutex<Vec<Vec<String>>>,
        /// URIs actually present in the (single mocked) remote playlist.
        remote: std::sync::Mutex<Vec<String>>,
        /// URIs passed to the last `replace_tracks` call.
        replaces: std::sync::Mutex<Vec<Vec<String>>>,
        /// When set, `replace_tracks`/`playlist_uris` fail with this message.
        fail_replace: std::sync::Mutex<Option<String>>,
        /// When true, `playlist_uris` reports a state that differs from intend.
        corrupt_remote: std::sync::Mutex<bool>,
    }

    impl MockSpotify {
        fn new() -> Self {
            Self {
                creates: std::sync::Mutex::new(0),
                adds: std::sync::Mutex::new(vec![]),
                remote: std::sync::Mutex::new(vec![]),
                replaces: std::sync::Mutex::new(vec![]),
                fail_replace: std::sync::Mutex::new(None),
                corrupt_remote: std::sync::Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl BackpackSpotifyOps for MockSpotify {
        async fn current_user_id(&self) -> Result<String> {
            Ok("user-1".to_string())
        }
        async fn create_playlist(
            &self,
            _user_id: &str,
            name: &str,
            _public: bool,
            _description: Option<&str>,
        ) -> Result<(String, String)> {
            *self.creates.lock().unwrap() += 1;
            // A freshly created playlist is empty.
            *self.remote.lock().unwrap() = vec![];
            Ok((
                "bp-id-1".to_string(),
                format!("https://open.spotify.com/playlist/bp-id-1#{}", name),
            ))
        }
        async fn add_tracks_to_playlist(&self, _playlist_id: &str, uris: &[String]) -> Result<()> {
            self.adds.lock().unwrap().push(uris.to_vec());
            self.remote.lock().unwrap().extend(uris.iter().cloned());
            Ok(())
        }
        async fn replace_tracks(&self, _playlist_id: &str, uris: &[String]) -> Result<()> {
            if let Some(msg) = self.fail_replace.lock().unwrap().clone() {
                anyhow::bail!(msg);
            }
            self.replaces.lock().unwrap().push(uris.to_vec());
            *self.remote.lock().unwrap() = uris.to_vec();
            Ok(())
        }
        async fn playlist_uris(&self, _playlist_id: &str) -> Result<Vec<String>> {
            let mut remote = self.remote.lock().unwrap().clone();
            if *self.corrupt_remote.lock().unwrap() {
                remote.push("spotify:track:stale".to_string());
            }
            Ok(remote)
        }
    }

    struct MockDeemix {
        queued: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl BackpackDeemixOps for MockDeemix {
        async fn ensure_queued(&self, url: &str) -> Result<()> {
            self.queued.lock().unwrap().push(url.to_string());
            Ok(())
        }
    }

    /// Add the `settings` + `deemix_downloads` tables to an aggregation DB.
    async fn create_settings_tables(pool: &SqlitePool) {
        sqlx::query(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE deemix_downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                spotify_playlist_url TEXT NOT NULL UNIQUE,
                playlist_name TEXT,
                status TEXT NOT NULL DEFAULT 'queued',
                track_count_total INTEGER DEFAULT 0,
                track_count_downloaded INTEGER DEFAULT 0,
                error_message TEXT,
                is_backpack INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER DEFAULT (unixepoch()),
                updated_at INTEGER DEFAULT (unixepoch())
            )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn settings_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE deemix_downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                spotify_playlist_url TEXT NOT NULL UNIQUE,
                playlist_name TEXT,
                status TEXT NOT NULL DEFAULT 'queued',
                track_count_total INTEGER DEFAULT 0,
                track_count_downloaded INTEGER DEFAULT 0,
                error_message TEXT,
                is_backpack INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER DEFAULT (unixepoch()),
                updated_at INTEGER DEFAULT (unixepoch())
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn materialize_creates_playlist_and_submits_once() {
        let pool = test_db().await;
        seed(&pool).await;
        // Add settings + deemix_downloads tables (not part of the aggregation schema).
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        let deemix = MockDeemix {
            queued: std::sync::Mutex::new(vec![]),
        };

        let out = materialize_backpack_playlist(&pool, &spotify, Some(&deemix))
            .await
            .unwrap();

        assert!(out.created);
        assert!(out.updated);
        assert_eq!(out.track_count, 1);
        assert!(out.deemix_submitted);
        assert_eq!(*spotify.creates.lock().unwrap(), 1);
        assert_eq!(*deemix.queued.lock().unwrap(), vec![out.spotify_url.clone().unwrap()]);

        // Single deemix_downloads row, marked is_backpack.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM deemix_downloads WHERE is_backpack = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);

        // Second run with unchanged set must be a no-op (no new create/submit).
        let out2 = materialize_backpack_playlist(&pool, &spotify, Some(&deemix))
            .await
            .unwrap();
        assert!(!out2.created);
        assert!(!out2.updated);
        assert!(!out2.deemix_submitted);
        assert_eq!(*spotify.creates.lock().unwrap(), 1);
        assert_eq!(deemix.queued.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn record_backpack_submit_replaces_sentinel() {
        let pool = settings_db().await;
        // Simulate migration-025 state: a sentinel is_backpack row.
        sqlx::query(
            "INSERT INTO deemix_downloads (spotify_playlist_url, playlist_name, is_backpack)
             VALUES (?, 'Backpack', 1)",
        )
        .bind(BACKPACK_SENTINEL_URL)
        .execute(&pool)
        .await
        .unwrap();

        let real = "https://open.spotify.com/playlist/realid".to_string();
        record_backpack_submit(&pool, &real).await.unwrap();

        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT spotify_playlist_url, is_backpack FROM deemix_downloads ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, vec![(real, 1)], "sentinel must be replaced by the real row");
    }

    // ── Mirror semantics, verification, guards ──────────────────────────

    /// Add a second backpack tag so the set can be grown/shrunk in tests.
    async fn seed_second_backpack_track(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (9, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (9, 4, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn mirror_shrinks_playlist_when_track_is_removed() {
        let pool = test_db().await;
        seed(&pool).await;
        seed_second_backpack_track(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        let deemix = MockDeemix {
            queued: std::sync::Mutex::new(vec![]),
        };

        // First run: both backpack tracks.
        materialize_backpack_playlist(&pool, &spotify, Some(&deemix))
            .await
            .unwrap();
        assert_eq!(
            *spotify.remote.lock().unwrap(),
            vec!["spotify:track:bbb", "spotify:track:ddd"],
        );

        // Remove the backpack tag on the 'Deep' playlist (subscription stays).
        sqlx::query("DELETE FROM service_playlist_tracks WHERE playlist_id = 9 AND track_id = 4")
            .execute(&pool)
            .await
            .unwrap();

        let out = materialize_backpack_playlist_with(
            &pool,
            &spotify,
            Some(&deemix),
            MaterializeOptions {
                force: true,
                submit_to_deemix: true,
                dry_run: false,
            },
        )
        .await
        .unwrap();

        assert!(out.updated);
        assert!(!out.verification_failed);
        // The remote playlist must now hold exactly the shrunk set (mirror).
        assert_eq!(*spotify.remote.lock().unwrap(), vec!["spotify:track:bbb"]);
        // Replace (not append) was used: exactly 2 replaces total.
        assert_eq!(spotify.replaces.lock().unwrap().len(), 2);
        assert!(spotify.adds.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dry_run_makes_no_calls_and_keeps_state() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        let out = materialize_backpack_playlist_with::<_, MockDeemix>(
            &pool,
            &spotify,
            None,
            MaterializeOptions {
                force: true,
                submit_to_deemix: false,
                dry_run: true,
            },
        )
        .await
        .unwrap();

        assert!(out.dry_run);
        assert!(!out.updated);
        assert_eq!(*spotify.creates.lock().unwrap(), 0);
        assert!(spotify.replaces.lock().unwrap().is_empty());
        assert!(get_setting(&pool, KEY_BACKPACK_SIGNATURE).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn verification_mismatch_does_not_advance_signature() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        *spotify.corrupt_remote.lock().unwrap() = true;

        let out = materialize_backpack_playlist_with::<_, MockDeemix>(
            &pool,
            &spotify,
            None,
            MaterializeOptions {
                force: true,
                submit_to_deemix: false,
                dry_run: false,
            },
        )
        .await
        .unwrap();

        assert!(out.verification_failed, "mismatch must be reported");
        assert!(
            get_setting(&pool, KEY_BACKPACK_SIGNATURE).await.unwrap().is_none(),
            "signature must NOT be set when verification fails"
        );
    }

    #[tokio::test]
    async fn gone_playlist_is_recreated() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        // Simulate a stale stored playlist that Spotify no longer accepts.
        set_setting(&pool, KEY_BACKPACK_PLAYLIST_ID, "dead-id").await.unwrap();
        set_setting(&pool, KEY_BACKPACK_PLAYLIST_URL, "https://open.spotify.com/playlist/dead-id")
            .await
            .unwrap();
        *spotify.fail_replace.lock().unwrap() = Some("Spotify API error: 404 Not Found".to_string());

        let out = materialize_backpack_playlist_with::<_, MockDeemix>(
            &pool,
            &spotify,
            None,
            MaterializeOptions {
                force: true,
                submit_to_deemix: false,
                dry_run: false,
            },
        )
        .await;

        // The first replace fails with 404, so the guard re-creates. Our mock keeps
        // failing on replace, so the retry surfaces the error — but the create must
        // have happened and the settings must point at the new playlist id.
        assert!(out.is_err());
        assert_eq!(*spotify.creates.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn coordinator_dirty_marker_roundtrip() {
        let pool = settings_db().await;
        assert!(get_setting(&pool, KEY_BACKPACK_DIRTY_AT).await.unwrap().is_none());

        mark_backpack_dirty(&pool).await.unwrap();
        assert!(get_setting(&pool, KEY_BACKPACK_DIRTY_AT).await.unwrap().is_some());

        // The coordinator flags an on-demand push exactly once.
        let coord = BackpackSyncCoordinator::new();
        assert!(!has_pending_push(&coord));
        coord.request_push();
        assert!(has_pending_push(&coord));
        assert!(coord.take_push_request());
        assert!(!has_pending_push(&coord));
        assert!(!coord.take_push_request());
    }

    #[tokio::test]
    async fn backpack_status_reports_size_and_flags() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let status = backpack_status(&pool).await.unwrap();
        assert_eq!(status.track_count, 1);
        assert_eq!(status.file_count, 1);
        assert!(!status.dirty);
        assert!(status.playlist_url.is_none());

        mark_backpack_dirty(&pool).await.unwrap();
        let status = backpack_status(&pool).await.unwrap();
        assert!(status.dirty);
        assert!(status.dirty_at.is_some());
    }
}
