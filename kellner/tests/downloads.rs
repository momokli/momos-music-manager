//! End-to-end integration tests for `kellner downloads` (issue #366).
//!
//! Drives the **real** `kellner` binary against a **local mock music-api**
//! (`tokio::net::TcpListener` on `127.0.0.1:0`) implementing the three v1
//! download routes:
//! - `POST /orders`            → `{"orderId":…,"status":"open","count":N}`
//! - `GET  /tracks?isrcs=…`    → per-ISRC `state`/`formats` (or `unknown`)
//! - `GET  /isrc/{isrc}/{fmt}` → the file bytes
//!
//! Nothing leaves the loopback interface; a 30 s hang guard wraps every run.
//! `state.json` is redirected into the tempdir via `KELLNER_CONFIG`.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Every request in arrival order.
    hits: Vec<Captured>,
    /// isrc → state string (or the sentinel `"unknown"`).
    states: BTreeMap<String, String>,
    /// isrc → servable formats (empty = "flac").
    formats: BTreeMap<String, Vec<String>>,
    /// Rounds of `GET /tracks` that still report every ISRC as `pending`.
    pending_rounds: u32,
    /// Rounds of `GET /tracks` that answer `500` (retry exercise).
    tracks_5xx_rounds: u32,
}

type Shared = Arc<Mutex<MockState>>;

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

