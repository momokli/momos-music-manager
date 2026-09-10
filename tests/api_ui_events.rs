//! UI-events API — integration tests (plan US4/E6).
//!
//! Coverage:
//!
//! 1. `POST /api/ui-events` is **always 204** — disabled flag, unknown
//!    type, `log.entry`, non-object/invalid payloads, garbage body — never
//!    a 4xx (the SPA hook is fire-and-forget).
//! 2. End-to-end flow with a running pipeline + process-wide emitter:
//!    view opens are ingested; the six user-triggered handlers emit exactly
//!    one `ui.action.*` per call (success AND error responses); flag off →
//!    no emission.
//!
//! NOTE on the global emitter: `emit::install()` keeps the first emitter
//! (there is no public uninstall for integration tests), so exactly ONE
//! test in this binary may install it — all pipeline-observing assertions
//! live in [`ui_events_flow_end_to_end`]. Other tests never install and
//! must not assert on pipeline contents.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{Pool, Sqlite, SqlitePool};

use momos_music_manager::AppState;
use momos_music_manager::config::ServiceCredentials;
use momos_music_manager::telemetry::flusher::{
    FlusherConfig, PipelineEnv, spawn as spawn_pipeline,
};
use momos_music_manager::telemetry::receiver::{ReceiverState, build_router, init_telemetry_db};

const TOKEN: &str = "ui-events-secret";

fn app_state_with_ui_flag(pool: Pool<Sqlite>, ui_events_enabled: bool) -> Arc<AppState> {
    let mut config = ServiceCredentials::defaults_for_test();
    config.telemetry_ui_events_enabled = ui_events_enabled;
    Arc::new(AppState {
        db: pool,
        config,
        task_manager: momos_music_manager::tasks::TaskManager::new(),
        embeddings: tokio::sync::Mutex::new(None),
        category_means: tokio::sync::Mutex::new(None),
        public_url: None,
    })
}

async fn create_db() -> Pool<Sqlite> {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .unwrap();
    // Main-chain migrations (same loop as tests/common).
    let mut dir = tokio::fs::read_dir("migrations").await.unwrap();
    let mut files = Vec::new();
    while let Some(entry) = dir.next_entry().await.unwrap() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "sql") {
            files.push(path);
        }
    }
    files.sort();
    for path in &files {
        let sql = tokio::fs::read_to_string(path).await.unwrap();
        let sql = sql.trim();
        if !sql.is_empty() {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
    }
    momos_music_manager::db::ensure_backpack_column(&pool)
        .await
        .unwrap();
    pool
}

/// Spawn the full app router on a random port.
async fn spawn_app(state: Arc<AppState>) -> String {
    let app = momos_music_manager::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// Spawn the telemetry receiver on a random port → (base, telemetry.db pool).
async fn spawn_receiver(dir: &tempfile::TempDir) -> (String, Pool<Sqlite>) {
    let db_path = dir.path().join("telemetry.db");
    let pool = init_telemetry_db(&db_path).await.unwrap();
    let state = Arc::new(ReceiverState::new(
        dir.path().join("snapshots"),
        Some(TOKEN.to_string()),
        Some(pool.clone()),
        30,
    ));
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), pool)
}

/// Poll until the query returns exactly `expected` rows for client `client`.
async fn wait_for_count(pool: &Pool<Sqlite>, client: &str, kind: &str, expected: i64) {
    for _ in 0..200 {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE client_id = ? AND type = ?",
        )
        .bind(client)
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        if count == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE client_id = ? AND type = ?",
    )
    .bind(client)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap_or(-1);
    panic!("timed out: client={client} type={kind} expected {expected}, last {count}");
}

async fn assert_count(pool: &Pool<Sqlite>, client: &str, kind: &str, expected: i64) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE client_id = ? AND type = ?",
    )
    .bind(client)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, expected, "client={client} type={kind}");
}

// ── 1. Endpoint: always 204, never 4xx ────────────────────────────────────

