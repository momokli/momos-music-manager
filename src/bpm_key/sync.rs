//! Reconcile + push BPM//key system playlists to Spotify.
//!
//! One sequential pass: derive the desired buckets, then for each bucket find
//! the `system_key` row (create the Spotify playlist when absent) and mirror the
//! item set via [`SpotifyClient::replace_playlist_items`]. Idempotent — an
//! unchanged library creates nothing new and rewrites the same item set.
//!
//! Rate-limit handling follows ADR-061: a `429` sets the process-wide cooldown
//! and **aborts the run**, leaving the remaining buckets for the next run.

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
use crate::spotify::retry::{extract_retry_after_secs, format_duration};
use crate::tasks::{TaskManager, TaskStatus};

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

    let client = SpotifyClient::from_stored_tokens(pool.clone(), config)
        .await
        .context("Spotify is not configured")?;
    let user_id = client
        .get_current_user_id()
        .await
        .context("Failed to resolve Spotify user id")?;

    let mut outcome = SyncOutcome {
        group_count: desired.len(),
        ..Default::default()
    };
    let total = desired.len();

    for (i, group) in desired.iter().enumerate() {
        if cancel_token.is_cancelled() {
            task_manager
                .add_log(task_id, "Task cancelled by user".to_string())
                .await;
            return Ok(outcome);
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
            return Ok(outcome);
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
                                warn!("Failed to record system playlist {}: {:#}", system_key, e);
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
                            return Ok(outcome);
                        }
                        SpotifyFail::Other(msg) => {
                            warn!("Failed to create system playlist '{}': {}", name, msg);
                            task_manager
                                .add_log(task_id, format!("Failed to create '{}': {}", name, msg))
                                .await;
                            outcome.failed += 1;
                            continue;
                        }
                    },
                }
            }
        };

        // 2. Mirror the item set (idempotent).
        match client
            .replace_playlist_items(&playlist_id, &group.uris)
            .await
        {
            Ok(()) => {
                if let Some((db_id, _)) = db::find_system_playlist(pool, &system_key).await? {
                    let _ = db::update_system_playlist_counts(pool, db_id, group.track_count).await;
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
                    return Ok(outcome);
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
    task_manager
        .update_progress(task_id, |p| {
            p.status = TaskStatus::Completed;
            p.percent = Some(100.0);
            p.message = summary.clone();
        })
        .await;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
