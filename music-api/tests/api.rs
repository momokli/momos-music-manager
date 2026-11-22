//! Contract tests for the public HTTP surface (no network, no ffmpeg).
//!
//! Each test builds its own app over a temp SQLite file and a temp data dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query};
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use music_api::{build_router, connect_db, db, AppState, Config};

const TOKEN: &str = "test-token";

struct Harness {
    _tmp: TempDir,
    app: Router,
    state: Arc<AppState>,
}

async fn harness() -> Harness {
    harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: String::new(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: None,
        deezer_base: None,
    })
    .await
}

/// Harness variant that configures the BPM/key analyzer base URL.
async fn harness_with(analyzer_url: Option<String>) -> Harness {
    harness_opts(HarnessOpts {
        analyzer_url,
        deezer_arl: String::new(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: None,
        deezer_base: None,
    })
    .await
}

struct HarnessOpts {
    analyzer_url: Option<String>,
    deezer_arl: String,
    deezer_gw_base: String,
    native_fake: Option<Arc<music_api::worker::NativeFake>>,
    deezer_base: Option<String>,
}

/// Full-control harness builder (ARL + gw-light base for tier tests).
async fn harness_opts(opts: HarnessOpts) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let config = Config {
        token: TOKEN.to_string(),
        bind: "127.0.0.1:0".to_string(),
        deezer_arl: opts.deezer_arl,
        download_quality: music_api::deezer_dl::Quality::Flac,
        data_dir: root.clone(),
        store_root: root.join("objects"),
        store_max_upload_bytes: music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
        deezer_base: opts
            .deezer_base
            .unwrap_or_else(|| "http://127.0.0.1:1".to_string()),
        deezer_gw_base: opts.deezer_gw_base,
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
        analyzer_url: opts.analyzer_url,
        analysis_concurrency: 1,
        analysis_precedence: vec![
            "traktor".to_string(),
            "essentia".to_string(),
            "reccobeats".to_string(),
        ],
    };

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
    let config_transcode_concurrency = config.transcode_concurrency;
    let state = Arc::new(AppState {
        pool,
        config,
        http: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
        native_session: Arc::new(tokio::sync::RwLock::new(None)),
        native_login: Default::default(),
        native_fake: opts.native_fake,
        store: music_api::store::Store::new(
            root.join("objects"),
            music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
        ),
        logs: music_api::logbuf::LogBuffer::new(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ytdlp_sem: Arc::new(tokio::sync::Semaphore::new(ytdlp_concurrency)),
        finalize_sem: Arc::new(tokio::sync::Semaphore::new(config_transcode_concurrency)),
        analysis_sem: Arc::new(tokio::sync::Semaphore::new(1)),
        analyzing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_backoff: Default::default(),
        arl_tier: Arc::new(tokio::sync::RwLock::new(None)),
    });

    // Mirror production startup: create the store table + tmp dir.
    music_api::store::init(&state.pool, &state.store)
        .await
        .unwrap();

    Harness {
        app: build_router(state.clone()),
        state,
        _tmp: tmp,
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
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

/// `POST /orders` with an explicit intent/target (U0b).
async fn create_order_typed(
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

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

async fn create_order_with_priority(app: &Router, isrcs: &[&str], priority: i64) -> String {
    let items: Vec<serde_json::Value> = isrcs
        .iter()
        .map(|i| serde_json::json!({ "isrc": i }))
        .collect();
    let (status, body) = send(
        app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                serde_json::json!({ "items": items, "priority": priority }),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create order failed");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    v["orderId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn health_is_public() {
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
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
async fn create_soundcloud_order_uses_composite_key() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [ { "source": "soundcloud", "externalId": "1837565340" } ]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create soundcloud order failed");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = v["orderId"].as_str().unwrap().to_string();

    let (st, b) = send(
        &h.app,
        authed("GET", &format!("/orders/{order_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ov: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(ov["items"][0]["isrc"], "soundcloud:1837565340");
    assert_eq!(ov["items"][0]["state"], "pending");
}

#[tokio::test]
async fn create_youtube_order_uses_composite_key() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [ { "source": "youtube", "externalId": "dQw4w9WgXcQ" } ]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create youtube order failed");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let order_id = v["orderId"].as_str().unwrap().to_string();

    let (st, b) = send(
        &h.app,
        authed("GET", &format!("/orders/{order_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ov: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(ov["items"][0]["isrc"], "youtube:dQw4w9WgXcQ");
    assert_eq!(ov["items"][0]["state"], "pending");
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
    let mp3_320 = write_file(
        &h.state.config.mp3_320_dir(),
        "AEA0D1846146.mp3",
        b"ID3fake",
    );
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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn object_store_round_trips_and_verifies() {
    let h = harness().await;
    let body = b"hello object store".to_vec();
    let hash = sha256_hex(&body);

    // Absent until uploaded.
    let (status, _) = send(
        &h.app,
        authed("HEAD", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Upload.
    let (status, _) = send(
        &h.app,
        authed("PUT", &format!("/objects/{hash}"))
            .header(header::CONTENT_TYPE, "audio/flac")
            .header("x-original-path", "/Music/flacs/x.flac")
            .body(Body::from(body.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // HEAD reports the size.
    let resp = h
        .app
        .clone()
        .oneshot(
            authed("HEAD", &format!("/objects/{hash}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[header::CONTENT_LENGTH],
        body.len().to_string().as_str()
    );

    // GET returns identical bytes.
    let (status, got) = send(
        &h.app,
        authed("GET", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, body);

    // Range request.
    let resp = h
        .app
        .clone()
        .oneshot(
            authed("GET", &format!("/objects/{hash}"))
                .header(header::RANGE, "bytes=0-4")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let part = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert_eq!(part, b"hello");

    // Re-upload is a no-op.
    let (status, resp_body) = send(
        &h.app,
        authed("PUT", &format!("/objects/{hash}"))
            .body(Body::from(body.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(v["present"], true);

    // Bulk check.
    let missing = sha256_hex(b"nope");
    let (status, check_body) = send(
        &h.app,
        authed("POST", "/objects/check")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({"hashes": [hash, missing]})))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&check_body).unwrap();
    assert_eq!(v["present"].as_array().unwrap().len(), 1);
    assert_eq!(v["missing"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn object_store_rejects_a_wrong_digest() {
    let h = harness().await;
    let body = b"payload".to_vec();
    let wrong = sha256_hex(b"different");

    let (status, _) = send(
        &h.app,
        authed("PUT", &format!("/objects/{wrong}"))
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing was stored under the claimed key.
    let (status, _) = send(
        &h.app,
        authed("HEAD", &format!("/objects/{wrong}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Object store: delete / prune / stats (#355, #703) ──────────────────────

#[tokio::test]
async fn object_orphan_reconcile_route_is_gone() {
    // #703: `prune_orphans` was removed — deletion happens only via an explicit
    // hash list on `POST /objects/prune`.
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        authed("POST", "/objects/prune/orphans")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "dryRun": true })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn put_object(h: &Harness, bytes: &[u8]) -> String {
    let hash = sha256_hex(bytes);
    let (status, _) = send(
        &h.app,
        authed("PUT", &format!("/objects/{hash}"))
            .body(Body::from(bytes.to_vec()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    hash
}

async fn head_status(h: &Harness, hash: &str) -> StatusCode {
    send(
        &h.app,
        authed("HEAD", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .0
}

async fn object_row_count(h: &Harness, hash: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM store_objects WHERE hash = ?")
        .bind(hash)
        .fetch_one(&h.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn object_delete_removes_file_and_row() {
    let h = harness().await;
    let hash = put_object(&h, b"delete-me").await;

    // Invalid key.
    let (status, _) = send(
        &h.app,
        authed("DELETE", "/objects/not-a-hash")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Delete once: file and row gone.
    let (status, _) = send(
        &h.app,
        authed("DELETE", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(head_status(&h, &hash).await, StatusCode::NOT_FOUND);
    assert_eq!(object_row_count(&h, &hash).await, 0);

    // Idempotent: a second delete is a 404.
    let (status, _) = send(
        &h.app,
        authed("DELETE", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unknown key → 404.
    let unknown = sha256_hex(b"never-stored");
    let (status, _) = send(
        &h.app,
        authed("DELETE", &format!("/objects/{unknown}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn object_prune_dry_run_then_real() {
    let h = harness().await;
    let body = b"prunable-object";
    let hash = put_object(&h, body).await;
    let absent = sha256_hex(b"absent");

    // dryRun reports but deletes nothing.
    let (status, resp) = send(
        &h.app,
        authed("POST", "/objects/prune")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "hashes": [hash, absent, "garbage"],
                "dryRun": true
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["deleted"].as_array().unwrap(), &[serde_json::json!(hash)]);
    assert_eq!(v["freedBytes"], body.len() as u64);
    let missing = v["missing"].as_array().unwrap();
    assert!(missing.contains(&serde_json::json!(absent)));
    assert!(missing.contains(&serde_json::json!("garbage")));
    assert_eq!(
        head_status(&h, &hash).await,
        StatusCode::OK,
        "dryRun kept the object"
    );

    // Real prune frees the bytes.
    let (status, resp) = send(
        &h.app,
        authed("POST", "/objects/prune")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "hashes": [hash],
                "dryRun": false
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["dryRun"], false);
    assert_eq!(v["freedBytes"], body.len() as u64);
    assert_eq!(head_status(&h, &hash).await, StatusCode::NOT_FOUND);

    // Idempotent: now it is "missing".
    let (status, resp) = send(
        &h.app,
        authed("POST", "/objects/prune")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "hashes": [hash] })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["dryRun"], false, "dryRun defaults to false");
    assert_eq!(v["deleted"].as_array().unwrap().len(), 0);
    assert_eq!(v["missing"].as_array().unwrap(), &[serde_json::json!(hash)]);
    assert_eq!(v["freedBytes"], 0);
}

#[tokio::test]
async fn object_stats_track_count_and_bytes() {
    let h = harness().await;

    let (status, resp) = send(
        &h.app,
        authed("GET", "/objects/stats").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["count"], 0);
    assert_eq!(v["totalBytes"], 0);

    let body = b"stats-object";
    let hash = put_object(&h, body).await;
    let (_, resp) = send(
        &h.app,
        authed("GET", "/objects/stats").body(Body::empty()).unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["count"], 1);
    assert_eq!(v["totalBytes"], body.len() as u64);

    let (status, _) = send(
        &h.app,
        authed("DELETE", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, resp) = send(
        &h.app,
        authed("GET", "/objects/stats").body(Body::empty()).unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["count"], 0);
    assert_eq!(v["totalBytes"], 0);
}

// ── Object store: prune/delete edge cases (#355) ───────────────────────────

async fn prune(h: &Harness, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let (status, resp) = send(
        &h.app,
        authed("POST", "/objects/prune")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(body))
            .unwrap(),
    )
    .await;
    (status, serde_json::from_slice(&resp).unwrap())
}

#[tokio::test]
async fn object_delete_rejects_non_hex_keys() {
    let h = harness().await;
    // 64 chars but uppercase → still invalid (must be lowercase hex).
    let upper = "A".repeat(64);
    for key in [upper.as_str(), "short", &sha256_hex(b"x")[..63]] {
        let (status, _) = send(
            &h.app,
            authed("DELETE", &format!("/objects/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "key {key:?}");
    }
}

#[tokio::test]
async fn object_prune_empty_list_is_a_noop() {
    let h = harness().await;
    // Seed an object to prove an empty batch touches nothing.
    let hash = put_object(&h, b"keep-me").await;

    for dry in [false, true] {
        let (status, v) = prune(&h, serde_json::json!({ "hashes": [], "dryRun": dry })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["deleted"].as_array().unwrap().len(), 0);
        assert_eq!(v["missing"].as_array().unwrap().len(), 0);
        assert_eq!(v["freedBytes"], 0);
        assert_eq!(v["dryRun"], dry);
    }
    assert_eq!(head_status(&h, &hash).await, StatusCode::OK);
}

#[tokio::test]
async fn object_prune_mixed_batch_partitions_and_frees() {
    let h = harness().await;
    let a = b"prune-a".to_vec();
    let b = b"prune-bb".to_vec();
    let ha = put_object(&h, &a).await;
    let hb = put_object(&h, &b).await;
    let absent = sha256_hex(b"not-there");

    // Real run over present + absent + invalid + a duplicate of an existing key.
    let (status, v) = prune(
        &h,
        serde_json::json!({
            "hashes": [ha, absent, "zzz", hb, ha],
            "dryRun": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let deleted = v["deleted"].as_array().unwrap();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&serde_json::json!(ha)));
    assert!(deleted.contains(&serde_json::json!(hb)));
    let missing = v["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 3);
    assert!(missing.contains(&serde_json::json!(absent)));
    assert!(missing.contains(&serde_json::json!("zzz")));
    // The duplicate occurrence is reported as missing (already gone).
    assert!(missing.contains(&serde_json::json!(ha)));
    assert_eq!(v["freedBytes"], (a.len() + b.len()) as u64);
    assert_eq!(head_status(&h, &ha).await, StatusCode::NOT_FOUND);
    assert_eq!(head_status(&h, &hb).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn object_prune_all_missing_or_invalid_is_idempotent() {
    let h = harness().await;
    let absent = sha256_hex(b"ghost");
    let body = serde_json::json!({ "hashes": [absent, "bad", ""] });

    for _ in 0..2 {
        let (status, v) = prune(&h, body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["deleted"].as_array().unwrap().len(), 0);
        assert_eq!(v["missing"].as_array().unwrap().len(), 3);
        assert_eq!(v["freedBytes"], 0);
        assert_eq!(v["dryRun"], false);
    }
}

#[tokio::test]
async fn object_stats_sums_multiple_objects() {
    let h = harness().await;
    let a = put_object(&h, b"one").await;
    let b = put_object(&h, b"two-two").await;

    let (status, resp) = send(
        &h.app,
        authed("GET", "/objects/stats").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["count"], 2);
    assert_eq!(v["totalBytes"], 10);

    // Prune one away; counters follow.
    let (status, v) = prune(&h, serde_json::json!({ "hashes": [a] })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["deleted"].as_array().unwrap().len(), 1);
    let (_, resp) = send(
        &h.app,
        authed("GET", "/objects/stats").body(Body::empty()).unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(v["count"], 1);
    assert_eq!(v["totalBytes"], 7);
    assert_eq!(head_status(&h, &b).await, StatusCode::OK);
}

// ── Durable queue: retries, lease recovery, counts ──────────────────────────

#[tokio::test]
async fn worker_status_reports_retry_cap() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        authed("GET", "/worker").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["paused"], false);
    assert_eq!(v["maxAttempts"], 3);
    assert_eq!(v["maxInflight"], 5);
}

#[tokio::test]
async fn queue_reports_state_counts() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146", "USQX91201487"]).await;
    let (status, body) = send(&h.app, authed("GET", "/queue").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["counts"]["pending"], 2);
}

#[tokio::test]
async fn retry_resets_a_failed_track() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;
    db::mark_terminal(&h.state.pool, "AEA0D1846146", "failed", "boom")
        .await
        .unwrap();

    let (status, body) = send(
        &h.app,
        authed("POST", "/isrc/aea0d1846146/retry")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["state"], "pending");

    let t = db::get_track(&h.state.pool, "AEA0D1846146")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "pending");
    assert_eq!(t.attempts, 0);
}

#[tokio::test]
async fn expired_lease_is_requeued() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;
    // Stranded in `downloading` with a lease already in the past (dead process).
    db::mark_downloading(&h.state.pool, "AEA0D1846146", "uuid-1", 0)
        .await
        .unwrap();

    let policy = db::RetryPolicy {
        max_attempts: 3,
        base_secs: 60,
        max_secs: 1800,
    };
    let recovered = db::recover_expired_leases(&h.state.pool, &policy, db::now())
        .await
        .unwrap();
    assert_eq!(recovered, 1);

    let t = db::get_track(&h.state.pool, "AEA0D1846146")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "pending");
    assert_eq!(t.attempts, 1);
    // The backoff window pushes it out of the due set until it elapses.
    assert_eq!(
        db::due_tracks(&h.state.pool, 10, db::now())
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn due_tracks_orders_by_priority_then_age() {
    let h = harness().await;
    // The high-priority order is created *after* the low-priority one, so age
    // alone would pick the wrong row — priority must win (the `next` function).
    create_order_with_priority(&h.app, &["AEA0D1846146"], 1).await;
    create_order_with_priority(&h.app, &["USQX91201487"], 9).await;

    let due = db::due_tracks(&h.state.pool, 10, db::now()).await.unwrap();
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].isrc, "USQX91201487");
    assert_eq!(due[1].isrc, "AEA0D1846146");
}

#[tokio::test]
async fn resolve_pending_respects_inflight_cap() {
    let h = harness().await; // max_inflight = 5
    let isrcs = [
        "AEA0D1846146",
        "USQX91201487",
        "USUM71807935",
        "GBUM71029604",
        "FRZ039800054",
        "USRC17607839",
    ];
    create_order(&h.app, &isrcs).await;
    // Five of the six are already dispatched (in flight).
    for isrc in &isrcs[..5] {
        db::mark_downloading(&h.state.pool, isrc, "uuid", db::now() + 60)
            .await
            .unwrap();
    }
    assert_eq!(db::count_downloading(&h.state.pool).await.unwrap(), 5);

    // Zero free slots → nothing new is pulled from the priority `next` set, and
    // no deemix/Deezer call is made for the sixth row.
    music_api::worker::resolve_pending(&h.state).await.unwrap();

    assert_eq!(db::count_downloading(&h.state.pool).await.unwrap(), 5);
    let t = db::get_track(&h.state.pool, isrcs[5])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "pending");
    assert_eq!(t.attempts, 0);
    assert_eq!(t.next_attempt_at, None);
}

// ── Per-ISRC metadata (BPM/key authority) ─────────────────────────────────

#[tokio::test]
async fn metadata_upsert_and_range_get() {
    let h = harness().await;

    // Upsert two ISRCs (hyphenated input is normalised; `musicalKey` alias works).
    let (status, body) = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [
                    { "isrc": "deq022280682", "bpm": 128.0, "key": "11m", "camelot": "8A", "source": "traktor" },
                    { "isrc": "USQX91201487", "bpm": 124.5, "musicalKey": "8A" }
                ]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["upserted"], 2);

    // Range lookup returns found items + the missing ISRCs.
    let (status, body) = send(
        &h.app,
        authed(
            "GET",
            "/metadata?isrcs=DEQ022280682,USQX91201487,NOTFOUND123",
        )
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let missing = v["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], "NOTFOUND123");
    let deq = items.iter().find(|m| m["isrc"] == "DEQ022280682").unwrap();
    assert_eq!(deq["bpm"], 128.0);
    assert_eq!(deq["musicalKey"], "11m");
    assert_eq!(deq["camelot"], "8A");
    let usq = items.iter().find(|m| m["isrc"] == "USQX91201487").unwrap();
    assert_eq!(usq["musicalKey"], "8A");
}

#[tokio::test]
async fn metadata_merge_keeps_non_null() {
    let h = harness().await;

    let _ = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{ "isrc": "USQX91201487", "bpm": 124.0, "source": "traktor" }]
            })))
            .unwrap(),
    )
    .await;
    // Second push: key only (bpm null) — must not wipe the stored bpm.
    let _ = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{ "isrc": "USQX91201487", "key": "8A", "source": "analysis" }]
            })))
            .unwrap(),
    )
    .await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/metadata/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["bpm"], 124.0, "bpm must survive a null-field push");
    assert_eq!(v["musicalKey"], "8A");
    assert_eq!(v["source"], "analysis");

    // Unknown single ISRC → 404.
    let (status, _) = send(
        &h.app,
        authed("GET", "/metadata/NOPE").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Analysis job (music-api as BPM/key authority) ─────────────────────────────

/// A stand-in for the Essentia analyzer: answers `POST /analyze` with a fixed
/// result. No real network — the mock lives on loopback.
async fn analyzer_mock(Json(_body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "bpm": 123.0, "key": "8A", "camelot": "8A" }))
}

/// Spawn the mock analyzer on an ephemeral port; returns its base URL.
async fn spawn_analyzer() -> String {
    let app = Router::new().route("/analyze", post(analyzer_mock));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Seed one `ready` track with a FLAC path. No file must exist — the analyzer is
/// mocked and never actually reads it.
async fn seed_ready_flac(h: &Harness, isrc: &str, flac: &str) {
    db::create_order(&h.state.pool, "o1", &[isrc.to_string()], 0, "library", None)
        .await
        .unwrap();
    db::mark_ready(
        &h.state.pool,
        isrc,
        "flac",
        Some(flac),
        Some("/x/320.mp3"),
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn analysis_stores_essentia_metadata() {
    let url = spawn_analyzer().await;
    let h = harness_with(Some(url)).await;
    seed_ready_flac(&h, "USQX91201487", "/data/flac/USQX91201487.flac").await;

    let track = db::get_track(&h.state.pool, "USQX91201487")
        .await
        .unwrap()
        .unwrap();
    let stored = music_api::worker::analyze_track(&h.state, &track)
        .await
        .unwrap();
    assert!(stored, "analyzer returned fields, so a row must be written");

    let m = db::get_metadata(&h.state.pool, "USQX91201487")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.bpm, Some(123.0));
    assert_eq!(m.musical_key.as_deref(), Some("8A"));
    assert_eq!(m.camelot.as_deref(), Some("8A"));
    assert_eq!(m.source.as_deref(), Some("essentia"));
    assert_eq!(
        m.key.as_deref(),
        Some("8A"),
        "key alias mirrors musical_key"
    );

    // Idempotent: the analysed track is no longer a candidate.
    let left = db::ready_flac_unaanalyzed(&h.state.pool, 10).await.unwrap();
    assert!(
        left.is_empty(),
        "analysed track must drop out of the work set"
    );
}

#[tokio::test]
async fn analysis_disabled_without_url() {
    let h = harness().await; // analyzer_url = None
    seed_ready_flac(&h, "USQX91201487", "/data/flac/USQX91201487.flac").await;
    let track = db::get_track(&h.state.pool, "USQX91201487")
        .await
        .unwrap()
        .unwrap();
    let stored = music_api::worker::analyze_track(&h.state, &track)
        .await
        .unwrap();
    assert!(!stored);
    assert!(db::get_metadata(&h.state.pool, "USQX91201487")
        .await
        .unwrap()
        .is_none());
    // Still a candidate (analysis was off, not done).
    let left = db::ready_flac_unaanalyzed(&h.state.pool, 10).await.unwrap();
    assert_eq!(left.len(), 1);
}

#[tokio::test]
async fn analysis_result_is_exposed_via_http() {
    // End-to-end through `GET /metadata/{isrc}`: the analyzer result lands in the
    // store and comes back over the wire (incl. the `key` alias).
    let url = spawn_analyzer().await;
    let h = harness_with(Some(url)).await;
    seed_ready_flac(&h, "USQX91201487", "/data/flac/USQX91201487.flac").await;
    let track = db::get_track(&h.state.pool, "USQX91201487")
        .await
        .unwrap()
        .unwrap();
    music_api::worker::analyze_track(&h.state, &track)
        .await
        .unwrap();

    let (status, body) = send(
        &h.app,
        authed("GET", "/metadata/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["bpm"], 123.0);
    assert_eq!(v["musicalKey"], "8A");
    assert_eq!(v["key"], "8A");
    assert_eq!(v["camelot"], "8A");
    assert_eq!(v["source"], "essentia");
}

// ── Data Vault 2.0 (hubs/links/satellites + precedence resolver) ──────────────

async fn count(h: &Harness, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(&h.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn vault_resolves_bpm_by_precedence() {
    let h = harness().await;

    // Two analyzers disagree on the BPM; traktor is first in the precedence list.
    let (status, _) = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [
                    { "isrc": "USQX91201487", "bpm": 124.0, "source": "essentia" },
                    { "isrc": "USQX91201487", "bpm": 128.0, "key": "8A", "camelot": "8A", "source": "traktor" }
                ]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &h.app,
        authed("GET", "/vault/track/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["isrc"], "USQX91201487");
    assert_eq!(v["resolved"]["bpm"], 128.0, "traktor outranks essentia");
    assert_eq!(v["resolved"]["key"], "8A");
    assert_eq!(v["resolved"]["camelot"], "8A");
    assert_eq!(
        v["resolved"]["source"], "traktor",
        "source = analyzer of the chosen BPM"
    );

    // satellites.analysis is transparent: every analyzer, newest per analyzer.
    let analysis = v["satellites"]["analysis"].as_array().unwrap();
    assert_eq!(analysis.len(), 2, "essentia + traktor");
    let bpm_of = |name: &str| -> f64 {
        analysis
            .iter()
            .find(|a| a["analyzer"] == name)
            .unwrap_or_else(|| panic!("analyzer {name} missing"))["bpm"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(bpm_of("traktor"), 128.0);
    assert_eq!(bpm_of("essentia"), 124.0);

    // Frozen top-level shape: exactly isrc/hub/resolved/links/satellites.
    let obj = v.as_object().unwrap();
    assert_eq!(obj.len(), 5, "unexpected fields: {obj:?}");
}

#[tokio::test]
async fn vault_tracks_batch_omits_unknown() {
    let h = harness().await;
    let _ = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{ "isrc": "USQX91201487", "bpm": 124.0, "source": "essentia" }]
            })))
            .unwrap(),
    )
    .await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/vault/tracks?isrcs=USQX91201487,NOTFOUND123")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let tracks = v["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1, "unknown ISRCs are omitted");
    assert_eq!(tracks[0]["isrc"], "USQX91201487");
    assert_eq!(tracks[0]["bpm"], 124.0);
}

#[tokio::test]
async fn vault_track_unknown_is_404() {
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        authed("GET", "/vault/track/NOPE")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn vault_analysis_worker_writes_satellite() {
    let url = spawn_analyzer().await;
    let h = harness_with(Some(url)).await;
    seed_ready_flac(&h, "USQX91201487", "/data/flac/USQX91201487.flac").await;
    let track = db::get_track(&h.state.pool, "USQX91201487")
        .await
        .unwrap()
        .unwrap();
    music_api::worker::analyze_track(&h.state, &track)
        .await
        .unwrap();

    // The analysis lands in the vault satellite, keyed by analyzer='essentia'.
    assert_eq!(
        count(
            &h,
            "SELECT COUNT(*) FROM sat_track_analysis WHERE analyzer = 'essentia'"
        )
        .await,
        1
    );

    let (status, body) = send(
        &h.app,
        authed("GET", "/vault/track/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["resolved"]["bpm"], 123.0);
    assert_eq!(v["resolved"]["key"], "8A");
    assert_eq!(v["resolved"]["source"], "essentia");
    assert_eq!(v["satellites"]["analysis"][0]["analyzer"], "essentia");
}

#[tokio::test]
async fn vault_ingest_writes_hubs_links_and_satellites() {
    let h = harness().await;
    let mut conn = h.state.pool.acquire().await.unwrap();
    music_api::vault::ingest_deezer(
        &mut conn,
        "USQX91201487",
        "836932812",
        "Sudno",
        "Molchat Doma",
        "Etazhi",
        Some("1234"),
        Some("5678"),
    )
    .await
    .unwrap();
    drop(conn);

    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_track").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_artist").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_album").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM link_track_artist").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM link_track_album").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM sat_track_meta").await, 1);
    assert_eq!(
        count(
            &h,
            "SELECT COUNT(*) FROM sat_artist_extid WHERE service = 'deezer'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &h,
            "SELECT COUNT(*) FROM sat_album_extid WHERE service = 'deezer'"
        )
        .await,
        1
    );

    // Idempotent: re-ingesting the same track writes nothing new.
    let mut conn = h.state.pool.acquire().await.unwrap();
    music_api::vault::ingest_deezer(
        &mut conn,
        "USQX91201487",
        "836932812",
        "Sudno",
        "Molchat Doma",
        "Etazhi",
        Some("1234"),
        Some("5678"),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_track").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM sat_track_meta").await, 1);
}

#[tokio::test]
async fn vault_track_detail_is_transparent_and_complete() {
    let h = harness().await;

    // Resolve path: hubs + links + extids + the metadata satellite (source deezer).
    let mut conn = h.state.pool.acquire().await.unwrap();
    music_api::vault::ingest_deezer(
        &mut conn,
        "USQX91201487",
        "836932812",
        "Sudno",
        "Molchat Doma",
        "Etazhi",
        Some("1234"),
        Some("5678"),
    )
    .await
    .unwrap();
    // Download-state snapshot (ready, all three formats).
    music_api::vault::load_download(
        &mut conn,
        "USQX91201487",
        "ready",
        Some("flac"),
        "flac,320,128",
        "hub",
    )
    .await
    .unwrap();
    drop(conn);

    // Two analyzers land in the analysis satellite.
    let (status, _) = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [
                    { "isrc": "USQX91201487", "bpm": 124.0, "key": "8A", "camelot": "8A", "source": "traktor" },
                    { "isrc": "USQX91201487", "bpm": 123.0, "source": "essentia" }
                ]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &h.app,
        authed("GET", "/vault/track/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Frozen top-level shape.
    assert_eq!(v["isrc"], "USQX91201487");
    assert_eq!(v.as_object().unwrap().len(), 5);

    // hub ← the newest sat_track_meta.
    assert_eq!(v["hub"]["title"], "Sudno");

    // resolved ← precedence (traktor outranks essentia).
    assert_eq!(v["resolved"]["bpm"], 124.0);
    assert_eq!(v["resolved"]["key"], "8A");
    assert_eq!(v["resolved"]["source"], "traktor");

    // links ← link_track_artist / link_track_album + *_extid satellites. The
    // hubs' business keys are normalised, so names come back uppercased.
    assert_eq!(v["links"]["artists"][0]["name"], "MOLCHAT DOMA");
    assert_eq!(v["links"]["artists"][0]["externalIds"]["deezer"], "1234");
    assert_eq!(v["links"]["album"]["title"], "ETAZHI");
    assert_eq!(v["links"]["album"]["externalIds"]["deezer"], "5678");

    // satellites ← transparent: every analyzer, newest per analyzer.
    let analysis = v["satellites"]["analysis"].as_array().unwrap();
    assert_eq!(analysis.len(), 2, "traktor + essentia");
    assert!(analysis.iter().any(|a| a["analyzer"] == "traktor"));
    assert!(analysis.iter().any(|a| a["analyzer"] == "essentia"));

    // satellites.meta ← the deezer source (the pushes above carried no meta).
    let meta = v["satellites"]["meta"].as_array().unwrap();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0]["recordSource"], "deezer");
    assert_eq!(meta[0]["title"], "Sudno");
    assert_eq!(meta[0]["artist"], "Molchat Doma");

    // satellites.download ← the newest snapshot, formats as a list.
    assert_eq!(v["satellites"]["download"]["state"], "ready");
    assert_eq!(v["satellites"]["download"]["sourceFormat"], "flac");
    assert_eq!(
        v["satellites"]["download"]["formats"],
        serde_json::json!(["flac", "320", "128"])
    );
}

#[tokio::test]
async fn vault_backfill_is_idempotent_and_batched() {
    let h = harness().await;

    // Seed operational rows: two ready tracks + one metadata row.
    db::create_order(
        &h.state.pool,
        "o1",
        &["USQX91201487".to_string(), "DEQ022280682".to_string()],
        0,
        "library",
        None,
    )
    .await
    .unwrap();
    db::mark_resolved(
        &h.state.pool,
        "USQX91201487",
        "836932812",
        "Sudno",
        "Molchat Doma",
        "Etazhi",
    )
    .await
    .unwrap();
    db::mark_ready(
        &h.state.pool,
        "USQX91201487",
        "flac",
        Some("/data/flac/USQX91201487.flac"),
        Some("/data/320/USQX91201487.mp3"),
        None,
    )
    .await
    .unwrap();
    db::upsert_metadata(
        &h.state.pool,
        &[music_api::models::MetadataItem {
            isrc: "USQX91201487".to_string(),
            bpm: Some(124.0),
            key: Some("8A".to_string()),
            camelot: Some("8A".to_string()),
            title: None,
            artist: None,
            album: None,
            genre: None,
            source: Some("traktor".to_string()),
        }],
    )
    .await
    .unwrap();

    // Batch size 1 forces multiple batches over the two tracks.
    let first = music_api::vault::backfill(&h.state.pool, 1).await.unwrap();
    assert_eq!(first.tracks, 2);
    assert!(first.batches >= 2, "batched: {}", first.batches);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_track").await, 2);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM sat_track_meta").await, 2);
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM sat_track_download").await,
        2
    );
    assert_eq!(
        count(
            &h,
            "SELECT COUNT(*) FROM sat_track_analysis WHERE analyzer = 'traktor'"
        )
        .await,
        1
    );

    // Re-running is a no-op (idempotent).
    let second = music_api::vault::backfill(&h.state.pool, 1).await.unwrap();
    assert_eq!(second.tracks, 2);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_track").await, 2);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM sat_track_meta").await, 2);
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM sat_track_download").await,
        2
    );
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM sat_track_analysis").await,
        1
    );

    // The backfilled analysis is visible through the resolver.
    let (status, body) = send(
        &h.app,
        authed("GET", "/vault/track/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["resolved"]["bpm"], 124.0);
    assert_eq!(v["resolved"]["source"], "traktor");
}

// ── Artist orders removed (#518): the routes must be gone ────────────────────

#[tokio::test]
async fn artist_order_routes_are_gone() {
    let h = harness().await;
    // No crawl/artist-order worker exists any more — the endpoints are gone.
    let (status, _) = send(
        &h.app,
        authed("POST", "/artist-orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "artist": "12345" })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &h.app,
        authed("GET", "/artist-orders/whatever")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ingest_file_tags_parses_ffprobe_fixture_into_vault() {
    let h = harness().await;

    // A realistic `ffprobe -show_format -show_streams -print_format json` body.
    let fixture = r#"{
        "streams": [{ "codec_type": "audio", "codec_name": "flac" }],
        "format": {
            "format_name": "flac",
            "duration": "343.414000",
            "tags": {
                "ISRC": "GBKQU2538819",
                "TITLE": "Sudno",
                "ARTIST": "Molchat Doma",
                "ALBUM": "Etazhi"
            }
        }
    }"#;
    let tags = music_api::tags::parse_ffprobe(fixture);
    assert_eq!(tags.isrc.as_deref(), Some("GBKQU2538819"));
    assert_eq!(tags.duration_ms, Some(343_414));

    // The ingest writes the hubs, links and the metadata satellite (with duration).
    let mut conn = h.state.pool.acquire().await.unwrap();
    let track_hk = music_api::vault::ingest_file_tags(
        &mut conn,
        tags.isrc.as_deref().unwrap(),
        tags.title.as_deref(),
        tags.artist.as_deref(),
        tags.album.as_deref(),
        tags.duration_ms,
        "deezer",
    )
    .await
    .unwrap()
    .expect("an ISRC is present, so a track hub is written");
    drop(conn);
    assert!(!track_hk.is_empty());

    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_track").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_artist").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM hub_album").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM link_track_artist").await, 1);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM link_track_album").await, 1);
    let duration: Option<i64> =
        sqlx::query_scalar("SELECT duration_ms FROM sat_track_meta WHERE track_hk = ?")
            .bind(&track_hk)
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(duration, Some(343_414));

    // The operational ledger row is `ready` with the FLAC path, so the analysis
    // worker picks it up.
    db::ingest_ready_track(
        &h.state.pool,
        "GBKQU2538819",
        "flac",
        Some("/data/flac/GBKQU2538819.flac"),
        None,
        None,
    )
    .await
    .unwrap();
    let track = db::get_track(&h.state.pool, "GBKQU2538819")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(track.state, "ready");
    assert_eq!(
        track.path_flac.as_deref(),
        Some("/data/flac/GBKQU2538819.flac")
    );
    assert_eq!(
        db::ready_flac_unaanalyzed(&h.state.pool, 10)
            .await
            .unwrap()
            .len(),
        1,
        "the ingested track is an analysis candidate"
    );

    // A file without an ISRC is a best-effort skip, not an error.
    let mut conn = h.state.pool.acquire().await.unwrap();
    let none = music_api::vault::ingest_file_tags(
        &mut conn,
        "",
        Some("Untitled"),
        Some("Someone"),
        None,
        None,
        "deezer",
    )
    .await
    .unwrap();
    assert!(none.is_none());
}

// ── unified work-queue ───────────────────────────────────────────────────────

fn post_work(body: serde_json::Value) -> Request<Body> {
    authed("POST", "/work")
        .header(header::CONTENT_TYPE, "application/json")
        .body(json_body(body))
        .unwrap()
}

#[tokio::test]
async fn work_enqueue_and_overview_round_trip() {
    let h = harness().await;

    let (status, body) = send(
        &h.app,
        post_work(serde_json::json!({
            "kind": "analyze",
            "ref": "10725209",
            "priority": 7,
            "recordSource": "hub"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["kind"], "analyze");
    assert_eq!(v["ref"], "10725209");
    assert_eq!(v["priority"], 7);

    let (status, body) = send(
        &h.app,
        authed("GET", "/work?kind=analyze")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["state"], "queued");
    assert_eq!(v["items"][0]["recordSource"], "hub");
    assert_eq!(v["counts"][0]["kind"], "analyze");
    assert_eq!(v["counts"][0]["state"], "queued");
    assert_eq!(v["counts"][0]["count"], 1);
}

#[tokio::test]
async fn work_enqueue_dedupes_and_raises_priority() {
    let h = harness().await;

    let _ = send(
        &h.app,
        post_work(serde_json::json!({"kind": "analyze", "ref": "X1", "priority": 3})),
    )
    .await;
    // A lower re-touch must not lower the stored priority...
    let _ = send(
        &h.app,
        post_work(serde_json::json!({"kind": "analyze", "ref": "X1", "priority": 1})),
    )
    .await;
    let items = music_api::work::items(&h.state.pool, Some("analyze"), 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 1, "same (kind, ref) must not duplicate");
    assert_eq!(items[0].priority, 3, "priority never lowers");

    // ...but a higher one raises it.
    let _ = send(
        &h.app,
        post_work(serde_json::json!({"kind": "analyze", "ref": "X1", "priority": 100})),
    )
    .await;
    let items = music_api::work::items(&h.state.pool, Some("analyze"), 10)
        .await
        .unwrap();
    assert_eq!(items[0].priority, 100);
}

#[tokio::test]
async fn work_next_is_priority_ordered_and_claim_is_exclusive() {
    let h = harness().await;
    let pool = &h.state.pool;
    music_api::work::enqueue(pool, "download", "LOW", 0, None)
        .await
        .unwrap();
    music_api::work::enqueue(pool, "download", "HIGH", 50, None)
        .await
        .unwrap();

    let now = music_api::db::now();
    let next = music_api::work::next(pool, "download", 1, now)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(
        next[0].reference, "HIGH",
        "`next` serves the highest priority first"
    );

    // Claim is exclusive: the same item cannot be claimed twice.
    let id = next[0].id;
    assert!(music_api::work::claim(pool, id, now).await.unwrap());
    assert!(!music_api::work::claim(pool, id, now).await.unwrap());
    assert_eq!(
        music_api::work::count_running(pool, "download")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        music_api::work::free_slots(pool, "download", 5)
            .await
            .unwrap(),
        4
    );
}

#[tokio::test]
async fn work_retry_backs_off_then_fails_out() {
    let h = harness().await;
    let pool = &h.state.pool;
    music_api::work::enqueue(pool, "download", "FLAKY", 0, None)
        .await
        .unwrap();
    let now = music_api::db::now();
    let item = music_api::work::next(pool, "download", 1, now)
        .await
        .unwrap()
        .remove(0);

    for i in 0..item.max_attempts {
        assert!(
            music_api::work::claim(pool, item.id, now).await.unwrap(),
            "attempt {i} claims"
        );
        let retryable = music_api::work::retry_later(pool, item.id, "boom", now + 60, now)
            .await
            .unwrap();
        if i + 1 < item.max_attempts {
            assert!(retryable, "still retryable after attempt {}", i + 1);
            // Backed off: not due again yet.
            assert!(music_api::work::next(pool, "download", 1, now)
                .await
                .unwrap()
                .is_empty());
        } else {
            assert!(!retryable, "the final attempt fails the item out");
        }
    }

    let counts = music_api::work::counts(pool).await.unwrap();
    assert!(counts
        .iter()
        .any(|c| c.kind == "download" && c.state == "failed" && c.count == 1));
}

#[tokio::test]
async fn work_recover_stale_requeues_running_items() {
    let h = harness().await;
    let pool = &h.state.pool;
    music_api::work::enqueue(pool, "analyze", "STUCK", 0, None)
        .await
        .unwrap();
    let now = music_api::db::now();
    let item = music_api::work::next(pool, "analyze", 1, now)
        .await
        .unwrap()
        .remove(0);
    assert!(music_api::work::claim(pool, item.id, now).await.unwrap());

    // Not stale yet — left alone.
    assert_eq!(
        music_api::work::recover_stale(pool, now, 1800)
            .await
            .unwrap(),
        0
    );
    // Far in the future, it is recovered.
    assert_eq!(
        music_api::work::recover_stale(pool, now + 3600, 1800)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        music_api::work::count_running(pool, "analyze")
            .await
            .unwrap(),
        0
    );
}

// ── analyze runs through the queue ───────────────────────────────────

#[tokio::test]
async fn analyze_kind_is_enqueued_and_claimed() {
    let url = spawn_analyzer().await;
    let h = harness_with(Some(url)).await;
    seed_ready_flac(&h, "USQX91201487", "/data/flac/USQX91201487.flac").await;

    // The reconciler seeds the item and the tick claims it.
    music_api::worker::advance_analyze(&h.state).await.unwrap();

    let items = music_api::work::items(&h.state.pool, Some("analyze"), 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 1, "a ready FLAC becomes one analyze item");
    assert_eq!(items[0].reference, "USQX91201487");
}

// ── transcode runs through the queue ─────────────────────────────────

/// Generate a short real FLAC with ffmpeg. `None` when ffmpeg is unavailable.
fn make_flac(path: &Path) -> Option<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.5",
            "-c:a",
            "flac",
        ])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok.then_some(())
}

#[tokio::test]
async fn transcode_kind_derives_formats_from_flac() {
    let h = harness().await;
    let flac = h.state.config.flac_dir().join("TRANSCODE1.flac");
    if make_flac(&flac).is_none() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }

    db::create_order(&h.state.pool, "o1", &["TRANSCODE1".to_string()], 0, "library", None)
        .await
        .unwrap();
    db::mark_ready(
        &h.state.pool,
        "TRANSCODE1",
        "flac",
        Some(&flac.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();

    music_api::worker::transcode_item(&h.state, "TRANSCODE1")
        .await
        .unwrap();

    let t = db::get_track(&h.state.pool, "TRANSCODE1")
        .await
        .unwrap()
        .unwrap();
    assert!(t.path_320.is_some(), "320 derived from the FLAC");
    assert!(t.path_128.is_some(), "128 derived from the FLAC");
    assert!(Path::new(t.path_320.as_deref().unwrap()).exists());
    assert!(Path::new(t.path_128.as_deref().unwrap()).exists());
    assert_eq!(t.formats(), vec!["flac", "320", "128"]);

    // Idempotent: a second run leaves the paths untouched.
    music_api::worker::transcode_item(&h.state, "TRANSCODE1")
        .await
        .unwrap();
    let t2 = db::get_track(&h.state.pool, "TRANSCODE1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t2.path_320, t.path_320);
}

// ── download runs through the queue ─────────────────────────────────

#[tokio::test]
async fn download_kind_tracks_the_tracks_ledger() {
    let h = harness().await;
    // An order materialises `download` items carrying the order priority.
    create_order_with_priority(&h.app, &["DLHIGH", "DLLOW"], 42).await;
    let items = music_api::work::items(&h.state.pool, Some("download"), 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .all(|i| i.state == "queued" && i.priority == 42));

    // Dispatching (mark_downloading) moves the item to `running`...
    db::mark_downloading(&h.state.pool, "DLHIGH", "uuid", db::now() + 60)
        .await
        .unwrap();
    let queued = music_api::work::next(&h.state.pool, "download", 5, db::now())
        .await
        .unwrap();
    assert!(
        queued.iter().all(|i| i.reference != "DLHIGH"),
        "the in-flight item left the queue"
    );
    assert_eq!(
        music_api::work::count_running(&h.state.pool, "download")
            .await
            .unwrap(),
        1
    );

    // ...and completing the download (mark_ready) finishes it.
    db::mark_ready(
        &h.state.pool,
        "DLHIGH",
        "mp3-320",
        None,
        Some("/x.mp3"),
        None,
    )
    .await
    .unwrap();
    let items = music_api::work::items(&h.state.pool, Some("download"), 10)
        .await
        .unwrap();
    let hi = items.iter().find(|i| i.reference == "DLHIGH").unwrap();
    assert_eq!(hi.state, "done");
}

// ── Backup-store as permanent source: variants + fallback (#359) ─────────────

/// Upload `bytes` under its own digest with the given extra headers.
async fn put_object_h(app: &Router, bytes: &[u8], headers: &[(&str, &str)]) -> String {
    let hash = sha256_hex(bytes);
    let mut b = authed("PUT", &format!("/objects/{hash}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let (status, _) = send(app, b.body(Body::from(bytes.to_vec())).unwrap()).await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::OK,
        "unexpected PUT status: {status}"
    );
    hash
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let (status, body) = send(app, authed("GET", uri).body(Body::empty()).unwrap()).await;
    let v = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn object_list_exposes_isrc_group_and_stem() {
    let h = harness().await;
    let hash = put_object_h(
        &h.app,
        b"listing-object",
        &[
            (header::CONTENT_TYPE.as_str(), "audio/flac"),
            ("x-isrc", "AEA0D1846146"),
            ("x-group-key", "grp-1"),
            ("x-stem-type", "vocals"),
            ("x-name", "vocals.flac"),
        ],
    )
    .await;

    let (status, v) = get_json(&h.app, "/objects?limit=1000").await;
    assert_eq!(status, StatusCode::OK);
    let obj = v["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["hash"] == serde_json::json!(hash))
        .expect("uploaded object listed");
    assert_eq!(obj["isrc"], "AEA0D1846146");
    assert_eq!(obj["groupKey"], "grp-1");
    assert_eq!(obj["stemType"], "vocals");
    // Existing shape is preserved.
    assert!(obj["size"].is_number());
    assert!(v["total"].is_number());
}

#[tokio::test]
async fn object_put_persists_stem_headers_and_coalesces() {
    let h = harness().await;
    let bytes = b"coalesce-object";
    let hash = put_object_h(
        &h.app,
        bytes,
        &[
            (header::CONTENT_TYPE.as_str(), "audio/flac"),
            ("x-isrc", "AEA0D1846146"),
            ("x-group-key", "grp-9"),
            ("x-stem-type", "bass"),
        ],
    )
    .await;

    // A bare re-upload (object already present) must NOT wipe the columns.
    let (status, _) = send(
        &h.app,
        authed("PUT", &format!("/objects/{hash}"))
            .body(Body::from(bytes.to_vec()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, v) = get_json(&h.app, "/objects?limit=1000").await;
    let obj = v["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["hash"] == serde_json::json!(hash))
        .unwrap();
    assert_eq!(obj["groupKey"], "grp-9");
    assert_eq!(obj["stemType"], "bass");
}

#[tokio::test]
async fn isrc_objects_lists_variants_and_stem_groups() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    put_object_h(
        &h.app,
        b"track-flac",
        &[("x-isrc", "AEA0D1846146"), ("x-name", "AEA0D1846146.flac")],
    )
    .await;
    put_object_h(
        &h.app,
        b"track-mp3",
        &[("x-isrc", "AEA0D1846146"), ("x-name", "AEA0D1846146.mp3")],
    )
    .await;
    put_object_h(
        &h.app,
        b"stem-vocals",
        &[
            ("x-isrc", "AEA0D1846146"),
            ("x-group-key", "grp-1"),
            ("x-stem-type", "vocals"),
            ("x-name", "vocals.flac"),
        ],
    )
    .await;

    let (status, v) = get_json(&h.app, "/isrc/aea0d1846146/objects").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["isrc"], "AEA0D1846146");
    assert_eq!(v["count"], 3);
    let kinds: Vec<&str> = v["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"flac"));
    assert!(kinds.contains(&"mp3"));
    assert!(kinds.contains(&"stem"));

    let groups = v["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["groupKey"], "grp-1");
    let stems = groups[0]["stems"].as_array().unwrap();
    assert_eq!(stems.len(), 1);
    assert_eq!(stems[0]["stemType"], "vocals");
    assert_eq!(stems[0]["kind"], "stem");
}

#[tokio::test]
async fn isrc_objects_unknown_is_404_known_empty_is_200() {
    let h = harness().await;

    // Entirely unknown → 404.
    let (status, _) = send(
        &h.app,
        authed("GET", "/isrc/ZZZZZZZZZZZZ/objects")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Known track but no objects → 200 with count 0.
    create_order(&h.app, &["AEA0D1846146"]).await;
    let (status, v) = get_json(&h.app, "/isrc/AEA0D1846146/objects").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["count"], 0);
    assert_eq!(v["variants"].as_array().unwrap().len(), 0);

    // Auth still required.
    let (_status, _) = send(
        &h.app,
        Request::builder()
            .method("GET")
            .uri("/isrc/AEA0D1846146/objects")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/isrc/AEA0D1846146/objects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_file_falls_back_to_the_store() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    let bytes = b"STORE-FLAC-BYTES".to_vec();
    put_object_h(
        &h.app,
        &bytes,
        &[
            (header::CONTENT_TYPE.as_str(), "audio/flac"),
            ("x-isrc", "AEA0D1846146"),
            ("x-name", "AEA0D1846146.flac"),
        ],
    )
    .await;

    // No local path recorded → served from the store.
    let resp = h
        .app
        .clone()
        .oneshot(
            authed("GET", "/isrc/AEA0D1846146/flac")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-source"], "store");
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "audio/flac");
    let got = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert_eq!(got, bytes);

    // The store also answers range requests.
    let resp = h
        .app
        .clone()
        .oneshot(
            authed("GET", "/isrc/AEA0D1846146/flac")
                .header(header::RANGE, "bytes=0-4")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers()["x-source"], "store");
}

#[tokio::test]
async fn get_file_prefers_local_over_store() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    // A local file is on disk AND a store object exists: local wins.
    let mp3_320 = write_file(
        &h.state.config.mp3_320_dir(),
        "AEA0D1846146.mp3",
        b"LOCAL-MP3",
    );
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
    put_object_h(
        &h.app,
        b"STORE-MP3",
        &[
            (header::CONTENT_TYPE.as_str(), "audio/mpeg"),
            ("x-isrc", "AEA0D1846146"),
            ("x-name", "AEA0D1846146.mp3"),
        ],
    )
    .await;

    let resp = h
        .app
        .clone()
        .oneshot(
            authed("GET", "/isrc/AEA0D1846146/320")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-source"], "local");
    let got = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert_eq!(got, b"LOCAL-MP3");
}

#[tokio::test]
async fn get_file_is_404_when_local_and_store_both_miss() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    let (status, _) = send(
        &h.app,
        authed("GET", "/isrc/AEA0D1846146/flac")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unknown format is still a 400.
    let (status, _) = send(
        &h.app,
        authed("GET", "/isrc/AEA0D1846146/ogg")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tracks_objects_include_kind_and_stem_type() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    put_object_h(
        &h.app,
        b"tracks-flac",
        &[("x-isrc", "AEA0D1846146"), ("x-name", "AEA0D1846146.flac")],
    )
    .await;
    put_object_h(
        &h.app,
        b"tracks-stem",
        &[
            ("x-isrc", "AEA0D1846146"),
            ("x-group-key", "grp-7"),
            ("x-stem-type", "drums"),
            ("x-name", "drums.flac"),
        ],
    )
    .await;

    let (status, v) = get_json(&h.app, "/tracks?isrcs=AEA0D1846146").await;
    assert_eq!(status, StatusCode::OK);
    let track = &v["tracks"][0];
    let objects = track["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 2);
    assert!(objects
        .iter()
        .all(|o| o["hash"].is_string() && o["size"].is_number()));
    let stem = objects
        .iter()
        .find(|o| o["kind"] == serde_json::json!("stem"))
        .expect("stem object present");
    assert_eq!(stem["stemType"], "drums");
    let flac = objects
        .iter()
        .find(|o| o["kind"] == serde_json::json!("flac"))
        .expect("flac object present");
    assert!(flac["stemType"].is_null());
}

#[tokio::test]
async fn get_file_falls_back_when_local_file_is_gone() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    // A local path is recorded, but the file is gone from disk.
    let local = write_file(
        &h.state.config.mp3_320_dir(),
        "AEA0D1846146.mp3",
        b"LOCAL-MP3",
    );
    db::mark_ready(
        &h.state.pool,
        "AEA0D1846146",
        "mp3",
        None,
        Some(&local.display().to_string()),
        None,
    )
    .await
    .unwrap();
    std::fs::remove_file(&local).unwrap();

    let bytes = b"STORE-MP3-BYTES".to_vec();
    put_object_h(
        &h.app,
        &bytes,
        &[
            (header::CONTENT_TYPE.as_str(), "audio/mpeg"),
            ("x-isrc", "AEA0D1846146"),
            ("x-name", "AEA0D1846146.mp3"),
        ],
    )
    .await;

    let resp = h
        .app
        .clone()
        .oneshot(
            authed("GET", "/isrc/AEA0D1846146/320")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-source"], "store");
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "audio/mpeg");
    let got = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert_eq!(got, bytes);
}

#[tokio::test]
async fn get_file_store_fallback_maps_both_mp3_formats() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    put_object_h(
        &h.app,
        b"STORE-MP3",
        &[
            (header::CONTENT_TYPE.as_str(), "audio/mpeg"),
            ("x-isrc", "AEA0D1846146"),
            ("x-name", "AEA0D1846146.mp3"),
        ],
    )
    .await;

    // 320 and 128 both map to the store's mp3 object (no bitrate in the store).
    for format in ["320", "128"] {
        let resp = h
            .app
            .clone()
            .oneshot(
                authed("GET", &format!("/isrc/AEA0D1846146/{format}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "format {format}");
        assert_eq!(resp.headers()["x-source"], "store", "format {format}");
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "audio/mpeg");
    }
}

#[tokio::test]
async fn object_put_fills_null_stem_columns_on_later_upload() {
    let h = harness().await;
    let bytes = b"late-stem-object";

    // First upload carries no stem metadata → columns stay NULL.
    let hash = put_object_h(&h.app, bytes, &[]).await;

    // A later upload of the same object supplies the headers: COALESCE fills
    // the previously NULL columns.
    let hash2 = put_object_h(
        &h.app,
        bytes,
        &[("x-group-key", "grp-late"), ("x-stem-type", "keys")],
    )
    .await;
    assert_eq!(hash, hash2);

    let (_, v) = get_json(&h.app, "/objects?limit=1000").await;
    let obj = v["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["hash"] == serde_json::json!(hash))
        .unwrap();
    assert_eq!(obj["groupKey"], "grp-late");
    assert_eq!(obj["stemType"], "keys");
}

#[tokio::test]
async fn isrc_objects_links_stems_by_group_key_without_isrc() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;

    // Primary object anchors the group_key; the stem carries only the group_key
    // (no isrc) — linkage is best-effort over group_key.
    put_object_h(
        &h.app,
        b"anchor-flac",
        &[
            ("x-isrc", "AEA0D1846146"),
            ("x-group-key", "grp-5"),
            ("x-name", "AEA0D1846146.flac"),
        ],
    )
    .await;
    put_object_h(
        &h.app,
        b"stem-no-isrc",
        &[
            ("x-group-key", "grp-5"),
            ("x-stem-type", "vocals"),
            ("x-name", "vocals.flac"),
        ],
    )
    .await;

    let (status, v) = get_json(&h.app, "/isrc/AEA0D1846146/objects").await;
    assert_eq!(status, StatusCode::OK);
    // Only the anchor object is a direct variant.
    assert_eq!(v["count"], 1);
    let groups = v["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["groupKey"], "grp-5");
    let stems = groups[0]["stems"].as_array().unwrap();
    assert_eq!(stems.len(), 1);
    assert_eq!(stems[0]["stemType"], "vocals");
}

#[tokio::test]
async fn object_routes_require_a_token() {
    let h = harness().await;

    // GET /objects
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/objects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // PUT /objects/{hash}
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/objects/deadbeef")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // GET /isrc/{isrc}/{format}
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/isrc/AEA0D1846146/flac")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Order archive: whole-playlist download (#358) ────────────────────────────

/// Parse a ZIP body into entry-name → bytes.
fn read_zip(bytes: &[u8]) -> std::collections::HashMap<String, Vec<u8>> {
    use std::io::Read;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("valid zip");
    let mut out = std::collections::HashMap::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        out.insert(name, buf);
    }
    out
}

async fn get_archive(app: &Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(authed("GET", uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

/// Set artist/title on a track (drives the archive entry-name schema).
async fn set_track_meta(pool: &sqlx::SqlitePool, isrc: &str, artist: &str, title: &str) {
    sqlx::query("UPDATE tracks SET artist = ?, title = ? WHERE isrc = ?")
        .bind(artist)
        .bind(title)
        .bind(isrc)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn archive_contains_all_ready_tracks() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA", "BBBB"]).await;

    let a = write_file(&h.state.config.flac_dir(), "AAAA.flac", b"AAA-FLAC");
    let b = write_file(&h.state.config.flac_dir(), "BBBB.flac", b"BBB-FLAC");
    db::mark_ready(
        &h.state.pool,
        "AAAA",
        "flac",
        Some(&a.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();
    db::mark_ready(
        &h.state.pool,
        "BBBB",
        "flac",
        Some(&b.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/zip");
    let cd = resp.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.starts_with("attachment"), "disposition: {cd}");
    assert!(
        cd.contains(&format!("order-{order}.zip")),
        "disposition: {cd}"
    );
    assert!(resp.headers().contains_key(header::CONTENT_LENGTH));

    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries["AAAA.flac"], b"AAA-FLAC");
    assert_eq!(entries["BBBB.flac"], b"BBB-FLAC");
}

#[tokio::test]
async fn archive_skips_not_ready_tracks() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA", "BBBB"]).await;

    let a = write_file(&h.state.config.flac_dir(), "AAAA.flac", b"AAA-FLAC");
    db::mark_ready(
        &h.state.pool,
        "AAAA",
        "flac",
        Some(&a.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();
    // BBBB stays pending → skipped.

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries["AAAA.flac"], b"AAA-FLAC");
}

#[tokio::test]
async fn archive_unknown_order_is_404() {
    let h = harness().await;
    let resp = get_archive(&h.app, "/orders/does-not-exist/archive").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn archive_without_servable_tracks_is_409() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA"]).await;
    // Track stays pending → nothing servable.

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let v: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(v["error"], "no downloadable tracks");
}

#[tokio::test]
async fn archive_format_param_selects_format() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA"]).await;

    let p = write_file(&h.state.config.mp3_320_dir(), "AAAA.mp3", b"AAA-MP3");
    db::mark_ready(
        &h.state.pool,
        "AAAA",
        "mp3",
        None,
        Some(&p.display().to_string()),
        None,
    )
    .await
    .unwrap();

    // 320 is servable → an .mp3 entry.
    let resp = get_archive(&h.app, &format!("/orders/{order}/archive?format=320")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries["AAAA.mp3"], b"AAA-MP3");

    // flac was never delivered → 409.
    let resp = get_archive(&h.app, &format!("/orders/{order}/archive?format=flac")).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn archive_invalid_format_is_400() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA"]).await;
    let resp = get_archive(&h.app, &format!("/orders/{order}/archive?format=ogg")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn archive_defaults_to_best_available_per_track() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA", "BBBB"]).await;

    // A: flac only. B: 320 only.
    let a = write_file(&h.state.config.flac_dir(), "AAAA.flac", b"A-FLAC");
    let b = write_file(&h.state.config.mp3_320_dir(), "BBBB.mp3", b"B-MP3");
    db::mark_ready(
        &h.state.pool,
        "AAAA",
        "flac",
        Some(&a.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();
    db::mark_ready(
        &h.state.pool,
        "BBBB",
        "mp3",
        None,
        Some(&b.display().to_string()),
        None,
    )
    .await
    .unwrap();

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries["AAAA.flac"], b"A-FLAC");
    assert_eq!(entries["BBBB.mp3"], b"B-MP3");
}

#[tokio::test]
async fn archive_dedupes_identical_filenames() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA", "BBBB"]).await;

    for isrc in ["AAAA", "BBBB"] {
        let p = write_file(
            &h.state.config.flac_dir(),
            &format!("{isrc}.flac"),
            isrc.as_bytes(),
        );
        db::mark_ready(
            &h.state.pool,
            isrc,
            "flac",
            Some(&p.display().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
        set_track_meta(&h.state.pool, isrc, "Same Artist", "Same Title").await;
    }

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries["Same Artist - Same Title.flac"], b"AAAA");
    assert_eq!(entries["Same Artist - Same Title (2).flac"], b"BBBB");
}

#[tokio::test]
async fn archive_falls_back_to_store_object() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA"]).await;

    // No local path recorded; bytes live only in the object store.
    put_object_h(
        &h.app,
        b"STORE-FLAC-BYTES",
        &[
            (header::CONTENT_TYPE.as_str(), "audio/flac"),
            ("x-isrc", "AAAA"),
            ("x-name", "AAAA.flac"),
        ],
    )
    .await;

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries["AAAA.flac"], b"STORE-FLAC-BYTES");
}

#[tokio::test]
async fn archive_requires_a_token() {
    let h = harness().await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/orders/anything/archive")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn archive_filename_sanitizes_key_fallback() {
    let h = harness().await;
    // A composite key (soundcloud:…) is the fallback name: `:` must be sanitized.
    let (status, body) = send(
        &h.app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [ { "source": "soundcloud", "externalId": "1837565340" } ]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let order = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["orderId"]
        .as_str()
        .unwrap()
        .to_string();

    let key = "soundcloud:1837565340";
    let p = write_file(&h.state.config.mp3_320_dir(), "sc.mp3", b"SC-MP3");
    db::mark_ready(
        &h.state.pool,
        key,
        "mp3",
        None,
        Some(&p.display().to_string()),
        None,
    )
    .await
    .unwrap();

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 1);
    let name = entries.keys().next().unwrap();
    assert!(!name.contains('/'), "name has slash: {name}");
    assert!(!name.contains(':'), "name has colon: {name}");
    assert_eq!(name, "soundcloud_1837565340.mp3");
    assert_eq!(entries[name.as_str()], b"SC-MP3");
}

#[tokio::test]
async fn archive_skips_track_with_missing_file() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA", "BBBB"]).await;

    // AAAA is ready with a real file.
    let a = write_file(&h.state.config.flac_dir(), "AAAA.flac", b"AAA-FLAC");
    db::mark_ready(
        &h.state.pool,
        "AAAA",
        "flac",
        Some(&a.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();
    // BBBB is marked ready but its recorded local file is gone (and no store
    // object exists) → not servable, must be skipped rather than erroring.
    let absent = h.state.config.flac_dir().join("BBBB.flac");
    assert!(!absent.exists());
    db::mark_ready(
        &h.state.pool,
        "BBBB",
        "flac",
        Some(&absent.display().to_string()),
        None,
        None,
    )
    .await
    .unwrap();

    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries["AAAA.flac"], b"AAA-FLAC");
}

#[tokio::test]
async fn archive_serves_128_when_only_128_available() {
    let h = harness().await;
    let order = create_order(&h.app, &["AAAA"]).await;

    // Only a 128 asset exists.
    let p = write_file(&h.state.config.mp3_128_dir(), "AAAA.mp3", b"AAA-128");
    db::mark_ready(
        &h.state.pool,
        "AAAA",
        "mp3",
        None,
        None,
        Some(&p.display().to_string()),
    )
    .await
    .unwrap();

    // Best-available default reaches the third tier (128) → an .mp3 entry.
    let resp = get_archive(&h.app, &format!("/orders/{order}/archive")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries["AAAA.mp3"], b"AAA-128");

    // An explicit ?format=128 selects it directly.
    let resp = get_archive(&h.app, &format!("/orders/{order}/archive?format=128")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = read_zip(&body_bytes(resp).await);
    assert_eq!(entries["AAAA.mp3"], b"AAA-128");
}

// ── Deezer account tier (`GET /account`, gw-light mock) ──────────────────────

/// A stand-in for Deezer's gw-light endpoint: answers `deezer.getUserData` with
/// a fixed HiFi payload. No real network — the mock lives on loopback.
async fn gw_light_mock() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "results": {
            "OFFER_NAME": "Deezer HiFi",
            "USER": {
                "USER_ID": 1234567,
                "OPTIONS": {
                    "web_sound_quality": { "lossless": true, "high": true, "low": true }
                }
            }
        }
    }))
}

/// Spawn the mock gw-light on an ephemeral port; returns its base URL.
async fn spawn_gw_light() -> String {
    let app = Router::new().route("/ajax/gw-light.php", get(gw_light_mock));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/ajax/gw-light.php")
}

#[tokio::test]
async fn account_requires_auth() {
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        Request::builder()
            .method("GET")
            .uri("/account")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn account_reports_hi_tier_from_gw_light() {
    let gw = spawn_gw_light().await;
    let h = harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: "fake-arl".to_string(),
        deezer_gw_base: gw,
        native_fake: None,
        deezer_base: None,
    })
    .await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["offer"], "Deezer HiFi");
    assert_eq!(v["lossless"], true);
    assert_eq!(v["high"], true);
    assert_eq!(v["low"], true);
    assert_eq!(v["userId"], 1234567);
    assert_eq!(v["canStreamFlac"], true);

    // Second call is served from the cache (same body).
    let (status2, body2) = send(
        &h.app,
        authed("GET", "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body2, body);
}

#[tokio::test]
async fn account_without_arl_is_conflict() {
    let h = harness().await; // deezer_arl empty
    let (status, body) = send(
        &h.app,
        authed("GET", "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "DEEZER_ARL not configured");
}

/// Mock gw-light answering with a Deezer Free (no lossless) payload.
async fn gw_light_mock_free() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "results": {
            "OFFER_NAME": "Deezer Free",
            "USER": {
                "USER_ID": 7,
                "OPTIONS": {
                    "web_sound_quality": { "lossless": false, "high": false, "low": true }
                }
            }
        }
    }))
}

async fn spawn_gw_light_free() -> String {
    let app = Router::new().route("/ajax/gw-light.php", get(gw_light_mock_free));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/ajax/gw-light.php")
}

#[tokio::test]
async fn account_reports_free_tier_without_flac() {
    let gw = spawn_gw_light_free().await;
    let h = harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: "fake-arl".to_string(),
        deezer_gw_base: gw,
        native_fake: None,
        deezer_base: None,
    })
    .await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["offer"], "Deezer Free");
    assert_eq!(v["lossless"], false);
    assert_eq!(v["high"], false);
    assert_eq!(v["low"], true);
    assert_eq!(v["userId"], 7);
    assert_eq!(v["canStreamFlac"], false);
}

#[tokio::test]
async fn account_returns_bad_gateway_when_gw_light_unreachable() {
    // Dead port: the on-demand gw-light probe fails → 502.
    let h = harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: "fake-arl".to_string(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: None,
        deezer_base: None,
    })
    .await;

    let (status, body) = send(
        &h.app,
        authed("GET", "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("deezer tier lookup failed"),
        "error was {:?}",
        v["error"]
    );
}

// ── Native Deezer download (#510, M2) ─────────────────────────────────────────
//
// The native path is driven end-to-end without any real Deezer/ARL: a loopback
// mock answers the ISRC lookup and a `NativeFake` supplies the audio bytes.

/// Canned Deezer public-API track lookup (no auth, loopback only).
async fn deezer_track_mock() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": 3135556,
        "readable": true,
        "title": "Sudno",
        "artist": { "id": 42, "name": "Molchat Doma" },
        "album": { "id": 7, "title": "Etazhi" }
    }))
}

/// Spawn the mock Deezer API on an ephemeral port; returns its base URL. The
/// lookup hits `/track/isrc:<isrc>` (one path segment), captured as a whole.
async fn spawn_deezer_mock() -> String {
    let app = Router::new().route("/track/{rest}", get(deezer_track_mock));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Fixture bytes that `lofty` can tag (real container).
fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// A harness with the native path wired to a loopback Deezer mock + fake bytes.
async fn harness_native(
    deezer_base: String,
    fake: music_api::worker::NativeFake,
    arl: &str,
) -> Harness {
    harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: arl.to_string(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: Some(Arc::new(fake)),
        deezer_base: Some(deezer_base),
    })
    .await
}

/// Poll until the row leaves the in-flight states *and* its run-log line is in
/// (the log is written after `mark_ready`, so state alone races).
async fn wait_settled(state: &Arc<AppState>, isrc: &str) {
    for _ in 0..500 {
        let t = db::get_track(&state.pool, isrc).await.unwrap().unwrap();
        let logged = state
            .logs
            .recent(50)
            .iter()
            .any(|l| l.msg.starts_with(&format!("{isrc}: ready (native,")));
        if t.state != "pending" && t.state != "downloading" && logged {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {isrc} to settle");
}

/// Read the ISRC back with `lofty` (ffprobe is not guaranteed in CI).
fn read_isrc(path: &str) -> Option<String> {
    music_api::tags::read_isrc(Path::new(path))
}

#[tokio::test]
async fn native_download_lands_ready_with_isrc_and_serves_file() {
    let deezer = spawn_deezer_mock().await;
    let fake = music_api::worker::NativeFake {
        bytes: fixture_bytes("tiny.flac"),
        extension: "flac".to_string(),
        no_flac: false,
    };
    let h = harness_native(deezer, fake, "").await;

    create_order(&h.app, &["USRC17607839"]).await;
    music_api::worker::resolve_pending(&h.state).await.unwrap();
    wait_settled(&h.state, "USRC17607839").await;

    let t = db::get_track(&h.state.pool, "USRC17607839")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "ready", "track did not land ready: {t:?}");
    assert_eq!(t.source_format.as_deref(), Some("flac"));
    assert_eq!(
        t.download_ref.as_deref(),
        Some("native:USRC17607839"),
        "native sentinel missing"
    );

    // The stored file carries the ISRC.
    let path = t.path_flac.as_deref().expect("flac path");
    assert_eq!(read_isrc(path).as_deref(), Some("USRC17607839"));

    // ...and it is servable over the API.
    let (status, body) = send(
        &h.app,
        authed("GET", "/isrc/USRC17607839/flac")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with(b"fLaC"), "not a FLAC body");

    // Run-log line: quality + source, no ARL.
    let logs = h.state.logs.recent(50);
    assert!(
        logs.iter().any(|l| l.msg.contains("ready (native, flac,")),
        "run-log line missing: {:?}",
        logs.iter().map(|l| &l.msg).collect::<Vec<_>>()
    );
    assert!(
        logs.iter()
            .all(|l| !l.msg.contains("DEEZER_ARL=") && !l.msg.contains("DEEMIX_ARL=")),
        "log leaked the ARL"
    );
}

#[tokio::test]
async fn native_download_no_lossless_tier_falls_back_to_320() {
    let deezer = spawn_deezer_mock().await;
    // `no_flac` simulates an account without the lossless tier.
    let fake = music_api::worker::NativeFake {
        bytes: fixture_bytes("tiny.mp3"),
        extension: "mp3".to_string(),
        no_flac: true,
    };
    let h = harness_native(deezer, fake, "").await;

    create_order(&h.app, &["USRC17607839"]).await;
    music_api::worker::resolve_pending(&h.state).await.unwrap();
    wait_settled(&h.state, "USRC17607839").await;

    let t = db::get_track(&h.state.pool, "USRC17607839")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "ready");
    assert_eq!(
        t.source_format.as_deref(),
        Some("mp3-320"),
        "expected the 320 fallback"
    );
    assert!(t.path_320.is_some(), "320 path missing");
    let logs = h.state.logs.recent(50);
    assert!(
        logs.iter()
            .any(|l| l.msg.contains("ready (native, mp3-320,")),
        "fallback run-log line missing"
    );
}

/// A native backend with an empty `DEEMIX_ARL` must warn once, attempt no login
/// (no network) and leave the session unset. The injected login closure records
/// any invocation, so "no attempt" is asserted directly.
#[tokio::test]
async fn native_empty_arl_warns_once_and_never_attempts_login() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let h = harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: String::new(), // no ARL configured
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: None,
        deezer_base: None,
    })
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let login = move |_: String| -> music_api::worker::NativeLoginFuture {
        seen.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(music_api::deezer_dl::Error::InvalidArl) })
    };

    assert!(
        !music_api::worker::ensure_native_login_with(&h.state, login).await,
        "empty ARL must not report the native path ready"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no login attempt expected");
    assert!(
        h.state.native_session.read().await.is_none(),
        "no session may be cached"
    );
    assert!(
        !h.state.native_login.blocked(db::now()),
        "no failure/backoff may be recorded"
    );
    // The warning guard is spent — i.e. exactly one warn was emitted.
    assert!(
        !h.state.native_login.warn_missing_arl(),
        "the missing-ARL warning must fire exactly once"
    );
}

/// A failing native login must engage the backoff and drop any session, so a
/// second call inside the window makes no new attempt. The login closure
/// counts invocations, proving the retry is skipped.
#[tokio::test]
async fn native_login_failure_backs_off_and_discards_session() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let h = harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: "invalid-arl-zzz".to_string(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: None,
        deezer_base: None,
    })
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let login = move |_: String| -> music_api::worker::NativeLoginFuture {
        seen.fetch_add(1, Ordering::SeqCst);
        // InvalidArl is ARL-free by construction; no network is touched.
        Box::pin(async { Err(music_api::deezer_dl::Error::InvalidArl) })
    };

    assert!(!music_api::worker::ensure_native_login_with(&h.state, login.clone()).await);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        h.state.native_session.read().await.is_none(),
        "session must be discarded on failure"
    );
    assert!(
        h.state.native_login.blocked(db::now()),
        "a failure must engage the backoff"
    );

    // Inside the cooldown window the login is not retried.
    assert!(!music_api::worker::ensure_native_login_with(&h.state, login).await);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "login must not be retried inside the backoff window"
    );
}

/// Native (`native:`-sentinel) in-flight rows must consume `MAX_INFLIGHT` slots
/// exactly like sidecar rows — the ledger counts every source.
#[tokio::test]
async fn resolve_pending_counts_native_rows_against_inflight_cap() {
    let h = harness().await; // max_inflight = 5
    let isrcs = [
        "AEA0D1846146",
        "USQX91201487",
        "USUM71807935",
        "GBUM71029604",
        "FRZ039800054",
        "USRC17607839",
    ];
    create_order(&h.app, &isrcs).await;
    for isrc in &isrcs[..5] {
        db::mark_downloading(
            &h.state.pool,
            isrc,
            &format!("native:{isrc}"),
            db::now() + 60,
        )
        .await
        .unwrap();
    }
    assert_eq!(db::count_downloading(&h.state.pool).await.unwrap(), 5);

    // No free slots: the sixth row must stay pending, undispatched.
    music_api::worker::resolve_pending(&h.state).await.unwrap();
    assert_eq!(db::count_downloading(&h.state.pool).await.unwrap(), 5);
    let t = db::get_track(&h.state.pool, isrcs[5])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "pending");
    assert_eq!(t.attempts, 0);
    assert_eq!(t.next_attempt_at, None);
}

/// A row already in flight on a native background task must not be dispatched a
/// second time, even when its queue item is offered again.
#[tokio::test]
async fn native_inflight_row_is_not_double_dispatched() {
    let deezer = spawn_deezer_mock().await;
    let fake = music_api::worker::NativeFake {
        bytes: fixture_bytes("tiny.flac"),
        extension: "flac".to_string(),
        no_flac: false,
    };
    let h = harness_native(deezer, fake, "").await;
    let isrc = "USRC17607839";
    create_order(&h.app, &[isrc]).await;

    // An in-flight native job: `downloading` with its own sentinel.
    db::mark_downloading(
        &h.state.pool,
        isrc,
        &format!("native:{isrc}"),
        db::now() + 60,
    )
    .await
    .unwrap();
    // Re-offer the queue item as the reconcile would on a later tick.
    music_api::work::finish_ref(
        &h.state.pool,
        music_api::work::kind::DOWNLOAD,
        isrc,
        db::now(),
    )
    .await
    .unwrap();
    music_api::work::enqueue(
        &h.state.pool,
        music_api::work::kind::DOWNLOAD,
        isrc,
        0,
        None,
    )
    .await
    .unwrap();

    music_api::worker::resolve_pending(&h.state).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let t = db::get_track(&h.state.pool, isrc).await.unwrap().unwrap();
    assert_eq!(
        t.state, "downloading",
        "native in-flight row must not be settled or restarted"
    );
    let expected_uuid = format!("native:{isrc}");
    assert_eq!(t.download_ref.as_deref(), Some(expected_uuid.as_str()));
    assert_eq!(db::count_downloading(&h.state.pool).await.unwrap(), 1);
    let starts = h
        .state
        .logs
        .recent(50)
        .iter()
        .filter(|l| l.msg.contains("native download started"))
        .count();
    assert_eq!(starts, 0, "no second native download may be started");
}
// ── #518: native-only HTTP contract — no backend/sidecar fields ──────────────

#[tokio::test]
async fn health_has_no_backend_fields() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert!(v.get("backend").is_none(), "backend must be gone: {v}");
    assert!(
        v.get("sidecarConfigured").is_none(),
        "sidecarConfigured must be gone: {v}"
    );
    assert!(v["arlTier"].is_null(), "tier must be null while unknown");
}

/// The cached ARL tier is public (`offer`/`lossless`/`canStreamFlac`) but never
/// the `userId` (that stays on the protected `GET /account`).
#[tokio::test]
async fn health_reports_cached_arl_tier_without_user_id() {
    let h = harness().await;
    *h.state.arl_tier.write().await = Some(music_api::deezer::ArlTier {
        offer: "HiFi".to_string(),
        lossless: true,
        high: true,
        low: true,
        user_id: Some(4242),
    });

    let (status, body) = send(
        &h.app,
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["arlTier"]["offer"], "HiFi");
    assert_eq!(v["arlTier"]["lossless"], true);
    assert_eq!(v["arlTier"]["canStreamFlac"], true);
    assert!(
        v["arlTier"]["userId"].is_null(),
        "userId must never appear in the public health payload"
    );
}

#[tokio::test]
async fn worker_has_no_backend_fields() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        authed("GET", "/worker").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v.get("backend").is_none(), "backend must be gone: {v}");
    assert!(
        v.get("sidecarConfigured").is_none(),
        "sidecarConfigured must be gone: {v}"
    );
    assert!(v["arlTier"].is_null());
}

#[tokio::test]
async fn queue_has_no_backend_fields() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;
    let (status, body) = send(&h.app, authed("GET", "/queue").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v.get("backend").is_none(), "backend must be gone: {v}");
    assert!(v["items"].as_array().unwrap().is_empty());
    assert_eq!(v["counts"]["pending"], 1);
}

/// A native tick must be self-contained: an in-flight native row is left to its
/// own background task and no external queue is consulted (there is none).
#[tokio::test]
async fn native_tick_is_self_contained() {
    let h = harness().await;
    create_order(&h.app, &["AEA0D1846146"]).await;
    db::mark_downloading(
        &h.state.pool,
        "AEA0D1846146",
        "native:AEA0D1846146",
        db::now() + 60,
    )
    .await
    .unwrap();

    music_api::worker::resolve_pending(&h.state).await.unwrap();

    let t = db::get_track(&h.state.pool, "AEA0D1846146")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "downloading");
    assert_eq!(t.download_ref.as_deref(), Some("native:AEA0D1846146"));
}

/// `POST /work {"kind":"crawl:deezer"}` → 409: there is no crawl worker any more.
#[tokio::test]
async fn crawl_work_kind_is_rejected() {
    let h = harness().await;
    let (status, body) = send(
        &h.app,
        post_work(serde_json::json!({"kind": "crawl:deezer", "ref": "10725209"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        v["error"].as_str().unwrap().contains("crawl"),
        "error was {:?}",
        v["error"]
    );
    // Nothing was enqueued.
    let counts = music_api::work::counts(&h.state.pool).await.unwrap();
    assert!(
        counts.iter().all(|c| c.kind != "crawl:deezer"),
        "crawl work must not be enqueued"
    );
}

/// The `tracks.deemix_uuid` → `download_ref` migration is idempotent and
/// preserves existing values; no `DROP TABLE` involved.
#[tokio::test]
async fn migration_renames_deemix_uuid_to_download_ref() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.db");
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();

    // Legacy schema: the identifier column still carries the old name.
    sqlx::query(
        "CREATE TABLE tracks (
            isrc TEXT PRIMARY KEY,
            state TEXT NOT NULL,
            deemix_uuid TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tracks (isrc, state, deemix_uuid, created_at, updated_at)
         VALUES ('ABC', 'ready', 'native:ABC', 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Two runs: the second is a no-op (idempotent).
    music_api::db::init(&pool).await.unwrap();
    music_api::db::init(&pool).await.unwrap();

    let old_cols: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name = 'deemix_uuid'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old_cols, 0, "the old column must be gone");

    let val: Option<String> =
        sqlx::query_scalar("SELECT download_ref FROM tracks WHERE isrc = 'ABC'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        val.as_deref(),
        Some("native:ABC"),
        "values must be preserved"
    );
}

// ── Deezer metadata for the hub (`/deezer/*`, loopback mock) ──────────────────
//
// Same seam as the native-download tests: `deezer_base` points at a loopback
// axum mock, so nothing here touches the real Deezer API.

/// Artist-albums mock. Echoes `limit`/`index` into `next` so the test can prove
/// the paging params actually reached the upstream call.
async fn deezer_artist_albums_mock(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit: usize = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50);
    let index: usize = q.get("index").and_then(|v| v.parse().ok()).unwrap_or(0);
    Json(serde_json::json!({
        "data": [{
            "id": 1137915301,
            "title": "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)",
            "release_date": "2026-11-13",
            "record_type": "album",
            "cover_medium": "https://cdn-images.dzcdn.net/images/cover/aaaa/250x250-000000-80-0-0.jpg",
            "cover_big": "https://cdn-images.dzcdn.net/images/cover/bbbb/500x500-000000-80-0-0.jpg"
        }],
        "total": 417,
        "next": format!(
            "https://api.deezer.com/artist/15167501/albums?limit={limit}&index={}",
            index + limit
        )
    }))
}

/// Album mock. Id `800` answers Deezer's "no data" object on HTTP 200, `999`
/// answers a plain HTTP 404, `500` an upstream server error.
async fn deezer_album_meta_mock(UrlPath(id): UrlPath<String>) -> Response {
    match id.as_str() {
        "800" => Json(serde_json::json!({
            "error": { "type": "DataException", "message": "no data", "code": 800 }
        }))
        .into_response(),
        "999" => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": { "type": "DataException", "message": "no data", "code": 800 }
            })),
        )
            .into_response(),
        "500" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        _ => Json(serde_json::json!({
            "id": 1137915301,
            "title": "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)",
            "release_date": "2026-11-13",
            "record_type": "album",
            "cover_big": "https://cdn-images.dzcdn.net/images/cover/bbbb/500x500-000000-80-0-0.jpg",
            "cover_medium": "https://cdn-images.dzcdn.net/images/cover/aaaa/250x250-000000-80-0-0.jpg",
            "tracks": {
                "data": [
                    { "id": 2, "title": "Kapitel 02: Die Ente fliegt", "duration": 60,
                      "isrc": "CH7812690466", "track_position": 2 },
                    { "id": 1, "title": "Kapitel 01: Die Robo-Ente (Folge 459)", "duration": 98,
                      "isrc": "CH7812690465", "track_position": 1 },
                    { "id": 3, "title": "Kapitel 03: Outro", "duration": 10, "track_position": 3 }
                ]
            }
        }))
        .into_response(),
    }
}

/// Artist-search mock. Echoes the received `q` as the hit's name, which proves
/// the query string was forwarded.
async fn deezer_search_mock(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let term = q.get("q").cloned().unwrap_or_default();
    Json(serde_json::json!({
        "data": [{
            "id": 15167501,
            "name": term,
            "picture_medium": "https://api.deezer.com/artist/15167501/image"
        }],
        "total": 1
    }))
}

/// Spawn the mock Deezer API on an ephemeral port; returns its base URL.
/// Tracklist mock: `/album/{id}` omits ISRCs, only this listing carries them.
async fn deezer_album_tracks_mock(UrlPath(_id): UrlPath<String>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": [
            { "id": 1, "title": "Kapitel 01: Die Robo-Ente (Folge 459)", "duration": 98,
              "isrc": "CH7812690465", "track_position": 1 },
            { "id": 2, "title": "Kapitel 02: Die Ente fliegt", "duration": 60,
              "isrc": "CH7812690466", "track_position": 2 },
            { "id": 3, "title": "Kapitel 03: Outro", "duration": 10,
              "isrc": "CH7812690467", "track_position": 3 }
        ],
        "next": null
    }))
}

async fn spawn_deezer_meta_mock() -> String {
    let app = Router::new()
        .route("/artist/{id}/albums", get(deezer_artist_albums_mock))
        .route("/album/{id}", get(deezer_album_meta_mock))
        .route("/album/{id}/tracks", get(deezer_album_tracks_mock))
        .route("/search/artist", get(deezer_search_mock));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Deezer metadata mock that counts upstream requests, so the catalogue cache
/// can be proven (second call without network).
async fn spawn_deezer_meta_mock_counted() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>)
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();

    async fn bump(
        axum::extract::State(hits): axum::extract::State<Arc<AtomicUsize>>,
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        hits.fetch_add(1, Ordering::SeqCst);
        next.run(req).await
    }

    let app = Router::new()
        .route("/artist/{id}/albums", get(deezer_artist_albums_mock))
        .route("/album/{id}", get(deezer_album_meta_mock))
        .route("/album/{id}/tracks", get(deezer_album_tracks_mock))
        .route("/search/artist", get(deezer_search_mock))
        .with_state(())
        .layer(axum::middleware::from_fn_with_state(counter, bump));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), hits)
}

async fn harness_deezer_meta(deezer_base: String) -> Harness {
    harness_opts(HarnessOpts {
        analyzer_url: None,
        deezer_arl: String::new(),
        deezer_gw_base: "http://127.0.0.1:1".to_string(),
        native_fake: None,
        deezer_base: Some(deezer_base),
    })
    .await
}

async fn get_json_authed(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let (status, body) = send(app, authed("GET", uri).body(Body::empty()).unwrap()).await;
    let v = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn toys_order_stores_intent_and_low_tier() {
    let h = harness().await;
    let (status, _) = create_order_typed(&h.app, &["TOY1"], "toys", "mp3-128").await;
    assert_eq!(status, StatusCode::OK);

    let track = db::get_track(&h.state.pool, "TOY1").await.unwrap().unwrap();
    assert_eq!(track.intent, "toys");
    assert_eq!(track.wanted_quality.as_deref(), Some("mp3-128"));
}

#[tokio::test]
async fn library_order_upgrades_a_toys_track() {
    let h = harness().await;

    // Toys first: 128k, ready.
    let _ = create_order_typed(&h.app, &["MIX1"], "toys", "mp3-128").await;
    db::mark_ready(
        &h.state.pool,
        "MIX1",
        "mp3-128",
        None,
        None,
        Some("/tmp/mix1.mp3"),
    )
    .await
    .unwrap();
    let track = db::get_track(&h.state.pool, "MIX1").await.unwrap().unwrap();
    assert_eq!(track.state, "ready");
    assert_eq!(track.quality_tier.as_deref(), Some("mp3-128"));

    // Now the library wants it in FLAC: same ISRC, same row, re-armed.
    let _ = create_order_typed(&h.app, &["MIX1"], "library", "flac").await;
    let track = db::get_track(&h.state.pool, "MIX1").await.unwrap().unwrap();
    assert_eq!(track.state, "pending", "upgrade must re-download");
    assert_eq!(track.intent, "library", "library wins over toys");
    assert_eq!(track.wanted_quality.as_deref(), Some("flac"));
}

#[tokio::test]
async fn toys_order_never_downgrades_a_library_track() {
    let h = harness().await;

    let _ = create_order_typed(&h.app, &["MIX2"], "library", "flac").await;
    db::mark_ready(
        &h.state.pool,
        "MIX2",
        "flac",
        Some("/tmp/mix2.flac"),
        None,
        None,
    )
    .await
    .unwrap();

    let _ = create_order_typed(&h.app, &["MIX2"], "toys", "mp3-128").await;
    let track = db::get_track(&h.state.pool, "MIX2").await.unwrap().unwrap();
    assert_eq!(track.state, "ready", "an existing FLAC stays ready");
    assert_eq!(track.intent, "library", "toys must not demote intent");
    assert_eq!(
        track.wanted_quality.as_deref(),
        Some("flac"),
        "a lower target must not lower the wish"
    );
}

#[tokio::test]
async fn invalid_intent_or_target_is_rejected() {
    let h = harness().await;
    let (status, body) = create_order_typed(&h.app, &["BAD1"], "kids", "flac").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown intent: {body}");

    let (status, body) = create_order_typed(&h.app, &["BAD2"], "toys", "lossless?!").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown target: {body}");
}

#[tokio::test]
async fn catalogue_serves_repeat_calls_without_upstream() {
    use std::sync::atomic::Ordering;

    let (deezer, hits) = spawn_deezer_meta_mock_counted().await;
    let h = harness_deezer_meta(deezer).await;

    let (status, first) = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    assert_eq!(status, StatusCode::OK);
    let after_first = hits.load(Ordering::SeqCst);
    assert!(after_first >= 1, "first call must reach Deezer");

    let (status, second) = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hits.load(Ordering::SeqCst), after_first, "second call must be cached");
    assert_eq!(first, second, "cache must return the identical payload");

    // Same for the artist page and the artist search.
    let _ = get_json_authed(&h.app, "/deezer/artist/15167501/albums").await;
    let after_artist = hits.load(Ordering::SeqCst);
    let _ = get_json_authed(&h.app, "/deezer/artist/15167501/albums").await;
    assert_eq!(hits.load(Ordering::SeqCst), after_artist, "artist page cached");

    let _ = get_json_authed(&h.app, "/deezer/search/artist?q=paw").await;
    let after_search = hits.load(Ordering::SeqCst);
    let _ = get_json_authed(&h.app, "/deezer/search/artist?q=paw").await;
    assert_eq!(hits.load(Ordering::SeqCst), after_search, "search cached");
}

#[tokio::test]
async fn catalogue_refreshes_once_after_ttl() {
    use std::sync::atomic::Ordering;

    let (deezer, hits) = spawn_deezer_meta_mock_counted().await;
    let h = harness_deezer_meta(deezer).await;

    let _ = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    let after_first = hits.load(Ordering::SeqCst);

    // Age the entry past the album TTL (7 days).
    sqlx::query("UPDATE catalogue_albums SET fetched_at = fetched_at - ?")
        .bind(music_api::db::CATALOGUE_ALBUM_TTL + 1)
        .execute(&h.state.pool)
        .await
        .expect("age the catalogue row");

    let _ = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    let after_refresh = hits.load(Ordering::SeqCst);
    assert!(
        after_refresh > after_first,
        "expired entry must be refreshed ({after_first} -> {after_refresh})"
    );

    // …and the refreshed entry is cached again.
    let _ = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        after_refresh,
        "refresh must re-populate the cache"
    );
}

#[tokio::test]
async fn deezer_album_detail_backfills_missing_isrcs() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    let (status, v) = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    assert_eq!(status, StatusCode::OK);

    // `/album/{id}` left the third track without an ISRC; the tracklist call
    // must fill it, otherwise the hub would skip the whole album.
    let tracks = v["tracks"].as_array().expect("tracks");
    assert_eq!(tracks.len(), 3);
    for track in tracks {
        assert!(
            track["isrc"].as_str().is_some_and(|s| !s.is_empty()),
            "every track needs an ISRC: {track}"
        );
    }
    assert_eq!(tracks[2]["isrc"], "CH7812690467");
}

#[tokio::test]
async fn deezer_meta_endpoints_require_a_token() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    for uri in [
        "/deezer/artist/15167501/albums",
        "/deezer/album/1137915301",
        "/deezer/search/artist?q=paw",
    ] {
        let (status, _) = send(
            &h.app,
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} must be protected");
    }
}

#[tokio::test]
async fn deezer_artist_albums_maps_the_page_and_forwards_paging() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;

    // Explicit paging: the echo in `next` proves limit/index reached Deezer.
    let (status, v) =
        get_json_authed(&h.app, "/deezer/artist/15167501/albums?limit=50&index=0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["total"], 417);
    assert_eq!(v["albums"].as_array().unwrap().len(), 1);
    let album = &v["albums"][0];
    assert_eq!(album["albumId"], "1137915301");
    assert_eq!(
        album["title"],
        "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)"
    );
    assert_eq!(album["releaseDate"], "2026-11-13");
    assert_eq!(album["recordType"], "album");
    // cover_big wins over cover_medium.
    assert_eq!(
        album["coverUrl"],
        "https://cdn-images.dzcdn.net/images/cover/bbbb/500x500-000000-80-0-0.jpg"
    );
    // Deezer does not deliver a track count here → always null.
    assert!(album["nbTracks"].is_null());
    // Cold catalogue: no length was stored yet → null (`seconds` is a hint).
    assert!(album["seconds"].is_null(), "cold catalogue must be null");
    assert_eq!(
        v["next"],
        "https://api.deezer.com/artist/15167501/albums?limit=50&index=50"
    );

    // Defaults (limit 50, index 0) produce the same upstream call.
    let (status, v) = get_json_authed(&h.app, "/deezer/artist/15167501/albums").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v["next"],
        "https://api.deezer.com/artist/15167501/albums?limit=50&index=50"
    );
}

