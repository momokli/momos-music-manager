//! Integration tests for the MMM data importer (issue #372): playlists +
//! settings, plus a regression smoke test for the existing tag importer.
//!
//! A deterministic MMM-shaped SQLite db is built programmatically (no binary
//! fixture). The importers are driven directly against `app.pool` with the
//! fixture path, mirroring `tests/traktor_import.rs`.

mod common;

use std::path::Path;

use music_hub::mmm_import;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// Build a fresh MMM-shaped SQLite db in `dir` and return `(sqlite_url, pool)`.
async fn mmm_db(dir: &Path) -> (String, SqlitePool) {
    let path = dir.join("mmm.db");
    let url = format!("sqlite:{}", path.display());
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open fixture db");

    let schema = [
        "CREATE TABLE service_tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            service TEXT NOT NULL,
            service_id TEXT NOT NULL,
            title TEXT NOT NULL,
            artist TEXT NOT NULL,
            isrc TEXT,
            UNIQUE(service, service_id))",
        "CREATE TABLE service_playlists (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            service TEXT NOT NULL,
            playlist_id TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT,
            playlist_kind TEXT NOT NULL DEFAULT 'curated',
            UNIQUE(service, playlist_id))",
        "CREATE TABLE service_playlist_tracks (
            playlist_id INTEGER NOT NULL,
            track_id INTEGER NOT NULL,
            position INTEGER,
            added_at INTEGER,
            deleted_at INTEGER,
            PRIMARY KEY (playlist_id, track_id))",
        "CREATE TABLE settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT 0)",
        // Tag tables — needed by the import_tags regression smoke test.
        "CREATE TABLE tag_categories (
            id INTEGER PRIMARY KEY, name TEXT, icon TEXT, sort_order INTEGER)",
        "CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT, category_id INTEGER)",
        "CREATE TABLE tag_parents (tag_id INTEGER, parent_tag_id INTEGER)",
        "CREATE TABLE tag_energy_levels (tag_id INTEGER, energy_level INTEGER)",
    ];
    for stmt in schema {
        sqlx::query(stmt).execute(&pool).await.expect("create table");
    }
    (url, pool)
}

/// Insert a hub track the importer can match against (service `spotify`).
async fn hub_track(pool: &SqlitePool, sid: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?1, 'X', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .fetch_one(pool)
    .await
    .expect("insert hub track")
}

/// Insert an MMM service track and return its id.
async fn mmm_track(pool: &SqlitePool, service: &str, sid: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO service_tracks (service, service_id, title, artist)
         VALUES (?1, ?2, ?2, 'X') RETURNING id",
    )
    .bind(service)
    .bind(sid)
    .fetch_one(pool)
    .await
    .expect("insert mmm track")
}

/// Insert an MMM playlist and return its id.
async fn mmm_playlist(pool: &SqlitePool, service: &str, pid: &str, name: &str, kind: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO service_playlists (service, playlist_id, name, description, playlist_kind)
         VALUES (?1, ?2, ?3, NULL, ?4) RETURNING id",
    )
    .bind(service)
    .bind(pid)
    .bind(name)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("insert mmm playlist")
}

async fn mmm_member(pool: &SqlitePool, pl: i64, tr: i64, pos: i64, deleted: bool) {
    let deleted_at: Option<i64> = deleted.then_some(1);
    sqlx::query(
        "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at, deleted_at)
         VALUES (?1, ?2, ?3, 0, ?4)",
    )
    .bind(pl)
    .bind(tr)
    .bind(pos)
    .bind(deleted_at)
    .execute(pool)
    .await
    .expect("insert mmm member");
}

async fn mmm_setting(pool: &SqlitePool, key: &str, value: &str) {
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, 0)")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await
        .expect("insert setting");
}

// (1) happy path: playlist row lands with exact fields / owner ───────────────

