//! End-to-end harness for the **full kellner loop** (issue #370, plan phase P6-1).
//!
//! One in-process mock serves **both** server roles on a single ephemeral
//! loopback port (`tokio::net::TcpListener` on `127.0.0.1:0`):
//!
//! - **hub** (`/api/hub/…`) — gated by the *device token*:
//!   `POST /api/hub/traktor/import`, `GET /api/hub/backpacks`,
//!   `GET /api/hub/backpacks/{id}/export`;
//! - **music-api** — gated by the *music-api token*:
//!   `POST /orders`, `GET /tracks?isrcs=…`, `GET /isrc/{isrc}/flac`,
//!   `POST /objects/check`, `PUT /objects/{sha256}` (the mock re-hashes the body
//!   against the key, like the real object store).
//!
//! The loop is driven as process steps against the **real** `kellner` binary:
//! NML upload → backpack export → order → fetch → Comment tag → backup. Nothing
//! leaves the loopback interface; a 30 s hang guard wraps every run.
//!
//! Covered acceptance criteria (#370):
//! - (a) exactly one device-token `POST /api/hub/traktor/import` with the NML body;
//! - (b) the export yields one track → exactly one `POST /orders` + one
//!   `GET /isrc/{isrc}/flac`; the file lands flat as `<root>/<isrc>.flac`, the
//!   Comment tag equals the deterministic render and `backpack_membership` is set;
//! - (c) `backup` issues one `POST /objects/check` + one `PUT /objects/{sha256}`
//!   and the mock store holds the **canonical** hash (Comment cleared) — the tag
//!   write must not falsify the dedup key;
//! - determinism + idempotency: a second pass orders/fetches/uploads nothing.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const DEVICE_TOKEN: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const MUSIC_API_TOKEN: &str = "music-api-token-0123456789abcdef0123456789abcdef";

const ISRC: &str = "DEABC1234567";
const RENDERED_COMMENT: &str = "a, b  BPM 128 | A-Minor | Energy 7";

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
    /// Object keys the mock store currently holds.
    store: HashSet<String>,
}

type Shared = Arc<Mutex<MockState>>;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn flac_fixture() -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("test.flac"),
    )
    .unwrap()
}

