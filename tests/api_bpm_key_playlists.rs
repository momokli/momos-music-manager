//! Integration tests for the BPM//key system-playlist feature
//! (`/api/bpm-key-playlists*`, the playlists `system` filter, the poller kind
//! helper and the auto-enqueue path).
//!
//! Seeds are hand-crafted SQL against the migrated in-memory DB (no Spotify).

mod common;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sqlx::{Pool, Sqlite};

use momos_music_manager::config::ServiceCredentials;
use momos_music_manager::tasks::{Task, TaskManager, TaskType};
use momos_music_manager::{AppState, build_router};

// ── Seeds ───────────────────────────────────────────────────────────────────

/// A mix of buckets, missing metadata and an unlinked file.
///
/// * files 1+2 → bucket 124/12A (124.0→124, 124.4→124; `12m`/`12A` collapse)
/// * file 3    → bucket 129/4B (128.6 rounds to 129; `4d`→`4B`)
/// * file 4    → no BPM   → excluded
/// * file 5    → blank key → excluded
/// * file 6    → no Spotify link → excluded
async fn seed_bpm_key_files(pool: &Pool<Sqlite>) {
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified,
                              bpm, musical_key, spotify_id, isrc)
           VALUES
             (1, '/test/a.flac', 'h1', 'flac', 100, 1, 124.0, '12m', 'sp1', NULL),
             (2, '/test/b.flac', 'h2', 'flac', 100, 1, 124.4, '12A', 'sp2', NULL),
             (3, '/test/c.flac', 'h3', 'flac', 100, 1, 128.6, '4d',  'sp3', NULL),
             (4, '/test/d.flac', 'h4', 'flac', 100, 1, NULL,  '5m',  'sp4', NULL),
             (5, '/test/e.flac', 'h5', 'flac', 100, 1, 130.0, '',    'sp5', NULL),
             (6, '/test/f.flac', 'h6', 'flac', 100, 1, 124.0, '12m', 'sp6', NULL)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO service_tracks (id, service, service_id, title, artist)
           VALUES
             (1, 'spotify', 'sp1', 'A', 'X'),
             (2, 'spotify', 'sp2', 'B', 'X'),
             (3, 'spotify', 'sp3', 'C', 'X'),
             (4, 'spotify', 'sp4', 'D', 'X'),
             (5, 'spotify', 'sp5', 'E', 'X')"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// A `128.0` flac + `128.5` stem sharing one track (via ISRC).
async fn seed_variant_pair(pool: &Pool<Sqlite>) {
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified,
                              bpm, musical_key, stem_type, isrc)
           VALUES
             (10, '/v/track.flac', 'h10', 'flac', 100, 1, 128.0, '12m', NULL,    'ISRC-V'),
             (11, '/v/track.wav',  'h11', 'wav',  100, 1, 128.5, '12m', 'drums', 'ISRC-V')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO service_tracks (id, service, service_id, title, artist, isrc)
           VALUES (20, 'spotify', 'spv', 'V', 'X', 'ISRC-V')"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

// ── Test app with Spotify "configured" (creds present, no tokens) ────────────
//
// The shared `common::spawn_test_app` has no Spotify credentials, so the
// `/sync` endpoint would 503. This builds a parallel app whose config claims
// Spotify credentials (but no stored tokens), letting us exercise the enqueue
// path and the success response shape without any network call.

