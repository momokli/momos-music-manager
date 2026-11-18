//! Tonies sync run (#502): mocked Tonies API **and** mocked music-api, no
//! network. Every base is pinned to a loopback mock, so a green run proves the
//! run never reaches `api.tonies.com` / the real music-api.
//!
//! Covered acceptance criteria:
//! * upload order per new chapter: `POST /v2/file` → bytes → multipart PUT →
//!   `POST …/chapters`, and the `fileId` flows into the chapter body;
//! * a `Replace` reuses the shared prefix and PATCHes the full list (no re-upload);
//! * the transcoding poll is bounded → `timeout`;
//! * a failed upload leaves a log entry (`status='error'` + `error`);
//! * the sync endpoint is owner-scoped (404 otherwise) and its toast names
//!   chapters, minutes and skipped tracks — with no credential in the HTML.

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use music_hub::config::Config;
use music_hub::tonies::client::ToniesClient;
use music_hub::tonies::run::{self, SyncOptions};
use music_hub::tonies::{Binding, Tokens};
use serde_json::{Value, json};

const REFRESH: &str = "refresh-token-xyz";
const PASSWORD: &str = "s3cr3t-p4ssw0rd";
const KEY: &str = "hub-tonies-test-secret";

/// Shared state of the combined Tonies + music-api mock.
#[derive(Clone)]
struct Mock {
    base: String,
    /// Existing chapters on the Kreativ-Tonie (`GET …/creativetonies/{ctid}`).
    chapters: Arc<Mutex<Vec<Value>>>,
    /// While true, every chapter is reported as still transcoding.
    transcoding: Arc<Mutex<bool>>,
    /// S3 PUT answers 500.
    put_fail: Arc<Mutex<bool>>,
    /// ISRCs the music-api mock considers ready.
    ready: Arc<Mutex<HashSet<String>>>,
    /// Ordered event log for the upload-order assertion.
    events: Arc<Mutex<Vec<String>>>,
    /// POSTed chapter bodies.
    posted: Arc<Mutex<Vec<Value>>>,
    /// Multipart PUT bodies.
    puts: Arc<Mutex<Vec<String>>>,
    /// PATCHed chapter lists.
    patched: Arc<Mutex<Vec<Value>>>,
    file_seq: Arc<Mutex<usize>>,
}

fn mock_state(base: String) -> Mock {
    Mock {
        base,
        chapters: Arc::new(Mutex::new(Vec::new())),
        transcoding: Arc::new(Mutex::new(false)),
        put_fail: Arc::new(Mutex::new(false)),
        ready: Arc::new(Mutex::new(HashSet::new())),
        events: Arc::new(Mutex::new(Vec::new())),
        posted: Arc::new(Mutex::new(Vec::new())),
        puts: Arc::new(Mutex::new(Vec::new())),
        patched: Arc::new(Mutex::new(Vec::new())),
        file_seq: Arc::new(Mutex::new(0)),
    }
}

