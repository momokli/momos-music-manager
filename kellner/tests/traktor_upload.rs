//! End-to-end integration tests for `kellner traktor-upload` (issue #364, P1-5).
//!
//! Drives the **real** `kellner` binary against a **local mock hub**
//! (`tokio::net::TcpListener` on `127.0.0.1:0`) that captures every HTTP
//! request. No network leaves the loopback interface.
//!
//! Covered acceptance criteria:
//! - a changed `collection.nml` produces exactly one `POST
//!   /api/hub/traktor/import` with `Authorization: Bearer <device token>`;
//! - an unchanged file (same mtime) produces **no** further upload;
//! - a modified file uploads again.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const DEVICE_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const NML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<NML VERSION="19">
 <COLLECTION ENTRIES="1">
  <ENTRY>
   <LOCATION DIR="/Music/" FILE="a.mp3" VOLUME="Mac"/>
   <TITLE>Alpha</TITLE>
   <ARTIST>Artist A</ARTIST>
   <INFO PLAYCOUNT="1" RATING="255"/>
  </ENTRY>
 </COLLECTION>
</NML>
"#;

/// A single captured HTTP request from kellner.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

type Hits = Arc<Mutex<Vec<Captured>>>;

/// Start a long-lived mock hub on an ephemeral loopback port, always answering
/// `200 OK` with a fixed import-stats body.
async fn start_mock() -> (String, Hits) {
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
            sink.lock().unwrap().push(cap);

            let body = r#"{"ok":true,"stats":{"entries":1,"matched":1,"playlists":1,"with_bpm":0,"with_key":0}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
        String::from_utf8_lossy(&buf[head_end + 4..]).to_string()
    } else {
        String::new()
    };
    Captured {
        method,
        path,
        authorization,
        body,
    }
}

/// Spawn the built `kellner` binary with a hard 30 s hang guard.
fn run_kellner(args: &[&str], config_path: &std::path::Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kellner"));
    cmd.args(args)
        .env("KELLNER_CONFIG", config_path)
        .env_remove("KELLNER_HUB_URL")
        .env_remove("KELLNER_MUSIC_API_URL")
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

/// Write a paired-device config pointing at the mock hub.
fn write_config(config_path: &std::path::Path, hub_url: &str) {
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        config_path,
        format!(
            "hub_url = \"{hub_url}\"\n\n[device]\ndevice_id = 1\ndevice_name = \"test\"\ndevice_token = \"{DEVICE_TOKEN}\"\npaired_at = \"0\"\n"
        ),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_nml_uploads_once_unchanged_skips() {
    let (url, hits) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");
    write_config(&cfg, &url);

    let nml = tmp.path().join("collection.nml");
    std::fs::write(&nml, NML).unwrap();
    let nml_arg = nml.to_string_lossy().to_string();

    let args = ["traktor-upload", "--collection-nml", nml_arg.as_str(), "--once"];

    // 1st run: file changed vs. empty state → exactly one upload.
    let (code, out) = tokio::task::spawn_blocking({
        let cfg = cfg.clone();
        let a = args.map(str::to_string);
        move || {
            let refs: Vec<&str> = a.iter().map(String::as_str).collect();
            run_kellner(&refs, &cfg)
        }
    })
    .await
    .unwrap();
    assert_eq!(code, 0, "run 1 output: {out}");
    assert_eq!(hits.lock().unwrap().len(), 1, "changed NML must upload once");

    {
        let captured = hits.lock().unwrap().clone();
        let req = &captured[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api/hub/traktor/import");
        assert_eq!(
            req.authorization.as_deref(),
            Some(format!("Bearer {DEVICE_TOKEN}").as_str()),
            "device token sent as Bearer credential"
        );
        assert!(req.body.contains("<NML"), "raw NML body was uploaded");
    }

    // 2nd run: same file, same mtime → no re-upload.
    let (code, out) = tokio::task::spawn_blocking({
        let cfg = cfg.clone();
        let a = args.map(str::to_string);
        move || {
            let refs: Vec<&str> = a.iter().map(String::as_str).collect();
            run_kellner(&refs, &cfg)
        }
    })
    .await
    .unwrap();
    assert_eq!(code, 0, "run 2 output: {out}");
    assert_eq!(
        hits.lock().unwrap().len(),
        1,
        "unchanged NML must NOT re-upload"
    );

    // 3rd run: modified file → upload again.
    std::thread::sleep(Duration::from_millis(20));
    let modified = NML.replace("PLAYCOUNT=\"1\"", "PLAYCOUNT=\"9\"");
    assert_ne!(modified, NML, "modified fixture differs");
    std::fs::write(&nml, modified).unwrap();

    let (code, out) = tokio::task::spawn_blocking({
        let cfg = cfg.clone();
        let a = args.map(str::to_string);
        move || {
            let refs: Vec<&str> = a.iter().map(String::as_str).collect();
            run_kellner(&refs, &cfg)
        }
    })
    .await
    .unwrap();
    assert_eq!(code, 0, "run 3 output: {out}");
    assert_eq!(
        hits.lock().unwrap().len(),
        2,
        "modified NML must upload again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_nml_reports_not_found_without_panic() {
    let (url, hits) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kellner").join("config.toml");
    write_config(&cfg, &url);

    let missing = tmp.path().join("does-not-exist.nml");
    let missing_arg = missing.to_string_lossy().to_string();

    let (code, out) = tokio::task::spawn_blocking({
        let cfg = cfg.clone();
        move || run_kellner(&["traktor-upload", "--collection-nml", &missing_arg, "--once"], &cfg)
    })
    .await
    .unwrap();

    assert_eq!(code, 0, "missing NML is not an error: {out}");
    assert!(out.to_lowercase().contains("not found"), "clear message: {out}");
    assert_eq!(hits.lock().unwrap().len(), 0, "nothing uploaded");
}
