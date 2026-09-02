//! Telemetry receiver — stores pushed snapshots + metadata on disk (behind
//! Caddy) and ingests event batches (`POST /api/telemetry`) into its own
//! `telemetry.db` (SQLite, sqlx migration chain `migrations/telemetry`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use futures::TryStreamExt;
use serde::Serialize;
use sqlx::{Pool, Row, Sqlite, SqlitePool};
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use crate::config::ServiceCredentials;
use crate::telemetry::events::{EventBatch, MAX_BATCH_BYTES, parse_ts};
use crate::telemetry::MetaPayload;

/// Default retention for ingested events (days). Prune keeps events whose
/// `received_at` is newer than now − retention_days.
pub const DEFAULT_RETENTION_DAYS: i64 = 30;
/// Periodic retention prune interval (also pruned on every ingest).
pub const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Shared state for the receiver router.
pub struct ReceiverState {
    pub base_dir: PathBuf,
    pub token: Option<String>,
    /// Event telemetry.db pool (None → event ingest returns 503).
    pub db: Option<SqlitePool>,
    /// Retention for ingested events, in days.
    pub retention_days: i64,
}

impl ReceiverState {
    /// Build a receiver state. `db` may be None (dev mode without event
    /// ingest) — `serve()` always opens one via [`init_telemetry_db`].
    pub fn new(
        base_dir: PathBuf,
        token: Option<String>,
        db: Option<SqlitePool>,
        retention_days: i64,
    ) -> Self {
        Self {
            base_dir,
            token,
            db,
            retention_days,
        }
    }
}

/// Run the receiver server (blocks until shutdown).
pub async fn serve(config: ServiceCredentials) -> Result<()> {
    let base_dir = PathBuf::from(&config.telemetry_receiver_base_dir);
    std::fs::create_dir_all(&base_dir)?;

    if config.telemetry_receiver_token.is_none() {
        warn!("Telemetry receiver running WITHOUT auth token — set [telemetry_receiver] token");
    }

    // Event ingest DB (separate chain from the main DB — see concept doc).
    let db_path = PathBuf::from(&config.telemetry_receiver_db_path);
    let pool = init_telemetry_db(&db_path).await?;

    let state = Arc::new(ReceiverState::new(
        base_dir,
        config.telemetry_receiver_token.clone(),
        Some(pool),
        config.telemetry_receiver_retention_days,
    ));

    let bind = config.telemetry_receiver_bind.clone();
    let app = build_router(state.clone());

    // Periodic retention prune while the receiver runs.
    {
        let state = state.clone();
        let retention_days = state.retention_days;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PRUNE_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                if let Some(db) = state.db.clone() {
                    if let Err(e) = prune_events(&db, retention_days).await {
                        warn!("telemetry retention prune failed: {e}");
                    }
                }
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    info!(
        "Telemetry receiver listening on {bind} (base_dir={}, events_db={}, retention={}d)",
        state.base_dir.display(),
        db_path.display(),
        state.retention_days
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// Open (create if missing) + migrate the event telemetry.db.
pub async fn init_telemetry_db(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!("create telemetry.db dir {}", parent.display())
        })?;
    }
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30));
    let pool = SqlitePool::connect_with(options).await?;
    sqlx::migrate!("migrations/telemetry").run(&pool).await?;
    Ok(pool)
}

/// Build the receiver router (extracted for testability).
pub fn build_router(state: Arc<ReceiverState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/telemetry", post(post_events))
        .route("/api/telemetry/{instance}/db/{ts}", put(put_db))
        .route("/api/telemetry/{instance}/meta/{ts}", post(post_meta))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BATCH_BYTES + 64 * 1024))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

// ── Event ingest ──────────────────────────────────────────────────────────

/// Accepted response body for `POST /api/telemetry`.
#[derive(Serialize)]
struct IngestResponse {
    accepted: u64,
    duplicates: u64,
}

