//! Tonies auto-sync (#503): the worker-side dirty hook + debounce, all offline.
//!
//! Mocks: a Spotify API (playlist items; also counts every request), plus the
//! combined Tonies + music-api mocks. Every base is pinned to loopback, so a
//! green run proves the auto-sync never reaches a real service and — crucially
//! (T4) — never issues a Spotify request.
//!
//! Covered acceptance criteria:
//! * T1 `playlist_refresh_sets_dirty_since_for_auto_bindings`
//! * T2 `unchanged_refresh_does_not_trigger_sync`
//! * T3 `quiet_period_suppresses_due_binding`
//! * T4 `autosync_makes_no_spotify_requests`

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use music_hub::config::Config;
use music_hub::tonies::run::{self, STATUS_OK};
use serde_json::{Value, json};

const NOW: &str = "2026-01-01T00:00:00+00:00";
const KEY: &str = "hub-tonies-test-secret";

// ── mock Spotify (playlist items + request counter) ─────────────────────────

#[derive(Clone, Default)]
struct SpotifyMock {
    /// Every path the auto-sync / hook asked for (must stay empty for T4).
    hits: Arc<Mutex<Vec<String>>>,
}

async fn playlist_items(
    State(s): State<SpotifyMock>,
    Path(_id): Path<String>,
    uri: axum::http::Uri,
) -> Json<Value> {
    s.hits.lock().unwrap().push(uri.path().to_string());
    Json(json!({ "items": [], "next": null }))
}

async fn mock_spotify() -> (String, SpotifyMock) {
    let st = SpotifyMock::default();
    let app = Router::new()
        .route("/playlists/{id}/items", get(playlist_items))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), st)
}

// ── mock Tonies + music-api ─────────────────────────────────────────────────

#[derive(Clone)]
struct TmMock {
    base: String,
    ready: Arc<Mutex<HashSet<String>>>,
    chapters: Arc<Mutex<Vec<Value>>>,
    file_seq: Arc<Mutex<usize>>,
    uploads: Arc<Mutex<Vec<Value>>>,
}

async fn token_handler(
    Form(_f): Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    Json(json!({"access_token": "acc", "refresh_token": "rt", "expires_in": 300}))
}

async fn get_tonie(
    State(m): State<TmMock>,
    Path((_h, _c)): Path<(String, String)>,
) -> Json<Value> {
    let chapters = m.chapters.lock().unwrap().clone();
    Json(json!({ "id": "ct1", "chapters": chapters }))
}

async fn file_handler(State(m): State<TmMock>) -> Json<Value> {
    let n = {
        let mut s = m.file_seq.lock().unwrap();
        *s += 1;
        *s
    };
    // Shape of the live API: the upload target is nested under `request`.
    Json(json!({
        "fileId": format!("file-{n}"),
        "request": {
            "url": format!("{}/s3/upload", m.base),
            "fields": {"key": format!("k{n}"), "policy": "p"}
        }
    }))
}

async fn put_handler(State(_m): State<TmMock>, _body: Bytes) -> StatusCode {
    StatusCode::OK
}

async fn chapter_handler(State(m): State<TmMock>, Json(body): Json<Value>) -> Json<Value> {
    m.uploads.lock().unwrap().push(body);
    Json(json!({"ok": true}))
}

async fn isrc_status(State(m): State<TmMock>, Path(isrc): Path<String>) -> Json<Value> {
    let ready = m.ready.lock().unwrap().contains(&isrc);
    Json(json!({ "isrc": isrc, "state": if ready { "ready" } else { "absent" } }))
}

async fn isrc_bytes(State(_m): State<TmMock>, Path((_isrc, _fmt)): Path<(String, String)>) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "audio/mpeg")],
        b"MP3BYTES".to_vec(),
    )
        .into_response()
}

async fn mock_tonies_music() -> (String, TmMock) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let st = TmMock {
        base: base.clone(),
        ready: Arc::new(Mutex::new(HashSet::new())),
        chapters: Arc::new(Mutex::new(Vec::new())),
        file_seq: Arc::new(Mutex::new(0)),
        uploads: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/realms/tonies/protocol/openid-connect/token", post(token_handler))
        .route("/v2/file", post(file_handler))
        .route("/s3/upload", post(put_handler))
        .route("/v2/households/{hid}/creativetonies/{ctid}/chapters", post(chapter_handler))
        .route("/v2/households/{hid}/creativetonies/{ctid}", get(get_tonie))
        .route("/isrc/{isrc}", get(isrc_status))
        .route("/isrc/{isrc}/{fmt}", get(isrc_bytes))
        .with_state(st.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, st)
}

fn cfg_tonies(base: &str) -> Config {
    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.tonies_auth_base = base.to_string();
    cfg.tonies_api_base = base.to_string();
    cfg.tonies_secret_key = Some(KEY.to_string());
    cfg.music_api_base = base.to_string();
    cfg.music_api_token = Some("music-token".to_string());
    cfg
}

