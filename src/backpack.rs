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
use crate::spotify::cooldown::cooldown as spotify_cooldown;
use crate::spotify::retry::extract_retry_after_secs;

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

/// Serialises Backpack materialisations process-wide.
///
/// The coordinator loop and the manual `POST /api/backpack/push` handler can
/// otherwise race on the same playlist: interleaved `replace` + `add` batches
/// were observed to leave a mixed/duplicated remote state (and made the
/// post-write verification read a moving target).
static MATERIALIZE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
///
/// # Why this does not use the view
///
/// `v_file_track_link` is a `UNION` of a hand-written match table and a
/// `files`-driven `JOIN ... ON (OR)` chain. SQLite cannot push a
/// `track_id IN (...)` predicate into that shape: it materialises the whole
/// view (every file × every service track, plus a correlated subquery per
/// file) and only then applies the filter. Measured through this crate's own
/// sqlx/bundled SQLite 3.46 on a production-scale library (~14k backpack
/// tracks / 13.7k files):
///
/// | query                                    | time    |
/// |------------------------------------------|---------|
/// | `SELECT v.file_id FROM v_file_track_link v WHERE v.track_id IN (...)` | **101.5 s** |
/// | `SELECT COUNT(*) ... WHERE v.track_id IN (SELECT ... FROM v_file_track_link)` | 27.0 s |
/// | [`get_file_ids_for_track_ids`] (this file) | **0.41 s** |
///
/// `file_id`-driven access to the same view is fine (~50 ms) — only the
/// `track_id` direction degenerates. The cost grows with
/// `rows(service_tracks) × rows(files) × IN-list size`, so it gets worse as
/// the library grows.
///
/// The set is computed instead by inverting the join in
/// [`get_file_ids_for_track_ids`].
pub async fn get_backpack_file_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>> {
    let ids = get_backpack_track_ids(pool).await?;
    get_file_ids_for_track_ids(pool, &ids).await
}

/// Resolve a set of `service_tracks.id` to the `files.id` linked to them,
/// without going through `v_file_track_link`'s slow `track_id` direction.
///
/// Semantics preserved from the view (see `023_file_track_corrections.sql`):
///   * a row exists when *any* match key agrees (ISRC, or the service-specific
///     id column), and for `service = 'local'` when `service_id = files.id`;
///   * `link_type = 'exclude'` rows remove an automatic match;
///   * `link_type = 'include'` rows add a link that never existed.
///
/// Returns ascending, deduplicated file ids.
pub async fn get_file_ids_for_track_ids(pool: &Pool<Sqlite>, ids: &[i64]) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // One bind per track id (a `VALUES` CTE) instead of repeating the whole
    // id list once per match arm — same plan, a fifth of the binds.
    let values = ids.iter().map(|_| "(?)").collect::<Vec<_>>().join(",");
    let sql = format!(
        "WITH backpack_track(track_id) AS (VALUES {values}),
         candidate(file_id, track_id) AS (
             SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON f.isrc = st.isrc
              WHERE st.isrc IS NOT NULL
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON st.service = 'spotify' AND st.service_id = f.spotify_id
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON st.service = 'soundcloud' AND st.service_id = f.soundcloud_id
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON st.service = 'youtube' AND st.service_id = f.youtube_id
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               -- `service_id` is the file id as text. The extra
               -- `CAST(f.id AS TEXT)` term keeps the comparison exactly as
               -- literal as the view's, so ids with unlikely spellings
               -- ('007', ' 7', '+7') behave identically; the integer
               -- comparison is what the index can serve.
               JOIN files f ON CAST(st.service_id AS INTEGER) = f.id
                           AND CAST(f.id AS TEXT) = st.service_id
              WHERE st.service = 'local'
         )
         SELECT DISTINCT c.file_id FROM candidate c
          WHERE NOT EXISTS (
              SELECT 1 FROM file_track_corrections ftc
               WHERE ftc.file_id = c.file_id
                 AND ftc.track_id = c.track_id
                 AND ftc.link_type = 'exclude'
          )
         UNION
         SELECT file_id FROM file_track_corrections
          WHERE link_type = 'include'
            AND track_id IN (SELECT track_id FROM backpack_track)
         ORDER BY 1"
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids {
        query = query.bind(id);
    }
    Ok(query.fetch_all(pool).await?)
}

