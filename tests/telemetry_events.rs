//! Telemetry events — integration tests (US7).
//!
//! End-to-end coverage of the event pipeline against a real local receiver:
//!
//! 1. emit → spool → flush → ingest → dedup (HTTP, temp dirs)
//! 2. offline survival: events stay in the spool and are delivered after a
//!    "restart" (new pipeline, same spool dir)
//! 3. SQL views aggregate seeded/ingested events correctly
//! 4. TaskManager lifecycle hooks emit exactly one started + terminal event
//!    per task (uses the process-wide emitter — see the note below).
//!
//! NOTE on test isolation: exactly one test here ([`task_lifecycle_events`])
//! installs the process-wide emitter; all other tests use local pipelines via
//! [`TelemetryPipeline::emit`], so the global stays untouched. Do NOT drive
//! TaskManager/scan/download flows from other tests in this binary while the
//! global emitter is installed (Rust runs tests in this file concurrently).

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use sqlx::{Pool, Row, Sqlite};
use tokio::net::TcpListener;

use momos_music_manager::telemetry::events::{EventBatch, EventType, TelemetryEvent};
use momos_music_manager::telemetry::flusher::{
    FlusherConfig, PipelineEnv, spawn as spawn_pipeline,
};
use momos_music_manager::telemetry::receiver::{ReceiverState, build_router, init_telemetry_db};

const TOKEN: &str = "integration-secret";

fn stamped_event(id: &str, client: &str, r#type: EventType, payload: serde_json::Value) -> TelemetryEvent {
    let mut event = TelemetryEvent::new(r#type, payload).with_envelope(client, "1.1.0-test", "linux");
    event.event_id = id.to_string();
    event
}

/// Spawn the receiver on a random local port; returns base URL + telemetry.db pool.
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), pool)
}

/// Flusher config pointing at a receiver with fast cadence (tests).
fn test_flusher_config(base_url: &str, spool_dir: &std::path::Path) -> FlusherConfig {
    let mut cfg = FlusherConfig::new(
        PipelineEnv {
            client_id: "client-x".to_string(),
            app_version: "1.1.0-test".to_string(),
            os: "linux".to_string(),
        },
        format!("{}/api/telemetry", base_url.trim_end_matches('/')),
        Some(TOKEN.to_string()),
        spool_dir.to_path_buf(),
    );
    cfg.flush_interval = Duration::from_millis(100);
    cfg.initial_backoff = Duration::from_millis(50);
    cfg
}

/// Poll `query` until it returns >= `expected` (or timeout + panic).
async fn wait_for_count(pool: &Pool<Sqlite>, query: &str, expected: i64) {
    for _ in 0..200 {
        let count: i64 = sqlx::query_scalar(query).fetch_one(pool).await.unwrap_or(0);
        if count >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let count: i64 = sqlx::query_scalar(query).fetch_one(pool).await.unwrap_or(-1);
    panic!("timed out waiting for {query} to reach {expected} (last: {count})");
}

// ── 1. E2E: emit → spool → flush → ingest → dedup ─────────────────────────

#[tokio::test]
async fn e2e_emit_spool_flush_ingest_dedup() {
    let receiver_dir = tempfile::tempdir().unwrap();
    let spool_dir = tempfile::tempdir().unwrap();
    let (base, pool) = spawn_receiver(&receiver_dir).await;

    let cfg = test_flusher_config(&base, spool_dir.path());
    let pipeline = spawn_pipeline(cfg);

    let events = vec![
        stamped_event(
            "10000000-0000-4000-8000-000000000001",
            "client-x",
            EventType::TaskStarted,
            json!({ "task_type": "scan_folder" }),
        ),
        stamped_event(
            "10000000-0000-4000-8000-000000000002",
            "client-x",
            EventType::TaskCompleted,
            json!({ "task_type": "scan_folder", "duration_ms": 500 }),
        ),
        stamped_event(
            "10000000-0000-4000-8000-000000000003",
            "client-x",
            EventType::ScanCompleted,
            json!({ "files_count": 12, "duration_ms": 500, "mode": "full" }),
        ),
    ];
    for event in &events {
        assert!(pipeline.emit(event.clone()), "emit must succeed");
    }

    // Wait until all three are ingested…
    wait_for_count(&pool, "SELECT COUNT(*) FROM events", 3).await;
    // …and the spool file is drained (ACK → compaction).
    wait_for_spool_empty(spool_dir.path()).await;

    // Dedup: resend the same batch → 202 with accepted=0, duplicates=3.
    let client = reqwest::Client::new();
    let batch = EventBatch::new("client-x", events);
    let resp = client
        .post(format!("{base}/api/telemetry"))
        .bearer_auth(TOKEN)
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 0);
    assert_eq!(body["duplicates"], 3);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3, "dedup must keep exactly 3 events");

    // Client row upserted.
    let clients: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clients")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(clients, 1);

    pipeline.shutdown().await;
}

