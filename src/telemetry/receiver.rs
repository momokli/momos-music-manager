//! Telemetry receiver — stores pushed snapshots + metadata on disk (behind Caddy).

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
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use crate::config::ServiceCredentials;
use crate::telemetry::MetaPayload;

/// Shared state for the receiver router.
pub struct ReceiverState {
    base_dir: PathBuf,
    token: Option<String>,
}

/// Run the receiver server (blocks until shutdown).
pub async fn serve(config: ServiceCredentials) -> Result<()> {
    let state = Arc::new(ReceiverState {
        base_dir: PathBuf::from(&config.telemetry_receiver_base_dir),
        token: config.telemetry_receiver_token.clone(),
    });
    std::fs::create_dir_all(&state.base_dir)?;

    if state.token.is_none() {
        warn!("Telemetry receiver running WITHOUT auth token — set [telemetry_receiver] token");
    }

    let bind = config.telemetry_receiver_bind.clone();
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    info!(
        "Telemetry receiver listening on {bind} (base_dir={})",
        state.base_dir.display()
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the receiver router (extracted for testability).
pub fn build_router(state: Arc<ReceiverState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/telemetry/{instance}/db/{ts}", put(put_db))
        .route("/api/telemetry/{instance}/meta/{ts}", post(post_meta))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

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
    use tower::util::ServiceExt;

    fn state(base_dir: PathBuf, token: Option<&str>) -> Arc<ReceiverState> {
        Arc::new(ReceiverState {
            base_dir,
            token: token.map(String::from),
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
}