/// Validate one event against the wire schema; returns `(type, ts_unix,
/// payload_json)` when OK. Rejects unknown types (allowlist via serde),
/// invalid ids and oversized payloads.
fn validate_event(event: &crate::telemetry::events::TelemetryEvent) -> Option<(String, i64, String)> {
    if !event.is_valid() {
        return None;
    }
    let ts_unix = parse_ts(&event.ts)?.timestamp();
    let payload_json = serde_json::to_string(&event.payload).ok()?;
    Some((event.r#type.as_str().to_string(), ts_unix, payload_json))
}

/// Handle `POST /api/telemetry` — ingest an event batch.
async fn post_events(
    State(state): State<Arc<ReceiverState>>,
    headers: HeaderMap,
    Json(batch): Json<EventBatch>,
) -> Response {
    if !authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let Some(db) = &state.db else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "event ingest not initialized",
        )
            .into_response();
    };
    if let Err(reason) = batch.validate() {
        return (StatusCode::BAD_REQUEST, format!("invalid batch: {reason}")).into_response();
    }

    let now = chrono::Utc::now().timestamp();
    let mut accepted: u64 = 0;
    let mut duplicates: u64 = 0;

    // 1. Upsert every client mentioned in the batch (envelope + per-event),
    //    then refresh last-seen/version/os fields from this batch.
    if let Err(e) = upsert_clients(db, &batch.client_id, &batch.events, now).await {
        warn!("telemetry client upsert failed: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("client upsert failed: {e}"),
        )
            .into_response();
    }

    // 2. Insert events with dedup (event_id PRIMARY KEY → INSERT OR IGNORE).
    for event in &batch.events {
        let Some((event_type, ts_unix, payload_json)) = validate_event(event) else {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid event: {}", event.event_id),
            )
                .into_response();
        };
        let result = sqlx::query(
            "INSERT OR IGNORE INTO events \
             (event_id, client_id, type, ts, received_at, app_version, os, payload) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.event_id)
        .bind(&event.client_id)
        .bind(&event_type)
        .bind(ts_unix)
        .bind(now)
        .bind(&event.app_version)
        .bind(&event.os)
        .bind(&payload_json)
        .execute(db)
        .await;
        match result {
            Ok(res) if res.rows_affected() > 0 => accepted += 1,
            Ok(_) => duplicates += 1,
            Err(e) => {
                warn!("telemetry event insert failed ({}): {e}", event.event_id);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("event insert failed: {e}"),
                )
                    .into_response();
            }
        }
    }

    // 3. Retention prune on ingest (cheap; periodic prune covers idle time).
    if let Err(e) = prune_events(db, state.retention_days).await {
        warn!("telemetry retention prune failed: {e}");
    }

    (
        StatusCode::ACCEPTED,
        Json(IngestResponse { accepted, duplicates }),
    )
        .into_response()
}

/// INSERT OR IGNORE every client referenced by the batch (envelope + events),
/// then refresh last_seen/version/os of the envelope client.
async fn upsert_clients(
    db: &Pool<Sqlite>,
    envelope_client_id: &str,
    events: &[crate::telemetry::events::TelemetryEvent],
    now: i64,
) -> Result<()> {
    let mut ids: Vec<&str> = events.iter().map(|e| e.client_id.as_str()).collect();
    ids.push(envelope_client_id);
    ids.sort_unstable();
    ids.dedup();

    for client_id in ids {
        sqlx::query(
            "INSERT OR IGNORE INTO clients (client_id, first_seen_at, last_seen_at) \
             VALUES (?, ?, ?)",
        )
        .bind(client_id)
        .bind(now)
        .bind(now)
        .execute(db)
        .await?;
    }

    let (last_version, last_os) = events
        .iter()
        .map(|e| (e.app_version.as_str(), e.os.as_str()))
        .next()
        .unwrap_or(("", ""));

    sqlx::query(
        "UPDATE clients SET last_seen_at = ?, \
         last_app_version = COALESCE(?, last_app_version), \
         last_os = COALESCE(?, last_os) \
         WHERE client_id = ?",
    )
    .bind(now)
    .bind(if last_version.is_empty() { None } else { Some(last_version) })
    .bind(if last_os.is_empty() { None } else { Some(last_os) })
    .bind(envelope_client_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Delete events older than `retention_days` (based on server received_at).
pub async fn prune_events(db: &Pool<Sqlite>, retention_days: i64) -> Result<u64> {
    let cutoff = chrono::Utc::now().timestamp() - retention_days.max(1) * 86_400;
    let res = sqlx::query("DELETE FROM events WHERE received_at < ?")
        .bind(cutoff)
        .execute(db)
        .await?;
    Ok(res.rows_affected())
}

// ── Snapshot ingest (unchanged) ───────────────────────────────────────────

async fn put_db(
    State(state): State<Arc<ReceiverState>>,
    AxumPath((instance, ts)): AxumPath<(String, String)>,
    req: Request,
) -> Response {
    if !authorized(&state, req.headers()) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if !valid_instance(&instance) || !valid_ts(&ts) {
        return (StatusCode::BAD_REQUEST, "invalid instance or ts").into_response();
    }

    let dir = state.base_dir.join(&instance).join(&ts);
    let db_path = dir.join("db.sqlite");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir: {e}")).into_response();
    }

    let stream = req.into_body().into_data_stream();
    let mut reader = StreamReader::new(stream.map_err(std::io::Error::other));
    let mut file = match tokio::fs::File::create(&db_path).await {
        Ok(f) => f,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("create: {e}")).into_response();
        }
    };
    if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}")).into_response();
    }

    if let Err(e) = update_latest(&state, &instance, &ts) {
        warn!("latest symlink failed for {instance}: {e}");
    }

    (StatusCode::CREATED, "ok").into_response()
}

