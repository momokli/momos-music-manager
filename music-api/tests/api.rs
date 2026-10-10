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
        store_root: root.join("objects"),
        store_max_upload_bytes: music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
        deezer_base: "http://127.0.0.1:1".to_string(),
        ffmpeg: "ffmpeg".to_string(),
        ffprobe: "ffprobe".to_string(),
        worker_interval: Duration::from_secs(5),
        download_timeout: Duration::from_secs(60),
        ssh_sources: Vec::new(),
        ssh_ingest_interval: Duration::from_secs(0),
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

    let store = music_api::store::Store::new(
        root.join("objects"),
        music_api::store::DEFAULT_MAX_UPLOAD_BYTES,
    );
    music_api::store::init(&pool, &store).await.unwrap();

    let state = Arc::new(AppState {
        pool,
        config,
        http: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
        deemix_login: Default::default(),
        store,
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

#[tokio::test]
async fn order_priority_reaches_the_queue() {
    let h = harness().await;
    let (status, _) = send(
        &h.app,
        authed("POST", "/orders")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{ "isrc": "AEA0D1846146" }],
                "priority": 42
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&h.app, authed("GET", "/queue").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = v["isrc"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["isrc"], "AEA0D1846146");
    assert_eq!(items[0]["priority"], 42);
}

#[tokio::test]
async fn object_meta_returns_tags() {
    let h = harness().await;
    let src = write_file(h._tmp.path(), "song.flac", b"fake-flac-bytes");
    let hash = sha256_hex(b"fake-flac-bytes");
    let meta = music_api::store::ImportMeta {
        content_type: Some("audio/flac".into()),
        original_path: Some("song.flac".into()),
        isrc: Some("DEQ022280682".into()),
        name: Some("song.flac".into()),
        group_key: Some("Artist - Title".into()),
        stem_type: None,
        title: Some("Altbau".into()),
        artist: Some("01099".into()),
        album: Some("liked".into()),
        genre: Some("Rap".into()),
        comment: Some("[___] liked".into()),
        bpm: None,
        musical_key: Some("11m".into()),
        duration_ms: Some(139_640),
    };
    music_api::store::import_file(&h.state.pool, &h.state.store, &hash, &src, &meta)
        .await
        .unwrap();

    let (status, body) = send(
        &h.app,
        authed("GET", &format!("/objects/{hash}/meta"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["isrc"], "DEQ022280682");
    assert_eq!(v["musicalKey"], "11m");
    assert_eq!(v["title"], "Altbau");
    assert_eq!(v["groupKey"], "Artist - Title");

    // Unknown object → 404.
    let (status, _) = send(
        &h.app,
        authed("GET", &format!("/objects/{}/meta", sha256_hex(b"nope")))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn metadata_upsert_and_get() {
    let h = harness().await;

    // Unknown ISRC → 404.
    let (status, _) = send(
        &h.app,
        authed("GET", "/metadata/DEQ022280682")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{
                    "isrc": "deq022280682",
                    "bpm": 128.0,
                    "key": "11m",
                    "title": "Altbau",
                    "source": "traktor"
                }]
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["upserted"], 1);

    // Normalised lookup (case-insensitive ISRC).
    let (status, body) = send(
        &h.app,
        authed("GET", "/metadata/DEQ022280682")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["isrc"], "DEQ022280682");
    assert_eq!(v["bpm"], 128.0);
    assert_eq!(v["musicalKey"], "11m");
    assert_eq!(v["source"], "traktor");
}

#[tokio::test]
async fn metadata_merge_keeps_non_null() {
    let h = harness().await;

    // First push: bpm only.
    let _ = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{ "isrc": "USQX91201487", "bpm": 124.0, "source": "mmm" }]
            })))
            .unwrap(),
    )
    .await;

    // Second push: key only (bpm null) — must not wipe the bpm.
    let _ = send(
        &h.app,
        authed("POST", "/metadata")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({
                "items": [{ "isrc": "USQX91201487", "key": "8A", "source": "traktor" }]
            })))
            .unwrap(),
    )
    .await;

    let (_, body) = send(
        &h.app,
        authed("GET", "/metadata/USQX91201487")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["bpm"], 124.0, "bpm must survive a null-field push");
    assert_eq!(v["musicalKey"], "8A");
}

#[tokio::test]
async fn ingest_ssh_requires_a_source() {
    let h = harness().await;

    // No SSH_SOURCES configured → 400.
    let (status, _) = send(
        &h.app,
        authed("POST", "/ingest/ssh")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(serde_json::json!({ "all": true })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Runs listing is empty.
    let (status, body) = send(
        &h.app,
        authed("GET", "/ingest/runs").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["runs"].as_array().unwrap().len(), 0);
}

/// A minimal (silent) WAV — WAV is ingested raw, so no real audio is needed.
fn fake_wav() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&36u32.to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&44100u32.to_le_bytes());
    v.extend_from_slice(&88200u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

fn multipart_body(boundary: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    b.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    b.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    b.extend_from_slice(bytes);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

#[tokio::test]
async fn upload_stores_and_hashes() {
    let h = harness().await;
    let wav = fake_wav();
    let hash = sha256_hex(&wav);
    let boundary = "X-BOUNDARY";

    let (status, body) = send(
        &h.app,
        authed("POST", "/objects")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart_body(boundary, "song.wav", &wav)))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "upload failed: {}",
        String::from_utf8_lossy(&body)
    );
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["hash"], hash);
    assert_eq!(v["stored"], true);

    // The object is now retrievable byte-for-byte.
    let (status, got) = send(
        &h.app,
        authed("GET", &format!("/objects/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, wav);
}

#[tokio::test]
async fn upload_dedup_is_noop() {
    let h = harness().await;
    let wav = fake_wav();
    let boundary = "X-BOUNDARY";
    let body = multipart_body(boundary, "song.wav", &wav);

    for expected_stored in [true, false] {
        let (status, resp) = send(
            &h.app,
            authed("POST", "/objects")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(v["stored"], expected_stored);
    }
}

#[tokio::test]
async fn upload_rejects_non_audio() {
    let h = harness().await;
    let boundary = "X-BOUNDARY";
    let (status, _) = send(
        &h.app,
        authed("POST", "/objects")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart_body(boundary, "notes.txt", b"hello")))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