/// Resolve the `files.id` of every file that shares a track with the given
/// backpack files, plus backpack files that are linked to no track at all.
///
/// This is the "variants of the same song" expansion the backpack effects
/// need (pick one best format per track). It replaces the view's slow
/// `track_id IN (SELECT ...)` form with the `file_id` direction (fast) plus
/// [`get_file_ids_for_track_ids`].
pub async fn get_backpack_family_file_ids(
    pool: &Pool<Sqlite>,
    backpack_file_ids: &[i64],
) -> Result<Vec<i64>> {
    if backpack_file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = backpack_file_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    // Tracks any backpack file belongs to (file_id direction: fast).
    let tracks_sql = format!(
        "SELECT DISTINCT track_id FROM v_file_track_link WHERE file_id IN ({placeholders})"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&tracks_sql);
    for id in backpack_file_ids {
        q = q.bind(id);
    }
    let tracks: Vec<i64> = q.fetch_all(pool).await?;

    // Every file on those tracks (key-driven: fast). This already covers the
    // backpack files themselves, since they are on those tracks.
    let mut file_ids = get_file_ids_for_track_ids(pool, &tracks).await?;

    // Backpack files linked to *no* track are their own group.
    let unlinked_sql = format!(
        "SELECT f.id FROM files f
          WHERE f.id IN ({placeholders})
            AND NOT EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = f.id)"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&unlinked_sql);
    for id in backpack_file_ids {
        q = q.bind(id);
    }
    file_ids.extend(q.fetch_all(pool).await?);

    file_ids.sort_unstable();
    file_ids.dedup();
    Ok(file_ids)
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
    // Never interleave two materialisations on the same playlist.
    let _materialize_guard = MATERIALIZE_LOCK.lock().await;

    let uris = resolve_backpack_track_uris(pool).await?;
    let signature = backpack_signature(&uris);

    let stored_id = get_setting(pool, KEY_BACKPACK_PLAYLIST_ID).await?;
    let stored_signature = get_setting(pool, KEY_BACKPACK_SIGNATURE).await?;
    let stored_url = get_setting(pool, KEY_BACKPACK_PLAYLIST_URL).await?;

    let unchanged = !opts.force
        && stored_id.is_some()
        && stored_signature.as_deref() == Some(signature.as_str());
    if unchanged {
        // Nothing pending: the materialised playlist already matches this set.
        // Clear the dirty marker here as well — otherwise it stays set forever
        // (only a real push cleared it), which keeps the coordinator's debounce
        // gate permanently open and re-resolves the whole Backpack set on every
        // tick.
        clear_backpack_dirty(pool).await?;
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

    let mut spotify_url =
        spotify_url.unwrap_or_else(|| format!("https://open.spotify.com/playlist/{playlist_id}"));

    // Mirror the (deduplicated, stably ordered) track set into the playlist.
    let mirrored = match spotify.replace_tracks(&playlist_id, &uris).await {
        Ok(()) => true,
        Err(e) if is_playlist_gone(&e) => {
            // The playlist was deleted manually or belongs to a different
            // account now — re-create it and retry once. Never wedge.
            warn!("Backpack playlist '{playlist_id}' is not authorable ({e:#}); re-creating it");
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
            let present = intended.intersection(&remote_set).count();
            let missing = intended.len().saturating_sub(present);
            let unexpected = remote_set.len().saturating_sub(present);

            // Spotify silently drops tracks that are unavailable in the user's
            // market from the write (measured: 2 of 6480 on a real library), so
            // an *exact* set match is not achievable and gating the signature
            // on it pinned the coordinator into a rebuild loop. Only a gross
            // deviation (a genuinely broken mirror) counts as a failure.
            let tolerance = (intended.len() / 100).max(5);
            if missing <= tolerance && unexpected <= tolerance {
                if missing > 0 || unexpected > 0 {
                    warn!(
                        "Backpack verification: {missing} missing / {unexpected} unexpected of {} intended — within tolerance (playlist {playlist_id})",
                        intended.len(),
                    );
                }
                false
            } else {
                let only_remote: Vec<&String> = remote_set
                    .difference(&intended)
                    .take(4)
                    .copied()
                    .collect();
                let only_intended: Vec<&String> = intended
                    .difference(&remote_set)
                    .take(4)
                    .copied()
                    .collect();
                warn!(
                    "Backpack verification mismatch (playlist {playlist_id}): intended {} | remote {} | missing {missing} unexpected {unexpected} (tolerance {tolerance}) | only_remote: {only_remote:?} | only_intended: {only_intended:?}",
                    intended.len(),
                    remote_set.len(),
                );
                true
            }
        }
        Err(e) => {
            // The write itself succeeded; we just could not read the playlist
            // back (typically rate limited right after the write burst). That
            // is inconclusive, not a mismatch — advance the signature anyway,
            // so a transient read failure cannot pin the coordinator into a
            // rebuild loop on every debounce.
            warn!(
                "Backpack verification read failed ({e:#}) — inconclusive, advancing signature"
            );
            false
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

/// First retry delay after a failed Backpack materialisation.
pub const BACKPACK_RETRY_BASE_SECS: u64 = 60;
/// Upper bound for the exponential Backpack retry backoff.
pub const BACKPACK_RETRY_MAX_SECS: u64 = 1800;

/// Exponential backoff (seconds) for the `failures`-th consecutive failed
/// Backpack materialisation: 60s, 120s, 240s, … capped at
/// [`BACKPACK_RETRY_MAX_SECS`]. A Spotify `Retry-After` (`retry_after`) always
/// wins when larger — the server's own deadline must be honoured.
pub fn backpack_retry_backoff_secs(failures: u32, retry_after: Option<u64>) -> u64 {
    let shift = failures.saturating_sub(1).min(16);
    let exponential = BACKPACK_RETRY_BASE_SECS
        .saturating_mul(1u64 << shift)
        .min(BACKPACK_RETRY_MAX_SECS);
    exponential.max(retry_after.unwrap_or(0))
}

/// Coordinates Backpack materialisation.
///
/// Mutations only set a dirty marker; the [`start_backpack_coordinator`] loop
/// materialises after a debounce. `request_push()` materialises on the next
/// tick regardless of the signature (used by the manual push endpoint).
///
/// A failed push is **not** retried every tick: [`Self::note_failure`] schedules
/// the next attempt with exponential backoff, extended to Spotify's
/// `Retry-After` when one was reported — a rate limit must never be turned into
/// a request storm.
pub struct BackpackSyncCoordinator {
    push_requested: std::sync::atomic::AtomicBool,
    /// Consecutive failed materialisation attempts (reset on success).
    consecutive_failures: std::sync::atomic::AtomicU32,
    /// Absolute unix second before which no automatic retry may run.
    next_attempt_unix: std::sync::atomic::AtomicI64,
}

impl BackpackSyncCoordinator {
    pub fn new() -> Self {
        Self {
            push_requested: std::sync::atomic::AtomicBool::new(false),
            consecutive_failures: std::sync::atomic::AtomicU32::new(0),
            next_attempt_unix: std::sync::atomic::AtomicI64::new(0),
        }
    }

    /// Seconds until the next backoff-scheduled retry, or `None` when a retry
    /// is allowed now.
    pub fn retry_in_secs(&self) -> Option<u64> {
        let remaining = self
            .next_attempt_unix
            .load(std::sync::atomic::Ordering::SeqCst)
            - now_secs();
        if remaining > 0 {
            Some(remaining as u64)
        } else {
            None
        }
    }

    /// Record a successful materialisation: clears the failure backoff.
    pub fn note_success(&self) {
        self.consecutive_failures
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.next_attempt_unix
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Record a failed materialisation and schedule the next retry with
    /// exponential backoff, extended to at least `retry_after` when Spotify
    /// reported a rate limit.
    pub fn note_failure(&self, retry_after: Option<u64>) {
        let failures = self
            .consecutive_failures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let delay = backpack_retry_backoff_secs(failures, retry_after);
        self.next_attempt_unix.store(
            now_secs().saturating_add(delay as i64),
            std::sync::atomic::Ordering::SeqCst,
        );
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
        self.push_requested
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for BackpackSyncCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod retry_backoff_tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        assert_eq!(backpack_retry_backoff_secs(1, None), 60);
        assert_eq!(backpack_retry_backoff_secs(2, None), 120);
        assert_eq!(backpack_retry_backoff_secs(3, None), 240);
        assert_eq!(backpack_retry_backoff_secs(4, None), 480);
        assert_eq!(
            backpack_retry_backoff_secs(100, None),
            BACKPACK_RETRY_MAX_SECS
        );
    }

    #[test]
    fn retry_after_wins_over_exponential_backoff() {
        assert_eq!(backpack_retry_backoff_secs(1, Some(3063)), 3063);
        assert_eq!(
            backpack_retry_backoff_secs(99, Some(1)),
            BACKPACK_RETRY_MAX_SECS
        );
    }

    #[test]
    fn coordinator_schedules_and_clears_backoff() {
        let coord = BackpackSyncCoordinator::new();
        assert!(coord.retry_in_secs().is_none());

        coord.note_failure(Some(120));
        let remaining = coord.retry_in_secs().expect("backoff after failure");
        assert!((119..=120).contains(&remaining), "got {remaining}");

        // A second failure keeps a (larger) schedule queued.
        coord.note_failure(None);
        assert!(coord.retry_in_secs().is_some());

        coord.note_success();
        assert!(coord.retry_in_secs().is_none());
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
        let reconcile_due =
            last_reconcile.elapsed().unwrap_or_default() >= BACKPACK_RECONCILE_INTERVAL;

        // Never retry a failed push on every tick: the failure backoff and the
        // process-wide Spotify cooldown both gate automatic attempts. A manual
        // push (`forced`) is a single user-initiated call and always gets
        // through.
        let retry_in = coordinator.retry_in_secs();
        let cooling = spotify_cooldown().remaining_secs();
        let blocked = retry_in.is_some() || cooling.is_some();

        if forced || ((debounce_elapsed || reconcile_due) && !blocked) {
            if blocked {
                debug!(
                    "Backpack coordinator: automatic retry suppressed (backoff {retry_in:?}, cooldown {cooling:?})"
                );
            }
            last_reconcile = SystemTime::now();
            match SpotifyClient::from_stored_tokens(db.clone(), &credentials).await {
                Ok(client) => {
                    let deemix = crate::deemix::DeemixClient::from_db(db.clone()).await;
                    let opts = MaterializeOptions {
                        force: forced,
                        submit_to_deemix: forced,
                        dry_run: false,
                    };
                    match materialize_backpack_playlist_with(&db, &client, deemix.as_ref(), opts)
                        .await
                    {
                        Ok(outcome) if outcome.updated => {
                            coordinator.note_success();
                            spotify_cooldown().clear();
                            info!(
                                "Backpack coordinator: materialised {} track(s) (forced={}, verification_failed={})",
                                outcome.track_count, forced, outcome.verification_failed,
                            );
                            record_push_status(&db, "ok", None).await;
                        }
                        Ok(_) => {
                            coordinator.note_success();
                            if forced {
                                record_push_status(&db, "ok", None).await;
                            }
                        }
                        Err(e) => {
                            let retry_after = extract_retry_after_secs(&e);
                            if let Some(secs) = retry_after {
                                spotify_cooldown().note_retry_after(secs);
                            }
                            coordinator.note_failure(retry_after);
                            let next_in = coordinator.retry_in_secs().unwrap_or(0);
                            warn!(
                                "Backpack coordinator: push failed: {e:#} — next automatic retry in {next_in}s"
                            );
                            record_push_status(&db, "error", Some(&format!("{e:#}"))).await;
                        }
                    }
                }
                Err(e) => {
                    // Not a Spotify API failure (usually missing/expired
                    // tokens) — back off anyway so this cannot spin every tick.
                    // Warn, not debug: this silently disabled the whole Backpack
                    // transport when it first happened.
                    coordinator.note_failure(None);
                    let next_in = coordinator.retry_in_secs().unwrap_or(0);
                    warn!(
                        "Backpack coordinator: Spotify not available — skipping (next attempt in {next_in}s): {e:#}"
                    );
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

    sqlx::query("DELETE FROM deemix_downloads WHERE is_backpack = 1 AND spotify_playlist_url != ?")
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
            self,
            user_id,
            name,
            public,
            description,
        )
        .await
    }

    async fn add_tracks_to_playlist(&self, playlist_id: &str, uris: &[String]) -> Result<()> {
        crate::spotify::client::SpotifyClient::add_tracks_to_playlist(self, playlist_id, uris).await
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
                spotify_id TEXT,
                soundcloud_id TEXT,
                youtube_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Table backing the view's manual includes / excludes; usually created
        // by migration 023, extended here so callers can seed corrections.
        sqlx::query(
            "CREATE TABLE file_track_corrections (
                file_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                link_type TEXT NOT NULL,
                reason TEXT,
                created_at INTEGER DEFAULT (unixepoch()),
                UNIQUE(file_id, track_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE VIEW v_file_track_link AS
             SELECT file_id, track_id FROM file_track_corrections WHERE link_type = 'include'
             UNION
             SELECT f.id AS file_id, st.id AS track_id
             FROM files f
             JOIN service_tracks st ON (
                 st.isrc = f.isrc
                 OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
                 OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
                 OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
                 OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
             )
             WHERE NOT EXISTS (
                 SELECT 1 FROM file_track_corrections ftc
                 WHERE ftc.file_id = f.id
                   AND ftc.track_id = st.id
                   AND ftc.link_type = 'exclude'
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

        sqlx::query(
            "INSERT INTO tag_categories (id, name, prefix, is_default) VALUES (1, 'Mood', 'M', 0)",
        )
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
        assert_eq!(
            ids,
            vec![2],
            "overlapping sources must dedupe to a single id"
        );
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

    /// The backpack file set must be exactly what `v_file_track_link` yields.
    ///
    /// `get_backpack_file_ids` deliberately avoids the view because SQLite
    /// cannot push a `track_id IN (...)` predicate into its `UNION … ON (OR)`
    /// shape and instead materialises the whole view (~65 s in production).
    /// This test pins the two together so the hand-written rewrite can never
    /// silently drift from the view's semantics.
    #[tokio::test]
    async fn backpack_file_ids_matches_the_view() {
        let pool = test_db().await;
        seed(&pool).await;

        // Extra playlists/tracks so the backpack set is larger than one id.
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (3, 1, 0), (3, 2, 0), (3, 3, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Track 1 also matches via ISRC (not just spotify_id).
        sqlx::query("UPDATE service_tracks SET isrc = 'ISRC-1' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE files SET isrc = 'ISRC-1' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        // A file with no matching service track at all.
        sqlx::query(
            "INSERT INTO files (id, file_path, file_type, file_size, spotify_id) VALUES
                (9, '/orphan.flac', 'flac', 1, 'zzz')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Corrections: exclude one automatic match, include one hand-made link
        // to a track that has no matching file.
        sqlx::query(
            "INSERT INTO file_track_corrections (file_id, track_id, link_type) VALUES
                (1, 1, 'exclude'),
                (1, 4, 'exclude'),
                (9, 5, 'include')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Track 4 is a backpack member with a matching file (id 2 → only ties to
        // track 3, so give track 4 a real file to exclude).
        sqlx::query("UPDATE files SET spotify_id = 'ddd' WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let expected_sql = format!(
            "SELECT DISTINCT v.file_id FROM v_file_track_link v WHERE v.track_id IN ({placeholders}) ORDER BY v.file_id"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&expected_sql);
        for id in &ids {
            q = q.bind(id);
        }
        let expected = q.fetch_all(&pool).await.unwrap();

        let actual = get_backpack_file_ids(&pool).await.unwrap();
        assert_eq!(
            actual, expected,
            "rewrite must return exactly the view's file set (backpack tracks: {ids:?})"
        );
        // Sanity: the fixture exercises exclusions and inclusions, i.e. the set
        // is not simply "every file".
        assert!(
            !actual.is_empty() && actual.len() < 6,
            "fixture should yield a partial set, got {actual:?}"
        );
    }

    /// The `service = 'local'` match arm must be index-driven and stay
    /// byte-identical to the view's `service_id = CAST(files.id AS TEXT)`.
    #[tokio::test]
    async fn backpack_file_ids_matches_local_service_tracks() {
        let pool = test_db().await;
        seed(&pool).await;

        // Local service tracks point at a file by id-as-text.
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist) VALUES
                (101, 'local', '1', 'L1', 'A'),
                (102, 'local', '2', 'L2', 'A'),
                (103, 'local', '999', 'L-missing', 'A')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (3, 101, 0), (3, 102, 0), (3, 103, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        // track 2 comes from the seeded 'Deep Mix' subscription.
        assert_eq!(ids, vec![2, 101, 102, 103]);

        let actual = get_backpack_file_ids(&pool).await.unwrap();
        // file 3 ties to the subscribed track 2; files 1 and 2 come from the
        // local tracks. The dangling local id 999 contributes nothing.
        assert_eq!(
            actual,
            vec![1, 2, 3],
            "local tracks resolve to their file by id; a dangling id contributes nothing"
        );

        // Cross-check against the view for the same predicate.
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let view_sql = format!(
            "SELECT DISTINCT v.file_id FROM v_file_track_link v WHERE v.track_id IN ({placeholders}) ORDER BY v.file_id"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&view_sql);
        for id in &ids {
            q = q.bind(id);
        }
        assert_eq!(actual, q.fetch_all(&pool).await.unwrap());
    }

    /// Empty backpack → no query at all, no error.
    #[tokio::test]
    async fn backpack_file_ids_empty_when_no_backpack_tracks() {
        let pool = test_db().await;

        let files = get_backpack_file_ids(&pool).await.unwrap();
        assert!(files.is_empty());
    }

    /// The track-family expansion must equal the view-based SQL it replaced
    /// (`SELECT ... WHERE track_id IN (SELECT ... FROM v_file_track_link)` plus
    /// the unlinked-files branch).
    #[tokio::test]
    async fn backpack_family_file_ids_matches_the_view_query() {
        let pool = test_db().await;
        seed(&pool).await;

        // Give a second file the same ISRC as an existing one, so one track has
        // two variants, and leave one file linked to no track at all.
        sqlx::query(
            "INSERT INTO files (id, file_path, file_type, file_size, isrc, spotify_id) VALUES
                (4, '/t1-alt.mp3', 'mp3', 1, NULL, 'aaa'),
                (5, '/orphan.flac', 'flac', 1, NULL, 'zzz')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Backpack = track 2 (subscribed) and its file (id 3) plus the orphan 5.
        let backpack_file_ids = vec![3i64, 5i64];

        let placeholders = backpack_file_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let legacy_sql = format!(
            "SELECT DISTINCT f.id FROM files f
              JOIN v_file_track_link v ON v.file_id = f.id
              WHERE v.track_id IN (
                  SELECT DISTINCT v2.track_id FROM v_file_track_link v2
                  WHERE v2.file_id IN ({placeholders})
              )
              UNION
              SELECT DISTINCT f.id FROM files f
              WHERE f.id IN ({placeholders})
                AND f.id NOT IN (SELECT file_id FROM v_file_track_link)
              ORDER BY 1"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&legacy_sql);
        for id in &backpack_file_ids {
            q = q.bind(id);
        }
        for id in &backpack_file_ids {
            q = q.bind(id);
        }
        let expected = q.fetch_all(&pool).await.unwrap();

        let actual = get_backpack_family_file_ids(&pool, &backpack_file_ids)
            .await
            .unwrap();
        assert_eq!(
            actual, expected,
            "family expansion must match the view query"
        );
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
        /// How many stale URIs `playlist_uris` reports on top of the real remote
        /// state (simulates a deviating/corrupt playlist).
        extra_stale: std::sync::Mutex<usize>,
        /// When set, `playlist_uris` (and only that) fails with this message.
        fail_playlist_uris: std::sync::Mutex<Option<String>>,
    }

    impl MockSpotify {
        fn new() -> Self {
            Self {
                creates: std::sync::Mutex::new(0),
                adds: std::sync::Mutex::new(vec![]),
                remote: std::sync::Mutex::new(vec![]),
                replaces: std::sync::Mutex::new(vec![]),
                fail_replace: std::sync::Mutex::new(None),
                extra_stale: std::sync::Mutex::new(0),
                fail_playlist_uris: std::sync::Mutex::new(None),
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
            if let Some(msg) = self.fail_playlist_uris.lock().unwrap().clone() {
                anyhow::bail!(msg);
            }
            let mut remote = self.remote.lock().unwrap().clone();
            for i in 0..*self.extra_stale.lock().unwrap() {
                remote.push(format!("spotify:track:stale{i}"));
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
        assert_eq!(
            *deemix.queued.lock().unwrap(),
            vec![out.spotify_url.clone().unwrap()]
        );

        // Single deemix_downloads row, marked is_backpack.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deemix_downloads WHERE is_backpack = 1")
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

        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT spotify_playlist_url, is_backpack FROM deemix_downloads ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![(real, 1)],
            "sentinel must be replaced by the real row"
        );
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
        assert!(
            get_setting(&pool, KEY_BACKPACK_SIGNATURE)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn verification_mismatch_does_not_advance_signature() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        // A gross deviation (12 stale URIs vs a tolerance of 5) must be reported
        // and must NOT advance the signature.
        *spotify.extra_stale.lock().unwrap() = 12;

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
            get_setting(&pool, KEY_BACKPACK_SIGNATURE)
                .await
                .unwrap()
                .is_none(),
            "signature must NOT be set when verification fails"
        );
    }

    #[tokio::test]
    async fn verification_read_failure_is_inconclusive_and_advances_signature() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        *spotify.fail_playlist_uris.lock().unwrap() =
            Some("Spotify API error: http error: status code 429 Too Many Requests".to_string());

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

        assert!(out.updated);
        assert!(
            !out.verification_failed,
            "a read failure is inconclusive, not a mismatch"
        );
        assert!(
            get_setting(&pool, KEY_BACKPACK_SIGNATURE)
                .await
                .unwrap()
                .is_some(),
            "the signature must advance so a transient read failure cannot pin a rebuild loop"
        );
    }

    #[tokio::test]
    async fn small_remote_deviation_is_tolerated_and_advances_signature() {
        // Spotify silently drops tracks that are unavailable in the user's
        // market from the write (measured: 2 of 6480 on a real library), so a
        // small deviation must not block the signature — gating on an exact
        // match is what pinned the coordinator into a rebuild loop.
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        *spotify.extra_stale.lock().unwrap() = 1;

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

        assert!(!out.verification_failed, "a small deviation is tolerated");
        assert!(
            get_setting(&pool, KEY_BACKPACK_SIGNATURE)
                .await
                .unwrap()
                .is_some(),
            "the signature must advance so a few dropped tracks cannot pin a rebuild loop"
        );
    }

    #[tokio::test]
    async fn unchanged_set_clears_the_dirty_marker() {
        // A signature-gated run that finds nothing to do must still clear the
        // dirty marker: leaving it set kept the coordinator's debounce gate open
        // and re-resolved the whole Backpack set on every tick.
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        materialize_backpack_playlist_with::<_, MockDeemix>(
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

        // A membership mutation marks the set dirty...
        mark_backpack_dirty(&pool).await.unwrap();
        assert!(get_setting(&pool, KEY_BACKPACK_DIRTY_AT).await.unwrap().is_some());

        // ...but the next (signature-gated) run has nothing to do and clears it.
        let out = materialize_backpack_playlist_with::<_, MockDeemix>(
            &pool,
            &spotify,
            None,
            MaterializeOptions {
                force: false,
                submit_to_deemix: false,
                dry_run: false,
            },
        )
        .await
        .unwrap();

        assert!(!out.updated, "an unchanged set must not re-materialise");
        assert!(
            get_setting(&pool, KEY_BACKPACK_DIRTY_AT).await.unwrap().is_none(),
            "the dirty marker must be cleared when there is nothing to do"
        );
    }

    #[tokio::test]
    async fn gone_playlist_is_recreated() {
        let pool = test_db().await;
        seed(&pool).await;
        create_settings_tables(&pool).await;

        let spotify = MockSpotify::new();
        // Simulate a stale stored playlist that Spotify no longer accepts.
        set_setting(&pool, KEY_BACKPACK_PLAYLIST_ID, "dead-id")
            .await
            .unwrap();
        set_setting(
            &pool,
            KEY_BACKPACK_PLAYLIST_URL,
            "https://open.spotify.com/playlist/dead-id",
        )
        .await
        .unwrap();
        *spotify.fail_replace.lock().unwrap() =
            Some("Spotify API error: 404 Not Found".to_string());

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
        assert!(
            get_setting(&pool, KEY_BACKPACK_DIRTY_AT)
                .await
                .unwrap()
                .is_none()
        );

        mark_backpack_dirty(&pool).await.unwrap();
        assert!(
            get_setting(&pool, KEY_BACKPACK_DIRTY_AT)
                .await
                .unwrap()
                .is_some()
        );

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