/// Return `(status_line, content_type, body_bytes)` for a captured request.
fn handle(shared: &Shared, cap: &Captured) -> (String, String, Vec<u8>) {
    let authed = cap.authorization.as_deref() == Some(&format!("Bearer {MUSIC_API_TOKEN}"));
    let json_ct = "application/json".to_string();
    if !authed {
        return ("401 Unauthorized".into(), json_ct, b"{\"error\":\"unauthorized\"}".to_vec());
    }

    let (path, query) = match cap.path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (cap.path.clone(), String::new()),
    };

    // POST /orders
    if cap.method == "POST" && path == "/orders" {
        let req: serde_json::Value = serde_json::from_slice(&cap.body).unwrap_or_default();
        let items = req
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let body = serde_json::json!({
            "orderId": "mock-order-1",
            "status": "open",
            "count": items.len(),
        });
        return ("200 OK".into(), json_ct, body.to_string().into_bytes());
    }

    // GET /tracks?isrcs=…
    if cap.method == "GET" && path == "/tracks" {
        let mut st = shared.lock().unwrap();
        if st.tracks_5xx_rounds > 0 {
            st.tracks_5xx_rounds -= 1;
            return ("500 Internal Server Error".into(), json_ct, b"{\"error\":\"boom\"}".to_vec());
        }
        let force_pending = st.pending_rounds > 0;
        if force_pending {
            st.pending_rounds -= 1;
        }
        let isrcs = parse_isrcs(&query);
        let mut tracks = Vec::new();
        for isrc in isrcs {
            let entry = st.states.get(&isrc).cloned();
            if force_pending {
                tracks.push(serde_json::json!({ "isrc": isrc, "state": "pending", "formats": [] }));
            } else if entry.as_deref() == Some("unknown") {
                tracks.push(serde_json::json!({ "isrc": isrc, "unknown": true }));
            } else {
                let formats = st.formats.get(&isrc).cloned().unwrap_or_else(|| vec!["flac".into()]);
                let state = entry.unwrap_or_else(|| "ready".into());
                tracks.push(serde_json::json!({ "isrc": isrc, "state": state, "formats": formats }));
            }
        }
        let body = serde_json::json!({ "tracks": tracks });
        return ("200 OK".into(), json_ct, body.to_string().into_bytes());
    }

    // GET /isrc/{isrc}/{format}
    if cap.method == "GET" && path.starts_with("/isrc/") {
        let rest = &path["/isrc/".len()..];
        if let Some((isrc, format)) = rest.split_once('/') {
            let st = shared.lock().unwrap();
            let formats = st.formats.get(isrc).cloned().unwrap_or_else(|| vec!["flac".into()]);
            if formats.iter().any(|f| f == format) {
                let body = format!("audio:{isrc}:{format}").into_bytes();
                return ("200 OK".into(), "audio/octet-stream".into(), body);
            }
            return (
                "404 Not Found".into(),
                json_ct.clone(),
                format!("{{\"error\":\"no {format} for this ISRC\"}}").into_bytes(),
            );
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
    let decoded = percent_decode(&raw);
    decoded
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

/// Spawn the built `kellner` binary with a hard 30 s hang guard.
///
/// `config_path` isolates `state.json` (written as its sibling).
fn run_kellner(args: &[&str], base_url: &str, config_path: &std::path::Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kellner"));
    cmd.args(args)
        .env("KELLNER_MUSIC_API_URL", base_url)
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

fn run_in_blocking(args: Vec<String>, url: String, config: std::path::PathBuf) -> (i32, String) {
    std::thread::spawn(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_kellner(&refs, &url, &config)
    })
    .join()
    .unwrap()
}

fn set_states(shared: &Shared, pairs: &[(&str, &str)]) {
    let mut st = shared.lock().unwrap();
    for (isrc, state) in pairs {
        st.states.insert((*isrc).to_string(), (*state).to_string());
    }
}

fn set_formats(shared: &Shared, isrc: &str, formats: &[&str]) {
    shared
        .lock()
        .unwrap()
        .formats
        .insert(isrc.to_string(), formats.iter().map(|s| s.to_string()).collect());
}

// Scenario 1: order → ready → two flat files + provenance ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orders_then_fetches_flat_files() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--isrc".into(),
            "GBAYE0000001".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config.clone(),
    );
    assert_eq!(code, 0, "output: {out}");

    // Exactly one order, ISRCs sorted.
    let st = state.lock().unwrap();
    let orders: Vec<_> = st
        .hits
        .iter()
        .filter(|h| h.method == "POST" && h.path == "/orders")
        .collect();
    assert_eq!(orders.len(), 1, "exactly one POST /orders");
    let body: serde_json::Value = serde_json::from_slice(&orders[0].body).unwrap();
    let isrcs: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["isrc"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(isrcs, vec!["GBAYE0000001".to_string(), "USAB12345678".to_string()]);
    drop(st);

    // Two flat files with deterministic names.
    assert_eq!(
        std::fs::read(coll.join("GBAYE0000001.flac")).unwrap(),
        b"audio:GBAYE0000001:flac"
    );
    assert_eq!(
        std::fs::read(coll.join("USAB12345678.flac")).unwrap(),
        b"audio:USAB12345678:flac"
    );

    // Provenance in state.json.
    let sj: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config.with_file_name("state.json")).unwrap())
            .unwrap();
    assert!(sj["downloads"]["GBAYE0000001"]["path"].is_string());
    assert_eq!(sj["downloads"]["USAB12345678"]["format"], "flac");
    assert_eq!(sj["orders"][0]["order_id"], "mock-order-1");
}

// Scenario 2: idempotency — a second run does no network work ───────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_run_is_idempotent() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let args = vec![
        "downloads".to_string(),
        "--isrc".into(),
        "USAB12345678".into(),
        "--root".into(),
        coll.to_string_lossy().to_string(),
    ];
    let (code, out) = run_in_blocking(args.clone(), url.clone(), config.clone());
    assert_eq!(code, 0, "run 1: {out}");
    assert_eq!(order_count(&state), 1);
    assert_eq!(file_fetch_count(&state), 1);

    let (code, out) = run_in_blocking(args, url, config);
    assert_eq!(code, 0, "run 2: {out}");
    assert_eq!(order_count(&state), 1, "second run must not order");
    assert_eq!(file_fetch_count(&state), 1, "second run must not fetch");
    assert!(out.contains("already present"), "operator sees the skip: {out}");
}

