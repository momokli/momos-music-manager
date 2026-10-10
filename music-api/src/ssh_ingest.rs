//! Part A — server-side SSH pull-ingest.
//!
//! `music-api` pulls a remote tree over SSH/rsync into a staging dir, ingests
//! every audio file into the content-addressed store (canonicalise → hash →
//! import → tags), then removes the staging copy. Idempotent: an object already
//! in the store is skipped, so re-running is cheap and safe.
//!
//! This reintroduces SSH **on the server side only** (see the plan / ADR): MMM
//! still never touches SSH; the store stays the single source of truth.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Pool, Sqlite};

use crate::AppState;
use crate::ingest::{self, AUDIO_EXTS};
use crate::store::{ImportMeta, Store};

/// One configured SSH source (`SSH_SOURCES` JSON array).
#[derive(Debug, Clone, Deserialize)]
pub struct SshSource {
    pub name: String,
    pub host: String,
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Remote directory to pull.
    pub path: String,
    /// Private key for the SSH connection (optional; agent/default otherwise).
    pub key: Option<String>,
    /// Include globs (empty = every audio file).
    #[serde(default)]
    pub include: Vec<String>,
}

fn default_user() -> String {
    "root".to_string()
}
fn default_port() -> u16 {
    22
}

/// Parse the `SSH_SOURCES` JSON array.
pub fn parse_sources(raw: &str) -> Result<Vec<SshSource>> {
    serde_json::from_str(raw).context("parse SSH_SOURCES (expected a JSON array)")
}

/// Counters for one source run.
#[derive(Debug, Default, Clone)]
pub struct RunStats {
    pub files: usize,
    pub stored: usize,
    pub skipped: usize,
    pub failed: usize,
    pub bytes: u64,
}

/// Pull one source into the store. `staging_root` is a scratch directory.
pub async fn run_source(
    pool: &Pool<Sqlite>,
    store: &Store,
    staging_root: &Path,
    source: &SshSource,
) -> Result<RunStats> {
    let staging = staging_root.join(&source.name);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("create staging {}", staging.display()))?;

    // 1. rsync the remote tree into staging.
    rsync_pull(source, &staging)?;

    // 2. Walk + ingest.
    let mut stats = RunStats::default();
    for entry in walkdir::WalkDir::new(&staging)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Skip partial-download temp files (`.NAME.stem.m4a.XXXXXX`).
        if name.starts_with('.') {
            continue;
        }
        let Some(ext) = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
        else {
            continue;
        };
        if !AUDIO_EXTS.contains(&ext.as_str()) || !ingest::included(&source.include, name) {
            continue;
        }
        let rel = path.strip_prefix(&staging).unwrap_or(path);
        stats.files += 1;
        match ingest::ingest_file(pool, store, path, rel, &ImportMeta::default()).await {
            Ok(o) if o.stored => {
                stats.stored += 1;
                stats.bytes += o.size;
            }
            Ok(_) => stats.skipped += 1,
            Err(e) => {
                tracing::warn!("ingest failed for {}: {e:#}", rel.display());
                stats.failed += 1;
            }
        }
    }

    // 3. Drop the staging copy.
    let _ = std::fs::remove_dir_all(&staging);
    Ok(stats)
}

/// Run one source and record it in `ingest_runs`. Returns `(run_id, stats)`.
pub async fn run_and_record(
    pool: &Pool<Sqlite>,
    store: &Store,
    staging_root: &Path,
    source: &SshSource,
) -> Result<(i64, RunStats)> {
    let run_id = crate::db::start_ingest_run(pool, "ssh", Some(&source.name)).await?;
    match run_source(pool, store, staging_root, source).await {
        Ok(s) => {
            crate::db::finish_ingest_run(
                pool,
                run_id,
                "done",
                s.files as i64,
                s.stored as i64,
                s.skipped as i64,
                s.failed as i64,
                s.bytes as i64,
                None,
            )
            .await?;
            Ok((run_id, s))
        }
        Err(e) => {
            let msg = format!("{e:#}");
            crate::db::finish_ingest_run(pool, run_id, "failed", 0, 0, 0, 0, 0, Some(&msg)).await?;
            Err(e)
        }
    }
}

