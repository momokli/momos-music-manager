//! Telemetry: push consistent DB snapshots + metadata to a central collector over HTTPS.

pub mod buffer;
pub mod client_id;
pub mod emit;
pub mod events;
pub mod flusher;
pub mod metrics;
pub mod receiver;
pub mod spool;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Sqlite, SqlitePool};
use tracing::{info, warn};

use crate::config::ServiceCredentials;
use crate::tasks::{Task, TaskManager, TaskStatus, TaskType};

/// Telemetry CLI subcommands.
#[derive(Subcommand, Debug)]
pub enum TelemetryCommand {
    /// Push a single consistent DB snapshot + metadata to the collector (one shot)
    Push,
    /// Run the telemetry receiver (collector) server
    Receive,
}

/// Entry point for `momos-music-manager telemetry ...`.
pub async fn run(cmd: TelemetryCommand) -> Result<()> {
    match cmd {
        TelemetryCommand::Push => {
            let config = ServiceCredentials::load();
            let db = SqlitePool::connect(&config.database_url).await?;
            push_once(&db, &config).await
        }
        TelemetryCommand::Receive => {
            let config = ServiceCredentials::load();
            receiver::serve(config).await
        }
    }
}

// ── Payload types (shared between client and receiver) ──────────────────────

/// Non-sensitive instance metadata. Never contains tokens, client secrets or API keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub hostname: String,
    pub version: String,
    pub instance: String,
    pub ts: String,
    pub db_size_bytes: u64,
    pub config: RedactedConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactedConfig {
    pub server_host: String,
    pub server_port: u16,
    pub public_url: Option<String>,
    pub global_poll_interval_secs: u64,
    pub maintainer_interval_secs: u64,
    pub maintainer_auto_prune: bool,
    pub spotify_configured: bool,
    pub soundcloud_configured: bool,
    pub youtube_configured: bool,
}

impl RedactedConfig {
    pub fn from_credentials(c: &ServiceCredentials) -> Self {
        Self {
            server_host: c.server_host.clone(),
            server_port: c.server_port,
            public_url: c.server_public_url.clone(),
            global_poll_interval_secs: c.global_poll_interval_secs,
            maintainer_interval_secs: c.maintainer_interval_secs,
            maintainer_auto_prune: c.maintainer_auto_prune,
            spotify_configured: c.is_spotify_configured(),
            soundcloud_configured: c.is_soundcloud_configured(),
            youtube_configured: c.is_youtube_configured(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TaskHistoryRow {
    pub id: String,
    pub task_type: String,
    pub status: String,
    pub service: Option<String>,
    pub result_summary: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub created_at_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaPayload {
    pub instance: InstanceMeta,
    pub metrics: metrics::Metrics,
    pub tasks: Vec<TaskHistoryRow>,
    pub logs: HashMap<String, String>,
}

// ── Client push ─────────────────────────────────────────────────────────────

const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_LOG_FILES: usize = 3;

/// Push a single consistent snapshot + metadata to the configured collector.
pub async fn push_once(db: &Pool<Sqlite>, config: &ServiceCredentials) -> Result<()> {
    if !config.telemetry_enabled {
        info!("Telemetry disabled — skipping push");
        return Ok(());
    }

    let base_url = config.telemetry_base_url.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "telemetry.base_url not configured (set [telemetry] base_url or MOMOS_TELEMETRY_BASE_URL)"
        )
    })?;
    let token = config.telemetry_token.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "telemetry.token not configured (set [telemetry] token or MOMOS_TELEMETRY_TOKEN)"
        )
    })?;
    let instance = validate_instance(&config.telemetry_instance)?;

    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    // 1. VACUUM INTO — consistent, WAL-safe snapshot.
    let tmp = tempfile::tempdir().context("create temp dir")?;
    let db_path = tmp.path().join("db.sqlite");
    let db_path_str = db_path.to_string_lossy().replace('\'', "''");
    let sql = format!("VACUUM INTO '{db_path_str}'");
    sqlx::query(&sql)
        .execute(db)
        .await
        .context("VACUUM INTO failed")?;
    let size = std::fs::metadata(&db_path)?.len();

    // 2. Push the DB snapshot (the canonical artifact).
    push_db(&client, base_url, token, instance, &ts, &db_path).await?;

    // 3. Build + push metadata (best-effort: never fail the whole push over meta).
    match build_meta_payload(db, config, instance, &ts, size).await {
        Ok(meta) => {
            if let Err(e) = push_meta(&client, base_url, token, instance, &ts, &meta).await {
                warn!("meta push failed (db was pushed): {e}");
            }
        }
        Err(e) => warn!("meta build failed (db was pushed): {e}"),
    }

    info!("Telemetry push ok: instance={instance} ts={ts} size={size} bytes");
    Ok(())
}