impl Mock {
    fn event(&self, e: impl Into<String>) {
        self.events.lock().unwrap().push(e.into());
    }
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

// ── mock handlers ───────────────────────────────────────────────────────────

async fn token_handler(
    Form(_form): Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    Json(json!({"access_token": "acc", "refresh_token": REFRESH, "expires_in": 300}))
}

async fn get_tonie(
    State(m): State<Mock>,
    Path((_hid, _ctid)): Path<(String, String)>,
) -> Json<Value> {
    let busy = *m.transcoding.lock().unwrap();
    let chapters: Vec<Value> = m
        .chapters
        .lock()
        .unwrap()
        .iter()
        .map(|c| {
            let mut c = c.clone();
            if busy {
                c["transcoding"] = json!(true);
            }
            c
        })
        .collect();
    Json(json!({ "id": "ct1", "chapters": chapters }))
}

async fn file_handler(State(m): State<Mock>) -> Json<Value> {
    let n = {
        let mut s = m.file_seq.lock().unwrap();
        *s += 1;
        *s
    };
    m.event("file");
    Json(json!({
        "fileId": format!("file-{n}"),
        "request": {
            "url": format!("{}/s3/upload", m.base),
            "fields": {"key": format!("k{n}"), "policy": "p"}
        }
    }))
}

async fn put_handler(State(m): State<Mock>, body: Bytes) -> StatusCode {
    if *m.put_fail.lock().unwrap() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    m.puts
        .lock()
        .unwrap()
        .push(String::from_utf8_lossy(&body).to_string());
    m.event("put");
    StatusCode::OK
}

async fn chapter_handler(State(m): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let file = body["file"].as_str().unwrap_or_default().to_string();
    m.event(format!("chapter:{file}"));
    m.posted.lock().unwrap().push(body);
    Json(json!({"ok": true}))
}

async fn patch_handler(State(m): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    m.event("patch");
    m.patched.lock().unwrap().push(body);
    Json(json!({"ok": true}))
}

async fn isrc_status(State(m): State<Mock>, Path(isrc): Path<String>) -> Json<Value> {
    let ready = m.ready.lock().unwrap().contains(&isrc);
    Json(json!({
        "isrc": isrc,
        "state": if ready { "ready" } else { "absent" }
    }))
}

async fn isrc_bytes(State(m): State<Mock>, Path((isrc, _fmt)): Path<(String, String)>) -> Response {
    m.event(format!("bytes:{isrc}"));
    (
        [(axum::http::header::CONTENT_TYPE, "audio/mpeg")],
        b"MP3BYTES".to_vec(),
    )
        .into_response()
}

/// Boot the combined Tonies + music-api mock; returns its base URL + state.
async fn mock_all() -> (String, Mock) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let st = mock_state(base.clone());
    let app = Router::new()
        .route(
            "/realms/tonies/protocol/openid-connect/token",
            post(token_handler),
        )
        .route("/v2/file", post(file_handler))
        .route("/s3/upload", post(put_handler))
        .route(
            "/v2/households/{hid}/creativetonies/{ctid}/chapters",
            post(chapter_handler),
        )
        .route(
            "/v2/households/{hid}/creativetonies/{ctid}",
            get(get_tonie).patch(patch_handler),
        )
        .route("/isrc/{isrc}", get(isrc_status))
        .route("/isrc/{isrc}/{fmt}", get(isrc_bytes))
        .with_state(st.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, st)
}

fn cfg_for(base: &str) -> Config {
    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.tonies_auth_base = base.to_string();
    cfg.tonies_api_base = base.to_string();
    cfg.tonies_secret_key = Some(KEY.to_string());
    cfg.music_api_base = base.to_string();
    cfg.music_api_token = Some("music-token".to_string());
    cfg
}

fn binding() -> Binding {
    Binding {
        id: 1,
        tonie_id: "ct1".into(),
        tonie_name: Some("Story".into()),
        source_kind: "playlist".into(),
        source_id: Some(7),
        format: "mp3-128".into(),
        cap_seconds: 5400,
        mode: "manual".into(),
        dirty_since: Some("2026-01-01T00:00:00+00:00".into()),
        last_sync_at: None,
        last_sync_status: None,
    }
}

fn track(isrc: Option<&str>, title: &str, seconds: i64) -> music_hub::tonies::DesiredTrack {
    music_hub::tonies::DesiredTrack {
        isrc: isrc.map(str::to_string),
        title: title.to_string(),
        duration_ms: seconds * 1000,
    }
}

/// A `fetch_bytes` closure backed by the (mocked) music-api.
fn fetcher(
    cfg: Config,
) -> impl Fn(&str) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send>>
{
    move |isrc: &str| {
        let cfg = cfg.clone();
        let isrc = isrc.to_string();
        Box::pin(async move { music_hub::music_api::file_bytes(&cfg, &isrc, "mp3-128").await })
    }
}

// ── tests: upload order + diff application ──────────────────────────────────

