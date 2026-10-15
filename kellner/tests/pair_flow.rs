//! End-to-end integration tests for the `kellner pair` flow (issue #362).
//!
//! These tests exercise the **real** `kellner` binary against a **local mock
//! hub** (`tokio::net::TcpListener` on `127.0.0.1:0`). No network leaves the
//! loopback interface and no real hub is contacted.
//!
//! Covered acceptance criteria:
//! - kellner sends `POST /api/hub/devices` with body keys exactly
//!   `{name, pairing_token, device_token}` (canonical wire protocol, #363).
//! - the approval URL is joined correctly (`hub_url` + `approval_path`).
//! - `--print-only` registers + prints but persists **nothing**.
//! - non-2xx hub responses make kellner abort with an error that never leaks
//!   the raw tokens.
//! - an already-paired device aborts without `--force` **before** any hub call.
//! - the `--wait` / `--no-open` paths complete without a TTY (no hang).

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A single captured HTTP request from kellner.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    body: String,
}

type Hits = Arc<Mutex<Vec<Captured>>>;

/// Start a one-connection-at-a-time mock hub on an ephemeral loopback port.
///
/// `status_line` e.g. `"200 OK"`; `respond` builds the response body from the
/// captured request (so a 200 can echo the pairing token the hub received).
/// Returns the base URL and the shared hit log. The accept loop lives until the
/// test process ends.
async fn start_mock<F>(status_line: &'static str, respond: F) -> (String, Hits)
where
    F: Fn(&Captured) -> String + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let hits: Hits = Arc::new(Mutex::new(Vec::new()));
    let sink = hits.clone();

    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
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
            let body = respond(&cap);
            sink.lock().unwrap().push(cap);

            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        }
    });

    (url, hits)
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
    let body = if head_end + 4 <= buf.len() {
        String::from_utf8_lossy(&buf[head_end + 4..]).to_string()
    } else {
        String::new()
    };
    Captured { method, path, body }
}

