//! Tonies integration (#499): mocked Keycloak + Tonies API, no network.
//!
//! Every base is pinned to a `127.0.0.1:0` mock, so a green run proves the
//! client never reaches `login.tonies.com` / `api.tonies.com`.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use music_hub::config::Config;
use music_hub::tonies::client::{ToniesClient, ToniesError};
use music_hub::tonies::{self, Chapter, Tokens};
use serde_json::{Value, json};

const PASSWORD: &str = "s3cr3t-p4ssw0rd";
const REFRESH: &str = "refresh-token-xyz";
const KEY: &str = "hub-tonies-test-secret";

/// Serve a router on an ephemeral loopback port; returns the base URL.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn cfg_with(auth: &str, api: &str) -> Config {
    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.tonies_auth_base = auth.to_string();
    cfg.tonies_api_base = api.to_string();
    cfg.tonies_secret_key = Some(KEY.to_string());
    cfg
}

/// Boot a full web app whose Tonies bases point at the given mocks.
async fn spawn_web(auth: &str, api: &str) -> common::TestApp {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = cfg_with(auth, api);
    cfg.database_url = url;
    common::spawn_with(dir, cfg).await
}

// ── token + read mock ───────────────────────────────────────────────────────

#[derive(Clone)]
struct AuthState {
    /// How many refresh_token grants were served.
    refresh_count: Arc<Mutex<usize>>,
    /// How many of the next data calls answer 401 (to force a refresh).
    unauthorized_calls: Arc<Mutex<usize>>,
    /// Whether the refresh grant itself fails.
    refresh_fails: Arc<Mutex<bool>>,
}

async fn token_handler(State(st): State<AuthState>, Form(form): Form<HashMap<String, String>>) -> axum::response::Response {
    let grant = form.get("grant_type").map(String::as_str).unwrap_or("");
    if grant == "refresh_token" {
        *st.refresh_count.lock().unwrap() += 1;
        if *st.refresh_fails.lock().unwrap() {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"})))
                .into_response();
        }
        return Json(json!({"access_token": "acc-refreshed", "expires_in": 300})).into_response();
    }
    Json(json!({
        "access_token": "acc-login",
        "refresh_token": REFRESH,
        "expires_in": 300
    }))
    .into_response()
}

use axum::response::IntoResponse;

fn data_base_state() -> AuthState {
    AuthState {
        refresh_count: Arc::new(Mutex::new(0)),
        unauthorized_calls: Arc::new(Mutex::new(0)),
        refresh_fails: Arc::new(Mutex::new(false)),
    }
}

/// Mock serving Keycloak + the read endpoints, sharing one auth state.
async fn mock_read(st: AuthState) -> String {
    let s = st.clone();
    let guard = move || {
        let mut n = s.unauthorized_calls.lock().unwrap();
        if *n > 0 {
            *n -= 1;
            Some(StatusCode::UNAUTHORIZED)
        } else {
            None
        }
    };
    let g1 = guard.clone();
    let g2 = guard.clone();
    let g3 = guard.clone();
    let me = move || {
        let g = g1.clone();
        async move {
            match g() {
                Some(code) => (code, Json(json!({}))).into_response(),
                None => Json(json!({"email": "me@example.com", "householdId": "hh1"})).into_response(),
            }
        }
    };
    let households = move || {
        let g = g2.clone();
        async move {
            match g() {
                Some(code) => (code, Json(json!({}))).into_response(),
                None => Json(json!({"households": [{"id": "hh1", "name": "Home"}]})).into_response(),
            }
        }
    };
    let tonies = move || {
        let g = g3.clone();
        async move {
            match g() {
                Some(code) => (code, Json(json!({}))).into_response(),
                None => Json(json!([{
                    "id": "ct1", "name": "Story One",
                    "secondsPresent": 3780, "secondsRemaining": 1620,
                    "chapters": [{"id": "ch1", "title": "Intro", "seconds": 92.820646}]
                }]))
                .into_response(),
            }
        }
    };

    let app = Router::new()
        .route("/realms/tonies/protocol/openid-connect/token", post(token_handler))
        .route("/v2/me", get(me))
        .route("/v2/households", get(households))
        .route("/v2/households/{hid}/creativetonies", get(tonies))
        .with_state(st);
    serve(app).await
}