async fn post_meta(
    State(state): State<Arc<ReceiverState>>,
    AxumPath((instance, ts)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(payload): Json<MetaPayload>,
) -> Response {
    if !authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if !valid_instance(&instance) || !valid_ts(&ts) {
        return (StatusCode::BAD_REQUEST, "invalid instance or ts").into_response();
    }

    let dir = state.base_dir.join(&instance).join(&ts);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir: {e}")).into_response();
    }

    if let Err(e) = write_json(&dir.join("instance.json"), &payload.instance).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write instance: {e}"),
        )
            .into_response();
    }
    if let Err(e) = write_json(&dir.join("metrics.json"), &payload.metrics).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write metrics: {e}"),
        )
            .into_response();
    }
    if let Err(e) = write_json(&dir.join("tasks.json"), &payload.tasks).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write tasks: {e}"),
        )
            .into_response();
    }

    let logs_dir = dir.join("logs");
    for (name, content) in &payload.logs {
        if !valid_log_name(name) {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&logs_dir) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("mkdir logs: {e}"),
            )
                .into_response();
        }
        if let Err(e) = tokio::fs::write(logs_dir.join(name), content).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("write log: {e}")).into_response();
        }
    }

    if let Err(e) = update_latest(&state, &instance, &ts) {
        warn!("latest symlink failed for {instance}: {e}");
    }

    (StatusCode::CREATED, "ok").into_response()
}

async fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    tokio::fs::write(path, json).await
}

fn authorized(state: &ReceiverState, headers: &HeaderMap) -> bool {
    let Some(expected) = state.token.as_deref() else {
        return true; // no token configured — dev mode
    };
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(expected.as_bytes(), token.as_bytes())
}

fn valid_instance(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn valid_ts(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.'))
}

fn valid_log_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Simple constant-time comparison (avoids early-exit on first mismatch).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(unix)]
fn update_latest(state: &ReceiverState, instance: &str, ts: &str) -> Result<()> {
    let latest = state.base_dir.join(instance).join("latest");
    if latest.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(&latest);
    }
    std::os::unix::fs::symlink(ts, &latest).context("symlink")
}

