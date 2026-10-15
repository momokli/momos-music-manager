//! End-to-end integration tests for `kellner backup` (issue #365, P4-5).
//!
//! Drives the **real** `kellner` binary against a **local mock music-api**
//! (`tokio::net::TcpListener` on `127.0.0.1:0`) that implements the two v1
//! object-store routes:
//! - `POST /objects/check` → `{"present":[…],"missing":[…]}`
//! - `PUT /objects/{sha256}` → verifies the body digest against the key
//!   (rejecting a mismatch with `400`, like the real server) and records the key.
//!
//! Nothing leaves the loopback interface.

use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lofty::config::WriteOptions;
use lofty::prelude::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MUSIC_API_TOKEN: &str = "music-api-token-0123456789abcdef0123456789abcdef";

/// A single captured HTTP request from kellner.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

#[derive(Default)]
struct MockState {
    /// Hashes the mock currently holds.
    store: HashSet<String>,
    /// Every request in arrival order.
    hits: Vec<Captured>,
}

type Shared = Arc<Mutex<MockState>>;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Start the mock music-api on an ephemeral loopback port.
async fn start_mock() -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let state: Shared = Arc::new(Mutex::new(MockState::default()));
    let sink = state.clone();

    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };

            // Read one complete request (headers + Content-Length body).
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match sock.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
                if let Some(he) = find_sub(&buf, b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&buf[..he]);
                    let want = content_length(&headers);
                    if buf.len() >= he + 4 + want {
                        break;
                    }
                }
            }

            let cap = parse_request(&buf);
            let (status, body) = handle(&sink, &cap);
            sink.lock().unwrap().hits.push(cap);

            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        }
    });

    (url, state)
}

/// Return `(status_line, response_body)` for a captured request.
fn handle(shared: &Shared, cap: &Captured) -> (String, String) {
    let authed = cap.authorization.as_deref() == Some(&format!("Bearer {MUSIC_API_TOKEN}"));
    if !authed {
        return (
            "401 Unauthorized".into(),
            r#"{"error":"unauthorized"}"#.into(),
        );
    }

    if cap.method == "POST" && cap.path == "/objects/check" {
        let req: serde_json::Value = serde_json::from_slice(&cap.body).unwrap_or_default();
        let mut present = Vec::new();
        let mut missing = Vec::new();
        if let Some(hashes) = req.get("hashes").and_then(|v| v.as_array()) {
            let store = &shared.lock().unwrap().store;
            for h in hashes.iter().filter_map(|v| v.as_str()) {
                if store.contains(h) {
                    present.push(h.to_string());
                } else {
                    missing.push(h.to_string());
                }
            }
        }
        let body = serde_json::json!({ "present": present, "missing": missing });
        return ("200 OK".into(), body.to_string());
    }

    if cap.method == "PUT" {
        if let Some(hash) = cap.path.strip_prefix("/objects/") {
            if sha256_hex(&cap.body) != hash {
                return (
                    "400 Bad Request".into(),
                    r#"{"error":"sha256 of the body does not match the key"}"#.into(),
                );
            }
            let mut st = shared.lock().unwrap();
            let newly = st.store.insert(hash.to_string());
            let body = if newly {
                serde_json::json!({ "stored": true, "size": cap.body.len() })
            } else {
                serde_json::json!({ "stored": false, "present": true })
            };
            let status = if newly { "201 Created" } else { "200 OK" };
            return (status.into(), body.to_string());
        }
    }

    ("404 Not Found".into(), r#"{"error":"not found"}"#.into())
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0)
}

fn parse_request(buf: &[u8]) -> Captured {
    let head_end = find_sub(buf, b"\r\n\r\n").unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let authorization = lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("authorization")
            .then(|| v.trim().to_string())
    });
    let body = if head_end + 4 <= buf.len() {
        buf[head_end + 4..].to_vec()
    } else {
        Vec::new()
    };
    Captured {
        method,
        path,
        authorization,
        body,
    }
}

fn put_count(shared: &Shared) -> usize {
    shared
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|h| h.method == "PUT")
        .count()
}

/// Spawn the built `kellner` binary with a hard 30 s hang guard.
fn run_kellner(args: &[&str], base_url: &str) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kellner"));
    cmd.args(args)
        .env("KELLNER_MUSIC_API_URL", base_url)
        .env("KELLNER_MUSIC_API_TOKEN", MUSIC_API_TOKEN)
        .env_remove("KELLNER_CONFIG")
        .env_remove("KELLNER_HUB_URL")
        .env_remove("KELLNER_COLLECTION_DIR")
        .env_remove("KELLNER_TRAKTOR_NML")
        .env_remove("KELLNER_TRAKTOR_POLL_SECS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn kellner");
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let out_h = std::thread::spawn(move || {
        let mut s = String::new();
        out.read_to_string(&mut s).ok();
        s
    });
    let err_h = std::thread::spawn(move || {
        let mut s = String::new();
        err.read_to_string(&mut s).ok();
        s
    });

    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            panic!("kellner child hung for >30s (args: {args:?})");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let mut combined = out_h.join().unwrap();
    combined.push_str(&err_h.join().unwrap());
    (status.code().unwrap_or(-1), combined)
}