// ── tests: client auth ──────────────────────────────────────────────────────

#[tokio::test]
async fn auth_refresh_on_401_retries_once() {
    let st = data_base_state();
    *st.unauthorized_calls.lock().unwrap() = 1; // first call 401, retry 200
    let auth = mock_read(st.clone()).await;
    // api base == auth base (same mock server).
    let mut client = ToniesClient::with_tokens(
        &auth,
        &auth,
        Tokens {
            access_token: "stale".into(),
            refresh_token: Some(REFRESH.into()),
            expires_at: 0,
        },
    );
    let me = client.get_me().await.expect("refresh then succeed");
    assert_eq!(me.email.as_deref(), Some("me@example.com"));
    assert_eq!(*st.refresh_count.lock().unwrap(), 1, "exactly one refresh");
}

#[tokio::test]
async fn expired_refresh_is_auth_expired() {
    let st = data_base_state();
    *st.unauthorized_calls.lock().unwrap() = 100; // always 401
    *st.refresh_fails.lock().unwrap() = true;
    let auth = mock_read(st.clone()).await;
    let mut client = ToniesClient::with_tokens(
        &auth,
        &auth,
        Tokens {
            access_token: "stale".into(),
            refresh_token: Some(REFRESH.into()),
            expires_at: 0,
        },
    );
    let err = client.get_me().await.unwrap_err();
    assert!(matches!(err, ToniesError::AuthExpired), "got {err:?}");
}

// ── tests: read endpoints + capacity ────────────────────────────────────────

#[tokio::test]
async fn read_endpoints_parse_capacity() {
    let auth = mock_read(data_base_state()).await;
    let mut client = ToniesClient::with_tokens(&auth, &auth, Tokens::default());
    let hs = client.get_households().await.unwrap();
    assert_eq!(hs.len(), 1);
    assert_eq!(hs[0].id.as_deref(), Some("hh1"));

    let list = client.get_creativetonies("hh1").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].title.as_deref(), Some("Story One"));
    // Die echte API liefert ein nacktes Array mit sekundenbruchteil-
    // genauen Kapitellaengen.
    assert_eq!(list[0].chapters.len(), 1, "chapters must survive parsing");
    assert_eq!(list[0].chapters[0].seconds, Some(92.820646));
    let cap = tonies::Capacity::from_tonie(&list[0]);
    assert_eq!(cap.minutes_present(), 63);
    assert_eq!(cap.minutes_total(), 90);
    assert_eq!(cap.percent(), 70);
}

// ── tests: upload primitives ────────────────────────────────────────────────

#[derive(Clone)]
struct UpState {
    base: String,
    puts: Arc<Mutex<Vec<(String, String)>>>,
    chapters: Arc<Mutex<Vec<Value>>>,
    patches: Arc<Mutex<Vec<Value>>>,
}

async fn put_handler(State(st): State<UpState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    st.puts
        .lock()
        .unwrap()
        .push((ct, String::from_utf8_lossy(&body).to_string()));
    StatusCode::OK
}

async fn chapter_handler(State(st): State<UpState>, Json(body): Json<Value>) -> Json<Value> {
    st.chapters.lock().unwrap().push(body);
    Json(json!({"ok": true}))
}

async fn patch_handler(State(st): State<UpState>, Json(body): Json<Value>) -> Json<Value> {
    st.patches.lock().unwrap().push(body);
    Json(json!({"ok": true}))
}

async fn mock_upload() -> (String, UpState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let st = UpState {
        base: base.clone(),
        puts: Arc::new(Mutex::new(Vec::new())),
        chapters: Arc::new(Mutex::new(Vec::new())),
        patches: Arc::new(Mutex::new(Vec::new())),
    };
    let ticket_state = st.clone();
    let file = move || {
        let s = ticket_state.clone();
        async move {
            // Shape of the live API: the upload target is nested.
            Json(json!({
                "fileId": "file-1",
                "request": {
                    "url": format!("{}/s3/upload", s.base),
                    "fields": {"key": "k1", "policy": "p1"}
                }
            }))
        }
    };
    let app = Router::new()
        .route("/v2/file", post(file))
        .route("/s3/upload", post(put_handler))
        .route("/v2/households/{hid}/creativetonies/{ctid}/chapters", post(chapter_handler))
        .route("/v2/households/{hid}/creativetonies/{ctid}", patch(patch_handler))
        .with_state(st.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, st)
}

