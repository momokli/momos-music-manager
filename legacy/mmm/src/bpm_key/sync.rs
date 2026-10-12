//! Reconcile + push BPM//key system playlists to Spotify.
//!
//! One sequential pass: derive the desired buckets, then for each bucket find
//! the `system_key` row (create the Spotify playlist when absent) and mirror the
//! item set via [`SpotifyClient::replace_playlist_items`]. Idempotent — an
//! unchanged library creates nothing new and rewrites the same item set.
//!
//! Rate-limit handling follows ADR-061: a `429` sets the process-wide cooldown
//! and **aborts the run**, leaving the remaining buckets for the next run.
//!
//! Every `Ok` exit — normal completion, cancellation or a rate-limit abort —
//! funnels through the single labeled loop in [`run_sync`] into [`finalize`], so
//! the task always reaches a terminal status. (Returning early used to skip
//! finalization and leave the task stuck at `Running`, whose `bpm_key_sync`
//! conflict key then rejected every later sync.)
//!
//! Calls are staged with a small inter-bucket delay and counted in
//! [`crate::spotify::metrics`] so the API load is visible.

use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::{Pool, Sqlite};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::bpm_key::Group;
use crate::config::ServiceCredentials;
use crate::db;
use crate::db::bpm_key::SystemPlaylist;
use crate::spotify::client::SpotifyClient;
use crate::spotify::cooldown::cooldown as spotify_cooldown;
use crate::spotify::metrics::{self, Source};
use crate::spotify::retry::{extract_retry_after_secs, format_duration};
use crate::tasks::{TaskManager, TaskStatus};

/// Pacing between two Spotify calls so a reconcile stages its requests instead
/// of bursting them. The run is resumable + cooldown-aware, so a delay is safe.
const INTER_CALL_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Outcome counters of one reconcile pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Buckets that met `min_tracks` and were considered.
    pub group_count: usize,
    /// Spotify playlists created this run.
    pub created: usize,
    /// Buckets whose item set was mirrored (new + existing).
    pub mirrored: usize,
    /// Buckets skipped because of an API error.
    pub failed: usize,
    /// Rows deleted in strict mode.
    pub deleted: usize,
    /// Aborted early on a rate-limit cooldown.
    pub aborted: bool,
}

/// How a Spotify call failed.
#[derive(Debug, PartialEq, Eq)]
enum SpotifyFail {
    /// Token lacks write scope (HTTP 403) — user must re-authenticate.
    NeedsReauth,
    /// HTTP 429 with a `Retry-After`.
    RateLimited(u64),
    /// Anything else.
    Other(String),
}

fn classify(err: &anyhow::Error) -> SpotifyFail {
    if let Some(secs) = extract_retry_after_secs(err) {
        return SpotifyFail::RateLimited(secs);
    }
    let msg = format!("{err:#}");
    if msg.contains("403") {
        SpotifyFail::NeedsReauth
    } else {
        SpotifyFail::Other(msg)
    }
}

/// System rows whose bucket is no longer desired (strict-mode cleanup).
fn vanished<'a>(
    existing: &'a [SystemPlaylist],
    desired_keys: &HashSet<String>,
) -> Vec<&'a SystemPlaylist> {
    existing
        .iter()
        .filter(|p| !desired_keys.contains(&p.system_key))
        .collect()
}

/// Shared exit funnel for [`run_sync`]: log the run summary and mark the task
/// `Completed` (100%). Every `Ok` path reaches this; `Err` paths are finalized
/// as `Failed` by the worker.
async fn finalize(task_manager: &TaskManager, task_id: &str, outcome: &SyncOutcome) {
    let summary = format!(
        "BPM//key sync: {} bucket(s), {} created, {} mirrored, {} failed, {} deleted{}",
        outcome.group_count,
        outcome.created,
        outcome.mirrored,
        outcome.failed,
        outcome.deleted,
        if outcome.aborted {
            ", aborted (rate-limit)"
        } else {
            ""
        }
    );
    info!("{}", summary);
    task_manager.add_log(task_id, summary.clone()).await;
    task_manager
        .update_progress_text(task_id, summary.clone())
        .await;
    // `update_progress` only touches the unified `Progress` struct; the status
    // surfaced by `TaskProgress` lives on the task itself, so it must be set
    // explicitly — otherwise the task sticks at `Running`.
    task_manager
        .update_task_status(task_id, TaskStatus::Completed)
        .await;
    task_manager
        .update_progress(task_id, |p| {
            p.status = TaskStatus::Completed;
            p.percent = Some(100.0);
            p.message = summary.clone();
        })
        .await;
}