/// #621: once an album is in the own catalogue (its detail was fetched), the
/// artist list must carry its length — filled from `catalogue_albums.seconds`,
/// not from another Deezer call.
#[tokio::test]
async fn deezer_artist_albums_carry_the_catalogued_duration() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;

    // Catalogue the album first (this is what fills `catalogue_albums.seconds`).
    let (status, album) = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(album["seconds"], 168);

    // Now the list resolves the same album's length from the catalogue.
    let (status, v) = get_json_authed(&h.app, "/deezer/artist/15167501/albums").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v["albums"][0]["seconds"], 168,
        "catalogued album's length in the list"
    );
}

/// #621: the length must also be filled on the page-cache path — and the cached
/// page must not carry a baked-in `seconds` (stale protection).
#[tokio::test]
async fn deezer_artist_albums_cache_hit_still_carries_seconds() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;

    // Catalogue the album, then prime the page cache with the (raw) list.
    let _ = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    let _ = get_json_authed(&h.app, "/deezer/artist/15167501/albums").await;

    // Second list call: served from the page cache, still decorated.
    let (status, v) = get_json_authed(&h.app, "/deezer/artist/15167501/albums").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v["albums"][0]["seconds"], 168,
        "cache hit must still carry the length"
    );

    // The cached page itself stays raw: no `seconds` baked into the payload.
    let cached = db::catalogue_page_get(&h.state.pool, "15167501", 0)
        .await
        .unwrap()
        .expect("page cached");
    assert!(
        cached["albums"][0]["seconds"].is_null(),
        "page cache must not burn in seconds: {cached}"
    );
}

