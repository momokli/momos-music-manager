//! Contract tests for the public HTTP surface (no network, no ffmpeg).
//!
//! Each test builds its own app over a temp SQLite file and a temp data dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use music_api::{AppState, Config, build_router, connect_db, db};

const TOKEN: &str = "test-token";

struct Harness {
    _tmp: TempDir,
    app: Router,
    state: Arc<AppState>,
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let config = Config {
        token: TOKEN.to_string(),
        bind: "127.0.0.1:0".to_string(),
        deemix_url: "http://127.0.0.1:1".to_string(),
        deemix_arl: String::new(),
        deemix_bitrate: 9,
        deemix_download_dir: root.join("incoming"),
        data_dir: root.clone(),
        deezer_base: "http://127.0.0.1:1".to_string(),
        ffmpeg: "ffmpeg".to_string(),
        ffprobe: "ffprobe".to_string(),
        worker_interval: Duration::from_secs(5),
        download_timeout: Duration::from_secs(60),
    };

    for dir in [
        config.data_dir.clone(),
        config.flac_dir(),
        config.mp3_320_dir(),
        config.mp3_128_dir(),
        config.deemix_download_dir.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    let pool = connect_db(&config).await.unwrap();
    db::init(&pool).await.unwrap();

    let state = Arc::new(AppState {
        pool,
        config,
        http: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
        deemix_login: Default::default(),
    });

    Harness {
        app: build_router(state.clone()),
        state,
        _tmp: tmp,
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn json_body(v: serde_json::Value) -> Body {
    Body::from(serde_json::to_vec(&v).unwrap())
}

fn authed(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
}

async fn create_order(app: &Router, isrcs: &[&str]) -> String {
    let items: Vec<serde_json::Value> = isrcs
        .iter()
        .map(|i| serde_json::json!({ "isrc": i }))
        .collect();
    let (status, body) = send(
        app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "items": items })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create order failed");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    v["orderId"].as_str().unwrap().to_string()
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[tokio::test]
async fn health_is_public() {
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        Request::builder().uri("/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn orders_require_a_token() {
    let h = harness().await;

    // No header.
    let (status, _) = send(
        &h.app,
        Request::builder()
            .method("POST")
            .uri("/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "items": [] })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Wrong token.
    let (status, _) = send(
        &h.app,
        Request::builder()
            .method("GET")
            .uri("/orders")
            .header(header::AUTHORIZATION, "Bearer nope")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_and_read_order() {
    let h = harness().await;

    // Hyphenated + bare form of the same ISRC must dedupe to one item.
    let order_id = create_order(&h.app, &["US-QX9-12-01487", "USQX91201487"]).await;

    let (status, body) = send(
        &h.app,
        authed("GET", &format!("/orders/{order_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "open");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["isrc"], "USQX91201487");
    assert_eq!(v["items"][0]["state"], "pending");

    // Unknown order → 404.
    let (status, _) = send(
        &h.app,
        authed("GET", "/orders/does-not-exist")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn order_listing_filters_by_status() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/orders?status=open")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["orders"].as_array().unwrap().len(), 1);

    let (_, body) = send(
        &h.app,
        authed("GET", "/orders?status=done")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["orders"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn isrc_status_and_unknown() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/isrc/aea0d1846146")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["isrc"], "AEA0D1846146");
    assert_eq!(v["state"], "pending");

    let (status, _) = send(
        &h.app,
        authed("GET", "/isrc/ZZZZZZZZZZZZ")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serves_ready_files_and_flips_order_to_done() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    // Seed a finished track (the worker would normally do this).
    let mp3_320 = write_file(&h.state.config.mp3_320_dir(), "AEA0D1846146.mp3", b"ID3fake");
    db::mark_ready(
        &h.state.pool,
        "AEA0D1846146",
        "mp3",
        None,
        Some(&mp3_320.display().to_string()),
        None,
    )
    .await
    .unwrap();

    let (status, body) = send(
        &h.app,
        authed("GET", "/isrc/AEA0D1846146/320")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ID3fake");

    // No FLAC was produced for this track.
    let (status, _) = send(
        &h.app,
        authed("GET", "/isrc/AEA0D1846146/flac")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unknown format.
    let (status, _) = send(
        &h.app,
        authed("GET", "/isrc/AEA0D1846146/ogg")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The order flips to done once its only item is ready.
    let (_, body) = send(
        &h.app,
        authed("GET", "/orders").body(Body::empty()).unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = v["orders"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(v["orders"][0]["status"], "done");

    let (_, body) = send(
        &h.app,
        authed("GET", &format!("/orders/{order_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "done");
}