/// `rsync -a` the remote tree into `dest`.
fn rsync_pull(source: &SshSource, dest: &Path) -> Result<()> {
    let mut ssh = format!(
        "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -p {}",
        source.port
    );
    if let Some(key) = &source.key {
        ssh.push_str(&format!(" -i {key}"));
    }
    let remote = format!("{}@{}:{}/", source.user, source.host, source.path);
    let dest_arg = format!("{}/", dest.display());

    let status = Command::new("rsync")
        .args([
            "-a",
            "--no-perms",
            "--no-owner",
            "--no-group",
            "-e",
            &ssh,
            &remote,
            &dest_arg,
        ])
        .status()
        .context("spawn rsync (is it installed?)")?;
    if !status.success() {
        anyhow::bail!("rsync exited with {status}");
    }
    Ok(())
}

// ── HTTP handlers ──────────────────────────────────────────────────────────

/// `POST /ingest/ssh` body.
#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    /// A single source by name.
    pub source: Option<String>,
    /// Every configured source.
    #[serde(default)]
    pub all: bool,
}

/// `POST /ingest/ssh` — start a pull in the background, return the run ids.
pub async fn post_ingest_ssh(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IngestRequest>,
) -> Response {
    let sources: Vec<SshSource> = if req.all {
        state.config.ssh_sources.clone()
    } else if let Some(name) = &req.source {
        state
            .config
            .ssh_sources
            .iter()
            .filter(|s| &s.name == name)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    if sources.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "no matching SSH source (configure SSH_SOURCES)" })),
        )
            .into_response();
    }

    let mut run_ids = Vec::new();
    for source in sources {
        let pool = state.pool.clone();
        let store = state.store.clone();
        let staging = state.config.data_dir.join("ssh-staging");
        let run_id = match crate::db::start_ingest_run(&pool, "ssh", Some(&source.name)).await {
            Ok(id) => id,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        };
        run_ids.push(run_id);
        tokio::spawn(async move {
            match run_source(&pool, &store, &staging, &source).await {
                Ok(s) => {
                    let _ = crate::db::finish_ingest_run(
                        &pool,
                        run_id,
                        "done",
                        s.files as i64,
                        s.stored as i64,
                        s.skipped as i64,
                        s.failed as i64,
                        s.bytes as i64,
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    let _ = crate::db::finish_ingest_run(
                        &pool,
                        run_id,
                        "failed",
                        0,
                        0,
                        0,
                        0,
                        0,
                        Some(&msg),
                    )
                    .await;
                }
            }
        });
    }
    (StatusCode::ACCEPTED, Json(json!({ "runIds": run_ids }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct RunsQuery {
    limit: Option<i64>,
}

/// `GET /ingest/runs` — recent runs, newest first.
pub async fn list_runs(State(state): State<Arc<AppState>>, Query(q): Query<RunsQuery>) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    match crate::db::list_ingest_runs(&state.pool, limit).await {
        Ok(runs) => Json(json!({ "runs": runs })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /ingest/runs/{id}` — one run.
pub async fn get_run(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<i64>) -> Response {
    match crate::db::get_ingest_run(&state.pool, id).await {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown run" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources() {
        let raw = r#"[{"name":"nas","host":"backup","path":"/volume1/media/stems",
                       "include":["*.stem.m4a"]}]"#;
        let s = parse_sources(raw).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "nas");
        assert_eq!(s[0].user, "root");
        assert_eq!(s[0].port, 22);
        assert_eq!(s[0].include, vec!["*.stem.m4a".to_string()]);
    }

    #[test]
    fn rejects_bad_json() {
        assert!(parse_sources("not json").is_err());
    }
}
