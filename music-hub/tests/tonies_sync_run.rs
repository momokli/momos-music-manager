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
//!   chapters, minutes and skipped tracks — with no credential in the HTML;
//! * album sources end-to-end: the cart order becomes the chapter order (and
//!   reordering the cart reorders the chapters), a successful run records the
//!   fitted albums in the listened ledger so the next run resolves past them,
//!   and no album run issues a Spotify request.

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
    /// Deezer album detail bodies (`/deezer/album/{id}` + the artist list).
    albums: Arc<Mutex<Vec<Value>>>,
    /// Bodies POSTed to `/orders` (album runs must not order what is ready).
    orders: Arc<Mutex<Vec<Value>>>,
    /// When true a `POST …/chapters` also appends to `chapters`, so a second
    /// run sees the state the first one produced (like the real API).
    apply_posts: Arc<Mutex<bool>>,
    /// Seconds already present on the Kreativ-Tonie (#624). Reported by
    /// `get_tonie` as `secondsPresent` (and `secondsRemaining` = total − present).
    seconds_present: Arc<Mutex<i64>>,
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
        albums: Arc::new(Mutex::new(Vec::new())),
        orders: Arc::new(Mutex::new(Vec::new())),
        apply_posts: Arc::new(Mutex::new(false)),
        seconds_present: Arc::new(Mutex::new(0)),
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
    let present = *m.seconds_present.lock().unwrap();
    let remaining = (5400 - present).max(0);
    Json(json!({
        "id": "ct1",
        "chapters": chapters,
        "secondsPresent": present,
        "secondsRemaining": remaining,
    }))
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
    let title = body["title"].as_str().unwrap_or_default().to_string();
    m.event(format!("chapter:{file}"));
    m.posted.lock().unwrap().push(body);
    if *m.apply_posts.lock().unwrap() {
        let mut chapters = m.chapters.lock().unwrap();
        let n = chapters.len() + 1;
        chapters.push(json!({"id": format!("ch{n}"), "title": title, "seconds": 100}));
    }
    Json(json!({"ok": true}))
}

/// `POST /orders` — recorded so a test can prove nothing was ordered.
async fn orders_handler(State(m): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    m.orders.lock().unwrap().push(body);
    Json(json!({"orderId": "order-1"}))
}

/// `GET /deezer/artist/{id}/albums` — the albums of the mock, newest first.
async fn deezer_artist_albums(State(m): State<Mock>) -> Json<Value> {
    let albums: Vec<Value> = m
        .albums
        .lock()
        .unwrap()
        .iter()
        .map(|a| {
            json!({
                "albumId": a["albumId"],
                "title": a["title"],
                "releaseDate": a["releaseDate"],
                "recordType": a["recordType"],
                "coverUrl": a["coverUrl"],
            })
        })
        .collect();
    let total = albums.len();
    Json(json!({ "albums": albums, "total": total, "next": null }))
}