/// Spawn the built `kellner` binary with a hard 30 s hang guard.
///
/// stdin is `/dev/null` so no path can block on a TTY Enter prompt; stdout and
/// stderr are drained on separate threads to avoid pipe deadlocks.
fn run_kellner(args: &[&str], config_path: &std::path::Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kellner"));
    cmd.args(args)
        .env("KELLNER_CONFIG", config_path)
        .env_remove("KELLNER_HUB_URL")
        .env_remove("KELLNER_MUSIC_API_URL")
        .env_remove("KELLNER_COLLECTION_DIR")
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

fn tokens_from(body: &str) -> (String, String) {
    let v: serde_json::Value = serde_json::from_str(body).expect("request body is JSON");
    let pairing = v["pairing_token"].as_str().expect("pairing_token").to_string();
    let device = v["device_token"].as_str().expect("device_token").to_string();
    (pairing, device)
}

fn ok_responder(cap: &Captured) -> String {
    let (pairing, _) = tokens_from(&cap.body);
    format!(
        r#"{{"device_id":4242,"approval_path":"/kellner/anheuern?token={pairing}"}}"#
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn print_only_sends_canonical_request_and_persists_nothing() {
    let (url, hits) = start_mock("200 OK", ok_responder).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || {
            run_kellner(
                &[
                    "pair",
                    "--hub-url",
                    &url,
                    "--name",
                    "e2e-device",
                    "--no-open",
                    "--print-only",
                ],
                &cfg,
            )
        }
    })
    .await
    .unwrap();

    assert_eq!(code, 0, "pair --print-only should exit 0; output:\n{output}");
    assert!(!cfg.exists(), "--print-only must not write a config file");

    let recorded = hits.lock().unwrap();
    assert_eq!(recorded.len(), 1, "exactly one hub request expected");
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].path, "/api/hub/devices");

    // Body keys must be exactly the three hub fields.
    let v: serde_json::Value = serde_json::from_str(&recorded[0].body).unwrap();
    let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["device_token", "name", "pairing_token"]);
    assert_eq!(v["name"], "e2e-device");
    let (pairing, device) = tokens_from(&recorded[0].body);
    assert_eq!(pairing.len(), 64);
    assert_eq!(device.len(), 64);

    // Approval URL is joined correctly and the device token never surfaces.
    assert!(
        output.contains(&format!("{url}/kellner/anheuern?token={pairing}")),
        "approval URL missing/incorrectly joined; output:\n{output}"
    );
    assert!(output.contains("device_id: 4242"));
    assert!(!output.contains(&device), "device_token leaked to output");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_persists_identity_end_to_end_without_tty() {
    let (url, hits) = start_mock("200 OK", ok_responder).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || {
            run_kellner(
                &["pair", "--hub-url", &url, "--name", "real-device", "--no-open"],
                &cfg,
            )
        }
    })
    .await
    .unwrap();

    assert_eq!(code, 0, "pair (non-print-only) should exit 0; output:\n{output}");
    let recorded = hits.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let (_, device) = tokens_from(&recorded[0].body);
    drop(recorded);

    let text = std::fs::read_to_string(&cfg).expect("config persisted");
    let v: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(v["device"]["device_id"].as_integer(), Some(4242));
    assert_eq!(v["device"]["device_name"].as_str(), Some("real-device"));
    assert_eq!(v["device"]["device_token"].as_str(), Some(device.as_str()));
    assert_eq!(v["hub_url"].as_str(), Some(url.as_str()));

    // The persisted token must never appear in the process output.
    assert!(!output.contains(&device), "device_token leaked to output");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_without_tty_completes_without_hanging() {
    // `--wait` with a non-TTY stdin (null): the Enter read hits EOF → returns,
    // so the flow completes and persists. The 30 s guard in run_kellner proves
    // it does not hang.
    let (url, hits) = start_mock("200 OK", ok_responder).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || {
            run_kellner(
                &["pair", "--hub-url", &url, "--name", "wait-device", "--no-open", "--wait"],
                &cfg,
            )
        }
    })
    .await
    .unwrap();

    assert_eq!(code, 0, "pair --wait without TTY should exit 0; output:\n{output}");
    assert!(cfg.exists(), "--wait path should still persist the identity");
    assert_eq!(hits.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborts_on_hub_400_without_token_leak() {
    let (url, hits) = start_mock("400 Bad Request", |_| r#"{"error":"bad request"}"#.to_string()).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || {
            run_kellner(
                &["pair", "--hub-url", &url, "--name", "neg-device", "--no-open", "--print-only"],
                &cfg,
            )
        }
    })
    .await
    .unwrap();

    assert_ne!(code, 0, "kellner must fail on HTTP 400; output:\n{output}");
    let recorded = hits.lock().unwrap();
    assert_eq!(recorded.len(), 1, "the hub was contacted once before failing");
    let (pairing, device) = tokens_from(&recorded[0].body);
    drop(recorded);

    assert!(!output.contains(&pairing), "pairing_token leaked in error");
    assert!(!output.contains(&device), "device_token leaked in error");
    assert!(!cfg.exists(), "failed pair must not persist a config file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborts_on_hub_500_without_token_leak() {
    let (url, hits) = start_mock("500 Internal Server Error", |_| r#"{"error":"boom"}"#.to_string()).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || {
            run_kellner(
                &["pair", "--hub-url", &url, "--name", "neg-device", "--no-open", "--print-only"],
                &cfg,
            )
        }
    })
    .await
    .unwrap();

    assert_ne!(code, 0, "kellner must fail on HTTP 500; output:\n{output}");
    let recorded = hits.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let (pairing, device) = tokens_from(&recorded[0].body);
    drop(recorded);

    assert!(!output.contains(&pairing), "pairing_token leaked in error");
    assert!(!output.contains(&device), "device_token leaked in error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn already_paired_without_force_aborts_before_hub_call() {
    let (url, hits) = start_mock("200 OK", ok_responder).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();

    // Pre-seed a stored identity.
    std::fs::write(
        &cfg,
        format!(
            "hub_url = \"https://old.example.test\"\n\n[device]\ndevice_id = 7\ndevice_name = \"old\"\ndevice_token = \"keep-me\"\npaired_at = \"1\"\n"
        ),
    )
    .unwrap();

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || run_kellner(&["pair", "--hub-url", &url, "--no-open"], &cfg)
    })
    .await
    .unwrap();

    assert_ne!(code, 0, "re-pair without --force must fail; output:\n{output}");
    assert!(
        output.to_lowercase().contains("already paired"),
        "expected an 'already paired' hint; output:\n{output}"
    );
    assert_eq!(
        hits.lock().unwrap().len(),
        0,
        "the guard must abort before contacting the hub"
    );
    // The stored identity is left untouched.
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["device"]["device_id"].as_integer(), Some(7));
    assert_eq!(v["device"]["device_token"].as_str(), Some("keep-me"));
}

/// `--force` re-pairs an already-stored device: the hub is contacted and the
/// identity is overwritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_repairs_over_stored_identity() {
    let (url, hits) = start_mock("200 OK", ok_responder).await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg,
        "hub_url = \"https://old.example.test\"\n\n[device]\ndevice_id = 7\ndevice_name = \"old\"\ndevice_token = \"old-token\"\npaired_at = \"1\"\n",
    )
    .unwrap();

    let (code, output) = tokio::task::spawn_blocking({
        let url = url.clone();
        let cfg = cfg.clone();
        move || {
            run_kellner(
                &["pair", "--hub-url", &url, "--name", "new-device", "--no-open", "--force"],
                &cfg,
            )
        }
    })
    .await
    .unwrap();

    assert_eq!(code, 0, "pair --force should exit 0; output:\n{output}");
    assert_eq!(hits.lock().unwrap().len(), 1);
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["device"]["device_id"].as_integer(), Some(4242));
    assert_eq!(v["device"]["device_name"].as_str(), Some("new-device"));
}