/// Run one reconcile pass. See the module docs for the semantics.
pub async fn run_sync(
    task_manager: &TaskManager,
    pool: &Pool<Sqlite>,
    config: &ServiceCredentials,
    task_id: &str,
    cancel_token: &CancellationToken,
    strict: bool,
) -> Result<SyncOutcome> {
    let settings = db::load_bpm_key_settings(pool).await?;
    let groups = db::derive_groups(pool).await?;
    let desired: Vec<&Group> = groups
        .iter()
        .filter(|g| g.track_count >= settings.min_tracks)
        .collect();

    task_manager
        .add_log(
            task_id,
            format!(
                "{} bucket(s) meet min_tracks={} (public={}, strict={})",
                desired.len(),
                settings.min_tracks,
                settings.public,
                strict
            ),
        )
        .await;

    let total = desired.len();
    let mut outcome = SyncOutcome {
        group_count: total,
        ..Default::default()
    };

    // A single labeled loop funnels *every* early exit — cancellation, rate-limit
    // cooldown and normal completion — through the shared finalization below.
    // Returning early from inside the loop used to skip finalization and leave
    // the task stuck at `Running` forever; its `bpm_key_sync` conflict key then
    // rejected every later sync as "already running".
    'sync: loop {
        // Pre-flight: a cancelled run or an active cooldown aborts before we
        // resolve a client or touch the network.
        if cancel_token.is_cancelled() {
            task_manager
                .add_log(task_id, "Task cancelled by user".to_string())
                .await;
            break 'sync;
        }
        if let Some(secs) = spotify_cooldown().remaining_secs() {
            task_manager
                .add_log(
                    task_id,
                    format!(
                        "Spotify rate-limit cooldown active ({} remaining) — aborting run, \
                         {} bucket(s) left for the next run",
                        format_duration(secs),
                        total
                    ),
                )
                .await;
            outcome.aborted = true;
            break 'sync;
        }

        let client = SpotifyClient::from_stored_tokens(pool.clone(), config)
            .await
            .context("Spotify is not configured")?;
        let user_id = client
            .get_current_user_id()
            .await
            .context("Failed to resolve Spotify user id")?;

        for (i, group) in desired.iter().enumerate() {
            if cancel_token.is_cancelled() {
                task_manager
                    .add_log(task_id, "Task cancelled by user".to_string())
                    .await;
                break 'sync;
            }
            if let Some(secs) = spotify_cooldown().remaining_secs() {
                let left = total - i;
                task_manager
                    .add_log(
                        task_id,
                        format!(
                            "Spotify rate-limit cooldown active ({} remaining) — aborting run, \
                             {} bucket(s) left for the next run",
                            format_duration(secs),
                            left
                        ),
                    )
                    .await;
                outcome.aborted = true;
                break 'sync;
            }

            let name = settings.name_for(
                group.bpm,
                &group.canonical_key,
                Some(group.track_count as usize),
            );
            let system_key = group.system_key();

            // 1. Find or create the Spotify playlist.
            let playlist_id = match db::find_system_playlist(pool, &system_key).await? {
                Some((db_id, pid)) => {
                    // Keep the stored display name in sync with the template.
                    let _ = db::update_system_playlist_name(pool, db_id, &name).await;
                    pid
                }
                None => {
                    // Stage creations instead of bursting them.
                    tokio::time::sleep(INTER_CALL_DELAY).await;
                    metrics::record(Source::BpmKeySync);
                    match client
                        .create_playlist(
                            &user_id,
                            &name,
                            settings.public,
                            Some("Auto-generated by Momo's Music Manager (BPM//key)"),
                        )
                        .await
                    {
                        Ok((pid, _url)) => {
                            match db::insert_system_playlist(pool, &name, &system_key, &pid).await {
                                Ok(_) => {
                                    outcome.created += 1;
                                    pid
                                }
                                Err(e) => {
                                    // Row insert failed after creation — skip but keep going.
                                    warn!(
                                        "Failed to record system playlist {}: {:#}",
                                        system_key, e
                                    );
                                    outcome.failed += 1;
                                    continue;
                                }
                            }
                        }
                        Err(e) => match classify(&e) {
                            SpotifyFail::NeedsReauth => {
                                task_manager
                                    .add_log(
                                        task_id,
                                        "Spotify token needs write permissions. \
                                         Re-authenticate on the Services page."
                                            .to_string(),
                                    )
                                    .await;
                                return Err(anyhow::anyhow!(
                                    "Spotify token needs write permissions (needsReauth)"
                                ));
                            }
                            SpotifyFail::RateLimited(secs) => {
                                spotify_cooldown().note_retry_after(secs);
                                task_manager
                                    .add_log(
                                        task_id,
                                        format!(
                                            "Rate limited creating '{}' (Retry-After {}) — aborting run",
                                            name,
                                            format_duration(secs)
                                        ),
                                    )
                                    .await;
                                outcome.aborted = true;
                                break 'sync;
                            }
                            SpotifyFail::Other(msg) => {
                                warn!("Failed to create system playlist '{}': {}", name, msg);
                                task_manager
                                    .add_log(
                                        task_id,
                                        format!("Failed to create '{}': {}", name, msg),
                                    )
                                    .await;
                                outcome.failed += 1;
                                continue;
                            }
                        },
                    }
                }
            };

            // 2. Mirror the item set (idempotent). Paced so a run stages its
            //    calls instead of bursting.
            tokio::time::sleep(INTER_CALL_DELAY).await;
            metrics::record(Source::BpmKeySync);
            match client
                .replace_playlist_items(&playlist_id, &group.uris)
                .await
            {
                Ok(()) => {
                    if let Some((db_id, _)) = db::find_system_playlist(pool, &system_key).await? {
                        let _ = db::update_system_playlist_counts(pool, db_id, group.track_count)
                            .await;
                    }
                    outcome.mirrored += 1;
                }
                Err(e) => match classify(&e) {
                    SpotifyFail::NeedsReauth => {
                        task_manager
                            .add_log(
                                task_id,
                                "Spotify token needs write permissions — aborting run".to_string(),
                            )
                            .await;
                        return Err(anyhow::anyhow!(
                            "Spotify token needs write permissions (needsReauth)"
                        ));
                    }
                    SpotifyFail::RateLimited(secs) => {
                        spotify_cooldown().note_retry_after(secs);
                        task_manager
                            .add_log(
                                task_id,
                                format!(
                                    "Rate limited mirroring '{}' (Retry-After {}) — aborting run",
                                    name,
                                    format_duration(secs)
                                ),
                            )
                            .await;
                        outcome.aborted = true;
                        break 'sync;
                    }
                    SpotifyFail::Other(msg) => {
                        warn!("Failed to mirror '{}': {}", name, msg);
                        task_manager
                            .add_log(task_id, format!("Failed to mirror '{}': {}", name, msg))
                            .await;
                        outcome.failed += 1;
                    }
                },
            }

            // 3. Progress.
            let pct = ((i + 1) as f32 / total as f32) * 100.0;
            task_manager
                .update_progress(task_id, |p| {
                    p.percent = Some(pct);
                    p.message = format!("{}/{} buckets synced", i + 1, total);
                })
                .await;
        }

        // 4. Strict cleanup: remove rows (and clear the remote playlist) for buckets
        //    that no longer exist. Off by default — empty groups are kept.
        if strict {
            let desired_keys: HashSet<String> = desired.iter().map(|g| g.system_key()).collect();
            let existing = db::list_system_playlists(pool).await?;
            for row in vanished(&existing, &desired_keys) {
                // Best-effort clear the remote playlist so it does not linger as a
                // stale mirror, then drop the row.
                metrics::record(Source::BpmKeySync);
                if let Err(e) = client.replace_playlist_items(&row.playlist_id, &[]).await {
                    if let SpotifyFail::RateLimited(secs) = classify(&e) {
                        spotify_cooldown().note_retry_after(secs);
                        outcome.aborted = true;
                        break;
                    }
                    warn!("Failed to clear vanished playlist '{}': {:#}", row.name, e);
                }
                db::delete_system_playlist(pool, row.id).await?;
                outcome.deleted += 1;
            }
        }

        break 'sync;
    }

    finalize(task_manager, task_id, &outcome).await;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    use crate::tasks::{Task, TaskType};

    fn row(id: i64, key: &str) -> SystemPlaylist {
        SystemPlaylist {
            id,
            name: format!("row {id}"),
            system_key: key.to_string(),
            playlist_id: format!("p{id}"),
        }
    }

    #[test]
    fn classify_detects_needs_reauth() {
        let e = anyhow::anyhow!("create playlist failed: 403 Forbidden");
        assert_eq!(classify(&e), SpotifyFail::NeedsReauth);
    }

    #[test]
    fn classify_other() {
        let e = anyhow::anyhow!("boom");
        assert!(matches!(classify(&e), SpotifyFail::Other(_)));
    }

    #[test]
    fn vanished_returns_only_undesired_rows() {
        let existing = vec![row(1, "bpm_key:124:12A"), row(2, "bpm_key:130:4B")];
        let desired: HashSet<String> = ["bpm_key:124:12A".to_string()].into_iter().collect();
        let gone = vanished(&existing, &desired);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].system_key, "bpm_key:130:4B");
    }

    /// Minimal schema so `load_bpm_key_settings` + `derive_groups` succeed while
    /// `SpotifyClient::from_stored_tokens` (never reached here — a cancel/cooldown
    /// aborts first) would fail.
    async fn preflight_pool() -> Pool<Sqlite> {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        for stmt in [
            r#"CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT 0
            )"#,
            r#"CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                bpm REAL,
                musical_key TEXT,
                stem_type TEXT,
                isrc TEXT,
                spotify_id TEXT
            )"#,
            r#"CREATE TABLE service_tracks (
                id INTEGER PRIMARY KEY,
                service TEXT NOT NULL,
                service_id TEXT NOT NULL,
                isrc TEXT,
                UNIQUE(service, service_id)
            )"#,
            r#"CREATE VIEW v_file_track_link AS
               SELECT f.id AS file_id, st.id AS track_id
               FROM files f
               JOIN service_tracks st ON (
                   st.isrc = f.isrc
                   OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
               )"#,
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        pool
    }

    /// Register a sync task and mark it `Running`, as the worker does.
    async fn start_running_sync(tm: &TaskManager) -> String {
        let task = Task::new(TaskType::SyncBpmKeyPlaylists { strict: false }, None);
        let id = task.id.clone();
        tm.start_task_unique(task).await.unwrap();
        tm.update_task_status(&id, TaskStatus::Running).await;
        id
    }

    #[tokio::test]
    async fn finalize_marks_aborted_run_completed() {
        let tm = TaskManager::new();
        let id = start_running_sync(&tm).await;

        let outcome = SyncOutcome {
            group_count: 3,
            created: 1,
            mirrored: 1,
            aborted: true,
            ..Default::default()
        };
        finalize(&tm, &id, &outcome).await;

        let p = tm.get_task(&id).await.unwrap();
        assert_eq!(p.status, TaskStatus::Completed);
        assert_eq!(p.percent, Some(100.0));
        assert!(
            p.progress.contains("aborted (rate-limit)"),
            "summary must mention the abort: {}",
            p.progress
        );
        assert!(
            p.logs.iter().any(|l| l.contains("aborted (rate-limit)")),
            "summary must be logged: {:?}",
            p.logs
        );
    }

    #[tokio::test]
    async fn run_sync_cancelled_run_finalizes_instead_of_sticking_running() {
        let tm = TaskManager::new();
        let pool = preflight_pool().await;
        let id = start_running_sync(&tm).await;

        let token = CancellationToken::new();
        token.cancel();

        let outcome = run_sync(
            &tm,
            &pool,
            &ServiceCredentials::defaults_for_test(),
            &id,
            &token,
            false,
        )
        .await
        .expect("a cancelled run must return Ok, not Err");
        assert!(!outcome.aborted, "cancellation is not a rate-limit abort");

        let p = tm.get_task(&id).await.unwrap();
        assert_eq!(
            p.status,
            TaskStatus::Completed,
            "a cancelled run must not stick at Running"
        );
    }

    #[tokio::test]
    async fn run_sync_rate_limit_abort_finalizes_instead_of_sticking_running() {
        let tm = TaskManager::new();
        let pool = preflight_pool().await;
        let id = start_running_sync(&tm).await;

        // Simulate a live rate-limit penalty. Touches the process-wide cooldown,
        // so clear it as soon as the run returns.
        spotify_cooldown().note_retry_after(3600);
        let result = run_sync(
            &tm,
            &pool,
            &ServiceCredentials::defaults_for_test(),
            &id,
            &CancellationToken::new(),
            false,
        )
        .await;
        spotify_cooldown().clear();

        let outcome = result.expect("a rate-limited run must return Ok(aborted), not Err");
        assert!(outcome.aborted, "the run must be marked aborted");

        let p = tm.get_task(&id).await.unwrap();
        assert_eq!(
            p.status,
            TaskStatus::Completed,
            "a rate-limited run must not stick at Running"
        );
        assert_eq!(p.percent, Some(100.0));
    }
}