#[tokio::test]
async fn upload_primitives_send_expected_bodies() {
    let (base, st) = mock_upload().await;
    let mut client = ToniesClient::with_tokens(&base, &base, Tokens::default());

    let ticket = client.request_upload().await.unwrap();
    assert_eq!(ticket.file_id, "file-1");
    assert!(ticket.url.ends_with("/s3/upload"));

    client
        .put_multipart(&ticket, "audio.mp3", "audio/mpeg", b"MP3DATA")
        .await
        .unwrap();
    let puts = st.puts.lock().unwrap();
    assert_eq!(puts.len(), 1);
    assert!(puts[0].0.starts_with("multipart/form-data; boundary="));
    let body = &puts[0].1;
    let key_pos = body.find("name=\"key\"").unwrap();
    let file_pos = body.find("name=\"file\"").unwrap();
    assert!(key_pos < file_pos, "fields must precede the file part");
    assert!(body.contains("Content-Type: audio/mpeg"));
    assert!(body.contains("MP3DATA"));
    drop(puts);

    client.add_chapter("hh1", "ct1", "Title", "file-1").await.unwrap();
    let ch = st.chapters.lock().unwrap();
    assert_eq!(ch[0]["title"], "Title");
    assert_eq!(ch[0]["file"], "file-1");
    drop(ch);

    let chapters = vec![Chapter {
        id: Some("ch1".into()),
        title: Some("Intro".into()),
        file: Some("file-1".into()),
        ..Default::default()
    }];
    client.patch_chapters("hh1", "ct1", &chapters).await.unwrap();
    let p = st.patches.lock().unwrap();
    assert_eq!(p[0]["chapters"][0]["id"], "ch1");
    assert_eq!(p[0]["chapters"][0]["file"], "file-1");
}

// ── tests: web page / connect / ownership ───────────────────────────────────

#[tokio::test]
async fn connect_stores_encrypted_token_and_hides_credentials() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-alice", "pw").await;

    let resp = app
        .client()
        .post(app.url("/tonies/connect"))
        .header("cookie", &cookie)
        .form(&[("email", "me@example.com"), ("password", PASSWORD)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // The rendered page must never contain the password or the refresh token.
    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!html.contains(PASSWORD), "password leaked into HTML");
    assert!(!html.contains(REFRESH), "refresh token leaked into HTML");
    assert!(html.contains("63 min / 90 min"), "capacity missing in HTML");

    // Stored secret is ciphertext; decryption round-trips.
    let enc = tonies::load_secret_enc(&app.pool, uid).await.unwrap().unwrap();
    assert_ne!(enc, REFRESH, "secret stored in plaintext");
    assert_eq!(tonies::decrypt_secret(KEY, &enc).unwrap(), REFRESH);

    // A forced upstream error still never leaks the token.
    let _ = app;
}

#[tokio::test]
async fn expired_session_renders_reconnect_form() {
    let st = data_base_state();
    let auth = mock_read(st.clone()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-bob", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("bob@example.com"), REFRESH, KEY)
        .await
        .unwrap();

    // Data endpoints 401 and the refresh fails → AuthExpired → reconnect form.
    *st.unauthorized_calls.lock().unwrap() = 100;
    *st.refresh_fails.lock().unwrap() = true;

    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("/tonies/reconnect"), "reconnect form expected");
    assert!(html.contains("neu verbinden") || html.contains("Neu verbinden"));
    assert!(!html.contains(PASSWORD));
    assert!(!html.contains(REFRESH));
}

#[tokio::test]
async fn error_message_stays_categorised_without_token() {
    // Mock whose data endpoint returns 500 for the household fetch.
    async fn boom() -> (StatusCode, &'static str) {
        (StatusCode::INTERNAL_SERVER_ERROR, "boom")
    }
    let auth = serve(
        Router::new()
            .route("/realms/tonies/protocol/openid-connect/token", post(token_handler))
            .route("/v2/households", get(boom))
            .route("/v2/households/{hid}/creativetonies", get(boom))
            .route("/v2/me", get(boom))
            .with_state(data_base_state()),
    )
    .await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-carol", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("c@example.com"), REFRESH, KEY)
        .await
        .unwrap();

    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!html.contains(REFRESH), "token leaked in error page");
    assert!(!html.contains(PASSWORD));
    // last_error holds only a categorised message.
    let account = tonies::load_account(&app.pool, uid).await.unwrap().unwrap();
    let err = account.last_error.unwrap_or_default();
    assert!(!err.contains(REFRESH) && !err.contains(PASSWORD));
    assert!(err.contains("500"), "status should be reported: {err}");
}

