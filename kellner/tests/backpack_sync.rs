//! End-to-end integration tests for `kellner backpack` (issue #367).
//!
//! Drives the **real** `kellner` binary against a **local mock hub + mock
//! music-api** (`tokio::net::TcpListener` on `127.0.0.1:0`). The hub paths
//! (`/api/hub/backpacks…`) authenticate with the device token; the music-api
//! paths (`/orders`, `/tracks`, `/isrc/…`) with the music-api token. Nothing
//! leaves the loopback interface; a 30 s hang guard wraps every run.
//!
//! `state.json` is redirected into the tempdir via `KELLNER_CONFIG` (it lives
//! next to `config.toml`). The fetched "audio" is a real `tests/fixtures/test.flac`
//! so `lofty` can read/write the Comment tag.
//!
//! Covered acceptance criteria:
//! - (a) a missing track is ordered + fetched exactly once and written flat;
//!   membership + rendered tags are recorded in `state.json`.
//! - (b) a second run orders/fetches nothing (idempotent).
//! - (c) a track that leaves every active backpack is soft-moved to
//!   `.kellner-trash/` (never hard-deleted) and its membership dropped.
//! - (d) a user `downloads` entry is never pruned.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const DEVICE_TOKEN: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
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
    /// Every request in arrival order.
    hits: Vec<Captured>,
    /// `GET /api/hub/backpacks` body items.
    backpacks: Vec<serde_json::Value>,
    /// backpack id → `GET /api/hub/backpacks/{id}/export` body.
    exports: BTreeMap<i64, serde_json::Value>,
    /// isrc → music-api track state (default `"ready"`).
    states: BTreeMap<String, String>,
}

type Shared = Arc<Mutex<MockState>>;

fn flac_fixture() -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("test.flac"),
    )
    .unwrap()
}

/// Start the mock hub + music-api on an ephemeral loopback port.
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
            let (status, ctype, body) = handle(&sink, &cap);
            sink.lock().unwrap().hits.push(cap);

            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let mut resp = head.into_bytes();
            resp.extend_from_slice(&body);
            let _ = sock.write_all(&resp).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        }
    });

    (url, state)
}

fn handle(shared: &Shared, cap: &Captured) -> (String, String, Vec<u8>) {
    let json_ct = "application/json".to_string();

    // Hub routes: device-token gated.
    if cap.path.starts_with("/api/hub/") {
        if cap.authorization.as_deref() != Some(&format!("Bearer {DEVICE_TOKEN}")) {
            return ("401 Unauthorized".into(), json_ct, b"{\"error\":\"unauthorized\"}".to_vec());
        }
        if cap.method == "GET" && cap.path == "/api/hub/backpacks" {
            let backpacks = shared.lock().unwrap().backpacks.clone();
            let body = serde_json::json!({ "backpacks": backpacks });
            return ("200 OK".into(), json_ct, body.to_string().into_bytes());
        }
        if cap.method == "GET" {
            if let Some(rest) = cap.path.strip_prefix("/api/hub/backpacks/") {
                if let Some(id_str) = rest.strip_suffix("/export") {
                    let id: i64 = id_str.parse().unwrap_or(-1);
                    let export = shared.lock().unwrap().exports.get(&id).cloned();
                    return match export {
                        Some(v) => ("200 OK".into(), json_ct, v.to_string().into_bytes()),
                        None => ("404 Not Found".into(), json_ct, b"{\"error\":\"unknown backpack\"}".to_vec()),
                    };
                }
            }
        }
        return ("404 Not Found".into(), json_ct, b"{\"error\":\"not found\"}".to_vec());
    }

    // music-api routes: music-api-token gated.
    if cap.authorization.as_deref() != Some(&format!("Bearer {MUSIC_API_TOKEN}")) {
        return ("401 Unauthorized".into(), json_ct, b"{\"error\":\"unauthorized\"}".to_vec());
    }

    let (path, query) = match cap.path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (cap.path.clone(), String::new()),
    };

    if cap.method == "POST" && path == "/orders" {
        let req: serde_json::Value = serde_json::from_slice(&cap.body).unwrap_or_default();
        let items = req.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let body = serde_json::json!({ "orderId": "mock-order-1", "status": "open", "count": items.len() });
        return ("200 OK".into(), json_ct, body.to_string().into_bytes());
    }

    if cap.method == "GET" && path == "/tracks" {
        let st = shared.lock().unwrap();
        let isrcs = parse_isrcs(&query);
        let mut tracks = Vec::new();
        for isrc in isrcs {
            let state = st.states.get(&isrc).cloned().unwrap_or_else(|| "ready".into());
            tracks.push(serde_json::json!({ "isrc": isrc, "state": state, "formats": ["flac"] }));
        }
        let body = serde_json::json!({ "tracks": tracks });
        return ("200 OK".into(), json_ct, body.to_string().into_bytes());
    }

    if cap.method == "GET" && path.starts_with("/isrc/") {
        let rest = &path["/isrc/".len()..];
        if let Some((_isrc, format)) = rest.split_once('/') {
            if format == "flac" {
                return ("200 OK".into(), "audio/octet-stream".into(), flac_fixture());
            }
            return ("404 Not Found".into(), json_ct, b"{\"error\":\"unsupported format\"}".to_vec());
        }
    }

    ("404 Not Found".into(), json_ct, b"{\"error\":\"not found\"}".to_vec())
}