// Scenario 3: polling — pending rounds first, then ready ────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polls_until_ready() {
    let (url, state) = start_mock().await;
    set_states(&state, &[("USAB12345678", "ready")]);
    state.lock().unwrap().pending_rounds = 2;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--poll-secs".into(),
            "1".into(),
            "--timeout-secs".into(),
            "20".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config,
    );
    assert_eq!(code, 0, "output: {out}");

    let polls = state
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|h| h.method == "GET" && h.path.starts_with("/tracks"))
        .count();
    assert!(polls >= 3, "must poll through the pending rounds (got {polls})");
    assert!(coll.join("USAB12345678.flac").exists());
}

// Scenario 4: terminal failure → exit ≠ 0, no file ──────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_state_exits_nonzero() {
    let (url, state) = start_mock().await;
    set_states(&state, &[("USAB12345678", "failed")]);
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--poll-secs".into(),
            "1".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config,
    );
    assert_ne!(code, 0, "a failed ISRC must exit non-zero: {out}");
    assert!(!coll.join("USAB12345678.flac").exists(), "no file on failure");
}

// Scenario 5: unknown ISRC → treated as error ───────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_is_treated_as_error() {
    let (url, state) = start_mock().await;
    set_states(&state, &[("USAB12345678", "unknown")]);
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--poll-secs".into(),
            "1".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config,
    );
    assert_ne!(code, 0, "unknown ISRC must exit non-zero: {out}");
}

// Scenario 6: timeout → exit ≠ 0, no false "complete" ───────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_exits_nonzero() {
    let (url, state) = start_mock().await;
    state.lock().unwrap().pending_rounds = 1000;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--poll-secs".into(),
            "1".into(),
            "--timeout-secs".into(),
            "2".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config,
    );
    assert_ne!(code, 0, "a timeout must exit non-zero: {out}");
    assert!(out.contains("pending"), "report names the pending count: {out}");
    assert!(!coll.join("USAB12345678.flac").exists());
}

// Scenario 7: transient 5xx is retried ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_5xx_is_retried() {
    let (url, state) = start_mock().await;
    set_states(&state, &[("USAB12345678", "ready")]);
    state.lock().unwrap().tracks_5xx_rounds = 1;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--poll-secs".into(),
            "1".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config,
    );
    assert_eq!(code, 0, "a single 5xx must be retried, not fatal: {out}");
    assert!(coll.join("USAB12345678.flac").exists());
}

// Scenario 8: format fallback flac → 320 → flat .mp3 ────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_fallback_to_320() {
    let (url, state) = start_mock().await;
    set_states(&state, &[("USAB12345678", "ready")]);
    set_formats(&state, "USAB12345678", &["320"]);
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url,
        config,
    );
    assert_eq!(code, 0, "output: {out}");
    assert_eq!(
        std::fs::read(coll.join("USAB12345678.mp3")).unwrap(),
        b"audio:USAB12345678:320"
    );
}

// Scenario 9: status report without args is read-only ───────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_report_without_network() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("config.toml");

    let (code, out) = run_in_blocking(
        vec![
            "downloads".into(),
            "--isrc".into(),
            "USAB12345678".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
        ],
        url.clone(),
        config.clone(),
    );
    assert_eq!(code, 0, "seed run: {out}");
    let hits_before = state.lock().unwrap().hits.len();

    // No ISRCs → status only.
    let (code, out) = run_in_blocking(vec!["downloads".into()], url, config);
    assert_eq!(code, 0, "status: {out}");
    assert_eq!(
        state.lock().unwrap().hits.len(),
        hits_before,
        "status report must not touch the network"
    );
    assert!(out.contains("mock-order-1"), "status lists the order: {out}");
    assert!(out.contains("USAB12345678"), "status lists the fetched ISRC: {out}");
}