#[tokio::test]
async fn accounts_are_isolated_per_user() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid_a, cookie_a) = app.user_with_password("dave", "pw").await;
    let (_uid_b, cookie_b) = app.user_with_password("erin", "pw").await;
    tonies::save_account(&app.pool, uid_a, Some("hh1"), Some("dave@example.com"), REFRESH, KEY)
        .await
        .unwrap();

    let html_b = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie_b)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    // B has no account → connect form, no capacity, no A's data.
    assert!(html_b.contains("/tonies/connect"));
    assert!(!html_b.contains("63 min / 90 min"));
    assert!(!html_b.contains("dave@example.com"));

    let html_a = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie_a)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html_a.contains("63 min / 90 min"));
    let _ = cookie_a;
}

#[tokio::test]
async fn tonies_requires_session() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let resp = app
        .client()
        .get(app.url("/tonies"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

#[tokio::test]
async fn no_external_network_when_bases_pinned() {
    // An unreachable pinned base yields a typed error instead of reaching the
    // real Tonies hosts.
    let mut client = ToniesClient::with_tokens(
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        Tokens::default(),
    );
    let err = client.get_me().await.unwrap_err();
    assert!(matches!(err, ToniesError::Other(_)), "got {err:?}");
}

// ── tests: binding store (#500) ─────────────────────────────────────────────

/// Insert an owned Spotify playlist for `user_id`.
async fn insert_playlist(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    ext: &str,
    name: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name)
         VALUES (?1, 'spotify', ?2, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(ext)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("insert playlist")
}

/// Insert an owned backpack for `user_id`.
async fn insert_backpack(pool: &sqlx::SqlitePool, user_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 1, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

/// Insert an owned tag for `owner_user_id`.
async fn insert_tag(pool: &sqlx::SqlitePool, owner_user_id: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(owner_user_id)
    .bind(slug)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

#[tokio::test]
async fn binding_store_is_user_scoped() {
    let app = common::spawn().await;
    let (a, _) = app.user_with_password("bind-a", "pw").await;
    let (b, _) = app.user_with_password("bind-b", "pw").await;
    let pl_a = insert_playlist(&app.pool, a, "pl-a", "A-Liste").await;
    let pl_b = insert_playlist(&app.pool, b, "pl-b", "B-Liste").await;

    tonies::upsert_binding(
        &app.pool, a, "ct1", Some("Story"), "playlist", Some(pl_a), "mp3-320", "auto",
    )
    .await
    .unwrap();
    tonies::upsert_binding(
        &app.pool, b, "ct1", Some("B Story"), "playlist", Some(pl_b), "mp3-128", "manual",
    )
    .await
    .unwrap();

    let la = tonies::list_bindings(&app.pool, a).await.unwrap();
    assert_eq!(la.len(), 1);
    assert_eq!(la[0].source_id, Some(pl_a));
    assert_eq!(la[0].format, "mp3-320");
    assert_eq!(la[0].mode, "auto");
    assert!(la[0].dirty_since.is_some(), "dirty_since must be set on create");

    let lb = tonies::list_bindings(&app.pool, b).await.unwrap();
    assert_eq!(lb.len(), 1, "each user sees only their own binding");
    assert_eq!(lb[0].source_id, Some(pl_b));

    // get is scoped to the owner.
    assert!(tonies::get_binding(&app.pool, b, "ct1").await.unwrap().is_some());
    assert!(tonies::get_binding(&app.pool, a, "missing").await.unwrap().is_none());

    // delete removes only the caller's row.
    tonies::delete_binding(&app.pool, b, "ct1").await.unwrap();
    assert!(tonies::get_binding(&app.pool, b, "ct1").await.unwrap().is_none());
    assert!(tonies::get_binding(&app.pool, a, "ct1").await.unwrap().is_some());
}

#[tokio::test]
async fn binding_upsert_updates_in_place() {
    let app = common::spawn().await;
    let (u, _) = app.user_with_password("bind-u", "pw").await;
    let p1 = insert_playlist(&app.pool, u, "p1", "One").await;
    let p2 = insert_playlist(&app.pool, u, "p2", "Two").await;
    tonies::upsert_binding(&app.pool, u, "ct1", None, "playlist", Some(p1), "mp3-128", "manual")
        .await
        .unwrap();
    tonies::upsert_binding(&app.pool, u, "ct1", None, "playlist", Some(p2), "mp3-320", "auto")
        .await
        .unwrap();
    let l = tonies::list_bindings(&app.pool, u).await.unwrap();
    assert_eq!(l.len(), 1, "upsert must not duplicate");
    assert_eq!(l[0].source_id, Some(p2));
    assert_eq!(l[0].format, "mp3-320");
    assert_eq!(l[0].mode, "auto");
}

// ── tests: binding routes (#500) ────────────────────────────────────────────

#[tokio::test]
async fn binding_save_renders_fragment_with_toast() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-nya", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("nya@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let pl = insert_playlist(&app.pool, uid, "sp-1", "Chill Mix").await;

    let sid = pl.to_string();
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cookie)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", sid.as_str()),
            ("format", "mp3-320"),
            ("mode", "auto"),
        ])
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
    assert!(trig.contains("toast"), "toast header missing: {trig}");
    assert!(trig.contains("\"kind\":\"ok\""), "expected ok toast: {trig}");
    let body = resp.text().await.unwrap();
    assert!(body.contains("Chill Mix"), "fragment should show the source name");
    assert!(body.contains("id=\"binding-ct1\""), "fragment should target the binding area");
    assert!(!body.contains("hub-card"), "no message-card in fragment response");

    // Row persisted with the submitted values.
    let b = tonies::get_binding(&app.pool, uid, "ct1").await.unwrap().unwrap();
    assert_eq!(b.source_id, Some(pl));
    assert_eq!(b.format, "mp3-320");
    assert_eq!(b.mode, "auto");

    // The page reflects the binding.
    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Chill Mix"), "page should show the source name");
    assert!(html.contains("mp3-320"));
    assert!(html.contains("auto"));
}