#[tokio::test]
async fn upload_order_and_replace_reuse_prefix() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    // A shared prefix (A) plus a divergent chapter (X) → kind == Replace.
    *m.chapters.lock().unwrap() = vec![
        json!({"id": "ch1", "title": "A", "seconds": 100}),
        json!({"id": "ch9", "title": "X", "seconds": 999}),
    ];

    let cfg = cfg_for(&base);
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());
    let desired = vec![
        track(Some("a"), "A", 100),
        track(Some("b"), "B", 200),
        track(Some("c"), "C", 300),
    ];
    let ready = run::readiness(&cfg, desired.iter().filter_map(|t| t.isrc.clone())).await;

    let outcome = run::run_sync(
        &mut client,
        "hh1",
        &binding(),
        desired,
        |isrc| ready.contains(isrc),
        fetcher(cfg.clone()),
        &SyncOptions::default(),
    )
    .await;

    assert_eq!(outcome.status, run::STATUS_OK);
    assert_eq!(outcome.chapters_before, 2);
    assert_eq!(outcome.chapters_after, 3);
    assert_eq!(outcome.added, 2);
    assert_eq!(outcome.removed, 1);

    // Each PUT carries the multipart file part with the audio bytes.
    let puts = m.puts.lock().unwrap();
    assert_eq!(puts.len(), 2);
    assert!(puts[0].contains("name=\"file\""));
    assert!(puts[0].contains("MP3BYTES"));
    drop(puts);

    // Deterministic order: file → bytes → put → chapter, per new track.
    assert_eq!(
        m.events(),
        vec![
            "file",
            "bytes:b",
            "put",
            "chapter:file-1",
            "file",
            "bytes:c",
            "put",
            "chapter:file-2",
            "patch",
        ]
    );

    // The fileId from POST /v2/file flows into the chapters body.
    let posted = m.posted.lock().unwrap();
    assert_eq!(posted.len(), 2);
    assert_eq!(posted[0]["file"], "file-1");
    assert_eq!(posted[0]["title"], "B");
    assert_eq!(posted[1]["file"], "file-2");
    assert_eq!(posted[1]["title"], "C");
    drop(posted);

    // The Replace PATCH reuses the shared prefix (no re-upload of "A").
    let patched = m.patched.lock().unwrap();
    assert_eq!(patched.len(), 1);
    let chapters = patched[0]["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 3);
    assert_eq!(chapters[0]["id"], "ch1", "reused chapter keeps its id");
    assert_eq!(chapters[1]["title"], "B");
    assert_eq!(chapters[2]["title"], "C");
}

#[tokio::test]
async fn append_posts_only_new_tracks() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a", "b"].iter().map(|s| s.to_string()).collect();
    *m.chapters.lock().unwrap() = vec![json!({"id": "ch1", "title": "A", "seconds": 100})];

    let cfg = cfg_for(&base);
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());
    let desired = vec![track(Some("a"), "A", 100), track(Some("b"), "B", 200)];
    let ready = run::readiness(&cfg, desired.iter().filter_map(|t| t.isrc.clone())).await;

    let outcome = run::run_sync(
        &mut client,
        "hh1",
        &binding(),
        desired,
        |isrc| ready.contains(isrc),
        fetcher(cfg.clone()),
        &SyncOptions::default(),
    )
    .await;

    assert_eq!(outcome.status, run::STATUS_OK);
    assert_eq!(outcome.added, 1);
    assert_eq!(outcome.removed, 0);
    assert_eq!(outcome.chapters_after, 2);
    // Append only POSTs the new chapter — no PATCH.
    assert!(
        m.patched.lock().unwrap().is_empty(),
        "append must not patch"
    );
    let posted = m.posted.lock().unwrap();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0]["title"], "B");
}

#[tokio::test]
async fn unchanged_tonie_is_a_noop_without_api_write() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a"].iter().map(|s| s.to_string()).collect();
    *m.chapters.lock().unwrap() = vec![json!({"id": "ch1", "title": "A", "seconds": 100})];

    let cfg = cfg_for(&base);
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());
    let desired = vec![track(Some("a"), "A", 100)];
    let ready = run::readiness(&cfg, desired.iter().filter_map(|t| t.isrc.clone())).await;

    let outcome = run::run_sync(
        &mut client,
        "hh1",
        &binding(),
        desired,
        |isrc| ready.contains(isrc),
        fetcher(cfg.clone()),
        &SyncOptions::default(),
    )
    .await;

    assert_eq!(outcome.status, run::STATUS_NOOP);
    assert_eq!(outcome.added, 0);
    assert!(
        m.events().is_empty(),
        "noop must not touch the API: {:?}",
        m.events()
    );
}

// ── tests: bounded transcoding poll ─────────────────────────────────────────