#[tokio::test]
async fn deezer_artist_albums_rejects_non_numeric_ids() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    let (status, v) = get_json_authed(&h.app, "/deezer/artist/abc/albums").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid artist id");

    let (status, v) = get_json_authed(&h.app, "/deezer/album/abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid album id");
}

#[tokio::test]
async fn deezer_album_maps_metadata_and_sorts_tracks() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    let (status, v) = get_json_authed(&h.app, "/deezer/album/1137915301").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["albumId"], "1137915301");
    assert_eq!(v["releaseDate"], "2026-11-13");
    assert_eq!(v["recordType"], "album");
    assert!(v["coverUrl"].is_string());
    // 60 + 98 + 10 seconds.
    assert_eq!(v["seconds"], 168);

    let tracks = v["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 3);
    let positions: Vec<i64> = tracks
        .iter()
        .map(|t| t["position"].as_i64().unwrap())
        .collect();
    assert_eq!(positions, vec![1, 2, 3]);
    assert_eq!(tracks[0]["isrc"], "CH7812690465");
    assert_eq!(tracks[0]["title"], "Kapitel 01: Die Robo-Ente (Folge 459)");
    assert_eq!(tracks[0]["durationMs"], 98_000);
    // A track the album payload left without an ISRC is backfilled from the
    // tracklist listing (see `deezer_album_detail_backfills_missing_isrcs`).
    assert_eq!(tracks[2]["isrc"], "CH7812690467");
}