// ── DB seeding helpers ──────────────────────────────────────────────────────

async fn pool() -> sqlx::SqlitePool {
    music_hub::db::connect("sqlite::memory:").await.unwrap()
}

async fn seed_user(pool: &sqlx::SqlitePool, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at) VALUES (?1, ?1, ?2) RETURNING id",
    )
    .bind(slug)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn link_spotify(pool: &sqlx::SqlitePool, uid: i64) {
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes, authorized_at, connected_at)
         VALUES (?1, 'spotify', 'mock-access', 'mock-refresh', '2099-01-01T00:00:00+00:00', 'test', ?2, ?2)",
    )
    .bind(uid)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_playlist(
    pool: &sqlx::SqlitePool,
    uid: i64,
    ext: &str,
    snapshot: Option<&str>,
    last_synced: Option<&str>,
    items_available: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists
             (user_id, service, playlist_id, name, enabled_for_fetch, items_available,
              snapshot_id, last_synced_snapshot, fetched_at)
         VALUES (?1, 'spotify', ?2, ?2, 1, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(uid)
    .bind(ext)
    .bind(items_available)
    .bind(snapshot)
    .bind(last_synced)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_binding(
    pool: &sqlx::SqlitePool,
    uid: i64,
    tonie_id: &str,
    source_id: i64,
    mode: &str,
    dirty_since: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, tonie_name, source_kind, source_id, format, mode,
              dirty_since, created_at, updated_at)
         VALUES (?1, ?2, ?2, 'playlist', ?3, 'mp3-128', ?4, ?5, ?6, ?6) RETURNING id",
    )
    .bind(uid)
    .bind(tonie_id)
    .bind(source_id)
    .bind(mode)
    .bind(dirty_since)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_track(
    pool: &sqlx::SqlitePool,
    sid: &str,
    isrc: Option<&str>,
    title: &str,
    duration_ms: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, isrc, title, duration_ms, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(title)
    .bind(duration_ms)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn link_playlist_track(pool: &sqlx::SqlitePool, playlist: i64, track: i64, pos: i64) {
    sqlx::query(
        "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
    )
    .bind(playlist)
    .bind(track)
    .bind(pos)
    .execute(pool)
    .await
    .unwrap();
}

async fn dirty_of(pool: &sqlx::SqlitePool, binding_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT dirty_since FROM hub_tonie_bindings WHERE id = ?1")
        .bind(binding_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── T1: hook marks only the matching auto bindings dirty ────────────────────

#[tokio::test]
async fn playlist_refresh_sets_dirty_since_for_auto_bindings() {
    let (sp_base, sp) = mock_spotify().await;
    let pool = pool().await;
    let uid = seed_user(&pool, "t1-hook").await;
    link_spotify(&pool, uid).await;

    // Playlist A changed (snapshot != last_synced) → the job body runs.
    let pl_a = insert_playlist(&pool, uid, "a", Some("snap-2"), Some("snap-1"), 0).await;
    // Playlist B unchanged.
    let pl_b = insert_playlist(&pool, uid, "b", Some("snap-1"), Some("snap-1"), 1).await;

    let auto_a = insert_binding(&pool, uid, "ct-auto-a", pl_a, "auto", None).await;
    let manual_a = insert_binding(&pool, uid, "ct-manual-a", pl_a, "manual", None).await;
    let auto_b = insert_binding(&pool, uid, "ct-auto-b", pl_b, "auto", None).await;

    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.spotify_api_base = sp_base;

    let worked = music_hub::worker::playlist_jobs(&pool, &cfg).await.unwrap();
    assert!(worked, "playlist A was selected");

    assert!(
        dirty_of(&pool, auto_a).await.is_some(),
        "auto binding of the changed playlist is marked dirty"
    );
    assert!(
        dirty_of(&pool, manual_a).await.is_none(),
        "manual binding is never marked dirty"
    );
    assert!(
        dirty_of(&pool, auto_b).await.is_none(),
        "auto binding of an unchanged playlist is untouched"
    );
    assert_eq!(sp.hits.lock().unwrap().len(), 1, "only playlist A refreshed");
}

// ── T2: an unchanged refresh makes no request and no sync ───────────────────

#[tokio::test]
async fn unchanged_refresh_does_not_trigger_sync() {
    let (sp_base, sp) = mock_spotify().await;
    let pool = pool().await;
    let uid = seed_user(&pool, "t2-noop").await;
    link_spotify(&pool, uid).await;

    // First refresh: the remote snapshot differs → the job body runs and the
    // hook marks the auto binding dirty (same path as T1).
    let pl = insert_playlist(&pool, uid, "same", Some("snap-2"), Some("snap-1"), 0).await;
    let auto = insert_binding(&pool, uid, "ct-auto", pl, "auto", None).await;

    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.spotify_api_base = sp_base;

    let worked = music_hub::worker::playlist_jobs(&pool, &cfg).await.unwrap();
    assert!(worked, "the changed playlist was refreshed");
    let dirty_first = dirty_of(&pool, auto)
        .await
        .expect("first refresh marks the binding dirty");
    assert_eq!(sp.hits.lock().unwrap().len(), 1, "one Spotify request");

    // Second refresh: `last_synced_snapshot` now equals `snapshot_id`, so the
    // job query skips the playlist — the body (and the hook) never run again.
    let worked = music_hub::worker::playlist_jobs(&pool, &cfg).await.unwrap();
    assert!(!worked, "no job selected for an unchanged playlist");
    assert_eq!(
        dirty_of(&pool, auto).await.as_deref(),
        Some(dirty_first.as_str()),
        "dirty_since is not re-marked by the second refresh"
    );
    assert_eq!(
        sp.hits.lock().unwrap().len(),
        1,
        "no extra Spotify request on the second refresh"
    );

    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tonie_syncs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0, "no run-log entry for an unchanged refresh");
}

// ── T3: the quiet period suppresses a freshly dirty binding ─────────────────

#[tokio::test]
async fn quiet_period_suppresses_due_binding() {
    let pool = pool().await;
    let uid = seed_user(&pool, "t3-quiet").await;
    let pl = insert_playlist(&pool, uid, "q", Some("s"), Some("s"), 1).await;

    let now = chrono::Utc::now();
    let fresh = now.to_rfc3339();
    let old = (now - chrono::Duration::seconds(301)).to_rfc3339();

    let fresh_id = insert_binding(&pool, uid, "ct-fresh", pl, "auto", Some(&fresh)).await;
    let old_id = insert_binding(&pool, uid, "ct-old", pl, "auto", Some(&old)).await;

    let d = music_hub::tonies::Debounce {
        quiet_secs: 300,
        min_interval_secs: 600,
    };
    assert!(
        !music_hub::tonies::is_due(Some(&fresh), None, now, &d),
        "dirty_since = now is inside the quiet window"
    );
    assert!(
        music_hub::tonies::is_due(Some(&old), None, now, &d),
        "dirty_since = now-301s is past the quiet window"
    );

    let due = music_hub::tonies::due_bindings(&pool, now, &d)
        .await
        .unwrap();
    let ids: Vec<i64> = due.iter().map(|b| b.id).collect();
    assert_eq!(ids, vec![old_id], "only the old binding is due");
    assert!(!ids.contains(&fresh_id));
}

// ── T4: the auto-sync issues zero Spotify requests ──────────────────────────

#[tokio::test]
async fn autosync_makes_no_spotify_requests() {
    let (sp_base, sp) = mock_spotify().await;
    let (base, m) = mock_tonies_music().await;
    *m.ready.lock().unwrap() = ["x"].iter().map(|s| s.to_string()).collect();

    let pool = pool().await;
    let uid = seed_user(&pool, "t4-nospotify").await;
    music_hub::tonies::save_account(&pool, uid, Some("hh1"), Some("a@example.com"), "refresh", KEY)
        .await
        .unwrap();

    let pl = insert_playlist(&pool, uid, "t4", Some("s"), Some("s"), 1).await;
    let tid = insert_track(&pool, "sp-1", Some("x"), "Song One", 60_000).await;
    link_playlist_track(&pool, pl, tid, 0).await;

    let due_since = (chrono::Utc::now() - chrono::Duration::seconds(1000)).to_rfc3339();
    let bid = insert_binding(&pool, uid, "ct1", pl, "auto", Some(&due_since)).await;

    let mut cfg = cfg_tonies(&base);
    cfg.spotify_api_base = sp_base;

    let ran = music_hub::tonies::run_due(&pool, &cfg, 10).await.unwrap();
    assert_eq!(ran, 1, "exactly one due binding ran");

    // The run is logged and reconciled (dirty cleared).
    let rows = run::list_syncs(&pool, bid, 5).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, STATUS_OK);
    assert!(dirty_of(&pool, bid).await.is_none(), "dirty_since cleared");

    // Acceptance T4: not a single Spotify request was made.
    assert!(
        sp.hits.lock().unwrap().is_empty(),
        "autosync must never touch Spotify: {:?}",
        sp.hits.lock().unwrap()
    );

    // …and the run genuinely wrote a chapter (not a silent no-op): the Tonies
    // mock received exactly one chapter POST, and the run-log recorded it.
    let uploads = m.uploads.lock().unwrap();
    assert_eq!(uploads.len(), 1, "exactly one chapter was written");
    assert_eq!(
        uploads[0].get("title").and_then(Value::as_str),
        Some("Song One"),
        "the written chapter carries the track title"
    );
    assert_eq!(rows[0].added, Some(1), "run-log records one added chapter");
    assert_eq!(rows[0].chapters_after, Some(1), "one chapter after the run");
}