fn parse_isrcs(query: &str) -> Vec<String> {
    let raw = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("isrcs="))
        .unwrap_or("")
        .to_string();
    percent_decode(&raw)
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok())?
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
        k.eq_ignore_ascii_case("authorization").then(|| v.trim().to_string())
    });
    let body = if head_end + 4 <= buf.len() {
        buf[head_end + 4..].to_vec()
    } else {
        Vec::new()
    };
    Captured { method, path, authorization, body }
}

fn order_count(shared: &Shared) -> usize {
    shared
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|h| h.method == "POST" && h.path == "/orders")
        .count()
}

fn file_fetch_count(shared: &Shared) -> usize {
    shared
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|h| h.method == "GET" && h.path.starts_with("/isrc/"))
        .count()
}

/// Write a paired-device config + music-api env, pointing everything at `url`.
fn write_config(config_path: &Path, url: &str) {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        config_path,
        format!(
            "hub_url = \"{url}\"\n\n[device]\ndevice_id = 1\ndevice_name = \"test\"\ndevice_token = \"{DEVICE_TOKEN}\"\npaired_at = \"0\"\n"
        ),
    )
    .unwrap();
}

/// Spawn the built `kellner` binary with a hard 30 s hang guard.
fn run_kellner(args: &[&str], url: &str, config_path: &Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kellner"));
    cmd.args(args)
        .env("KELLNER_MUSIC_API_URL", url)
        .env("KELLNER_MUSIC_API_TOKEN", MUSIC_API_TOKEN)
        .env("KELLNER_CONFIG", config_path)
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

fn run_in_blocking(args: Vec<String>, url: String, config: PathBuf) -> (i32, String) {
    std::thread::spawn(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_kellner(&refs, &url, &config)
    })
    .join()
    .unwrap()
}

fn backpack_json(id: i64, name: &str, is_active: bool) -> serde_json::Value {
    serde_json::json!({ "id": id, "name": name, "is_active": is_active })
}

fn export_json(id: i64, name: &str, tracks: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({ "backpack": { "id": id, "name": name }, "tracks": tracks })
}

fn track_json(isrc: &str, tags: &[&str], bpm: Option<f64>, key: Option<&str>, energy: Option<f64>) -> serde_json::Value {
    serde_json::json!({
        "track_id": 42,
        "isrc": isrc,
        "title": "t",
        "artists": "a",
        "tags": tags,
        "bpm": bpm,
        "key": key,
        "energy": energy,
    })
}

