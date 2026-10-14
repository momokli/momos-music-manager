//! Contract tests for the public HTTP surface (no network, no ffmpeg).
//!
//! Each test builds its own app over a temp SQLite file and a temp data dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
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
    harness_with(None).await
}

/// Harness variant that configures the BPM/key analyzer base URL.
async fn harness_with(analyzer_url: Option<String>) -> Harness {
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
        ytdlp: "yt-dlp".to_string(),
        ytdlp_dir: root.join("ytdlp"),
        ytdlp_concurrency: 2,
        transcode_concurrency: 2,
        worker_interval: Duration::from_secs(5),
        download_timeout: Duration::from_secs(60),
        queue_max_attempts: 3,
        queue_retry_base_secs: 60,
        queue_retry_max_secs: 1800,
        analyzer_url,
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
        config.deemix_download_dir.clone(),
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
        deemix_login: Default::default(),
        store: music_api::store::Store::new(
            root.join("objects"),
            music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
        ),
        logs: music_api::logbuf::LogBuffer::new(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ytdlp_sem: Arc::new(tokio::sync::Semaphore::new(ytdlp_concurrency)),
        finalize_sem: Arc::new(tokio::sync::Semaphore::new(config_transcode_concurrency)),
        finalizing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_sem: Arc::new(tokio::sync::Semaphore::new(1)),
        analyzing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_backoff: Default::default(),
        artist_ingesting: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
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
    db::create_order(&h.state.pool, "o1", &[isrc.to_string()], 0)
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

// ── Artist orders (whole-discography download + ingest) ──────────────────────

#[tokio::test]
async fn artist_order_create_and_get() {
    let h = harness().await;

    // A bare Deezer artist id is canonicalised to its URL. No ARL is configured,
    // so no deemix call is made and the order stays `open`.
    let (status, body) = send(
        &h.app,
        authed("POST", "/artist-orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                serde_json::json!({ "artist": "12345", "priority": 7 }),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "open");
    assert_eq!(v["artist"], "https://www.deezer.com/artist/12345");
    let order_id = v["orderId"].as_str().unwrap().to_string();

    let (status, body) = send(
        &h.app,
        authed("GET", &format!("/artist-orders/{order_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["orderId"], order_id);
    assert_eq!(v["artist"], "12345");
    assert_eq!(v["artistUrl"], "https://www.deezer.com/artist/12345");
    assert_eq!(v["status"], "open");
    assert_eq!(v["priority"], 7);
    assert_eq!(v["added"], 0);
    assert!(v["items"].as_array().unwrap().is_empty());

    // Unknown id → 404.
    let (status, _) = send(
        &h.app,
        authed("GET", "/artist-orders/nope")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artist_order_rejects_a_bad_artist() {
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        authed("POST", "/artist-orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "artist": "not an artist" })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn artist_ingest_parses_ffprobe_fixture_into_vault() {
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
        "deemix",
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
        "deemix",
    )
    .await
    .unwrap();
    assert!(none.is_none());
}