#[tokio::test]
async fn transcoding_poll_times_out_without_hanging() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a"].iter().map(|s| s.to_string()).collect();
    *m.transcoding.lock().unwrap() = true; // never settles
    // At least one chapter so the poll can observe the transcoding flag.
    *m.chapters.lock().unwrap() = vec![json!({"id": "ch0", "title": "Old", "seconds": 10})];

    let cfg = cfg_for(&base);
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());
    let desired = vec![track(Some("a"), "A", 100)];
    let ready = run::readiness(&cfg, desired.iter().filter_map(|t| t.isrc.clone())).await;

    let opts = SyncOptions {
        poll_attempts: 2,
        poll_interval: Duration::from_millis(1),
    };
    let outcome = run::run_sync(
        &mut client,
        "hh1",
        &binding(),
        desired,
        |isrc| ready.contains(isrc),
        fetcher(cfg.clone()),
        &opts,
    )
    .await;

    assert_eq!(outcome.status, run::STATUS_TIMEOUT);
    assert_eq!(outcome.added, 1, "upload still happened before the poll");
    assert!(run::summary(&outcome).contains("Transcoding läuft noch"));
}

// ── tests: failed upload leaves a log entry ─────────────────────────────────

#[tokio::test]
async fn failed_upload_is_logged_as_error() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a"].iter().map(|s| s.to_string()).collect();
    *m.put_fail.lock().unwrap() = true;

    let pool = music_hub::db::connect("sqlite::memory:").await.unwrap();
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at)
         VALUES ('sync-fail', 'Fail', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, source_kind, source_id, format, mode, created_at, updated_at)
         VALUES (?1, 'ct1', 'playlist', NULL, 'mp3-128', 'manual',
                 '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(uid)
    .fetch_one(&pool)
    .await
    .unwrap();

    // claim_run *before* any API write → the log entry exists no matter what.
    let run::Claim::Started(run_id) = run::claim_run(&pool, bid).await.unwrap() else {
        panic!("expected a started run");
    };

    let cfg = cfg_for(&base);
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());
    let desired = vec![track(Some("a"), "A", 100)];
    let ready = run::readiness(&cfg, desired.iter().filter_map(|t| t.isrc.clone())).await;
    let mut b = binding();
    b.id = bid;
    let outcome = run::run_sync(
        &mut client,
        "hh1",
        &b,
        desired,
        |isrc| ready.contains(isrc),
        fetcher(cfg.clone()),
        &SyncOptions::default(),
    )
    .await;

    assert_eq!(outcome.status, run::STATUS_ERROR);
    assert!(
        outcome.error.as_deref().unwrap_or("").contains("500"),
        "error should carry the status: {:?}",
        outcome.error
    );

    run::finish_run(&pool, run_id, &outcome).await.unwrap();
    let rows = run::list_syncs(&pool, bid, 5).await.unwrap();
    assert_eq!(rows.len(), 1, "a failed run must still be logged");
    assert_eq!(rows[0].status, run::STATUS_ERROR);
    assert!(rows[0].error.as_deref().unwrap_or("").contains("500"));
    // The failed upload never reached the chapters endpoint.
    assert!(m.posted.lock().unwrap().is_empty());
}

// ── tests: web endpoint (ownership, toast, no credential) ───────────────────

/// Boot the full web app with the Tonies + music-api bases pinned at `base`.
async fn spawn_web(base: &str) -> common::TestApp {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = cfg_for(base);
    cfg.database_url = format!("sqlite:{}/hub.db", dir.path().display());
    common::spawn_with(dir, cfg).await
}