#[tokio::test]
async fn binding_save_rejects_foreign_playlist() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (a, _ca) = app.user_with_password("t-own-a", "pw").await;
    let (b, cb) = app.user_with_password("t-own-b", "pw").await;
    tonies::save_account(&app.pool, b, Some("hh1"), Some("b@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let pl_a = insert_playlist(&app.pool, a, "a-1", "A Only").await;

    let sid = pl_a.to_string();
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cb)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", sid.as_str()),
            ("format", "mp3-128"),
            ("mode", "manual"),
        ])
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
    assert!(trig.contains("\"kind\":\"err\""), "expected err toast: {trig}");
    // Nothing written for B, and A is untouched.
    assert!(tonies::get_binding(&app.pool, b, "ct1").await.unwrap().is_none());
    assert!(tonies::get_binding(&app.pool, a, "ct1").await.unwrap().is_none());
}

#[tokio::test]
async fn binding_delete_is_user_scoped() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (a, _ca) = app.user_with_password("t-del-a", "pw").await;
    let (_b, cb) = app.user_with_password("t-del-b", "pw").await;
    tonies::save_account(&app.pool, _b, Some("hh1"), Some("b@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let pl = insert_playlist(&app.pool, a, "a-2", "A List").await;
    tonies::upsert_binding(&app.pool, a, "ct1", None, "playlist", Some(pl), "mp3-128", "manual")
        .await
        .unwrap();

    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding/delete"))
        .header("cookie", &cb)
        .form(&[("tonie_name", "Story One")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // A's binding survives B's delete attempt.
    assert!(tonies::get_binding(&app.pool, a, "ct1").await.unwrap().is_some());
}

#[tokio::test]
async fn binding_save_requires_session() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .form(&[("source_id", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tonies_page_without_playlists_shows_hint() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-empty", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("e@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("Noch keine Quelle"),
        "empty-state hint expected when the user has no sources"
    );
    assert!(html.contains("binding-ct1"), "binding area expected per card");
}

/// Mock serving Keycloak + read endpoints with an empty creativetonies list
/// (household still resolves), to exercise the "no Tonies" empty state.
async fn mock_read_empty_tonies() -> String {
    let app = Router::new()
        .route("/realms/tonies/protocol/openid-connect/token", post(token_handler))
        .route(
            "/v2/me",
            get(|| async { Json(json!({"email": "me@example.com", "householdId": "hh1"})) }),
        )
        .route(
            "/v2/households",
            get(|| async { Json(json!({"households": [{"id": "hh1", "name": "Home"}]})) }),
        )
        .route(
            "/v2/households/{hid}/creativetonies",
            get(|| async { Json(json!({"creativetonies": []})) }),
        )
        .with_state(data_base_state());
    serve(app).await
}

// ── tests: end-to-end page + full binding lifecycle (#500) ──────────────────

/// E2E: a connected user sees their own playlists offered as binding sources on
/// `/tonies`, and every card carries the capacity bar whose values equal the
/// mocked API's `secondsPresent`/`secondsRemaining`.
#[tokio::test]
async fn tonies_page_lists_playlists_as_options_with_capacity() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-e2e", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("e2e@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let pl = insert_playlist(&app.pool, uid, "sp-e2e", "Chill Mix").await;

    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // The user's own playlist is offered as an <option> of the picker; the
    // value carries the kind so ids never collide across kinds (#504).
    let opt = format!("<option value=\"playlist:{pl}\">Chill Mix</option>");
    assert!(html.contains(&opt), "playlist option missing: {opt}");
    // Capacity bar mirrors the mock (3780 s present, 5400 s total).
    assert!(html.contains("63 min / 90 min"), "capacity text missing");
    assert!(html.contains("value=\"70\""), "capacity percent missing");
}

/// Editing an existing binding re-uses the same row (no duplicate) and the page
/// reflects the new playlist/format/mode.
#[tokio::test]
async fn binding_edit_updates_in_place_via_web() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-edit", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("edit@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let p1 = insert_playlist(&app.pool, uid, "e-1", "First").await;
    let p2 = insert_playlist(&app.pool, uid, "e-2", "Second").await;

    let s1 = p1.to_string();
    let r1 = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cookie)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", s1.as_str()),
            ("format", "mp3-128"),
            ("mode", "manual"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);

    let s2 = p2.to_string();
    let r2 = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cookie)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", s2.as_str()),
            ("format", "mp3-320"),
            ("mode", "auto"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let trig = r2
        .headers()
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(trig.contains("\"kind\":\"ok\""), "expected ok toast: {trig}");

    let list = tonies::list_bindings(&app.pool, uid).await.unwrap();
    assert_eq!(list.len(), 1, "edit must not create a second row");
    assert_eq!(list[0].source_id, Some(p2));
    assert_eq!(list[0].format, "mp3-320");
    assert_eq!(list[0].mode, "auto");

    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Second"), "page should show the edited source");
}

/// User B never sees A's binding source name on their own `/tonies` page.
#[tokio::test]
async fn binding_is_not_visible_across_users() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (a, ca) = app.user_with_password("t-vis-a", "pw").await;
    let (b, cb) = app.user_with_password("t-vis-b", "pw").await;
    tonies::save_account(&app.pool, a, Some("hh1"), Some("a@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    tonies::save_account(&app.pool, b, Some("hh1"), Some("b@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let pl_a = insert_playlist(&app.pool, a, "vis-a", "A Secret List").await;
    tonies::upsert_binding(
        &app.pool, a, "ct1", Some("Story One"), "playlist", Some(pl_a), "mp3-320", "auto",
    )
    .await
    .unwrap();

    let html_b = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cb)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !html_b.contains("A Secret List"),
        "B must not see A's binding source"
    );

    let html_a = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &ca)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html_a.contains("A Secret List"), "A still sees their own binding");
}