async fn push_db(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    instance: &str,
    ts: &str,
    db_path: &Path,
) -> Result<()> {
    let file = tokio::fs::File::open(db_path).await?;
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    let url = format!(
        "{}/api/telemetry/{instance}/db/{ts}",
        base_url.trim_end_matches('/')
    );
    let resp = client
        .put(&url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/vnd.sqlite3")
        .body(body)
        .send()
        .await
        .context("db upload failed")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!(
            "collector returned {status}: {}",
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(())
}

async fn push_meta(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    instance: &str,
    ts: &str,
    meta: &MetaPayload,
) -> Result<()> {
    let url = format!(
        "{}/api/telemetry/{instance}/meta/{ts}",
        base_url.trim_end_matches('/')
    );
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(meta)
        .send()
        .await
        .context("meta upload failed")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!(
            "collector returned {status}: {}",
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(())
}

async fn build_meta_payload(
    db: &Pool<Sqlite>,
    config: &ServiceCredentials,
    instance: &str,
    ts: &str,
    db_size: u64,
) -> Result<MetaPayload> {
    let metrics = metrics::collect_metrics(db).await?;
    let tasks = collect_tasks(db, 100).await?;
    let logs = collect_logs(&resolve_log_dir());
    Ok(MetaPayload {
        instance: InstanceMeta {
            hostname: hostname(),
            version: env!("MMM_VERSION").to_string(),
            instance: instance.to_string(),
            ts: ts.to_string(),
            db_size_bytes: db_size,
            config: RedactedConfig::from_credentials(config),
        },
        metrics,
        tasks,
        logs,
    })
}

fn validate_instance(instance: &str) -> Result<&str> {
    let instance = instance.trim();
    if instance.is_empty()
        || !instance
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("telemetry.instance is invalid: {instance:?}");
    }
    Ok(instance)
}

async fn collect_tasks(pool: &Pool<Sqlite>, limit: i64) -> Result<Vec<TaskHistoryRow>> {
    let rows = sqlx::query_as::<_, TaskHistoryRow>(
        "SELECT id, task_type, status, service, result_summary, error_message, \
         started_at, completed_at, created_at_secs \
         FROM task_history ORDER BY created_at_secs DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub fn resolve_log_dir() -> PathBuf {
    PathBuf::from(std::env::var("MOMOS_LOG_DIR").unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/momos-music-manager/logs")
            .to_string_lossy()
            .to_string()
    }))
}

pub fn collect_logs(log_dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return out;
    };

    let mut current = None;
    let mut rotated: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "server.log" {
            current = Some(name);
        } else if name.starts_with("server.log") {
            rotated.push(name);
        }
    }
    rotated.sort();
    rotated.reverse();

    let mut selected: Vec<String> = Vec::new();
    if let Some(c) = current {
        selected.push(c);
    }
    selected.extend(rotated);
    selected.truncate(MAX_LOG_FILES);

    for name in selected {
        let content = read_tail(&log_dir.join(&name), MAX_LOG_BYTES);
        if !content.is_empty() {
            out.insert(name, content);
        }
    }
    out
}

fn read_tail(path: &Path, max_bytes: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(&bytes).to_string();
    }
    let mut start = bytes.len() - max_bytes;
    while start < bytes.len() && bytes[start] != b'\n' {
        start += 1;
    }
    if start < bytes.len() {
        start += 1; // skip the newline, start on a clean line boundary
    }
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
}

/// Effective periodic full-DB push interval.
///
/// The explicit option `full_db_interval_secs`
/// (`MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS` / `[telemetry]
/// full_db_interval_secs`, default 0 = OFF) wins when > 0; the legacy
/// `interval_secs` key (analytics era) stays effective as backward-compatible
/// alias when the explicit one is unset/0. `0` = periodic loop off
/// (one-shot trigger via `telemetry push` CLI still works).
pub fn effective_full_db_interval_secs(
    full_db_interval_secs: u64,
    legacy_interval_secs: u64,
) -> u64 {
    if full_db_interval_secs > 0 {
        full_db_interval_secs
    } else {
        legacy_interval_secs
    }
}

