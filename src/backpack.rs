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

use anyhow::{Context, Result};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use tracing::{debug, info};

use crate::db::settings::{
    KEY_BACKPACK_PLAYLIST_ID, KEY_BACKPACK_PLAYLIST_URL, KEY_BACKPACK_SIGNATURE, get_setting,
    set_setting,
};

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
    let uris = resolve_backpack_track_uris(pool).await?;
    let signature = backpack_signature(&uris);

    let stored_id = get_setting(pool, KEY_BACKPACK_PLAYLIST_ID).await?;
    let stored_signature = get_setting(pool, KEY_BACKPACK_SIGNATURE).await?;

    let unchanged = stored_id.is_some() && stored_signature.as_deref() == Some(signature.as_str());
    if unchanged {
        debug!("Backpack unchanged ({signature}), skipping materialisation");
        let url = get_setting(pool, KEY_BACKPACK_PLAYLIST_URL).await?;
        return Ok(MaterializeOutcome {
            track_count: uris.len(),
            created: false,
            updated: false,
            spotify_url: url,
            deemix_submitted: false,
        });
    }

    if uris.is_empty() {
        debug!("Backpack set is empty; nothing to materialise");
        return Ok(MaterializeOutcome {
            track_count: 0,
            created: false,
            updated: false,
            spotify_url: None,
            deemix_submitted: false,
        });
    }

    // Ensure the playlist exists (create once, then reuse the persisted id).
    let created = stored_id.is_none();
    let (playlist_id, spotify_url) = match stored_id {
        Some(id) => (
            id.clone(),
            get_setting(pool, KEY_BACKPACK_PLAYLIST_URL)
                .await?
                .unwrap_or_else(|| format!("https://open.spotify.com/playlist/{id}")),
        ),
        None => {
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
            (id, url)
        }
    };

    // Add the (deduplicated) tracks. `add_tracks_to_playlist` batches by 100.
    spotify.add_tracks_to_playlist(&playlist_id, &uris).await?;
    set_setting(pool, KEY_BACKPACK_SIGNATURE, &signature).await?;
    info!(
        "Backpack materialised: {} track(s) into '{}' ({})",
        uris.len(),
        BACKPACK_PLAYLIST_NAME,
        spotify_url
    );

    // Transport = 1: submit only this one URL to deemix.
    let deemix_submitted = match deemix {
        Some(client) => {
            client.ensure_queued(&spotify_url).await?;
            record_backpack_submit(pool, &spotify_url).await?;
            true
        }
        None => {
            debug!("deemix not configured; skipping Backpack submit");
            false
        }
    };

    Ok(MaterializeOutcome {
        track_count: uris.len(),
        created,
        updated: true,
        spotify_url: Some(spotify_url),
        deemix_submitted,
    })
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
            Ok((
                "bp-id-1".to_string(),
                format!("https://open.spotify.com/playlist/bp-id-1#{}", name),
            ))
        }
        async fn add_tracks_to_playlist(&self, _playlist_id: &str, uris: &[String]) -> Result<()> {
            self.adds.lock().unwrap().push(uris.to_vec());
            Ok(())
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

        let spotify = MockSpotify {
            creates: std::sync::Mutex::new(0),
            adds: std::sync::Mutex::new(vec![]),
        };
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
}
