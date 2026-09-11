//! Mocked-HTTP tests for the owned deemix lifecycle (issue #32, section B.1).
//!
//! These tests drive [`DeemixClient`] against a local in-process mock of the
//! deemix-pyweb HTTP API (bound to `127.0.0.1:0`). No real deemix instance and
//! no real ARL is involved — the "ARL" used here is a hardcoded placeholder
//! seeded only into an in-memory SQLite DB.
//!
//! Covered lifecycle:
//!   1. ARL auth + automatic re-auth on HTTP 401 (session expiry).
//!   2. Action-level re-auth on `{"result": false, "errid": "NotLoggedIn"}`.
//!   3. Full queue control: add / retry / delete.
//!   4. Status/progress polling (`get_download_progress`).
//!   5. Download verification with target quality `stem` > `flac` > `mp3`.

mod common;

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use momos_music_manager::deemix::{AudioQuality, DeemixClient};
use serde_json::{Value, json};

/// Shared, mutable state for the mock deemix server.
#[derive(Default)]
struct MockState {
    /// When true, the next `GET /api/getQueue` returns HTTP 401 (session expiry).
    fail_next_get_queue: bool,
    get_queue_calls: usize,
    login_calls: usize,
    add_calls: Vec<String>,
    retry_calls: Vec<String>,
    remove_calls: Vec<String>,
    /// When true, the next `POST /api/addToQueue` returns `NotLoggedIn`.
    add_not_logged_in: bool,
}

type Shared = Arc<Mutex<MockState>>;

/// Spotify playlist id used in the mock queue (matches the URL under test).
const PLAYLIST_ID: &str = "37i9dQZF1DXcBWIGoYBM5M";
const PLAYLIST_URL: &str = "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M";
const UUID: &str = "uuid-1";

fn queue_body() -> Value {
    json!({
        "queue": {
            UUID: {
                "type": "spotify",
                "id": PLAYLIST_ID,
                "bitrate": 320,
                "uuid": UUID,
                "title": "Test Playlist",
                "artist": "Various Artists",
                "cover": null,
                "explicit": false,
                "size": 1,
                "downloaded": 1,
                "failed": 0,
                "progress": 100,
                "errors": [],
                "files": [
                    {
                        "album_urls": null,
                        "album_path": null,
                        "album_filename": null,
                        "filename": "Track.flac",
                        "data": null,
                        "path": "/music/Track.flac"
                    }
                ],
                "__type__": "playlist",
                "status": "completed"
            }
        },
        "queue_order": [UUID]
    })
}

async fn get_queue_handler(State(st): State<Shared>) -> impl IntoResponse {
    let mut s = st.lock().unwrap();
    s.get_queue_calls += 1;
    if s.fail_next_get_queue {
        s.fail_next_get_queue = false;
        return (StatusCode::UNAUTHORIZED, Json(json!({}))).into_response();
    }
    (StatusCode::OK, Json(queue_body())).into_response()
}

async fn login_handler(State(st): State<Shared>) -> impl IntoResponse {
    let mut s = st.lock().unwrap();
    s.login_calls += 1;
    (
        StatusCode::OK,
        Json(json!({
            "status": 1,
            "arl": "test-arl-placeholder",
            "user": {"name": "test"},
            "childs": [],
            "current_child": null
        })),
    )
        .into_response()
}

async fn add_handler(
    State(st): State<Shared>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let mut s = st.lock().unwrap();
    if let Some(url) = body.get("url").and_then(Value::as_str) {
        s.add_calls.push(url.to_string());
    }
    if s.add_not_logged_in {
        s.add_not_logged_in = false;
        return (
            StatusCode::OK,
            Json(json!({"result": false, "errid": "NotLoggedIn"})),
        )
            .into_response();
    }
    (StatusCode::OK, Json(json!({"result": true}))).into_response()
}

async fn retry_handler(
    State(st): State<Shared>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let mut s = st.lock().unwrap();
    if let Some(uuid) = body.get("uuid").and_then(Value::as_str) {
        s.retry_calls.push(uuid.to_string());
    }
    (StatusCode::OK, Json(json!({"result": true}))).into_response()
}

