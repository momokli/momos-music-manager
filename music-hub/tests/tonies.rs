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
                None => Json(json!({"creativetonies": [{
                    "id": "ct1", "name": "Story One",
                    "secondsPresent": 3780, "secondsRemaining": 1620,
                    "chapters": [{"id": "ch1", "title": "Intro"}]
                }]}))
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
            Json(json!({
                "fileId": "file-1",
                "url": format!("{}/s3/upload", s.base),
                "fields": {"key": "k1", "policy": "p1"}
            }))
        }
    };
    let app = Router::new()
        .route("/v2/file", post(file))
        .route("/s3/upload", put(put_handler))
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