/// Copy a bundled fixture into `dest`, optionally setting its Comment tag.
fn write_fixture(dest: &std::path::Path, fixture: &str, comment: Option<&str>) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(fixture);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::copy(&src, dest).unwrap();
    if let Some(comment) = comment {
        let mut tagged = lofty::read_from_path(dest).unwrap();
        let tag = tagged.primary_tag_mut().expect("fixture has a primary tag");
        tag.insert_text(lofty::tag::ItemKey::Comment, comment.to_string());
        tagged.save_to_path(dest, WriteOptions::default()).unwrap();
    }
}

fn run_in_blocking(args: Vec<String>, url: String) -> (i32, String) {
    std::thread::spawn(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_kellner(&refs, &url)
    })
    .join()
    .unwrap()
}

// Scenario 1: upload of missing objects, Bearer-authenticated ────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_missing_objects_with_bearer() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(&tmp.path().join("a.mp3"), "test.mp3", None);
    write_fixture(&tmp.path().join("b.flac"), "test.flac", None);
    let root = tmp.path().to_string_lossy().to_string();

    let (code, out) = run_in_blocking(
        vec!["backup".into(), "--root".into(), root, "--once".into()],
        url,
    );
    assert_eq!(code, 0, "output: {out}");

    let st = state.lock().unwrap();
    assert_eq!(
        st.hits.iter().filter(|h| h.method == "PUT").count(),
        2,
        "two distinct objects must be uploaded"
    );
    assert_eq!(st.store.len(), 2, "both hashes present afterwards");
    assert!(
        st.hits
            .iter()
            .any(|h| h.method == "POST" && h.path == "/objects/check"),
        "a bulk check must precede the uploads"
    );
    assert!(
        st.hits
            .iter()
            .filter(|h| h.method == "PUT")
            .all(|h| h.authorization.as_deref() == Some(&format!("Bearer {MUSIC_API_TOKEN}"))),
        "every PUT must carry the Bearer token"
    );
}

// Scenario 2: idempotency — a second run uploads nothing ─────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_run_uploads_nothing() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(&tmp.path().join("a.mp3"), "test.mp3", None);
    let root = tmp.path().to_string_lossy().to_string();

    let args = vec!["backup".to_string(), "--root".into(), root, "--once".into()];
    let (code, out) = run_in_blocking(args.clone(), url.clone());
    assert_eq!(code, 0, "run 1 output: {out}");
    assert_eq!(put_count(&state), 1, "first run uploads once");

    let (code, out) = run_in_blocking(args, url);
    assert_eq!(code, 0, "run 2 output: {out}");
    assert_eq!(put_count(&state), 1, "unchanged library must NOT re-upload");
    assert!(
        out.contains("already present"),
        "operator sees the skip: {out}"
    );
}

// Scenario 3: Comment-only difference dedupes to one object ─────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comment_only_difference_dedupes() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(&tmp.path().join("a.mp3"), "test.mp3", Some("backup A"));
    write_fixture(&tmp.path().join("b.mp3"), "test.mp3", Some("backup B"));
    let root = tmp.path().to_string_lossy().to_string();

    let (code, out) = run_in_blocking(
        vec!["backup".into(), "--root".into(), root, "--once".into()],
        url,
    );
    assert_eq!(code, 0, "output: {out}");

    let st = state.lock().unwrap();
    assert_eq!(
        st.store.len(),
        1,
        "files differing only in Comment must collapse to one object"
    );
    assert_eq!(st.hits.iter().filter(|h| h.method == "PUT").count(), 1);
}

// Scenario 4: --dry-run classifies but never PUTs ───────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_never_uploads() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(&tmp.path().join("a.mp3"), "test.mp3", None);
    write_fixture(&tmp.path().join("b.flac"), "test.flac", None);
    let root = tmp.path().to_string_lossy().to_string();

    let (code, out) = run_in_blocking(
        vec![
            "backup".into(),
            "--dry-run".into(),
            "--root".into(),
            root,
            "--once".into(),
        ],
        url,
    );
    assert_eq!(code, 0, "output: {out}");
    assert_eq!(put_count(&state), 0, "--dry-run must not upload");
    assert!(
        state.lock().unwrap().store.is_empty(),
        "the mock store stays empty in a dry run"
    );
    assert!(
        out.contains("would upload"),
        "dry-run names the planned uploads: {out}"
    );
}
