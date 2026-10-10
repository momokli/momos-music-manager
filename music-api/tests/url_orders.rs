//! Contract tests for URL-based orders (YouTube / SoundCloud) and metadata.
//!
//! These tests do **not** invoke yt-dlp: they only exercise the HTTP surface,
//! DB persistence and validation. Download behaviour is covered by the worker
//! unit tests and manual end-to-end runs.

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
        store_root: root.join("objects"),
        store_max_upload_bytes: music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
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
        store: music_api::store::Store::new(
            root.join("objects"),
            music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
        ),
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
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, body)
}

fn post_json(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn url_orders_require_a_token() {
    let h = harness().await;
    let req = Request::builder()
        .method("POST")
        .uri("/url-orders")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"items":[{"url":"https://soundcloud.com/a/b"}]}"#,
        ))
        .unwrap();
    let (status, _) = send(&h.app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_unsupported_url() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://example.com/x"}]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("unsupported URL"), "body: {text}");
}

#[tokio::test]
async fn rejects_empty_order() {
    let h = harness().await;
    let (status, _) = send(&h.app, post_json("/url-orders", r#"{"items":[]}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn creates_and_reads_url_order() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://soundcloud.com/momokli/sets/discover"}]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = json["orderId"].as_str().unwrap().to_string();
    assert_eq!(json["count"], 1);

    let (status, body) = send(&h.app, get(&format!("/url-orders/{order_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "open");
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["provider"], "soundcloud");
    assert_eq!(items[0]["state"], "pending");
}

#[tokio::test]
async fn deduplicates_urls() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://youtu.be/x"},{"url":"https://youtu.be/x"}]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["count"], 1);
}

#[tokio::test]
async fn prioritizes_url_order() {
    let h = harness().await;
    let (_, body) = send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://soundcloud.com/a/b"}]}"#,
        ),
    )
    .await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = json["orderId"].as_str().unwrap().to_string();

    let (status, _) = send(
        &h.app,
        post_json(
            &format!("/url-orders/{order_id}/prioritize"),
            r#"{"priority":42}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(&h.app, get(&format!("/url-orders/{order_id}"))).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["priority"], 42);
}

#[tokio::test]
async fn pauses_url_order() {
    let h = harness().await;
    let (_, body) = send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://soundcloud.com/a/b"}]}"#,
        ),
    )
    .await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = json["orderId"].as_str().unwrap().to_string();

    let (status, _) = send(
        &h.app,
        post_json(
            &format!("/url-orders/{order_id}/status"),
            r#"{"status":"paused"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(&h.app, get(&format!("/url-orders/{order_id}"))).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "paused");
}

#[tokio::test]
async fn rejects_invalid_status() {
    let h = harness().await;
    let (_, body) = send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://soundcloud.com/a/b"}]}"#,
        ),
    )
    .await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = json["orderId"].as_str().unwrap().to_string();

    let (status, _) = send(
        &h.app,
        post_json(
            &format!("/url-orders/{order_id}/status"),
            r#"{"status":"bogus"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lists_url_orders() {
    let h = harness().await;
    send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://soundcloud.com/a/b"}]}"#,
        ),
    )
    .await;

    let (status, body) = send(&h.app, get("/url-orders")).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["orders"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn queue_lists_pending_url_tracks() {
    let h = harness().await;
    send(
        &h.app,
        post_json(
            "/url-orders",
            r#"{"items":[{"url":"https://soundcloud.com/a/b"}]}"#,
        ),
    )
    .await;

    let (status, body) = send(&h.app, get("/queue")).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["url"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_url_order_is_404() {
    let h = harness().await;
    let (status, _) = send(&h.app, get("/url-orders/does-not-exist")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
