//! Shared harness for the music-api integration tests (temp SQLite + temp dirs).
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use music_api::{build_router, connect_db, db, AppState, Config};
use tempfile::TempDir;
use tower::ServiceExt;

pub const TOKEN: &str = "test-token";

pub struct Harness {
    pub _tmp: TempDir,
    pub app: Router,
    pub state: Arc<AppState>,
}

fn config(root: PathBuf) -> Config {
    Config {
        token: TOKEN.to_string(),
        bind: "127.0.0.1:0".to_string(),
        deezer_arl: String::new(),
        download_quality: music_api::deezer_dl::Quality::Flac,
        data_dir: root.clone(),
        store_root: root.join("objects"),
        store_max_upload_bytes: music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
        deezer_base: "http://127.0.0.1:1".to_string(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        ffmpeg: "ffmpeg".to_string(),
        ffprobe: "ffprobe".to_string(),
        ytdlp: "yt-dlp".to_string(),
        ytdlp_dir: root.join("ytdlp"),
        ytdlp_concurrency: 2,
        transcode_concurrency: 2,
        worker_interval: Duration::from_secs(5),
        download_timeout: Duration::from_secs(60),
        queue_max_attempts: 3,
        queue_retry_base_secs: 60,
        queue_retry_max_secs: 1800,
        max_inflight: 5,
        analyzer_url: None,
        analysis_concurrency: 1,
        analysis_precedence: vec![
            "traktor".to_string(),
            "essentia".to_string(),
            "reccobeats".to_string(),
        ],
    }
}

/// Build a fresh app over a temp SQLite file and a temp data dir.
pub async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let config = config(root.clone());

    for dir in [
        config.data_dir.clone(),
        config.flac_dir(),
        config.mp3_320_dir(),
        config.mp3_128_dir(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    let pool = connect_db(&config).await.unwrap();
    db::init(&pool).await.unwrap();

    let ytdlp_concurrency = config.ytdlp_concurrency;
    let transcode_concurrency = config.transcode_concurrency;
    let analysis_concurrency = config.analysis_concurrency;
    let store_root = config.store_root.clone();
    let state = Arc::new(AppState {
        pool,
        config,
        http: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
        native_session: Arc::new(tokio::sync::RwLock::new(None)),
        native_login: Default::default(),
        native_fake: None,
        store: music_api::store::Store::new(store_root, music_api::store::DEFAULT_MAX_UPLOAD_BYTES),
        logs: music_api::logbuf::LogBuffer::new(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ytdlp_sem: Arc::new(tokio::sync::Semaphore::new(ytdlp_concurrency)),
        finalize_sem: Arc::new(tokio::sync::Semaphore::new(transcode_concurrency)),
        analysis_sem: Arc::new(tokio::sync::Semaphore::new(analysis_concurrency)),
        analyzing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_backoff: Default::default(),
        arl_tier: Arc::new(tokio::sync::RwLock::new(None)),
    });

    music_api::store::init(&state.pool, &state.store)
        .await
        .unwrap();

    Harness {
        app: build_router(state.clone()),
        state,
        _tmp: tmp,
    }
}

pub async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

pub fn json_body(v: serde_json::Value) -> Body {
    Body::from(serde_json::to_vec(&v).unwrap())
}

pub fn authed(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
}

/// `POST /orders` with an explicit intent/target.
pub async fn create_order_typed(
    app: &Router,
    isrcs: &[&str],
    intent: &str,
    target: &str,
) -> (StatusCode, serde_json::Value) {
    let items: Vec<serde_json::Value> = isrcs
        .iter()
        .map(|i| serde_json::json!({ "isrc": i }))
        .collect();
    let (status, body) = send(
        app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": items, "intent": intent, "target": target
            })))
            .unwrap(),
    )
    .await;
    let v = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}

pub async fn get_json_authed(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let (status, body) = send(app, authed("GET", uri).body(Body::empty()).unwrap()).await;
    let v = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}