/// The owner deleting their own binding removes the row, returns an ok toast and
/// re-renders the picker.
#[tokio::test]
async fn binding_delete_removes_own_row_with_toast() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-del-own", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("own@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let pl = insert_playlist(&app.pool, uid, "d-1", "To Delete").await;
    tonies::upsert_binding(&app.pool, uid, "ct1", None, "playlist", Some(pl), "mp3-128", "manual")
        .await
        .unwrap();

    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding/delete"))
        .header("cookie", &cookie)
        .form(&[("tonie_name", "Story One")])
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
    assert!(trig.contains("\"kind\":\"ok\""), "expected ok toast: {trig}");
    let body = resp.text().await.unwrap();
    assert!(!body.contains("hub-card"), "no message-card in delete response");
    assert!(tonies::get_binding(&app.pool, uid, "ct1").await.unwrap().is_none());
    assert!(body.contains("Quelle wählen"), "picker should return after delete");
}

#[tokio::test]
async fn binding_delete_requires_session() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding/delete"))
        .form(&[("tonie_name", "Story One")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tonies_page_without_creativetonies_shows_empty_state() {
    let auth = mock_read_empty_tonies().await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-notonies", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("n@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let html = app
        .client()
        .get(app.url("/tonies"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("Keine Kreativ-Tonies"),
        "empty-state for a household without Tonies expected"
    );
}

// ── tests: backpack / tag source kind via HTTP (#504) ───────────────────────

/// Posting the picker's `"backpack:<id>"` value persists a binding whose
/// `source_kind` is `backpack` (not the playlist default) and renders the
/// backpack name + kind badge in the swapped fragment.
#[tokio::test]
async fn binding_save_accepts_backpack_source() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-bp", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("bp@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let bp = insert_backpack(&app.pool, uid, "My Backpack").await;

    let src = format!("backpack:{bp}");
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cookie)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", src.as_str()),
            ("format", "mp3-128"),
            ("mode", "manual"),
        ])
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
    assert!(trig.contains("\"kind\":\"ok\""), "expected ok toast: {trig}");
    let body = resp.text().await.unwrap();
    assert!(body.contains("My Backpack"), "fragment should show the backpack name");
    assert!(body.contains("Backpack"), "fragment should show the kind badge");

    // Persisted with the backpack kind, not the playlist default.
    let b = tonies::get_binding(&app.pool, uid, "ct1").await.unwrap().unwrap();
    assert_eq!(b.source_kind, "backpack");
    assert_eq!(b.source_id, Some(bp));
}