async fn spawn_app_with_spotify_config() -> (reqwest::Client, String, Pool<Sqlite>, Arc<AppState>) {
    let pool = common::create_test_db().await;

    let mut creds = ServiceCredentials::defaults_for_test();
    creds.spotify_client_id = Some("test-client-id".to_string());
    creds.spotify_client_secret = Some("test-client-secret".to_string());

    let state = Arc::new(AppState {
        db: pool.clone(),
        config: creds,
        task_manager: TaskManager::new(),
        embeddings: tokio::sync::Mutex::new(None),
        category_means: tokio::sync::Mutex::new(None),
        public_url: None,
    });
    let app = build_router(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{}", addr);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    for _ in 0..50 {
        if client
            .get(format!("{base}/api/version"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return (client, base, pool, state);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("test server did not start");
}

fn sync_task_count(tasks: &[momos_music_manager::tasks::TaskProgress]) -> usize {
    tasks
        .iter()
        .filter(|t| t.task_type == "sync_bpm_key_playlists")
        .count()
}

// ── Preview ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn preview_empty_library_returns_no_groups() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .get(format!("{}/api/bpm-key-playlists/preview", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["total"], 0);
    assert!(json["data"]["groups"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn preview_returns_exact_groups_rounded_and_normalised() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_bpm_key_files(&pool).await;

    let resp = client
        .get(format!("{}/api/bpm-key-playlists/preview", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let groups = json["data"]["groups"].as_array().unwrap();
    assert_eq!(
        json["data"]["total"], 2,
        "missing BPM/key and unlinked files excluded"
    );
    assert_eq!(groups.len(), 2);

    let g0 = &groups[0];
    assert_eq!(g0["bpm"], 124);
    assert_eq!(g0["key"], "12m");
    assert_eq!(g0["name"], "124bpm // 12m");
    assert_eq!(g0["systemKey"], "bpm_key:124:12A");
    assert_eq!(g0["fileCount"], 2);
    assert_eq!(g0["trackCount"], 2);
    assert_eq!(g0["exists"], false);
    assert!(g0["spotifyPlaylistId"].is_null());
    assert!(g0["spotifyUrl"].is_null());

    let g1 = &groups[1];
    assert_eq!(g1["bpm"], 129, "128.6 rounds to 129");
    assert_eq!(g1["key"], "4d");
    assert_eq!(g1["systemKey"], "bpm_key:129:4B");
    assert_eq!(g1["trackCount"], 1);
}

#[tokio::test]
async fn preview_variant_pair_yields_single_bucket() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_variant_pair(&pool).await;

    let resp = client
        .get(format!("{}/api/bpm-key-playlists/preview", base))
        .send()
        .await
        .unwrap();
    let json: Value = resp.json().await.unwrap();
    let groups = json["data"]["groups"].as_array().unwrap();

    assert_eq!(groups.len(), 1, "flac+stem must not split into two buckets");
    assert_eq!(
        groups[0]["bpm"], 128,
        "the non-stem flac is the representative"
    );
    assert_eq!(groups[0]["key"], "12m");
    assert_eq!(groups[0]["trackCount"], 1);
    assert_eq!(groups[0]["fileCount"], 2);
}

#[tokio::test]
async fn preview_marks_existing_system_playlists() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_bpm_key_files(&pool).await;

    sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, playlist_kind, system_key, imported_at, updated_at)
         VALUES ('spotify', 'abc', '124bpm // 12m', 'generated', 'bpm_key:124:12A', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let json: Value = client
        .get(format!("{}/api/bpm-key-playlists/preview", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let g0 = &json["data"]["groups"][0];
    assert_eq!(g0["exists"], true);
    assert_eq!(g0["spotifyPlaylistId"], "abc");
    assert_eq!(g0["spotifyUrl"], "https://open.spotify.com/playlist/abc");
}

// ── List ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_system_playlists_empty() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let json: Value = client
        .get(format!("{}/api/bpm-key-playlists", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json["data"]["total"], 0);
    assert!(json["data"]["playlists"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn list_system_playlists_with_track_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_bpm_key_files(&pool).await;

    sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, playlist_kind, system_key, imported_at, updated_at)
         VALUES ('spotify', 'abc', '124bpm // 12m', 'generated', 'bpm_key:124:12A', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let json: Value = client
        .get(format!("{}/api/bpm-key-playlists", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(json["data"]["total"], 1);
    let p = &json["data"]["playlists"][0];
    assert_eq!(p["name"], "124bpm // 12m");
    assert_eq!(p["systemKey"], "bpm_key:124:12A");
    assert_eq!(p["spotifyPlaylistId"], "abc");
    assert_eq!(p["spotifyUrl"], "https://open.spotify.com/playlist/abc");
    assert_eq!(p["trackCount"], 2);
}

// ── Settings ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn settings_defaults() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let json: Value = client
        .get(format!("{}/api/bpm-key-playlists/settings", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s = &json["data"]["settings"];
    assert_eq!(s["enabled"], false);
    assert_eq!(s["nameTemplate"], "{bpm}bpm // {key}");
    assert_eq!(s["namePrefix"], "");
    assert_eq!(s["minTracks"], 1);
    assert_eq!(s["public"], false);
    assert_eq!(s["keyStyle"], "md");
    assert_eq!(s["strict"], false);
    assert_eq!(s["scheduleEnabled"], false);
    assert_eq!(s["scheduleIntervalSecs"], 3600);
}

#[tokio::test]
async fn settings_put_is_partial_and_persists() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let json: Value = client
        .put(format!("{}/api/bpm-key-playlists/settings", base))
        .json(&serde_json::json!({ "enabled": true, "minTracks": 3, "keyStyle": "camelot" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s = &json["data"]["settings"];
    assert_eq!(s["enabled"], true);
    assert_eq!(s["minTracks"], 3);
    assert_eq!(s["keyStyle"], "camelot");
    // Untouched keys keep their defaults.
    assert_eq!(s["nameTemplate"], "{bpm}bpm // {key}");
    assert_eq!(s["strict"], false);

    // Persisted — a fresh GET reflects the change.
    let again: Value = client
        .get(format!("{}/api/bpm-key-playlists/settings", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["data"]["settings"]["enabled"], true);
    assert_eq!(again["data"]["settings"]["minTracks"], 3);
}

#[tokio::test]
async fn settings_key_style_flips_preview_rendering() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_bpm_key_files(&pool).await;

    client
        .put(format!("{}/api/bpm-key-playlists/settings", base))
        .json(&serde_json::json!({ "keyStyle": "camelot" }))
        .send()
        .await
        .unwrap();

    let json: Value = client
        .get(format!("{}/api/bpm-key-playlists/preview", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json["data"]["groups"][0]["key"], "12A");
    assert_eq!(json["data"]["groups"][0]["name"], "124bpm // 12A");
}

// ── Sync endpoint ───────────────────────────────────────────────────────────

#[tokio::test]
async fn sync_returns_503_without_spotify_config() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_bpm_key_files(&pool).await;

    let resp = client
        .post(format!("{}/api/bpm-key-playlists/sync", base))
        .json(&serde_json::json!({ "strict": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
    let json: Value = resp.json().await.unwrap();
    assert!(json["error"].as_str().unwrap().contains("Spotify"));
}

#[tokio::test]
async fn sync_starts_task_and_reports_group_count() {
    let (client, base, pool, state) = spawn_app_with_spotify_config().await;
    seed_bpm_key_files(&pool).await;

    let resp = client
        .post(format!("{}/api/bpm-key-playlists/sync", base))
        .json(&serde_json::json!({ "strict": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    assert!(
        json["data"]["taskId"].as_str().unwrap().len() > 0,
        "a task id must be returned"
    );
    assert_eq!(json["data"]["groupCount"], 2);

    // A sync task was registered on the manager.
    let tasks = state.task_manager.list_tasks().await;
    assert_eq!(sync_task_count(&tasks), 1);
}

#[tokio::test]
async fn sync_without_body_is_accepted() {
    let (client, base, pool, _state) = spawn_app_with_spotify_config().await;
    seed_bpm_key_files(&pool).await;

    let resp = client
        .post(format!("{}/api/bpm-key-playlists/sync", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ── Auto-enqueue (post-scan / post-import path) ──────────────────────────────

#[tokio::test]
async fn auto_enqueue_is_disabled_by_default() {
    let (_client, _base, pool, state) = spawn_app_with_spotify_config().await;

    momos_music_manager::tasks::maybe_auto_enqueue_bpm_key_sync(&state.task_manager, &pool).await;

    let tasks = state.task_manager.list_tasks().await;
    assert_eq!(sync_task_count(&tasks), 0);
}

#[tokio::test]
async fn auto_enqueue_creates_one_task_when_enabled() {
    let (_client, _base, pool, state) = spawn_app_with_spotify_config().await;
    momos_music_manager::db::set_bool(&pool, momos_music_manager::db::KEY_BPMKEY_ENABLED, true)
        .await
        .unwrap();

    momos_music_manager::tasks::maybe_auto_enqueue_bpm_key_sync(&state.task_manager, &pool).await;

    let tasks = state.task_manager.list_tasks().await;
    assert_eq!(sync_task_count(&tasks), 1);
}

#[tokio::test]
async fn auto_enqueue_coalesces_via_conflict_key() {
    let (_client, _base, pool, state) = spawn_app_with_spotify_config().await;
    momos_music_manager::db::set_bool(&pool, momos_music_manager::db::KEY_BPMKEY_ENABLED, true)
        .await
        .unwrap();

    // Simulate an in-flight sync (conflict key `bpm_key_sync`) that never finishes.
    let pending = Task::new(TaskType::SyncBpmKeyPlaylists { strict: false }, None);
    state.task_manager.start_task_unique(pending).await.unwrap();

    // Both auto-enqueues must be rejected by the conflict key.
    momos_music_manager::tasks::maybe_auto_enqueue_bpm_key_sync(&state.task_manager, &pool).await;
    momos_music_manager::tasks::maybe_auto_enqueue_bpm_key_sync(&state.task_manager, &pool).await;

    let tasks = state.task_manager.list_tasks().await;
    assert_eq!(sync_task_count(&tasks), 1, "bursts coalesce into one task");
}

// ── Playlists `system` filter + generated exclusion ──────────────────────────

#[tokio::test]
async fn playlists_system_filter_excludes_generated_by_default() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await; // 2 curated spotify playlists

    sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, playlist_kind, system_key, imported_at, updated_at)
         VALUES ('spotify', 'gen1', '124bpm // 12m', 'generated', 'bpm_key:124:12A', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Default → generated hidden.
    let json: Value = client
        .get(format!("{}/api/playlists", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<String> = json["data"]["playlists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"124bpm // 12m".to_string()));
    assert_eq!(json["data"]["total"], 2);

    // system=only → just the generated one.
    let json: Value = client
        .get(format!("{}/api/playlists?system=only", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json["data"]["total"], 1);
    assert_eq!(json["data"]["playlists"][0]["name"], "124bpm // 12m");
    assert_eq!(json["data"]["playlists"][0]["playlistKind"], "generated");

    // system=include → everything.
    let json: Value = client
        .get(format!("{}/api/playlists?system=include", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json["data"]["total"], 3);
}

#[tokio::test]
async fn playlists_dto_carries_playlist_kind() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let json: Value = client
        .get(format!("{}/api/playlists", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for p in json["data"]["playlists"].as_array().unwrap() {
        assert_eq!(p["playlistKind"], "curated");
    }
}

// ── Regression: generated playlists stay out of the tag/usage surfaces ───────

#[tokio::test]
async fn generated_system_playlists_excluded_from_tag_and_snapshot_surfaces() {
    let pool = common::create_test_db().await;

    // A generated system playlist whose name would otherwise match a tag, plus a
    // curated playlist with no matching tag.
    sqlx::query("INSERT INTO tags (name, category_id) VALUES ('bpm_key:124:12A', 1)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, playlist_kind, system_key, snapshot_id, imported_at, updated_at)
         VALUES ('spotify', 'gen1', 'bpm_key:124:12A', 'generated', 'bpm_key:124:12A', 'gsnap', 0, 0),
                ('spotify', 'cur1', 'Untagged Curated', 'curated', NULL, 'csnap', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let without_tags = momos_music_manager::db::get_playlists_without_tags(&pool)
        .await
        .unwrap();
    assert!(
        without_tags.iter().all(|p| p.playlist_kind != "generated"),
        "generated rows must never appear in get_playlists_without_tags"
    );
    assert!(without_tags.iter().any(|p| p.name == "Untagged Curated"));

    let snapshots = momos_music_manager::db::get_spotify_playlist_snapshots(&pool)
        .await
        .unwrap();
    assert!(snapshots.iter().all(|(_, pid, _)| pid != "gen1"));
    assert!(snapshots.iter().any(|(_, pid, _)| pid == "cur1"));

    // The poller's full kind map, however, still knows the generated row.
    let kinds = momos_music_manager::db::get_spotify_playlist_kinds(&pool)
        .await
        .unwrap();
    assert!(
        kinds
            .iter()
            .any(|(pid, k)| pid == "gen1" && k == "generated")
    );
}