/// Start the combined mock hub + music-api on an ephemeral loopback port.
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
    let json_ct = "application/json".to_string();

    // ── hub routes: device-token gated ──────────────────────────────────────
    if cap.path.starts_with("/api/hub/") {
        if cap.authorization.as_deref() != Some(&format!("Bearer {DEVICE_TOKEN}")) {
            return (
                "401 Unauthorized".into(),
                json_ct,
                b"{\"error\":\"unauthorized\"}".to_vec(),
            );
        }
        if cap.method == "POST" && cap.path == "/api/hub/traktor/import" {
            let body =
                r#"{"ok":true,"stats":{"entries":1,"matched":1,"playlists":1,"with_bpm":0,"with_key":0}}"#;
            return ("200 OK".into(), json_ct, body.as_bytes().to_vec());
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
                        None => (
                            "404 Not Found".into(),
                            json_ct,
                            b"{\"error\":\"unknown backpack\"}".to_vec(),
                        ),
                    };
                }
            }
        }
        return (
            "404 Not Found".into(),
            json_ct,
            b"{\"error\":\"not found\"}".to_vec(),
        );
    }

    // ── music-api routes: music-api-token gated ─────────────────────────────
    if cap.authorization.as_deref() != Some(&format!("Bearer {MUSIC_API_TOKEN}")) {
        return (
            "401 Unauthorized".into(),
            json_ct,
            b"{\"error\":\"unauthorized\"}".to_vec(),
        );
    }

    let (path, query) = match cap.path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (cap.path.clone(), String::new()),
    };

    // Object store: bulk check + content-addressed PUT (re-hash like the server).
    if cap.method == "POST" && path == "/objects/check" {
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
        return ("200 OK".into(), json_ct, body.to_string().into_bytes());
    }

    if cap.method == "PUT" {
        if let Some(hash) = path.strip_prefix("/objects/") {
            if sha256_hex(&cap.body) != hash {
                return (
                    "400 Bad Request".into(),
                    json_ct,
                    b"{\"error\":\"sha256 of the body does not match the key\"}".to_vec(),
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
            return (status.into(), json_ct, body.to_string().into_bytes());
        }
    }

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

    if cap.method == "GET" && path == "/tracks" {
        let isrcs = parse_isrcs(&query);
        let mut tracks = Vec::new();
        for isrc in isrcs {
            tracks.push(serde_json::json!({ "isrc": isrc, "state": "ready", "formats": ["flac"] }));
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
            return (
                "404 Not Found".into(),
                json_ct,
                b"{\"error\":\"unsupported format\"}".to_vec(),
            );
        }
    }

    (
        "404 Not Found".into(),
        json_ct,
        b"{\"error\":\"not found\"}".to_vec(),
    )
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

/// Number of captured requests matching `method` and an exact `path`.
fn hits_of(shared: &Shared, method: &str, path: &str) -> usize {
    shared
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|h| h.method == method && h.path == path)
        .count()
}

fn order_count(shared: &Shared) -> usize {
    hits_of(shared, "POST", "/orders")
}

fn fetch_count(shared: &Shared) -> usize {
    shared
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|h| h.method == "GET" && h.path.starts_with("/isrc/"))
        .count()
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

fn check_count(shared: &Shared) -> usize {
    hits_of(shared, "POST", "/objects/check")
}

/// Write a paired-device config + hub URL, pointing everything at `url`.
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

fn state_json(config_path: &Path) -> serde_json::Value {
    let path = config_path.with_file_name("state.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// One active backpack with one deterministically tagged track.
fn seed_one_track(shared: &Shared) {
    let mut st = shared.lock().unwrap();
    st.backpacks = vec![serde_json::json!({ "id": 1, "name": "Party", "is_active": true })];
    st.exports.insert(
        1,
        serde_json::json!({
            "backpack": { "id": 1, "name": "Party" },
            "tracks": [{
                "track_id": 42,
                "isrc": ISRC,
                "title": "t",
                "artists": "a",
                "tags": ["b", "a"],
                "bpm": 128.0,
                "key": "A-Minor",
                "energy": 7.0,
            }],
        }),
    );
}

// Full loop: upload → export → order → fetch → tag → backup, then idempotent ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_loop_upload_export_order_fetch_tag_backup() {
    let (url, state) = start_mock().await;
    let tmp = tempfile::tempdir().unwrap();
    let coll = tmp.path().join("collection");
    std::fs::create_dir_all(&coll).unwrap();
    let config = tmp.path().join("kellner").join("config.toml");
    write_config(&config, &url);

    // Deterministic NML for the Traktor upload step.
    let nml = tmp.path().join("collection.nml");
    std::fs::write(&nml, NML).unwrap();
    let nml_arg = nml.to_string_lossy().to_string();

    seed_one_track(&state);

    // ── (a) NML upload: exactly one device-token POST with the NML body ─────
    let (code, out) = run_in_blocking(
        vec!["traktor-upload".into(), "--collection-nml".into(), nml_arg, "--once".into()],
        url.clone(),
        config.clone(),
    );
    assert_eq!(code, 0, "traktor-upload: {out}");
    assert_eq!(
        hits_of(&state, "POST", "/api/hub/traktor/import"),
        1,
        "exactly one Traktor import"
    );
    {
        let hits = state.lock().unwrap().hits.clone();
        let import = hits
            .iter()
            .find(|h| h.path == "/api/hub/traktor/import")
            .expect("import captured");
        assert_eq!(
            import.authorization.as_deref(),
            Some(format!("Bearer {DEVICE_TOKEN}").as_str()),
            "device token sent as Bearer credential"
        );
        assert!(
            find_sub(&import.body, b"<NML").is_some(),
            "raw NML body was uploaded"
        );
    }

    // ── (b) backpack: export → one order → one fetch → flat file + tag ──────
    let (code, out) = run_in_blocking(
        vec![
            "backpack".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
            "--poll-secs".into(),
            "1".into(),
        ],
        url.clone(),
        config.clone(),
    );
    assert_eq!(code, 0, "backpack: {out}");
    assert_eq!(order_count(&state), 1, "exactly one order");
    assert_eq!(fetch_count(&state), 1, "exactly one file fetch");

    let flat = coll.join(format!("{ISRC}.flac"));
    assert!(flat.is_file(), "flat file written at {}", flat.display());

    // The Comment tag in the file matches the deterministic render.
    {
        use lofty::prelude::*;
        let tagged = lofty::read_from_path(&flat).unwrap();
        let comment = tagged
            .primary_tag()
            .and_then(|t| t.comment())
            .map(|c| c.to_string());
        assert_eq!(comment.as_deref(), Some(RENDERED_COMMENT));
    }

    // Membership + canonical content hash recorded in state.json.
    let sj = state_json(&config);
    let m = &sj["backpack_membership"][ISRC];
    assert!(m.is_object(), "membership recorded: {sj}");
    assert_eq!(m["backpack_id"], 1);
    assert_eq!(m["tags_rendered"], RENDERED_COMMENT);
    let membership_hash = m["content_hash"]
        .as_str()
        .expect("content hash recorded")
        .to_string();
    assert_eq!(membership_hash.len(), 64, "sha256 hex key");

    // ── (c) backup: one check + one PUT of the canonical hash ───────────────
    let (code, out) = run_in_blocking(
        vec![
            "backup".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
            "--once".into(),
        ],
        url.clone(),
        config.clone(),
    );
    assert_eq!(code, 0, "backup: {out}");
    assert_eq!(check_count(&state), 1, "one bulk object check");
    assert_eq!(put_count(&state), 1, "one object uploaded");
    {
        let st = state.lock().unwrap();
        assert!(
            st.store.contains(&membership_hash),
            "the mock store holds the canonical (Comment-cleared) hash — the tag write must not falsify the dedup key"
        );
    }

    // ── Idempotency: a second pass orders/fetches/uploads nothing ───────────
    let (code, out) = run_in_blocking(
        vec![
            "backpack".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
            "--poll-secs".into(),
            "1".into(),
        ],
        url.clone(),
        config.clone(),
    );
    assert_eq!(code, 0, "backpack run 2: {out}");
    assert_eq!(order_count(&state), 1, "second pass must not order");
    assert_eq!(fetch_count(&state), 1, "second pass must not fetch");

    let (code, out) = run_in_blocking(
        vec![
            "backup".into(),
            "--root".into(),
            coll.to_string_lossy().to_string(),
            "--once".into(),
        ],
        url,
        config,
    );
    assert_eq!(code, 0, "backup run 2: {out}");
    assert_eq!(put_count(&state), 1, "second pass must not re-upload");
}