#[tokio::test]
async fn import_playlists_creates_rows() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    hub_track(&app.pool, "msp1").await;
    let pl = mmm_playlist(&mmm, "spotify", "ple1", "Deep House", "curated").await;
    let tr = mmm_track(&mmm, "spotify", "msp1").await;
    mmm_member(&mmm, pl, tr, 0, false).await;
    sqlx::query("UPDATE service_playlists SET description = 'A set' WHERE id = ?1")
        .bind(pl)
        .execute(&mmm)
        .await
        .unwrap();

    let s = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.playlists, 1);
    assert_eq!(s.linked, 1);
    assert_eq!(s.matched, 1);
    assert_eq!(s.unmatched, 0);

    let (uid, service, pid, name, desc, items): (i64, String, String, String, String, i64) =
        sqlx::query_as(
            "SELECT user_id, service, playlist_id, name, description, items_available
               FROM hub_playlists WHERE playlist_id = 'ple1'",
        )
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(uid, app.seed.alice, "owned by the slug user");
    assert_eq!(service, "spotify");
    assert_eq!(pid, "ple1");
    assert_eq!(name, "Deep House");
    assert_eq!(desc, "A set");
    assert_eq!(items, 1);
}

// (2) memberships linked by (service, service_track_id) with positions ───────

#[tokio::test]
async fn import_playlists_links_tracks_by_service_id() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    let a = hub_track(&app.pool, "mspA").await;
    let b = hub_track(&app.pool, "mspB").await;
    let pl = mmm_playlist(&mmm, "spotify", "ple2", "Ordered", "curated").await;
    let ta = mmm_track(&mmm, "spotify", "mspA").await;
    let tb = mmm_track(&mmm, "spotify", "mspB").await;
    mmm_member(&mmm, pl, ta, 0, false).await;
    mmm_member(&mmm, pl, tb, 7, false).await;

    let s = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.linked, 2);

    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT track_id, position FROM hub_playlist_tracks
          WHERE playlist_id = (SELECT id FROM hub_playlists WHERE playlist_id = 'ple2')
          ORDER BY position",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(a, 0), (b, 7)], "positions preserved");
}

// (3) idempotent re-run: stable counts and items_available ──────────────────

#[tokio::test]
async fn import_playlists_idempotent() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    hub_track(&app.pool, "msp1").await;
    hub_track(&app.pool, "msp2").await;
    let pl = mmm_playlist(&mmm, "spotify", "ple3", "Idem", "curated").await;
    let t1 = mmm_track(&mmm, "spotify", "msp1").await;
    let t2 = mmm_track(&mmm, "spotify", "msp2").await;
    mmm_member(&mmm, pl, t1, 0, false).await;
    mmm_member(&mmm, pl, t2, 1, false).await;

    let s1 = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    let s2 = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();

    let n_pl: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_playlists WHERE playlist_id = 'ple3'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let n_link: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks
          WHERE playlist_id = (SELECT id FROM hub_playlists WHERE playlist_id = 'ple3')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n_pl, 1, "no duplicate playlist");
    assert_eq!(n_link, 2, "no duplicate memberships");
    assert_eq!(s1.playlists, s2.playlists);
    assert_eq!(s1.linked, s2.linked);

    let items: i64 =
        sqlx::query_scalar("SELECT items_available FROM hub_playlists WHERE playlist_id = 'ple3'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(items, 2, "items_available stable across re-run");
}

// (4) unmatched memberships are counted, not written ────────────────────────

#[tokio::test]
async fn import_playlists_unmatched_counted() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    hub_track(&app.pool, "msp1").await;
    let pl = mmm_playlist(&mmm, "spotify", "ple4", "Mixed", "curated").await;
    let known = mmm_track(&mmm, "spotify", "msp1").await;
    let ghost = mmm_track(&mmm, "spotify", "no-hub-track").await;
    mmm_member(&mmm, pl, known, 0, false).await;
    mmm_member(&mmm, pl, ghost, 1, false).await;

    let s = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.matched, 1);
    assert_eq!(s.unmatched, 1, "unmatched counted");
    assert_eq!(s.linked, 1, "only the matched membership is written");

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks
          WHERE playlist_id = (SELECT id FROM hub_playlists WHERE playlist_id = 'ple4')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