async fn insert_playlist(pool: &sqlx::SqlitePool, user_id: i64, ext: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(user_id)
    .bind(ext)
    .bind(name)
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
         VALUES ('spotify', ?1, ?2, ?3, ?4, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(title)
    .bind(duration_ms)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn sync_endpoint_summarises_and_never_leaks_credentials() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["r1"].iter().map(|s| s.to_string()).collect();
    let app = spawn_web(&base).await;
    let (uid, cookie) = app.user_with_password("sync-alice", "pw").await;
    music_hub::tonies::save_account(
        &app.pool,
        uid,
        Some("hh1"),
        Some("a@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();
    let pl = insert_playlist(&app.pool, uid, "sp-1", "Mix").await;
    music_hub::tonies::upsert_binding(
        &app.pool,
        uid,
        "ct1",
        Some("Story"),
        "playlist",
        Some(pl),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();

    let t1 = insert_track(&app.pool, "s1", Some("r1"), "Ready One", 3_600_000).await;
    let t2 = insert_track(&app.pool, "s2", Some("missing"), "Gone", 60_000).await;
    let t3 = insert_track(&app.pool, "s3", None, "NoIsrc", 60_000).await;
    for (pos, t) in [t1, t2, t3].iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(pl)
        .bind(t)
        .bind(pos as i64)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    let resp = app
        .client()
        .post(app.url("/tonies/ct1/sync"))
        .header("cookie", &cookie)
        .form(&[("tonie_name", "Story")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let trig = resp
        .headers()
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(trig.contains("1 Kapitel"), "chapters missing: {trig}");
    assert!(trig.contains("60 min"), "minutes missing: {trig}");
    assert!(
        trig.contains("1 nicht im Vault"),
        "vault skip missing: {trig}"
    );
    assert!(trig.contains("1 ohne ISRC"), "isrc skip missing: {trig}");
    assert!(!trig.contains(REFRESH) && !trig.contains(PASSWORD));

    let body = resp.text().await.unwrap();
    assert!(!body.contains(REFRESH), "refresh token leaked into HTML");
    assert!(!body.contains(PASSWORD), "password leaked into HTML");
    assert!(
        body.contains("Sync-Verlauf"),
        "run log expected in the fragment"
    );
    assert!(
        body.contains("Jetzt synchronisieren"),
        "sync button expected"
    );

    // The run is persisted for the owner's binding.
    let b = music_hub::tonies::get_binding(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();
    let rows = run::list_syncs(&app.pool, b.id, 5).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, run::STATUS_OK);
    assert_eq!(rows[0].added, Some(1));

    // A second immediate run is rate-limited (toast, no new row).
    let resp2 = app
        .client()
        .post(app.url("/tonies/ct1/sync"))
        .header("cookie", &cookie)
        .form(&[("tonie_name", "Story")])
        .send()
        .await
        .unwrap();
    let trig2 = resp2
        .headers()
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        trig2.contains("Bitte kurz warten"),
        "rate-limit toast missing: {trig2}"
    );
    assert_eq!(run::list_syncs(&app.pool, b.id, 5).await.unwrap().len(), 1);
}

#[tokio::test]
async fn sync_endpoint_is_owner_scoped() {
    let (base, _m) = mock_all().await;
    let app = spawn_web(&base).await;
    let (a, _ca) = app.user_with_password("sync-own-a", "pw").await;
    let (b, cb) = app.user_with_password("sync-own-b", "pw").await;
    music_hub::tonies::save_account(
        &app.pool,
        b,
        Some("hh1"),
        Some("b@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();
    let pl = insert_playlist(&app.pool, a, "a-1", "A Only").await;
    music_hub::tonies::upsert_binding(
        &app.pool,
        a,
        "ct1",
        Some("Story"),
        "playlist",
        Some(pl),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();

    // B has no binding for ct1 → 404 (and never sees A's).
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/sync"))
        .header("cookie", &cb)
        .form(&[("tonie_name", "Story")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(run::list_syncs(&app.pool, 1, 5).await.unwrap().is_empty());

    // A missing binding is a 404 too.
    let resp = app
        .client()
        .post(app.url("/tonies/nope/sync"))
        .header("cookie", &cb)
        .form(&[("tonie_name", "Story")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Acceptance: a transcoding poll timeout must still leave a `status='timeout'`
/// **log entry** (not just an in-memory outcome), and it wrote nothing that
/// reconciles the Tonie, so the `dirty_since` signal stays.
#[tokio::test]
async fn timeout_run_is_logged_as_timeout() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a"].iter().map(|s| s.to_string()).collect();
    *m.transcoding.lock().unwrap() = true; // never settles
    // At least one chapter so the poll can observe the transcoding flag.
    *m.chapters.lock().unwrap() = vec![json!({"id": "ch0", "title": "Old", "seconds": 10})];

    let pool = music_hub::db::connect("sqlite::memory:").await.unwrap();
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at)
         VALUES ('sync-timeout', 'Timeout', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, source_kind, source_id, format, mode, dirty_since,
              created_at, updated_at)
         VALUES (?1, 'ct1', 'playlist', NULL, 'mp3-128', 'manual',
                 '2026-01-01T00:00:00+00:00',
                 '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(uid)
    .fetch_one(&pool)
    .await
    .unwrap();

    let run::Claim::Started(run_id) = run::claim_run(&pool, bid).await.unwrap() else {
        panic!("expected a started run");
    };

    let cfg = cfg_for(&base);
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());
    let desired = vec![track(Some("a"), "A", 100)];
    let ready = run::readiness(&cfg, desired.iter().filter_map(|t| t.isrc.clone())).await;
    let mut b = binding();
    b.id = bid;
    let opts = SyncOptions {
        poll_attempts: 2,
        poll_interval: Duration::from_millis(1),
    };
    let outcome = run::run_sync(
        &mut client,
        "hh1",
        &b,
        desired,
        |isrc| ready.contains(isrc),
        fetcher(cfg.clone()),
        &opts,
    )
    .await;

    assert_eq!(outcome.status, run::STATUS_TIMEOUT);
    run::finish_run(&pool, run_id, &outcome).await.unwrap();

    let rows = run::list_syncs(&pool, bid, 5).await.unwrap();
    assert_eq!(rows.len(), 1, "a timed-out run must still be logged");
    assert_eq!(rows[0].status, run::STATUS_TIMEOUT);
    assert_eq!(rows[0].added, Some(1), "upload happened before the poll");

    let dirty: Option<String> =
        sqlx::query_scalar("SELECT dirty_since FROM hub_tonie_bindings WHERE id = ?1")
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        dirty.is_some(),
        "a timeout wrote nothing reconciled — dirty_since must stay"
    );
}

/// Acceptance: a second run while one is live is refused with `Busy` —
/// verified end-to-end through the endpoint's `HX-Trigger` toast, and no second
/// run row is created.
#[tokio::test]
async fn busy_sync_returns_busy_toast() {
    let (base, _m) = mock_all().await;
    let app = spawn_web(&base).await;
    let (uid, cookie) = app.user_with_password("sync-busy", "pw").await;
    music_hub::tonies::save_account(
        &app.pool,
        uid,
        Some("hh1"),
        Some("busy@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();
    let pl = insert_playlist(&app.pool, uid, "busy-1", "Busy").await;
    music_hub::tonies::upsert_binding(
        &app.pool,
        uid,
        "ct1",
        Some("Story"),
        "playlist",
        Some(pl),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    let b = music_hub::tonies::get_binding(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();

    // Simulate a live run started just now (younger than STALE_SECS).
    sqlx::query(
        "INSERT INTO hub_tonie_syncs (binding_id, started_at, status)
         VALUES (?1, ?2, 'running')",
    )
    .bind(b.id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.pool)
    .await
    .unwrap();

    let resp = app
        .client()
        .post(app.url("/tonies/ct1/sync"))
        .header("cookie", &cookie)
        .form(&[("tonie_name", "Story")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let trig = resp
        .headers()
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    // The HX-Trigger header JSON-escapes umlauts, so assert on a distinct
    // ASCII substring of the Busy message (vs. the rate-limit message).
    assert!(
        trig.contains("bereits") && trig.contains("Sync"),
        "busy toast missing: {trig}"
    );
    // No second row: still exactly the pre-existing live run.
    assert_eq!(run::list_syncs(&app.pool, b.id, 5).await.unwrap().len(), 1);
}

#[tokio::test]
async fn sync_endpoint_requires_session() {
    let (base, _m) = mock_all().await;
    let app = spawn_web(&base).await;
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/sync"))
        .form(&[("tonie_name", "Story")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── tests: source kinds (#504) ──────────────────────────────────────────────

/// Acceptance #2: a binding whose source no longer exists (e.g. the playlist was
/// deleted) is **not** cascaded away — the run reports a clear error instead.
#[tokio::test]
async fn execute_binding_reports_missing_source_and_keeps_binding() {
    let (base, _m) = mock_all().await;
    let app = spawn_web(&base).await;
    let (uid, _cookie) = app.user_with_password("src-gone", "pw").await;
    music_hub::tonies::save_account(
        &app.pool,
        uid,
        Some("hh1"),
        Some("g@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();
    // A binding pointing at a playlist id that does not exist.
    music_hub::tonies::upsert_binding(
        &app.pool,
        uid,
        "ct1",
        Some("Story"),
        "playlist",
        Some(999_999),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    let b = music_hub::tonies::get_binding(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();

    let cfg = cfg_for(&base);
    let outcome = music_hub::tonies::execute_binding(&app.pool, &cfg, uid, &b).await;
    assert_eq!(outcome.status, run::STATUS_ERROR);
    let err = outcome.error.unwrap_or_default();
    assert!(
        err.contains("Quelle"),
        "error should name the source problem: {err}"
    );

    // The binding survives the deleted source (acceptance #2).
    assert!(
        music_hub::tonies::get_binding(&app.pool, uid, "ct1")
            .await
            .unwrap()
            .is_some(),
        "binding must remain after its source was deleted"
    );
}

/// #504 ownership guard at run level: a binding of user A that (e.g. after an
/// id reuse) points at a source owned by user B must fail loudly and never read
/// the foreign source; the binding itself survives.
#[tokio::test]
async fn execute_binding_rejects_foreign_source_and_keeps_binding() {
    let (base, _m) = mock_all().await;
    let app = spawn_web(&base).await;
    let (uid_a, _cookie) = app.user_with_password("src-owner-a", "pw").await;
    let (uid_b, _cookie_b) = app.user_with_password("src-owner-b", "pw").await;
    music_hub::tonies::save_account(
        &app.pool,
        uid_a,
        Some("hh1"),
        Some("g@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();

    // A playlist owned by B, then A's binding points at that id.
    let foreign_playlist: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, fetched_at)
         VALUES (?1, 'spotify', 'pl-b', 'Bob Mix', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(uid_b)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_ne!(uid_a, uid_b, "two distinct users are required");

    music_hub::tonies::upsert_binding(
        &app.pool,
        uid_a,
        "ct1",
        Some("Story"),
        "playlist",
        Some(foreign_playlist),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    let b = music_hub::tonies::get_binding(&app.pool, uid_a, "ct1")
        .await
        .unwrap()
        .unwrap();

    let cfg = cfg_for(&base);
    let outcome = music_hub::tonies::execute_binding(&app.pool, &cfg, uid_a, &b).await;
    assert_eq!(outcome.status, run::STATUS_ERROR);
    let err = outcome.error.unwrap_or_default();
    assert!(
        err.contains("Quelle"),
        "error should name the source problem: {err}"
    );
    assert!(
        music_hub::tonies::get_binding(&app.pool, uid_a, "ct1")
            .await
            .unwrap()
            .is_some(),
        "binding must remain after a foreign-source rejection"
    );
}

/// A backpack source is resolved and synced like a playlist: the run dispatches
/// on `source_kind`, not on a hard-coded playlist lookup.
#[tokio::test]
async fn execute_binding_with_backpack_source_syncs() {
    let (base, m) = mock_all().await;
    *m.ready.lock().unwrap() = ["a"].iter().map(|s| s.to_string()).collect();
    let app = spawn_web(&base).await;
    let (uid, _cookie) = app.user_with_password("src-bp", "pw").await;
    music_hub::tonies::save_account(
        &app.pool,
        uid,
        Some("hh1"),
        Some("bp@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();

    let pl = insert_playlist(&app.pool, uid, "bp-pl", "Bundle Mix").await;
    let t = insert_track(&app.pool, "bp-s1", Some("a"), "A", 100_000).await;
    sqlx::query(
        "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, 0)",
    )
    .bind(pl)
    .bind(t)
    .execute(&app.pool)
    .await
    .unwrap();
    let bp: i64 = sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, 'Bundle', 0, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(uid)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_playlist_id, position, added_at)
         VALUES (?1, 'playlist', ?2, 0, '2026-01-01T00:00:00+00:00')",
    )
    .bind(bp)
    .bind(pl)
    .execute(&app.pool)
    .await
    .unwrap();

    music_hub::tonies::upsert_binding(
        &app.pool,
        uid,
        "ct1",
        Some("Story"),
        "backpack",
        Some(bp),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    let b = music_hub::tonies::get_binding(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.source_kind, "backpack");

    let cfg = cfg_for(&base);
    let outcome = music_hub::tonies::execute_binding(&app.pool, &cfg, uid, &b).await;
    assert_eq!(outcome.status, run::STATUS_OK, "error: {:?}", outcome.error);
    assert_eq!(outcome.added, 1);
}