/// `GET /deezer/album/{id}` — the album detail carrying the tracklist ISRCs.
async fn deezer_album(State(m): State<Mock>, Path(id): Path<String>) -> Response {
    let found = m
        .albums
        .lock()
        .unwrap()
        .iter()
        .find(|a| a["albumId"].as_str() == Some(id.as_str()))
        .cloned();
    match found {
        Some(v) => Json(v).into_response(),
        None => (StatusCode::NOT_FOUND, "unknown album").into_response(),
    }
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
        .route("/orders", post(orders_handler))
        .route("/deezer/artist/{id}/albums", get(deezer_artist_albums))
        .route("/deezer/album/{id}", get(deezer_album))
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
        source_name: None,
        format: "mp3-128".into(),
        cap_seconds: 5400,
        mode: "manual".into(),
        order_by: "playlist".into(),
        album_mode: "newest".into(),
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
        None,
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
        body.contains("<summary>Verlauf</summary>"),
        "run log expected in the fragment"
    );
    assert!(
        body.contains("class=\"tonie-sync\""),
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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

/// #624: a playlist-bound Tonie orders its *missing* tracks against the Tonie's
/// remaining time, in Tonie order — and never touches Spotify. Album paths keep
/// their own ordering; this is the playlist-only contextual order.
#[tokio::test]
async fn playlist_binding_orders_missing_tracks_within_remaining_time() {
    let (base, m) = mock_all().await;
    let (sp_base, sp) = mock_spotify_never_called().await;
    let cfg = cfg_album_run(&base, &sp_base);
    let (_dir, pool, uid) = album_run_db().await;

    // Only "keep" is already in the vault; everything else must be ordered.
    *m.ready.lock().unwrap() = ["keep"].iter().map(|s| s.to_string()).collect();
    // 100 s still free on the Tonie (cap 5400 − present 5300).
    *m.seconds_present.lock().unwrap() = 5300;

    let pl = insert_playlist(&pool, uid, "pl-order", "Order Mix").await;
    // Tonie order: keep(ready, 60s) → new1(30s) → new2(80s) → new3(30s).
    let specs: [(&str, &str, &str, i64); 4] = [
        ("s0", "keep", "Keep", 60_000),
        ("s1", "new1", "New1", 30_000),
        ("s2", "new2", "New2", 80_000),
        ("s3", "new3", "New3", 30_000),
    ];
    for (i, (sid, isrc, title, ms)) in specs.iter().enumerate() {
        let t = insert_track(&pool, sid, Some(isrc), title, *ms).await;
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(pl)
        .bind(t)
        .bind(i as i64)
        .execute(&pool)
        .await
        .unwrap();
    }

    music_hub::tonies::upsert_binding(
        &pool,
        uid,
        "ct1",
        Some("Story"),
        "playlist",
        Some(pl),
        None,
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    let b = music_hub::tonies::get_binding(&pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.source_kind, "playlist");

    let _outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;

    let orders = m.orders.lock().unwrap().clone();
    assert_eq!(orders.len(), 1, "exactly one order expected: {orders:?}");
    let body = &orders[0];
    assert_eq!(body["priority"], json!(5));
    assert_eq!(body["intent"], json!("toys"));
    assert_eq!(body["target"], json!("mp3-128"));
    let isrcs: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["isrc"].as_str().unwrap_or_default().to_string())
        .collect();
    // remaining 100: keep(60, ready) spends 60 → 40 left; new1(30) fits and is
    // missing → ordered; new2(80) would overflow → break (new3 never reached).
    assert_eq!(
        isrcs,
        vec!["new1"],
        "only the missing, fitting ISRC in Tonie order"
    );

    assert!(
        sp.hits.lock().unwrap().is_empty(),
        "the playlist path must never touch Spotify: {:?}",
        sp.hits.lock().unwrap()
    );
}

/// #624 negative case: when every desired track is already in the vault there
/// is nothing to order — no `/orders` call at all.
#[tokio::test]
async fn playlist_binding_orders_nothing_when_all_ready() {
    let (base, m) = mock_all().await;
    let (sp_base, sp) = mock_spotify_never_called().await;
    let cfg = cfg_album_run(&base, &sp_base);
    let (_dir, pool, uid) = album_run_db().await;

    *m.ready.lock().unwrap() = ["a", "b"].iter().map(|s| s.to_string()).collect();
    *m.seconds_present.lock().unwrap() = 5300;

    let pl = insert_playlist(&pool, uid, "pl-ready", "Ready Mix").await;
    for (i, (sid, isrc, title)) in [("r0", "a", "A"), ("r1", "b", "B")]
        .iter()
        .enumerate()
    {
        let t = insert_track(&pool, sid, Some(isrc), title, 30_000).await;
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(pl)
        .bind(t)
        .bind(i as i64)
        .execute(&pool)
        .await
        .unwrap();
    }

    music_hub::tonies::upsert_binding(
        &pool,
        uid,
        "ct1",
        Some("Story"),
        "playlist",
        Some(pl),
        None,
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    let b = music_hub::tonies::get_binding(&pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();

    let _outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;

    assert!(
        m.orders.lock().unwrap().is_empty(),
        "all-ready playlist must order nothing: {:?}",
        m.orders.lock().unwrap()
    );
    assert!(sp.hits.lock().unwrap().is_empty());
}

// ── tests: album sources end-to-end (cart → resolver → run → chapters) ──────
//
// `tonies_albums.rs` stops at the resolver and `tonies_albums_web.rs` at the
// cart rows. These tests cover the actual write path: the album branch of
// `execute_binding` (resolver → fit → diff → upload → ledger), i.e. what a
// child ends up hearing on the tonie.

/// One album detail body for the mock music-api (2–4 short chapters).
fn album(id: &str, title: &str, released: &str, isrcs: &[&str]) -> Value {
    let tracks: Vec<Value> = isrcs
        .iter()
        .enumerate()
        .map(|(i, isrc)| {
            json!({
                "isrc": isrc,
                "title": format!("{title} Kapitel {:02}", i + 1),
                "durationMs": 100_000,
                "position": i + 1,
            })
        })
        .collect();
    json!({
        "albumId": id,
        "title": title,
        "releaseDate": released,
        "recordType": "album",
        "coverUrl": format!("https://cdn.example/{id}.jpg"),
        "seconds": isrcs.len() as i64 * 100,
        "tracks": tracks,
    })
}

/// Titles of the chapter bodies the Tonies mock received, in POST order.
fn posted_titles(m: &Mock) -> Vec<String> {
    m.posted
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Titles of the chapter list the Tonies mock was PATCHed with (the state the
/// tonie plays afterwards).
fn patched_titles(m: &Mock) -> Vec<String> {
    m.patched
        .lock()
        .unwrap()
        .last()
        .map(|b| {
            b["chapters"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| c["title"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// A binding on an album source.
fn album_binding(album_mode: &str, artist: Option<i64>, cap_seconds: i64) -> Binding {
    Binding {
        id: 1,
        tonie_id: "ct1".into(),
        tonie_name: Some("Story".into()),
        source_kind: "albums".into(),
        source_id: artist,
        source_name: None,
        format: "mp3-128".into(),
        cap_seconds,
        mode: "manual".into(),
        order_by: "playlist".into(),
        album_mode: album_mode.into(),
        dirty_since: None,
        last_sync_at: None,
        last_sync_status: None,
    }
}

fn cart_item(album_id: &str, title: &str) -> music_hub::tonies::AlbumItem {
    music_hub::tonies::AlbumItem {
        album_id: album_id.to_string(),
        album_title: title.to_string(),
        seconds: 200,
        position: 0,
    }
}

/// A counting Spotify mock that answers 404 to everything: the album path must
/// never be seen here (and a hit is a hard test failure, not a network call).
#[derive(Clone, Default)]
struct SpotifyHits {
    hits: Arc<Mutex<Vec<String>>>,
}

async fn spotify_anything(State(s): State<SpotifyHits>, uri: axum::http::Uri) -> StatusCode {
    s.hits.lock().unwrap().push(uri.path().to_string());
    StatusCode::NOT_FOUND
}

async fn mock_spotify_never_called() -> (String, SpotifyHits) {
    let st = SpotifyHits::default();
    let app = Router::new()
        .fallback(spotify_anything)
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), st)
}

/// Config with the Tonies/music-api bases on `base` and Spotify pinned to the
/// counting mock, so any accidental Spotify call is recorded instead of dialled.
fn cfg_album_run(base: &str, spotify_base: &str) -> Config {
    let mut cfg = cfg_for(base);
    cfg.spotify_api_base = spotify_base.to_string();
    cfg
}

/// A migrated temp DB with one connected user (token stored under [`KEY`]).
async fn album_run_db() -> (tempfile::TempDir, sqlx::SqlitePool, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = music_hub::db::connect(&url).await.expect("connect");
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at)
         VALUES ('album-run', 'Album Run', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    music_hub::tonies::save_account(
        &pool,
        uid,
        Some("hh1"),
        Some("album@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();
    (dir, pool, uid)
}

/// Acceptance 1: the cart order *is* the chapter order — and reordering the
/// cart reorders the chapters on the next run.
#[tokio::test]
async fn cart_order_becomes_chapter_order() {
    let (base, m) = mock_all().await;
    // Let the mock adopt uploaded chapters, so the second run diffs against a
    // tonie that already carries A1, A2, B1, B2.
    *m.apply_posts.lock().unwrap() = true;
    *m.albums.lock().unwrap() = vec![
        album("A", "Folge A", "2026-01-01", &["A1", "A2"]),
        album("B", "Folge B", "2026-01-02", &["B1", "B2"]),
    ];
    *m.ready.lock().unwrap() = ["A1", "A2", "B1", "B2"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let (sp_base, sp) = mock_spotify_never_called().await;
    let cfg = cfg_album_run(&base, &sp_base);
    let (_dir, pool, uid) = album_run_db().await;

    music_hub::tonies::cart::replace_all(
        &pool,
        uid,
        "ct1",
        &[cart_item("A", "Folge A"), cart_item("B", "Folge B")],
    )
    .await
    .unwrap();

    let b = album_binding("basket", None, 5400);
    let outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;
    assert_eq!(outcome.status, run::STATUS_OK, "error: {:?}", outcome.error);
    assert_eq!(outcome.added, 4);
    assert_eq!(
        posted_titles(&m),
        vec![
            "Folge A Kapitel 01",
            "Folge A Kapitel 02",
            "Folge B Kapitel 01",
            "Folge B Kapitel 02",
        ],
        "cart order (A before B) must become chapter order A1, A2, B1, B2"
    );
    assert!(
        m.orders.lock().unwrap().is_empty(),
        "ready albums must not be ordered: {:?}",
        m.orders.lock().unwrap()
    );

    // Reorder the cart: B first. The next run must turn the chapters around.
    music_hub::tonies::cart::replace_all(
        &pool,
        uid,
        "ct1",
        &[cart_item("B", "Folge B"), cart_item("A", "Folge A")],
    )
    .await
    .unwrap();
    m.posted.lock().unwrap().clear();
    m.patched.lock().unwrap().clear();

    let outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;
    assert_eq!(outcome.status, run::STATUS_OK, "error: {:?}", outcome.error);
    assert_eq!(
        posted_titles(&m),
        vec![
            "Folge B Kapitel 01",
            "Folge B Kapitel 02",
            "Folge A Kapitel 01",
            "Folge A Kapitel 02",
        ],
        "after moving B to the top the chapters must be B1, B2, A1, A2"
    );
    // The PATCH list is what the tonie plays — it must be in the new order too.
    assert_eq!(
        patched_titles(&m),
        vec![
            "Folge B Kapitel 01",
            "Folge B Kapitel 02",
            "Folge A Kapitel 01",
            "Folge A Kapitel 02",
        ],
        "the reordered tonie must hold B1, B2, A1, A2"
    );
    assert!(
        sp.hits.lock().unwrap().is_empty(),
        "the album path must never touch Spotify: {:?}",
        sp.hits.lock().unwrap()
    );
}

/// Acceptance 2: a successful album run records what it put on the tonie in
/// `hub_tonie_albums_seen` — and only that — so the next run resolves past it.
#[tokio::test]
async fn listened_albums_are_marked_and_resolved_past() {
    let (base, m) = mock_all().await;
    *m.albums.lock().unwrap() = vec![
        album("459", "Folge 459", "2026-11-13", &["A1", "A2"]),
        album("458", "Folge 458", "2026-11-06", &["B1", "B2"]),
        album("457", "Folge 457", "2026-10-30", &["C1", "C2"]),
    ];
    *m.ready.lock().unwrap() = ["A1", "A2", "B1", "B2", "C1", "C2"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let (sp_base, _sp) = mock_spotify_never_called().await;
    let cfg = cfg_album_run(&base, &sp_base);
    let (_dir, pool, uid) = album_run_db().await;

    // 500 s fit exactly two of the three 200 s albums.
    let b = album_binding("newest", Some(15167501), 500);
    let outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;
    assert_eq!(outcome.status, run::STATUS_OK, "error: {:?}", outcome.error);
    assert_eq!(
        posted_titles(&m),
        vec![
            "Folge 459 Kapitel 01",
            "Folge 459 Kapitel 02",
            "Folge 458 Kapitel 01",
            "Folge 458 Kapitel 02",
        ]
    );

    // Exactly the albums that fit are recorded as listened (the ledger is per
    // user, not per tonie — migration 062).
    let mut seen: Vec<String> = music_hub::tonies::seen_ids(&pool, uid)
        .await
        .unwrap()
        .into_iter()
        .collect();
    seen.sort();
    assert_eq!(seen, vec!["458", "459"], "only the fitted albums are seen");

    // The next run resolves past them: only the unheard album is sent.
    m.posted.lock().unwrap().clear();
    let outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;
    assert_eq!(outcome.status, run::STATUS_OK, "error: {:?}", outcome.error);
    assert_eq!(
        posted_titles(&m),
        vec!["Folge 457 Kapitel 01", "Folge 457 Kapitel 02"],
        "already-heard albums must not be re-sent"
    );
    let mut seen: Vec<String> = music_hub::tonies::seen_ids(&pool, uid)
        .await
        .unwrap()
        .into_iter()
        .collect();
    seen.sort();
    assert_eq!(seen, vec!["457", "458", "459"]);
}

/// Acceptance 3: the whole album run — driven through the auto-sync driver like
/// the worker does it — issues zero Spotify requests.
#[tokio::test]
async fn album_run_makes_no_spotify_requests() {
    let (sp_base, sp) = mock_spotify_never_called().await;
    let (base, m) = mock_all().await;
    *m.albums.lock().unwrap() = vec![
        album("A", "Folge A", "2026-01-01", &["A1", "A2"]),
        album("B", "Folge B", "2026-01-02", &["B1", "B2"]),
    ];
    *m.ready.lock().unwrap() = ["A1", "A2", "B1", "B2"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let (_dir, pool, uid) = album_run_db().await;
    music_hub::tonies::cart::replace_all(
        &pool,
        uid,
        "ct1",
        &[cart_item("A", "Folge A"), cart_item("B", "Folge B")],
    )
    .await
    .unwrap();

    let due_since = (chrono::Utc::now() - chrono::Duration::seconds(1000)).to_rfc3339();
    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, tonie_name, source_kind, source_id, format, cap_seconds,
              mode, order_by, album_mode, dirty_since, created_at, updated_at)
         VALUES (?1, 'ct1', 'Story', 'albums', NULL, 'mp3-128', 5400,
                 'auto', 'playlist', 'basket', ?2, ?3, ?3)
         RETURNING id",
    )
    .bind(uid)
    .bind(&due_since)
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&pool)
    .await
    .unwrap();

    let cfg = cfg_album_run(&base, &sp_base);
    let ran = music_hub::tonies::run_due(&pool, &cfg, 10).await.unwrap();
    assert_eq!(ran, 1, "exactly one due binding ran");

    let rows = run::list_syncs(&pool, bid, 5).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, run::STATUS_OK);
    assert_eq!(rows[0].added, Some(4), "the run wrote four chapters");

    // The run genuinely filled the tonie from the cart…
    assert_eq!(
        posted_titles(&m),
        vec![
            "Folge A Kapitel 01",
            "Folge A Kapitel 02",
            "Folge B Kapitel 01",
            "Folge B Kapitel 02",
        ]
    );
    // …and never asked Spotify for anything.
    assert!(
        sp.hits.lock().unwrap().is_empty(),
        "an album run must never touch Spotify: {:?}",
        sp.hits.lock().unwrap()
    );
}

/// #619: an album that does not fit is skipped as a unit — and that skip must
/// reach the *persisted* run log, not only the in-memory outcome. This drives
/// the real `execute_binding` album branch, so it exercises the
/// `fit.skipped` → run-log wiring the fix added (the unit tests call
/// `merge_album_skips` directly and bypass that wiring).
#[tokio::test]
async fn album_skip_lands_in_the_persisted_run_log() {
    let (base, m) = mock_all().await;
    // Three 200 s albums; a 500 s cap fits two, so the third is CapReached.
    *m.albums.lock().unwrap() = vec![
        album("459", "Folge 459", "2026-11-13", &["A1", "A2"]),
        album("458", "Folge 458", "2026-11-06", &["B1", "B2"]),
        album("457", "Folge 457", "2026-10-30", &["C1", "C2"]),
    ];
    *m.ready.lock().unwrap() = ["A1", "A2", "B1", "B2", "C1", "C2"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let (sp_base, _sp) = mock_spotify_never_called().await;
    let cfg = cfg_album_run(&base, &sp_base);
    let (_dir, pool, uid) = album_run_db().await;

    // Persist the binding so the run-log row hangs on a real binding_id.
    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, tonie_name, source_kind, source_id, format, cap_seconds,
              mode, order_by, album_mode, dirty_since, created_at, updated_at)
         VALUES (?1, 'ct1', 'Story', 'albums', 15167501, 'mp3-128', 500,
                 'manual', 'playlist', 'newest', NULL,
                 '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(uid)
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut b = album_binding("newest", Some(15167501), 500);
    b.id = bid;
    // Mirror the auto-sync driver: claim → execute → finish is what actually
    // persists the run-log row.
    let run::Claim::Started(run_id) = run::claim_run(&pool, bid).await.unwrap() else {
        panic!("expected Started");
    };
    let outcome = music_hub::tonies::execute_binding(&pool, &cfg, uid, &b).await;
    run::finish_run(&pool, run_id, &outcome).await.unwrap();
    assert_eq!(outcome.status, run::STATUS_OK, "error: {:?}", outcome.error);

    // The in-memory outcome carries the album skip with its reason…
    assert_eq!(
        outcome.skipped.len(),
        1,
        "one album skip expected: {:?}",
        outcome.skipped
    );
    assert_eq!(outcome.skipped[0].title, "Folge 457");
    assert_eq!(
        outcome.skipped[0].reason,
        music_hub::tonies::SkipReason::CapReached
    );
    assert_eq!(outcome.skipped[0].isrc, None);

    // …and it is persisted in the run log, so the history page can show it.
    let rows = run::list_syncs(&pool, bid, 5).await.unwrap();
    assert_eq!(rows.len(), 1);
    let skipped = &rows[0].skipped;
    assert_eq!(
        skipped.len(),
        1,
        "the album skip must be persisted: {:?}",
        skipped
    );
    assert_eq!(skipped[0].title, "Folge 457");
    assert_eq!(skipped[0].reason, "cap_reached");
    assert_eq!(skipped[0].isrc, None);
}