// (5) generated playlists are skipped ───────────────────────────────────────

#[tokio::test]
async fn import_playlists_skips_generated() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    mmm_playlist(&mmm, "local", "gen1", "Daily-2024", "generated").await;

    let s = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.skipped_playlists, 1);
    assert_eq!(s.playlists, 0);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_playlists WHERE playlist_id = 'gen1'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "generated playlist not imported");
}

// (6) archived memberships (deleted_at NOT NULL) are skipped ────────────────

#[tokio::test]
async fn import_playlists_skips_archived_tracks() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    hub_track(&app.pool, "msp1").await;
    hub_track(&app.pool, "msp2").await;
    let pl = mmm_playlist(&mmm, "spotify", "ple6", "Archived", "curated").await;
    let t1 = mmm_track(&mmm, "spotify", "msp1").await;
    let t2 = mmm_track(&mmm, "spotify", "msp2").await;
    mmm_member(&mmm, pl, t1, 0, false).await;
    mmm_member(&mmm, pl, t2, 1, true).await; // archived

    let s = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.skipped_tracks, 1);
    assert_eq!(s.linked, 1);
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks
          WHERE playlist_id = (SELECT id FROM hub_playlists WHERE playlist_id = 'ple6')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "archived membership not written");
}

// (7) liked playlists get is_liked = 1 ──────────────────────────────────────

#[tokio::test]
async fn import_playlists_liked_flag() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    mmm_playlist(&mmm, "spotify", "liked1", "Liked", "liked").await;
    mmm_playlist(&mmm, "spotify", "cur1", "Curated", "curated").await;

    let s = mmm_import::import_playlists(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.playlists, 2);
    assert_eq!(s.liked, 1);

    let (l, c): (i64, i64) = (
        sqlx::query_scalar("SELECT is_liked FROM hub_playlists WHERE playlist_id = 'liked1'")
            .fetch_one(&app.pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT is_liked FROM hub_playlists WHERE playlist_id = 'cur1'")
            .fetch_one(&app.pool)
            .await
            .unwrap(),
    );
    assert_eq!(l, 1);
    assert_eq!(c, 0);
}

// (8) unknown user slug bails ───────────────────────────────────────────────

#[tokio::test]
async fn import_playlists_unknown_user_bails() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, _mmm) = mmm_db(dir.path()).await;

    let err = mmm_import::import_playlists(&app.pool, &url, "nobody")
        .await
        .expect_err("unknown user must fail");
    assert!(err.to_string().contains("not found"), "got: {err}");
}

// (9) settings whitelist maps to mmm.* with 1:1 values ──────────────────────

