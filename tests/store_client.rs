//! Contract tests for [`StoreClient`] against a mock upstream object store.
//!
//! Guards the exact wire shapes the `.200` store serves (see
//! `plans/proposed/remote-object-store.md`): the `{data:…}`-less envelopes, the
//! `Bearer` header, digest rejection on `PUT`, and idempotent re-upload.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use momos_music_manager::store::StoreClient;

const TOKEN: &str = "test-token";

/// Shared object store state: hash → bytes.
type Store = Arc<Mutex<HashMap<String, Vec<u8>>>>;

#[derive(Clone)]
struct MockState {
    objects: Store,
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"))
}

async fn spawn_mock() -> String {
    async fn get_handler(State(state): State<MockState>, Path(hash): Path<String>) -> Response {
        let objects = state.objects.lock().unwrap();
        match objects.get(&hash) {
            Some(bytes) => (StatusCode::OK, bytes.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn put_handler(
        State(state): State<MockState>,
        Path(hash): Path<String>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        if !authorized(&headers) {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
        if hex(Sha256::digest(&body)) != hash {
            return (StatusCode::BAD_REQUEST, "digest mismatch").into_response();
        }
        let mut objects = state.objects.lock().unwrap();
        if objects.contains_key(&hash) {
            axum::Json(json!({ "stored": false, "present": true })).into_response()
        } else {
            let size = body.len();
            objects.insert(hash, body.to_vec());
            (
                StatusCode::CREATED,
                axum::Json(json!({ "stored": true, "size": size })),
            )
                .into_response()
        }
    }

    #[derive(Deserialize)]
    struct CheckRequest {
        hashes: Vec<String>,
    }

    async fn check_handler(
        State(state): State<MockState>,
        axum::Json(req): axum::Json<CheckRequest>,
    ) -> Response {
        let objects = state.objects.lock().unwrap();
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for h in req.hashes {
            if objects.contains_key(&h) {
                present.push(h);
            } else {
                missing.push(h);
            }
        }
        axum::Json(json!({ "present": present, "missing": missing })).into_response()
    }

    let state = MockState {
        objects: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/objects/check", post(check_handler))
        .route("/objects/{hash}", get(get_handler).put(put_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// Round-trip: `PUT` a canonical object, `HEAD` it, `check` a mixed list, and
/// `GET` it back byte-identically. Re-uploading is a no-op.
#[tokio::test]
async fn client_round_trips_the_contract() {
    let base = spawn_mock().await;
    let client = StoreClient::new(&base, TOKEN);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("canon.flac");
    let bytes = b"CANONICAL-BYTES".to_vec();
    std::fs::write(&file, &bytes).unwrap();
    let hash = hex(Sha256::digest(&bytes));

    // Not present yet.
    assert!(!client.head(&hash).await.expect("head miss"));

    // PUT → newly stored.
    let stored = client
        .put(&hash, &file, "/test/canon.flac", Some("USXXX0000001"))
        .await
        .expect("put");
    assert!(stored, "first upload should report stored=true");

    // HEAD → present.
    assert!(client.head(&hash).await.expect("head hit"));

    // Re-upload → already present, not an error.
    let stored_again = client
        .put(&hash, &file, "/test/canon.flac", None)
        .await
        .expect("put again");
    assert!(!stored_again, "second upload should report stored=false");

    // check → present/missing split.
    let other = "0".repeat(64);
    let result = client
        .check(&[hash.clone(), other.clone()])
        .await
        .expect("check");
    assert_eq!(result.present, vec![hash.clone()]);
    assert_eq!(result.missing, vec![other]);

    // GET round-trips the bytes.
    let dest = dir.path().join("out.flac");
    client.get_to(&hash, &dest).await.expect("get_to");
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
}

/// A digest that does not match the body must be rejected (`400`) — a `PUT`
/// cannot claim a key it does not own.
#[tokio::test]
async fn client_rejects_mismatched_digest() {
    let base = spawn_mock().await;
    let client = StoreClient::new(&base, TOKEN);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("canon.flac");
    std::fs::write(&file, b"some bytes").unwrap();

    let wrong = "a".repeat(64);
    let err = client
        .put(&wrong, &file, "/test/canon.flac", None)
        .await
        .expect_err("mismatched digest must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("400") || msg.to_lowercase().contains("digest"),
        "error should mention the rejection, got: {msg}"
    );
}

/// A wrong token must surface as an error, not as a silent success.
#[tokio::test]
async fn client_reports_unauthorized() {
    let base = spawn_mock().await;
    let client = StoreClient::new(&base, "wrong-token");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("canon.flac");
    std::fs::write(&file, b"some bytes").unwrap();
    let hash = hex(Sha256::digest(b"some bytes"));

    let err = client
        .put(&hash, &file, "/test/canon.flac", None)
        .await
        .expect_err("a 401 must be an error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("401") || msg.to_lowercase().contains("unauthor"),
        "error should mention the rejection, got: {msg}"
    );
}