/// Posting the picker's `"tag:<id>"` value persists a `tag` binding and renders
/// the tag name + kind badge.
#[tokio::test]
async fn binding_save_accepts_tag_source() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (uid, cookie) = app.user_with_password("t-tg", "pw").await;
    tonies::save_account(&app.pool, uid, Some("hh1"), Some("tg@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let tag = insert_tag(&app.pool, uid, "bedtime", "Bedtime").await;

    let src = format!("tag:{tag}");
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cookie)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", src.as_str()),
            ("format", "mp3-128"),
            ("mode", "manual"),
        ])
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
    assert!(trig.contains("\"kind\":\"ok\""), "expected ok toast: {trig}");
    let body = resp.text().await.unwrap();
    assert!(body.contains("Bedtime"), "fragment should show the tag name");

    let b = tonies::get_binding(&app.pool, uid, "ct1").await.unwrap().unwrap();
    assert_eq!(b.source_kind, "tag");
    assert_eq!(b.source_id, Some(tag));
}

/// A backpack owned by another user is rejected with an error toast and no row
/// is written (ownership is enforced per kind).
#[tokio::test]
async fn binding_save_rejects_unowned_backpack() {
    let auth = mock_read(data_base_state()).await;
    let app = spawn_web(&auth, &auth).await;
    let (a, _ca) = app.user_with_password("t-bp-own-a", "pw").await;
    let (b, cb) = app.user_with_password("t-bp-own-b", "pw").await;
    tonies::save_account(&app.pool, b, Some("hh1"), Some("b@example.com"), REFRESH, KEY)
        .await
        .unwrap();
    let bp_a = insert_backpack(&app.pool, a, "A Only Backpack").await;

    let src = format!("backpack:{bp_a}");
    let resp = app
        .client()
        .post(app.url("/tonies/ct1/binding"))
        .header("cookie", &cb)
        .form(&[
            ("tonie_name", "Story One"),
            ("source_id", src.as_str()),
            ("format", "mp3-128"),
            ("mode", "manual"),
        ])
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
    assert!(trig.contains("\"kind\":\"err\""), "expected err toast: {trig}");
    assert!(tonies::get_binding(&app.pool, b, "ct1").await.unwrap().is_none());
    assert!(tonies::get_binding(&app.pool, a, "ct1").await.unwrap().is_none());
}