#[tokio::test]
async fn import_settings_maps_whitelist() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    mmm_setting(&mmm, "backpack.sync_enabled", "1").await;
    mmm_setting(&mmm, "bpmkey.name_prefix", "[Phase]").await;
    mmm_setting(&mmm, "spotify.liked_sync_interval_secs", "600").await;

    let s = mmm_import::import_settings(&app.pool, &url).await.unwrap();
    assert_eq!(s.imported, 3);

    let v: String =
        sqlx::query_scalar("SELECT value FROM hub_settings WHERE key = 'mmm.backpack_sync_enabled'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(v, "1");
    let p: String =
        sqlx::query_scalar("SELECT value FROM hub_settings WHERE key = 'mmm.bpmkey.name_prefix'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(p, "[Phase]");
    let i: String = sqlx::query_scalar(
        "SELECT value FROM hub_settings WHERE key = 'mmm.liked_sync_interval_secs'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(i, "600");
}

// (10) operational + unknown keys are dropped, counters match ───────────────

#[tokio::test]
async fn import_settings_drops_operational_and_unknown() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    mmm_setting(&mmm, "backpack.sync_enabled", "1").await; // keep
    mmm_setting(&mmm, "autoupdate.enabled", "1").await; // operational
    mmm_setting(&mmm, "autoupdate.channel", "stable").await; // operational
    mmm_setting(&mmm, "telemetry.last_push_at", "123").await; // operational
    mmm_setting(&mmm, "spotify.liked_sync_last_at", "123").await; // operational
    mmm_setting(&mmm, "bpmkey.last_sync_at", "123").await; // operational
    mmm_setting(&mmm, "weird.unknown_key", "x").await; // unknown

    let s = mmm_import::import_settings(&app.pool, &url).await.unwrap();
    assert_eq!(s.imported, 1);
    assert_eq!(s.skipped_operational, 5);
    assert_eq!(s.skipped_unknown, 1);

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_settings
          WHERE key IN ('mmm.autoupdate.enabled','mmm.autoupdate.channel',
                        'mmm.telemetry.last_push_at','mmm.liked_sync_last_at',
                        'mmm.bpmkey.last_sync_at','mmm.weird.unknown_key')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "operational + unknown keys not imported");
}

// (11) no token/secret/api_key keys ever land in hub_settings ───────────────

#[tokio::test]
async fn import_settings_no_secrets_copied() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    mmm_setting(&mmm, "spotify.access_token", "SECRET").await;
    mmm_setting(&mmm, "service_config.client_secret", "SECRET").await;
    mmm_setting(&mmm, "some.api_key", "SECRET").await;
    mmm_setting(&mmm, "backpack.sync_enabled", "0").await; // one legit key

    let s = mmm_import::import_settings(&app.pool, &url).await.unwrap();
    assert_eq!(s.imported, 1, "only the whitelisted key");

    let leaked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_settings
          WHERE lower(key) LIKE '%token%' OR lower(key) LIKE '%secret%' OR lower(key) LIKE '%api_key%'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "no secret-looking key copied");
}

// (12) settings import is idempotent, updated_at refreshed ──────────────────

#[tokio::test]
async fn import_settings_idempotent() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    mmm_setting(&mmm, "backpack.sync_enabled", "1").await;
    mmm_setting(&mmm, "bpmkey.enabled", "0").await;

    let s1 = mmm_import::import_settings(&app.pool, &url).await.unwrap();
    assert_eq!(s1.imported, 2);

    // Age the rows so a second import must refresh updated_at.
    sqlx::query("UPDATE hub_settings SET updated_at = '1970-01-01T00:00:00+00:00' WHERE key LIKE 'mmm.%'")
        .execute(&app.pool)
        .await
        .unwrap();

    let s2 = mmm_import::import_settings(&app.pool, &url).await.unwrap();
    assert_eq!(s2.imported, 2);

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_settings WHERE key LIKE 'mmm.%'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n, 2, "no duplicates on re-run");

    let stale: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_settings
          WHERE key LIKE 'mmm.%' AND updated_at = '1970-01-01T00:00:00+00:00'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(stale, 0, "updated_at refreshed by re-run");
}

// (13) regression: the existing tag importer still works (smoke) ────────────

#[tokio::test]
async fn import_tags_still_works() {
    let app = common::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (url, mmm) = mmm_db(dir.path()).await;
    sqlx::query("INSERT INTO tag_categories (id, name, icon, sort_order) VALUES (1, 'Mood', '', 0)")
        .execute(&mmm)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tags (id, name, category_id) VALUES (1, 'Chill', 1)")
        .execute(&mmm)
        .await
        .unwrap();

    let s = mmm_import::import_tags(&app.pool, &url, "alice")
        .await
        .unwrap();
    assert_eq!(s.groups, 1);
    assert_eq!(s.tags, 1);

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tags WHERE name = 'Chill'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "tag imported via the unchanged path");
}