fn state_json(config_path: &Path) -> serde_json::Value {
    let path = config_path.with_file_name("state.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// Configure one active backpack with one tagged track.
fn seed_one_track(shared: &Shared, isrc: &str, tags: &[&str], bpm: Option<f64>, key: Option<&str>, energy: Option<f64>) {
    let mut st = shared.lock().unwrap();
    st.backpacks = vec![backpack_json(1, "Party", true)];
    st.exports.insert(
        1,
        export_json(1, "Party", vec![track_json(isrc, tags, bpm, key, energy)]),
    );
}

// (a) + (b) fetch once, record membership, then idempotent ────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetches_missing_then_is_idempotent() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("kellner").join("config.toml");
    write_config(&config, &url);

    seed_one_track(&state, "DEABC1234567", &["b", "a"], Some(128.0), Some("A-Minor"), Some(7.0));

    let args = vec![
        "backpack".to_string(),
        "--root".into(),
        coll.to_string_lossy().to_string(),
        "--poll-secs".into(),
        "1".into(),
    ];
    let (code, out) = run_in_blocking(args.clone(), url.clone(), config.clone());
    assert_eq!(code, 0, "run 1: {out}");

    // Exactly one order + one fetch, flat file written.
    assert_eq!(order_count(&state), 1, "one order");
    assert_eq!(file_fetch_count(&state), 1, "one fetch");
    assert!(coll.join("DEABC1234567.flac").is_file(), "flat file written");

    // Membership + rendered tags recorded.
    let sj = state_json(&config);
    let m = &sj["backpack_membership"]["DEABC1234567"];
    assert!(m.is_object(), "membership recorded: {sj}");
    assert_eq!(m["backpack_id"], 1);
    assert_eq!(m["tags_rendered"], "a, b  BPM 128 | A-Minor | Energy 7");
    assert!(m["content_hash"].as_str().unwrap().len() == 64);
    assert!(m["path"].as_str().unwrap().ends_with("DEABC1234567.flac"));

    // The Comment tag in the file matches the render.
    {
        use lofty::prelude::*;
        let tagged = lofty::read_from_path(coll.join("DEABC1234567.flac")).unwrap();
        let comment = tagged.primary_tag().and_then(|t| t.comment()).map(|c| c.to_string());
        assert_eq!(comment.as_deref(), Some("a, b  BPM 128 | A-Minor | Energy 7"));
    }

    // (b) second run: nothing ordered or fetched.
    let (code, out) = run_in_blocking(args, url, config.clone());
    assert_eq!(code, 0, "run 2: {out}");
    assert_eq!(order_count(&state), 1, "second run must not order");
    assert_eq!(file_fetch_count(&state), 1, "second run must not fetch");
    assert!(out.contains("0 ordered"), "operator sees the skip: {out}");
}

// (c) leaving every backpack soft-moves to trash ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leaving_all_backpacks_soft_prunes() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("kellner").join("config.toml");
    write_config(&config, &url);

    seed_one_track(&state, "DEABC1234567", &["Techno"], None, None, None);
    let (code, out) = run_in_blocking(
        vec![
            "backpack".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url.clone(),
        config.clone(),
    );
    assert_eq!(code, 0, "seed run: {out}");
    assert!(coll.join("DEABC1234567.flac").is_file());
    assert!(state_json(&config)["backpack_membership"]["DEABC1234567"].is_object());

    // The track leaves all active backpacks (backpack still active, empty).
    {
        let mut st = state.lock().unwrap();
        st.exports.insert(1, export_json(1, "Party", vec![]));
    }

    let (code, out) = run_in_blocking(
        vec![
            "backpack".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config.clone(),
    );
    assert_eq!(code, 0, "prune run: {out}");

    // File gone from the root, present (not deleted) under .kellner-trash.
    assert!(!coll.join("DEABC1234567.flac").exists(), "flat file removed");
    let trashed = coll.join(".kellner-trash").join("DEABC1234567.flac");
    assert!(trashed.is_file(), "soft-moved, never deleted");
    // Membership dropped.
    assert!(state_json(&config)["backpack_membership"]["DEABC1234567"].is_null());
    assert!(out.contains("pruned"), "operator sees the prune: {out}");
}

// (d) user downloads are never pruned ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_downloads_are_never_pruned() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("kellner").join("config.toml");
    write_config(&config, &url);

    // A user-fetched file (#366) + its state entry, NOT part of any backpack.
    let user_file = coll.join("USER00000001.flac");
    std::fs::copy(flac_fixture_path(), &user_file).unwrap();
    std::fs::write(
        config.with_file_name("state.json"),
        serde_json::json!({
            "downloads": {
                "USER00000001": {
                    "path": user_file.to_string_lossy(),
                    "format": "flac",
                    "order_id": "user-order",
                    "fetched_at": 1
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    // Backpack syncs a *different* track.
    seed_one_track(&state, "DEABC1234567", &["Techno"], None, None, None);
    let (code, out) = run_in_blocking(
        vec![
            "backpack".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config.clone(),
    );
    assert_eq!(code, 0, "run: {out}");

    // The user file survives, its state entry is intact.
    assert!(user_file.is_file(), "user download must not be pruned");
    let sj = state_json(&config);
    assert_eq!(sj["downloads"]["USER00000001"]["order_id"], "user-order");
    assert!(!coll.join(".kellner-trash").join("USER00000001.flac").exists());
}

fn flac_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test.flac")
}