#[tokio::test]
async fn endpoint_always_204_when_disabled() {
    let pool = create_db().await;
    let base = spawn_app(app_state_with_ui_flag(pool, false)).await;
    let client = reqwest::Client::new();

    for (label, body) in [
        ("valid view", serde_json::json!({"type": "ui.view.opened", "payload": {"view": "dashboard"}})),
        ("unknown type", serde_json::json!({"type": "ui.clicked", "payload": {}})),
        ("log.entry", serde_json::json!({"type": "log.entry", "payload": {"level": "warn"}})),
        ("non-object payload", serde_json::json!({"type": "ui.view.opened", "payload": [1, 2]})),
        ("invalid view id", serde_json::json!({"type": "ui.view.opened", "payload": {"view": "../etc"}})),
        ("missing payload", serde_json::json!({"type": "ui.view.opened"})),
    ] {
        let resp = client
            .post(format!("{base}/api/ui-events"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 204, "{label} must be a silent 204");
    }
    // Garbage body must also be 204 (never an axum 400/422).
    let resp = client
        .post(format!("{base}/api/ui-events"))
        .body("this is {{{ not json")
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "garbage body must be a silent 204");
}

#[tokio::test]
async fn endpoint_204_for_invalid_even_when_enabled() {
    // Flag on but NO pipeline installed — still 204, and no panic.
    let pool = create_db().await;
    let base = spawn_app(app_state_with_ui_flag(pool, true)).await;
    let client = reqwest::Client::new();

    for (label, body) in [
        ("unknown ui.action", serde_json::json!({"type": "ui.action.delete_everything", "payload": {}})),
        ("invalid view", serde_json::json!({"type": "ui.view.opened", "payload": {"view": "Dashboard"}})),
        ("log.entry via endpoint", serde_json::json!({"type": "log.entry", "payload": {}})),
        ("payload array", serde_json::json!({"type": "ui.action.scan_folder", "payload": []})),
        ("empty body object", serde_json::json!({})),
    ] {
        let resp = client
            .post(format!("{base}/api/ui-events"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 204, "{label}");
    }
}

// ── 2. End-to-end: view opens + handler emission (single-installer test) ──

#[tokio::test]
async fn ui_events_flow_end_to_end() {
    // Shutdown any leftover emitter (kept from a failed prior run in this
    // process) so install() below lands.
    momos_music_manager::telemetry::emit::shutdown_global();

    let receiver_dir = tempfile::tempdir().unwrap();
    let spool_dir = tempfile::tempdir().unwrap();
    let (receiver_base, pool) = spawn_receiver(&receiver_dir).await;

    let mut flusher_cfg = FlusherConfig::new(
        PipelineEnv {
            client_id: "client-ui".to_string(),
            app_version: "1.3.0-test".to_string(),
            os: "linux".to_string(),
        },
        format!("{receiver_base}/api/telemetry"),
        Some(TOKEN.to_string()),
        spool_dir.path().to_path_buf(),
    );
    flusher_cfg.flush_interval = Duration::from_millis(100);
    flusher_cfg.initial_backoff = Duration::from_millis(50);
    let pipeline = spawn_pipeline(flusher_cfg);
    let emitter = momos_music_manager::telemetry::emit::EventEmitter::new(
        PipelineEnv {
            client_id: "client-ui".to_string(),
            app_version: "1.3.0-test".to_string(),
            os: "linux".to_string(),
        },
        pipeline,
    );
    momos_music_manager::telemetry::emit::install(emitter);

    let client = reqwest::Client::new();

    // ── a) Endpoint with flag ON: view open is ingested ──
    let db_on = create_db().await;
    let base_on = spawn_app(app_state_with_ui_flag(db_on.clone(), true)).await;
    let resp = client
        .post(format!("{base_on}/api/ui-events"))
        .json(&serde_json::json!({"type": "ui.view.opened", "payload": {"view": "dashboard"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    wait_for_count(&pool, "client-ui", "ui.view.opened", 1).await;

    // ── b) scan_folder: 404 → exactly one ok:false event ──
    let resp = client
        .post(format!("{base_on}/api/folders/999/scan"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    wait_for_count(&pool, "client-ui", "ui.action.scan_folder", 1).await;

    // ── c) scan_folder: existing folder → 200 + exactly one ok:true event ──
    sqlx::query(
        "INSERT INTO folders (id, folder_path, active, scan_recursive, max_depth) \
         VALUES (1, '/nonexistent-ui-test-dir', 1, 0, 1)",
    )
    .execute(&db_on)
    .await
    .unwrap();
    let resp = client
        .post(format!("{base_on}/api/folders/1/scan?mode=incremental"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    wait_for_count(&pool, "client-ui", "ui.action.scan_folder", 2).await;
    // Exactly one ok:false (404) + one ok:true (200) — one emit per call.
    let payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE client_id = 'client-ui' AND type = 'ui.action.scan_folder'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut ok_values: Vec<bool> = payloads
        .iter()
        .map(|p| serde_json::from_str::<serde_json::Value>(p).unwrap()["ok"] == true)
        .collect();
    ok_values.sort_unstable();
    assert_eq!(ok_values, vec![false, true]);

    // ── d) run_backup: folder without backup_path → 400 + ok:false ──
    let resp = client
        .post(format!("{base_on}/api/storage/backup/1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    wait_for_count(&pool, "client-ui", "ui.action.run_backup", 1).await;

    // ── e) restore_dump without confirm → 400 + ok:false (single event) ──
    // A proper multipart body is required for the Multipart extractor to
    // reach the handler (its rejection would otherwise 400 without emit).
    let boundary = "----ui-events-test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dump.json\"\r\nContent-Type: application/json\r\n\r\n{{}}\r\n--{boundary}--\r\n"
    );
    let resp = client
        .post(format!("{base_on}/api/restore"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .unwrap();
    if resp.status() != 400 {
        panic!("restore status {} body: {:?}", resp.status(), resp.text().await.unwrap());
    }
    wait_for_count(&pool, "client-ui", "ui.action.restore_dump", 1).await;

    // ── f) deemix_enqueue: empty URL → 400; valid URL → 200 (no deemix
    //        server configured in the test DB → local insert only) ──
    let resp = client
        .post(format!("{base_on}/api/services/deemix/queue"))
        .json(&serde_json::json!({"url": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = client
        .post(format!("{base_on}/api/services/deemix/queue"))
        .json(&serde_json::json!({"url": "https://open.spotify.com/playlist/123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    wait_for_count(&pool, "client-ui", "ui.action.deemix_enqueue", 2).await;

    // Give the flusher a moment to settle, then assert exact totals.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_count(&pool, "client-ui", "ui.view.opened", 1).await;
    assert_count(&pool, "client-ui", "ui.action.scan_folder", 2).await;
    assert_count(&pool, "client-ui", "ui.action.run_backup", 1).await;
    assert_count(&pool, "client-ui", "ui.action.restore_dump", 1).await;
    assert_count(&pool, "client-ui", "ui.action.deemix_enqueue", 2).await;
    assert_count(&pool, "client-ui", "ui.action.traktor_import", 0).await;
    assert_count(&pool, "client-ui", "ui.action.recompute_embeddings", 0).await;

    // ── g) Flag OFF: the same user action emits nothing ──
    let db_off = create_db().await;
    let base_off = spawn_app(app_state_with_ui_flag(db_off, false)).await;
    let resp = client
        .post(format!("{base_off}/api/folders/999/scan"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "handler behavior is unchanged by the flag");
    let resp = client
        .post(format!("{base_off}/api/ui-events"))
        .json(&serde_json::json!({"type": "ui.view.opened", "payload": {"view": "files"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Counts unchanged: flag off ⇒ the pipeline never saw these actions.
    assert_count(&pool, "client-ui", "ui.action.scan_folder", 2).await;
    assert_count(&pool, "client-ui", "ui.view.opened", 1).await;
}