async fn remove_handler(
    State(st): State<Shared>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let mut s = st.lock().unwrap();
    if let Some(uuid) = body.get("uuid").and_then(Value::as_str) {
        s.remove_calls.push(uuid.to_string());
    }
    (StatusCode::OK, Json(json!({"result": true}))).into_response()
}

/// Start the mock deemix server; return (base_url, shared state).
async fn start_mock(shared: Shared) -> String {
    let app = Router::new()
        .route("/api/getQueue", get(get_queue_handler))
        .route("/api/loginArl", post(login_handler))
        .route("/api/addToQueue", post(add_handler))
        .route("/api/retryDownload", post(retry_handler))
        .route("/api/removeFromQueue", post(remove_handler))
        .with_state(shared);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind mock deemix server");
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base
}

/// Build a `DeemixClient` pointing at the mock server, backed by an in-memory
/// DB with a placeholder ARL seeded into `service_config`.
async fn build_client(base_url: &str) -> DeemixClient {
    let pool = common::create_test_db().await;
    sqlx::query(
        "INSERT INTO service_config (service, access_token, metadata_json, is_connected, created_at, updated_at)
         VALUES ('deemix', 'test-arl-placeholder', ?, 1, 0, 0)",
    )
    .bind(format!(r#"{{"host":"{base_url}"}}"#))
    .execute(&pool)
    .await
    .unwrap();

    DeemixClient::new(base_url, pool)
}

#[tokio::test]
async fn get_queue_reauths_on_http_401() {
    let shared = Shared::default();
    {
        shared.lock().unwrap().fail_next_get_queue = true;
    }
    let base = start_mock(shared.clone()).await;
    let client = build_client(&base).await;

    let queue = client.get_queue().await.expect("get_queue should succeed");

    assert_eq!(queue.len(), 1, "mock queue should return one item");
    let s = shared.lock().unwrap();
    assert_eq!(s.get_queue_calls, 2, "first call 401, then one retry");
    assert_eq!(s.login_calls, 1, "should re-authenticate once via loginArl");
}

#[tokio::test]
async fn add_reauths_on_not_logged_in_result() {
    let shared = Shared::default();
    {
        shared.lock().unwrap().add_not_logged_in = true;
    }
    let base = start_mock(shared.clone()).await;
    let client = build_client(&base).await;

    client
        .add_to_queue(PLAYLIST_URL)
        .await
        .expect("add should succeed after re-auth");

    let s = shared.lock().unwrap();
    assert_eq!(s.add_calls, vec![PLAYLIST_URL.to_string(), PLAYLIST_URL.to_string()]);
    assert_eq!(s.login_calls, 1, "should re-authenticate once via loginArl");
}

#[tokio::test]
async fn full_lifecycle_add_poll_verify_delete() {
    let shared = Shared::default();
    let base = start_mock(shared.clone()).await;
    let client = build_client(&base).await;

    // add
    client.add_to_queue(PLAYLIST_URL).await.unwrap();

    // progress poll
    let progress = client
        .get_download_progress(PLAYLIST_URL)
        .await
        .unwrap()
        .expect("playlist should be in queue");
    assert_eq!(progress.uuid, UUID);
    assert_eq!(progress.progress, 100);
    assert_eq!(progress.downloaded, 1);
    assert_eq!(progress.total, 1);
    assert!(progress.finished, "completed status should be terminal");
    assert!(!progress.has_errors);

    // verification
    let verification = client
        .verify_download(PLAYLIST_URL)
        .await
        .unwrap()
        .expect("playlist should be in queue");
    assert!(verification.verified);
    assert!(verification.completed);
    assert_eq!(verification.file_count, 1);
    assert_eq!(verification.best_quality, Some(AudioQuality::Flac));

    // delete
    client.remove_from_queue(UUID).await.unwrap();

    let s = shared.lock().unwrap();
    assert_eq!(s.add_calls, vec![PLAYLIST_URL.to_string()]);
    assert_eq!(s.remove_calls, vec![UUID.to_string()]);
    assert!(s.retry_calls.is_empty(), "no retry in the happy path");
}