#[cfg(not(unix))]
fn update_latest(_state: &ReceiverState, _instance: &str, _ts: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, header};
    use serde_json::json;
    use tower::util::ServiceExt;

    /// Receiver state pointing at a temp dir with an initialized telemetry.db.
    async fn state_with_db(
        dir: &tempfile::TempDir,
        token: Option<&str>,
    ) -> (Arc<ReceiverState>, SqlitePool) {
        let db_path = dir.path().join("telemetry.db");
        let pool = init_telemetry_db(&db_path).await.unwrap();
        let state = Arc::new(ReceiverState {
            base_dir: dir.path().to_path_buf(),
            token: token.map(String::from),
            db: Some(pool.clone()),
            retention_days: 30,
        });
        (state, pool)
    }

    fn state(base_dir: PathBuf, token: Option<&str>) -> Arc<ReceiverState> {
        Arc::new(ReceiverState {
            base_dir,
            token: token.map(String::from),
            db: None,
            retention_days: 30,
        })
    }

    fn meta_body() -> axum::body::Body {
        axum::body::Body::from(
            r#"{
                "instance": {"hostname": "mac", "version": "1.0.0", "instance": "macbook", "ts": "2026-08-26T12-00-00", "db_size_bytes": 10, "config": {"server_host": "127.0.0.1", "server_port": 3000, "public_url": null, "global_poll_interval_secs": 900, "maintainer_interval_secs": 3600, "maintainer_auto_prune": false, "spotify_configured": false, "soundcloud_configured": false, "youtube_configured": false}},
                "metrics": {"task_history_total": 2, "task_counts_by_status": [], "task_counts_by_type": [], "failed_tasks_24h": 0, "table_row_counts": []},
                "tasks": [],
                "logs": {"server.log": "hello\nworld\n"}
            }"#,
        )
    }

    fn event_batch_body() -> axum::body::Body {
        let body = json!({
            "client_id": "client-123",
            "sent_at": "2026-09-01T00:42:00Z",
            "events": [{
                "event_id": "11111111-1111-4111-8111-111111111111",
                "client_id": "client-123",
                "app_version": "1.1.0-dev+abc",
                "os": "macos",
                "ts": "2026-09-01T00:41:59Z",
                "type": "task.completed",
                "payload": {"task_type": "ScanFolder", "status": "completed", "duration_ms": 12400}
            }]
        });
        axum::body::Body::from(serde_json::to_string(&body).unwrap())
    }

    #[test]
    fn valid_instance_rejects_traversal() {
        assert!(valid_instance("macbook"));
        assert!(valid_instance("music-server"));
        assert!(!valid_instance("../etc"));
        assert!(!valid_instance("a/b"));
        assert!(!valid_instance(""));
        assert!(!valid_instance("a b"));
    }

    #[test]
    fn valid_ts_rejects_traversal() {
        assert!(valid_ts("2026-08-26T12-00-00"));
        assert!(!valid_ts("../etc"));
        assert!(!valid_ts("a/b"));
        assert!(!valid_ts(""));
        assert!(!valid_ts(".."));
    }

    #[test]
    fn constant_time_eq_matches() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"wrong"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
    }

    #[tokio::test]
    async fn put_db_requires_auth() {
        let dir = tempfile::tempdir().unwrap();
        let router = build_router(state(dir.path().to_path_buf(), Some("secret")));

        let req = Request::builder()
            .method("PUT")
            .uri("/api/telemetry/macbook/db/2026-08-26T12-00-00")
            .body(axum::body::Body::from("sqlite-bytes"))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn put_db_stores_snapshot_and_updates_latest() {
        let dir = tempfile::tempdir().unwrap();
        let router = build_router(state(dir.path().to_path_buf(), Some("secret")));

        let req = Request::builder()
            .method("PUT")
            .uri("/api/telemetry/macbook/db/2026-08-26T12-00-00")
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(axum::body::Body::from("sqlite-bytes"))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let latest = dir.path().join("macbook").join("latest");
        assert!(latest.symlink_metadata().is_ok());
        let ts = std::fs::read_link(&latest).unwrap();
        let db = dir.path().join("macbook").join(ts).join("db.sqlite");
        assert_eq!(std::fs::read_to_string(&db).unwrap(), "sqlite-bytes");
    }

    #[tokio::test]
    async fn put_db_rejects_bad_instance() {
        let dir = tempfile::tempdir().unwrap();
        let router = build_router(state(dir.path().to_path_buf(), Some("secret")));

        let req = Request::builder()
            .method("PUT")
            .uri("/api/telemetry/..%2Fetc/db/2026-08-26T12-00-00")
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(axum::body::Body::from("x"))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_meta_stores_files() {
        let dir = tempfile::tempdir().unwrap();
        let router = build_router(state(dir.path().to_path_buf(), Some("secret")));

        let req = Request::builder()
            .method("POST")
            .uri("/api/telemetry/macbook/meta/2026-08-26T12-00-00")
            .header(header::AUTHORIZATION, "Bearer secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(meta_body())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let base = dir.path().join("macbook").join("2026-08-26T12-00-00");
        assert!(base.join("instance.json").exists());
        assert!(base.join("metrics.json").exists());
        assert!(base.join("tasks.json").exists());
        let log = std::fs::read_to_string(base.join("logs").join("server.log")).unwrap();
        assert_eq!(log, "hello\nworld\n");
    }

    // ── Event ingest tests ───────────────────────────────────────────

    #[tokio::test]
    async fn post_events_requires_auth() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _pool) = state_with_db(&dir, Some("secret")).await;
        let router = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/telemetry")
            .header(header::CONTENT_TYPE, "application/json")
            .body(event_batch_body())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn post_events_accepts_batch_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (state, pool) = state_with_db(&dir, Some("secret")).await;
        let router = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/telemetry")
            .header(header::AUTHORIZATION, "Bearer secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(event_batch_body())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["accepted"], 1);
        assert_eq!(body["duplicates"], 0);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let client_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clients")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(client_count, 1);
    }

    #[tokio::test]
    async fn post_events_dedups_on_event_id() {
        let dir = tempfile::tempdir().unwrap();
        let (state, pool) = state_with_db(&dir, Some("secret")).await;
        let router = build_router(state.clone());
        let router2 = build_router(state);

        let resp1 = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(event_batch_body())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);

        let resp2 = router2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/telemetry")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(event_batch_body())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::ACCEPTED);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "duplicate batch must not create duplicates");
    }

    #[tokio::test]
    async fn post_events_rejects_unknown_event_type() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _pool) = state_with_db(&dir, Some("secret")).await;
        let router = build_router(state);

        let body = r#"{
            "client_id": "client-123",
            "sent_at": "2026-09-01T00:42:00Z",
            "events": [{
                "event_id": "11111111-1111-4111-8111-111111111111",
                "client_id": "client-123",
                "app_version": "1.1.0",
                "os": "macos",
                "ts": "2026-09-01T00:41:59Z",
                "type": "ui.clicked",
                "payload": {}
            }]
        }"#;
        let req = Request::builder()
            .method("POST")
            .uri("/api/telemetry")
            .header(header::AUTHORIZATION, "Bearer secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        // Unknown type fails serde deserialization → axum rejects the Json
        // extractor with 422 (Unprocessable Entity) — a validation error just
        // like the 400 path for handler-level validation.
        assert!(
            resp.status() == StatusCode::BAD_REQUEST
                || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "unexpected status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn prune_events_removes_old_events() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("telemetry.db");
        let pool = init_telemetry_db(&db_path).await.unwrap();

        // Seed a client + two events: one fresh, one 40 days old.
        let now = chrono::Utc::now().timestamp();
        sqlx::query("INSERT OR IGNORE INTO clients (client_id, first_seen_at, last_seen_at) VALUES ('c1', ?, ?)")
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO events (event_id, client_id, type, ts, received_at, app_version, os, payload) \
             VALUES ('a', 'c1', 'task.completed', ?, ?, '1.0', 'macos', '{}')",
        )
        .bind(now)
        .bind(now - 40 * 86_400)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO events (event_id, client_id, type, ts, received_at, app_version, os, payload) \
             VALUES ('b', 'c1', 'task.completed', ?, ?, '1.0', 'macos', '{}')",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let removed = prune_events(&pool, 30).await.unwrap();
        assert_eq!(removed, 1);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn validate_event_rejects_invalid() {
        let good = crate::telemetry::events::TelemetryEvent::new(
            crate::telemetry::events::EventType::TaskStarted,
            json!({}),
        )
        .with_envelope("client-123", "1.1.0", "macos");
        assert!(validate_event(&good).is_some());

        let mut bad = good.clone();
        bad.event_id = "not-a-uuid".to_string();
        assert!(validate_event(&bad).is_none());
    }
}