#[tokio::test]
async fn deezer_album_not_found_is_404() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;

    // Deezer's "no data" on HTTP 200 …
    let (status, v) = get_json_authed(&h.app, "/deezer/album/800").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["error"], "deezer album not found");

    // … and an upstream HTTP 404 mean the same thing.
    let (status, v) = get_json_authed(&h.app, "/deezer/album/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["error"], "deezer album not found");
}

#[tokio::test]
async fn deezer_upstream_failures_are_502() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    let (status, v) = get_json_authed(&h.app, "/deezer/album/500").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(v["error"], "deezer request failed");

    // Dead upstream: transport failure, still a short 502.
    let dead = harness_deezer_meta("http://127.0.0.1:1".to_string()).await;
    let (status, v) = get_json_authed(&dead.app, "/deezer/artist/15167501/albums").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(v["error"], "deezer request failed");
}

#[tokio::test]
async fn deezer_search_artist_forwards_the_query() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    let (status, v) = get_json_authed(&h.app, "/deezer/search/artist?q=paw%20patrol&limit=10").await;
    assert_eq!(status, StatusCode::OK);
    let artists = v["artists"].as_array().unwrap();
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0]["artistId"], "15167501");
    // The mock echoes `q` as the name → the query string was forwarded.
    assert_eq!(artists[0]["name"], "paw patrol");
    assert_eq!(
        artists[0]["coverUrl"],
        "https://api.deezer.com/artist/15167501/image"
    );
}

#[tokio::test]
async fn deezer_search_requires_a_query() {
    let h = harness_deezer_meta(spawn_deezer_meta_mock().await).await;
    for uri in [
        "/deezer/search/artist",
        "/deezer/search/artist?q=",
        "/deezer/search/artist?q=%20%20",
    ] {
        let (status, v) = get_json_authed(&h.app, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(v["error"], "q is required");
    }
}