/// Start the in-app telemetry loop — pushes a full DB snapshot (VACUUM INTO)
/// + metadata every `interval_secs` (see [`effective_full_db_interval_secs`]).
pub async fn start_telemetry_loop(
    db: Pool<Sqlite>,
    config: ServiceCredentials,
    task_manager: TaskManager,
    interval_secs: u64,
) {
    let interval = std::time::Duration::from_secs(interval_secs);
    loop {
        tokio::time::sleep(interval).await;

        let task_id = task_manager
            .start_task(Task::new(TaskType::TelemetryPush, None))
            .await;
        task_manager
            .update_task_status(&task_id, TaskStatus::Running)
            .await;
        task_manager
            .add_log(&task_id, "Telemetry push starting...".to_string())
            .await;

        match push_once(&db, &config).await {
            Ok(()) => {
                task_manager
                    .add_log(&task_id, "Telemetry push completed".to_string())
                    .await;
                task_manager
                    .update_task_status(&task_id, TaskStatus::Completed)
                    .await;
            }
            Err(e) => {
                let msg = format!("Telemetry push failed: {e}");
                warn!("{msg}");
                task_manager.add_log(&task_id, msg).await;
                task_manager
                    .update_task_status(&task_id, TaskStatus::Failed)
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_instance_accepts_and_rejects() {
        assert_eq!(validate_instance("macbook").unwrap(), "macbook");
        assert_eq!(
            validate_instance("  music-server  ").unwrap(),
            "music-server"
        );
        assert!(validate_instance("../etc").is_err());
        assert!(validate_instance("a/b").is_err());
        assert!(validate_instance("").is_err());
    }

    #[test]
    fn redacted_config_omits_secrets() {
        let mut cfg = ServiceCredentials::defaults_for_test();
        cfg.spotify_client_id = Some("spotify-id".into());
        cfg.spotify_client_secret = Some("spotify-s3cret".into());
        cfg.soundcloud_api_key = Some("sc-api-key".into());
        cfg.telemetry_token = Some("telemetry-tok".into());
        cfg.telemetry_receiver_token = Some("receiver-tok".into());

        let redacted = RedactedConfig::from_credentials(&cfg);
        let json = serde_json::to_string(&redacted).unwrap();

        assert!(!json.contains("spotify-s3cret"));
        assert!(!json.contains("sc-api-key"));
        assert!(!json.contains("telemetry-tok"));
        assert!(!json.contains("receiver-tok"));
        assert!(!json.contains("client_secret"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("token"));
        assert!(json.contains("spotify_configured"));
    }

    #[test]
    fn read_tail_truncates_from_front() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        std::fs::write(&path, "line1\nline2\nline3\nline4\n").unwrap();

        let tail = read_tail(&path, 11); // less than full, picks up from a newline
        assert!(tail.ends_with("line4\n"));
        assert!(!tail.contains("line1"));
    }

    #[test]
    fn effective_full_db_interval_prefers_explicit_over_legacy() {
        // Explicit option wins when set.
        assert_eq!(effective_full_db_interval_secs(86400, 3600), 86400);
        assert_eq!(effective_full_db_interval_secs(60, 0), 60);
    }

    #[test]
    fn effective_full_db_interval_falls_back_to_legacy() {
        // Legacy `interval_secs` alias stays effective (backward compat).
        assert_eq!(effective_full_db_interval_secs(0, 3600), 3600);
    }

    #[test]
    fn effective_full_db_interval_zero_means_off() {
        // Default: both unset/0 → periodic loop off.
        assert_eq!(effective_full_db_interval_secs(0, 0), 0);
    }

    #[test]
    fn collect_logs_prefers_current_then_recent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.log"), "current").unwrap();
        std::fs::write(dir.path().join("server.log.2026-08-25"), "yesterday").unwrap();
        std::fs::write(dir.path().join("server.log.2026-08-24"), "older").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), "nope").unwrap();

        let logs = collect_logs(dir.path());
        assert_eq!(logs.len(), 3); // current + 2 rotated (MAX_LOG_FILES = 3)
        assert_eq!(logs.get("server.log").unwrap(), "current");
        assert_eq!(logs.get("server.log.2026-08-25").unwrap(), "yesterday");
        assert!(!logs.contains_key("unrelated.txt"));
    }
}