async fn wait_for_spool_empty(spool_dir: &std::path::Path) {
    let spool_file = spool_dir.join("telemetry-events.jsonl");
    for _ in 0..100 {
        let content = tokio::fs::read_to_string(&spool_file)
            .await
            .unwrap_or_default();
        if content.trim().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let content = tokio::fs::read_to_string(&spool_file)
        .await
        .unwrap_or_default();
    panic!("spool not drained after ACK, still has: {content}");
}

#[tokio::test]
async fn ingest_rejects_bad_auth_and_bad_payload() {
    let dir = tempfile::tempdir().unwrap();
    let (base, _pool) = spawn_receiver(&dir).await;
    let client = reqwest::Client::new();

    // No token → 401.
    let resp = client
        .post(format!("{base}/api/telemetry"))
        .json(&EventBatch::new("client-x", vec![]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Wrong token → 401.
    let resp = client
        .post(format!("{base}/api/telemetry"))
        .bearer_auth("nope")
        .json(&EventBatch::new("client-x", vec![]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Valid auth but empty batch → 400.
    let resp = client
        .post(format!("{base}/api/telemetry"))
        .bearer_auth(TOKEN)
        .json(&EventBatch::new("client-x", vec![]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── 2. Offline survival + spool reload after restart ──────────────────────

#[tokio::test]
async fn offline_events_survive_restart_and_are_delivered() {
    // Endpoint that is NOT listening → flusher keeps retrying, events spool.
    let dead_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead_listener.local_addr().unwrap();
    drop(dead_listener); // port now closed

    let spool_dir = tempfile::tempdir().unwrap();
    let mut offline_cfg = test_flusher_config(&format!("http://{dead_addr}"), spool_dir.path());
    offline_cfg.initial_backoff = Duration::from_millis(50);
    let pipeline = spawn_pipeline(offline_cfg);

    let event = stamped_event(
        "20000000-0000-4000-8000-000000000001",
        "client-x",
        EventType::TaskFailed,
        json!({ "task_type": "backup_folder", "duration_ms": 10 }),
    );
    assert!(pipeline.emit(event));

    // Give the worker time to append to the spool (no server to flush to).
    tokio::time::sleep(Duration::from_millis(400)).await;
    let spool_file = spool_dir.path().join("telemetry-events.jsonl");
    let spool_content = tokio::fs::read_to_string(&spool_file)
        .await
        .unwrap_or_default();
    assert!(
        spool_content.contains("20000000-0000-4000-8000-000000000001"),
        "event must be spooled while offline"
    );

    // "Restart": drop the old pipeline (its worker is killed with the test
    // runtime), then start a fresh one against a live receiver.
    drop(pipeline);

    let receiver_dir = tempfile::tempdir().unwrap();
    let (base, pool) = spawn_receiver(&receiver_dir).await;
    let cfg = test_flusher_config(&base, spool_dir.path());
    let pipeline = spawn_pipeline(cfg);

    wait_for_count(&pool, "SELECT COUNT(*) FROM events", 1).await;
    let type_col: String = sqlx::query_scalar(
        "SELECT type FROM events WHERE event_id = '20000000-0000-4000-8000-000000000001'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(type_col, "task.failed");

    pipeline.shutdown().await;
}

// ── 3. Views aggregate seeded events ───────────────────────────────────────

async fn seed_event(
    pool: &Pool<Sqlite>,
    event_id: &str,
    client: &str,
    r#type: &str,
    ts: i64,
    payload: serde_json::Value,
    now: i64,
) {
    sqlx::query(
        "INSERT OR IGNORE INTO clients (client_id, first_seen_at, last_seen_at, last_app_version, last_os) \
         VALUES (?, ?, ?, '1.1.0', 'linux')",
    )
    .bind(client)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO events (event_id, client_id, type, ts, received_at, app_version, os, payload) \
         VALUES (?, ?, ?, ?, ?, '1.1.0', 'linux', ?)",
    )
    .bind(event_id)
    .bind(client)
    .bind(r#type)
    .bind(ts)
    .bind(now)
    .bind(payload.to_string())
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn views_aggregate_seeded_events() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("telemetry.db");
    let pool = init_telemetry_db(&db_path).await.unwrap();

    let now = chrono::Utc::now().timestamp();
    let hour_ts = now - now.rem_euclid(3600); // start of current hour

    // Client A: 2 completed + 1 failed tasks, 3 scans, deemix + spotdl downloads.
    seed_event(
        &pool, "a-task-c1", "client-a", "task.completed", hour_ts,
        json!({ "task_type": "scan_folder", "duration_ms": 400 }), now,
    )
    .await;
    seed_event(
        &pool, "a-task-c2", "client-a", "task.completed", hour_ts,
        json!({ "task_type": "deemix_sync", "duration_ms": 900 }), now,
    )
    .await;
    seed_event(
        &pool, "a-task-f1", "client-a", "task.failed", hour_ts + 60,
        json!({ "task_type": "backup_folder", "duration_ms": 50 }), now,
    )
    .await;
    seed_event(
        &pool, "a-scan-1", "client-a", "scan.completed", hour_ts,
        json!({ "files_count": 3, "duration_ms": 100, "mode": "full" }), now,
    )
    .await;
    seed_event(
        &pool, "a-scan-2", "client-a", "scan.completed", hour_ts + 120,
        json!({ "files_count": 7, "duration_ms": 300, "mode": "full" }), now,
    )
    .await;
    seed_event(
        &pool, "a-scan-3", "client-a", "scan.completed", hour_ts + 240,
        json!({ "files_count": 1, "duration_ms": 900, "mode": "incremental" }), now,
    )
    .await;
    seed_event(
        &pool, "a-dl-deemix", "client-a", "download.completed", hour_ts,
        json!({ "source": "deemix", "kind": "playlist" }), now,
    )
    .await;
    seed_event(
        &pool, "a-dl-spotdl", "client-a", "download.failed", hour_ts,
        json!({ "source": "spotdl", "kind": "track" }), now,
    )
    .await;

    // Client B: quiet client (only started a task) → no completed/failed.
    seed_event(
        &pool, "b-task-s1", "client-b", "task.started", hour_ts,
        json!({ "task_type": "scan_folder" }), now,
    )
    .await;

    // v_tasks_per_hour
    let rows = sqlx::query(
        "SELECT client_id, task_type, event_type, events FROM v_tasks_per_hour \
         WHERE client_id = 'client-a' AND event_type = 'task.completed' ORDER BY task_type",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "two completed task types for client-a");
    let first: String = rows[0].get("task_type");
    let n: i64 = rows[0].get("events");
    assert_eq!((first.as_str(), n), ("deemix_sync", 1));

    // v_error_rate: client-a 2 completed / 1 failed → 50%
    let row = sqlx::query(
        "SELECT failed, completed, error_rate_pct FROM v_error_rate WHERE client_id = 'client-a'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let failed: i64 = row.get("failed");
    let completed: i64 = row.get("completed");
    let pct: f64 = row.get("error_rate_pct");
    assert_eq!((failed, completed), (1, 2));
    assert!((pct - 50.0).abs() < 0.01, "error rate {pct} != 50");

    // v_downloads_by_source: deemix completed + spotdl failed
    let rows = sqlx::query(
        "SELECT source, event_type FROM v_downloads_by_source \
         WHERE client_id = 'client-a' ORDER BY source",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let s0: String = rows[0].get("source");
    let e0: String = rows[0].get("event_type");
    let s1: String = rows[1].get("source");
    assert_eq!((s0.as_str(), e0.as_str()), ("deemix", "download.completed"));
    assert_eq!(s1.as_str(), "spotdl");

    // v_scan_duration_trend: 3 scans, avg = (100+300+900)/3 = 433, p95 = 900
    let row = sqlx::query(
        "SELECT scans, avg_duration_ms, p95_duration_ms FROM v_scan_duration_trend \
         WHERE client_id = 'client-a'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let scans: i64 = row.get("scans");
    let avg: i64 = row.get("avg_duration_ms");
    let p95: i64 = row.get("p95_duration_ms");
    assert_eq!(scans, 3);
    assert_eq!(avg, 433);
    assert_eq!(p95, 900, "p95 of [100,300,900] should be 900");

    // v_clients_last_seen: both clients present
    let clients: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM v_clients_last_seen")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(clients, 2);

    // v_client_versions
    let row = sqlx::query(
        "SELECT last_app_version FROM v_client_versions WHERE client_id = 'client-a'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let v: String = row.get("last_app_version");
    assert_eq!(v, "1.1.0");
}

// ── 4. TaskManager lifecycle hooks (exactly-once per lifecycle) ────────────

#[tokio::test]
async fn task_lifecycle_events() {
    // This is the ONLY test in this binary that installs the global emitter.
    momos_music_manager::telemetry::emit::shutdown_global();

    let receiver_dir = tempfile::tempdir().unwrap();
    let spool_dir = tempfile::tempdir().unwrap();
    let (base, pool) = spawn_receiver(&receiver_dir).await;

    let cfg = test_flusher_config(&base, spool_dir.path());
    let pipeline = spawn_pipeline(cfg);
    let emitter = momos_music_manager::telemetry::emit::EventEmitter::new(
        PipelineEnv {
            client_id: "client-lifecycle".to_string(),
            app_version: "1.1.0-test".to_string(),
            os: "linux".to_string(),
        },
        pipeline,
    );
    momos_music_manager::telemetry::emit::install(emitter);

    let tm = momos_music_manager::tasks::TaskManager::new();

    // Task 1: completes successfully → started + completed.
    let task = momos_music_manager::tasks::Task::new(
        momos_music_manager::tasks::TaskType::BackpackSync,
        None,
    );
    let id = tm.start_task(task).await;
    tm.update_task_status(&id, momos_music_manager::tasks::TaskStatus::Running)
        .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    tm.update_task_status(&id, momos_music_manager::tasks::TaskStatus::Completed)
        .await;

    // Task 2: fails with an error message containing a home path → started +
    // failed, sanitized payload.
    let fail_task = momos_music_manager::tasks::Task::new(
        momos_music_manager::tasks::TaskType::TelemetryPush,
        None,
    );
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut err = fail_task.error_message.lock().unwrap();
        *err = Some(format!("{home}/secret/file.flac: boom"));
    }
    let fail_id = tm.start_task(fail_task).await;
    tm.update_task_status(&fail_id, momos_music_manager::tasks::TaskStatus::Running)
        .await;
    tm.update_task_status(&fail_id, momos_music_manager::tasks::TaskStatus::Failed)
        .await;

    // Wait for ingestion.
    wait_for_count(
        &pool,
        "SELECT COUNT(*) FROM events WHERE type = 'task.completed' AND client_id = 'client-lifecycle'",
        1,
    )
    .await;
    wait_for_count(
        &pool,
        "SELECT COUNT(*) FROM events WHERE type = 'task.failed' AND client_id = 'client-lifecycle'",
        1,
    )
    .await;
    wait_for_count(
        &pool,
        "SELECT COUNT(*) FROM events WHERE type = 'task.started' AND client_id = 'client-lifecycle'",
        2,
    )
    .await;

    // Exactly one completed + one failed; started exactly twice (once per task).
    let completed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE type = 'task.completed' AND client_id = 'client-lifecycle'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed, 1, "exactly one task.completed per lifecycle");

    let failed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE type = 'task.failed' AND client_id = 'client-lifecycle'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed, 1, "exactly one task.failed per lifecycle");

    // Sanitization: no absolute home path in the failed payload.
    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM events WHERE type = 'task.failed' AND client_id = 'client-lifecycle'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(
        !payload.contains(&home),
        "failed payload leaks home path: {payload}"
    );

    // Remove the global so the next test in this binary stays isolated.
    momos_music_manager::telemetry::emit::shutdown_global();
}
