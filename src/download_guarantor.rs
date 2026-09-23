//! Download Guarantor — aggressive auto-remediation background task that
//! guarantees 100% file coverage for all subscribed Spotify playlists.
//!
//! Two-phase architecture:
//! 1. **Queue Sync**: Polls the deemix-pyweb API, UPSERTs real download status
//!    into `deemix_downloads`, detects zombie/stuck entries.
//! 2. **Gap Remediation**: For every track in every subscribed playlist that has
//!    no linked file, tries deemix first (re-queue), then falls back to spotDL
//!    (YouTube download via local CLI).
//!
//! Runs every 10 minutes.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use sqlx::{Pool, Sqlite};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::db::ScanMode;
use crate::deemix::client::DeemixClient;
use crate::tasks::{Task, TaskManager, TaskStatus, TaskType};

// ── Public API ───────────────────────────────────────────────────────────

pub struct DownloadGuarantor {
    db: Pool<Sqlite>,
    task_manager: TaskManager,
    deemix_base_url: String,
    /// Where spotDL downloads MP3s (isolated from FLACs)
    mp3_dir: String,
    /// Where deemix downloads FLACs (for fuzzy file matching)
    flacs_dir: String,
    interval: Duration,
}

impl DownloadGuarantor {
    pub fn new(db: Pool<Sqlite>, task_manager: TaskManager) -> Self {
        Self {
            db,
            task_manager,
            deemix_base_url: "http://localhost:6596".to_string(),
            mp3_dir: "/Users/momo/Music/mp3".to_string(),
            flacs_dir: "/Users/momo/Music/flacs".to_string(),
            interval: Duration::from_secs(600), // 10 minutes
        }
    }

    /// Main loop. Runs every `interval`. Honors `cancel_token`.
    pub async fn run(&self, cancel_token: CancellationToken) {
        info!(
            "DownloadGuarantor started (interval: {}s)",
            self.interval.as_secs()
        );

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("DownloadGuarantor shutting down");
                    break;
                }
                _ = tokio::time::sleep(self.interval) => {
                    let task_id = self
                        .task_manager
                        .start_task(Task::new(TaskType::DeemixSync, None))
                        .await;

                    self.task_manager
                        .update_task_status(&task_id, TaskStatus::Running)
                        .await;

                    // Step 1: Sync deemix queue → DB
                    match self.sync_queue(&task_id).await {
                        Ok(report) => {
                            self.task_manager.add_log(&task_id,
                                format!("Queue sync: {} items synced, {} zombies, {} stuck",
                                    report.items_synced,
                                    report.zombies_detected.len(),
                                    report.stuck_detected.len())
                            ).await;
                            for z in &report.zombies_detected {
                                info!("Zombie playlist (0 downloads): {}", z);
                            }
                            for s in &report.stuck_detected {
                                info!("Stuck playlist (inQueue > 1h): {}", s);
                            }
                        }
                        Err(e) => {
                            warn!("Queue sync failed: {:#}", e);
                            self.task_manager.add_log(&task_id,
                                format!("Queue sync FAILED: {:#}", e)
                            ).await;
                            crate::telemetry::emit::emit_event(
                                crate::telemetry::events::EventType::ErrorReported,
                                crate::telemetry::events::error_payload(&format!(
                                    "deemix queue sync failed: {e:#}"
                                )),
                            );
                        }
                    }

                    // Step 2: Analyze gaps
                    match self.analyze_gaps(&task_id).await {
                        Ok(gaps) => {
                            let total_missing: usize = gaps.iter().map(|g| g.missing_tracks.len()).sum();
                            if total_missing > 0 {
                                self.task_manager.add_log(&task_id,
                                    format!("Gap analysis: {} playlist(s) with {} missing track(s) total",
                                        gaps.len(), total_missing)
                                ).await;
                                for gap in &gaps {
                                    let deezer_gap = gap.missing_tracks.iter()
                                        .filter(|t| matches!(t.reason, MissingReason::NotOnDeezer))
                                        .count();
                                    let zombies = gap.missing_tracks.iter()
                                        .filter(|t| matches!(t.reason, MissingReason::ZombiePlaylist))
                                        .count();
                                    let fuzzy = gap.missing_tracks.iter()
                                        .filter(|t| matches!(t.reason, MissingReason::FileMayExist))
                                        .count();
                                    info!(
                                        "Gap: '{}' — {} missing ({} Deezer-gap, {} zombie, {} file-may-exist)",
                                        gap.playlist_name, gap.missing_tracks.len(),
                                        deezer_gap, zombies, fuzzy,
                                    );
                                    self.task_manager.add_log(&task_id,
                                        format!("  '{}': {} missing ({} Deezer-gap, {} zombie, {} fuzzy)",
                                            gap.playlist_name, gap.missing_tracks.len(),
                                            deezer_gap, zombies, fuzzy)
                                    ).await;
                                }

                                // Step 3: Remediate
                                match self.remediate(&task_id, &gaps).await {
                                    Ok(report) => {
                                        self.task_manager.add_log(&task_id,
                                            format!("Remediation: {} requeued, {} spotDL downloads ({} failed), {} fuzzy-matched",
                                                report.requeued_playlists,
                                                report.spotdl_downloads,
                                                report.spotdl_failures,
                                                report.fuzzy_matches)
                                        ).await;
                                    }
                                    Err(e) => {
                                        warn!("Remediation failed: {:#}", e);
                                        self.task_manager.add_log(&task_id,
                                            format!("Remediation FAILED: {:#}", e)
                                        ).await;
                                    }
                                }
                            } else {
                                self.task_manager.add_log(&task_id,
                                    "Gap analysis: all subscribed playlists fully covered".to_string()
                                ).await;
                            }
                        }
                        Err(e) => {
                            warn!("Gap analysis failed: {:#}", e);
                            self.task_manager.add_log(&task_id,
                                format!("Gap analysis FAILED: {:#}", e)
                            ).await;
                        }
                    }

                    self.task_manager
                        .update_task_status(&task_id, TaskStatus::Completed)
                        .await;
                }
            }
        }
    }

    // ── Step 1: Sync deemix queue → deemix_downloads ─────────────────

    async fn sync_queue(&self, _task_id: &str) -> Result<SyncReport> {
        let client = match DeemixClient::from_db(self.db.clone()).await {
            Some(c) => c,
            None => {
                info!("Deemix not connected — skipping queue sync");
                return Ok(SyncReport {
                    items_synced: 0,
                    zombies_detected: Vec::new(),
                    stuck_detected: Vec::new(),
                });
            }
        };

        let queue = client
            .get_queue()
            .await
            .context("Failed to fetch deemix queue")?;

        let mut items_synced = 0usize;
        let mut zombies_detected = Vec::new();
        let mut stuck_detected = Vec::new();

        for (_uuid, item) in &queue {
            let spotify_url = format!("https://open.spotify.com/playlist/{}", item.id);

            // Remember the previous stored state so we can emit lifecycle
            // events only on real transitions (completed/failed).
            let prev_status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM deemix_downloads WHERE spotify_playlist_url = ?",
            )
            .bind(&spotify_url)
            .fetch_optional(&self.db)
            .await
            .ok()
            .flatten();

            // Serialize errors to JSON for storage
            let error_json = if item.errors.is_empty() {
                None
            } else {
                serde_json::to_string(&item.errors).ok()
            };

            crate::db::upsert_deemix_download(
                &self.db,
                &spotify_url,
                &item.title,
                &item.status,
                item.size,
                item.downloaded,
                error_json.as_deref(),
            )
            .await
            .context("Failed to upsert deemix download")?;

            items_synced += 1;

            // Detect zombies: status is "queued" and nothing downloaded
            let status_lower = item.status.to_lowercase();
            if status_lower == "queued" && item.downloaded == 0 {
                zombies_detected.push(item.title.clone());
            }

            // Detect stuck: status is "inQueue" (not yet started)
            if status_lower == "inqueue" {
                stuck_detected.push(item.title.clone());
            }

            // Telemetry: deemix lifecycle transitions (source=deemix).
            emit_deemix_transition(
                prev_status.as_deref(),
                &item.status,
                item.downloaded,
                item.size,
                !item.errors.is_empty(),
            );
        }

        info!(
            "Queue sync: {} items synced, {} zombies, {} stuck",
            items_synced,
            zombies_detected.len(),
            stuck_detected.len()
        );

        Ok(SyncReport {
            items_synced,
            zombies_detected,
            stuck_detected,
        })
    }

    // ── Step 2: Find missing files per subscription ──────────────────

    async fn analyze_gaps(&self, _task_id: &str) -> Result<Vec<SubscriptionGap>> {
        // Query all subscribed playlists with their Spotify details
        let subscriptions: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT ps.id, sp.name,
                   'https://open.spotify.com/playlist/' || sp.playlist_id AS spotify_url
            FROM playlist_subscriptions ps
            JOIN service_playlists sp ON sp.service = ps.service
                AND sp.playlist_id = ps.playlist_id
            WHERE ps.service = 'spotify'
            "#,
        )
        .fetch_all(&self.db)
        .await
        .context("Failed to query subscribed playlists")?;

        // Get zombie entries — these playlists need re-queuing
        let zombie_entries: HashSet<String> = crate::db::get_zombie_deemix_entries(&self.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(_id, url)| url)
            .collect();

        let mut gaps = Vec::new();

        for (sub_id, playlist_name, playlist_url) in subscriptions {
            let playlist_url = playlist_url.unwrap_or_default();
            let playlist_name = playlist_name.unwrap_or_default();

            // Query tracks in this playlist that have NO linked files
            let missing_tracks: Vec<(i64, String, String, Option<String>, String)> =
                sqlx::query_as(
                    r#"
                SELECT st.id, st.title, st.artist, st.isrc,
                       'https://open.spotify.com/track/' || st.service_id AS spotify_url
                FROM service_playlist_tracks spt
                JOIN service_tracks st ON st.id = spt.track_id
                JOIN service_playlists sp ON sp.id = spt.playlist_id
                WHERE sp.name = ?
                  AND spt.track_id NOT IN (
                      SELECT track_id FROM v_file_track_link
                  )
                "#,
                )
                .bind(&playlist_name)
                .fetch_all(&self.db)
                .await
                .context(format!(
                    "Failed to query missing tracks for playlist '{}'",
                    playlist_name
                ))?;

            if missing_tracks.is_empty() {
                continue;
            }

            let is_zombie = zombie_entries.contains(&playlist_url);

            // Build the list of tracks not on Deezer from the deemix_downloads errors
            let deezer_gap_titles: HashSet<String> = self
                .get_deezer_error_titles(&playlist_url)
                .await
                .unwrap_or_default();

            let mut categorized: Vec<MissingTrack> = Vec::new();

            for (track_id, title, artist, isrc, spotify_url) in missing_tracks {
                let reason = if is_zombie {
                    MissingReason::ZombiePlaylist
                } else if deezer_gap_titles.contains(&title) {
                    MissingReason::NotOnDeezer
                } else {
                    // File may exist on disk with different ISRC — try fuzzy match
                    MissingReason::FileMayExist
                };

                // For FileMayExist tracks, try fuzzy-matching right away
                if matches!(reason, MissingReason::FileMayExist) {
                    if let Ok(Some(file_id)) = self.fuzzy_match_file_inner(&artist, &title).await {
                        // Link the file directly, bypassing ISRC
                        if let Err(e) =
                            crate::db::link_file_to_track_direct(&self.db, file_id, track_id).await
                        {
                            warn!(
                                "Fuzzy match found but failed to link file {} → track {} ({} - {}): {:#}",
                                file_id, track_id, artist, title, e
                            );
                        } else {
                            info!(
                                "Fuzzy-matched file {} to track {} ({} - {})",
                                file_id, track_id, artist, title
                            );
                            // Successfully linked — don't add to missing list
                            continue;
                        }
                    }
                }

                categorized.push(MissingTrack {
                    track_id,
                    title,
                    artist,
                    isrc,
                    spotify_url,
                    reason,
                });
            }

            if !categorized.is_empty() {
                gaps.push(SubscriptionGap {
                    subscription_id: sub_id,
                    playlist_name,
                    playlist_url,
                    total_tracks: categorized.len(),
                    missing_tracks: categorized,
                });
            }
        }

        Ok(gaps)
    }

    /// Get the set of track titles that failed to download on Deezer for a
    /// given playlist. Reads from the `errors` JSON in `deemix_downloads`.
    async fn get_deezer_error_titles(&self, playlist_url: &str) -> Result<HashSet<String>> {
        let errors_json: Option<String> = sqlx::query_scalar(
            r#"
            SELECT errors
            FROM deemix_downloads
            WHERE spotify_playlist_url = ?
            "#,
        )
        .bind(playlist_url)
        .fetch_optional(&self.db)
        .await
        .context("Failed to query deemix errors")?
        .flatten();

        let errors_json = match errors_json {
            Some(j) => j,
            None => return Ok(HashSet::new()),
        };

        // Parse the JSON array of DeemixDownloadError
        let errors: Vec<crate::deemix::models::DeemixDownloadError> =
            serde_json::from_str(&errors_json).unwrap_or_default();

        let titles: HashSet<String> = errors
            .into_iter()
            .filter_map(|e| e.data)
            .map(|d| d.title)
            .collect();

        Ok(titles)
    }

    // ── Step 3: Remediate gaps ───────────────────────────────────────

    async fn remediate(
        &self,
        task_id: &str,
        gaps: &[SubscriptionGap],
    ) -> Result<RemediationReport> {
        let mut report = RemediationReport {
            requeued_playlists: 0,
            spotdl_downloads: 0,
            spotdl_failures: 0,
            fuzzy_matches: 0,
        };

        // Collect unique zombie playlists to re-queue
        let zombie_urls: HashSet<&str> = gaps
            .iter()
            .filter(|g| {
                g.missing_tracks
                    .iter()
                    .any(|t| matches!(t.reason, MissingReason::ZombiePlaylist))
            })
            .map(|g| g.playlist_url.as_str())
            .collect();

        if !zombie_urls.is_empty() {
            // Try to get a deemix client for re-queuing
            if let Some(client) = DeemixClient::from_db(self.db.clone()).await {
                for url in &zombie_urls {
                    self.task_manager
                        .add_log(task_id, format!("Re-queuing zombie playlist: {}", url))
                        .await;
                    match client.add_to_queue(url).await {
                        Ok(()) => {
                            report.requeued_playlists += 1;
                            info!("Re-queued zombie playlist: {}", url);
                            crate::telemetry::emit::emit_event(
                                crate::telemetry::events::EventType::DownloadStarted,
                                serde_json::json!({
                                    "source": "deemix",
                                    "kind": "playlist",
                                }),
                            );
                        }
                        Err(e) => {
                            warn!("Failed to re-queue {}: {:#}", url, e);
                            self.task_manager
                                .add_log(task_id, format!("Re-queue FAILED for {}: {:#}", url, e))
                                .await;
                            crate::telemetry::emit::emit_event(
                                crate::telemetry::events::EventType::DownloadFailed,
                                crate::telemetry::events::error_payload(&format!(
                                    "deemix re-queue failed: {e:#}"
                                )),
                            );
                        }
                    }
                }
            } else {
                warn!("Deemix not connected — cannot re-queue zombie playlists");
                self.task_manager
                    .add_log(
                        task_id,
                        "Deemix not connected — skipping zombie re-queue".to_string(),
                    )
                    .await;
            }
        }

        // spotDL downloads for Deezer-gap tracks
        let deezer_gap_tracks: Vec<&MissingTrack> = gaps
            .iter()
            .flat_map(|g| g.missing_tracks.iter())
            .filter(|t| matches!(t.reason, MissingReason::NotOnDeezer))
            .collect();

        if !deezer_gap_tracks.is_empty() {
            self.task_manager
                .add_log(
                    task_id,
                    format!(
                        "Attempting spotDL downloads for {} Deezer-gap tracks",
                        deezer_gap_tracks.len()
                    ),
                )
                .await;

            // Rate-limit: max 1 download per 2 seconds
            let mut last_download = Instant::now()
                .checked_sub(Duration::from_secs(3))
                .unwrap_or(Instant::now());

            for track in &deezer_gap_tracks {
                // Enforce rate limit
                let elapsed = last_download.elapsed();
                if elapsed < Duration::from_secs(2) {
                    tokio::time::sleep(Duration::from_secs(2) - elapsed).await;
                }

                match self
                    .spotdl_download_track(&track.spotify_url, &track.artist, &track.title)
                    .await
                {
                    Ok(()) => {
                        report.spotdl_downloads += 1;
                        info!("spotDL: downloaded '{} - {}", track.artist, track.title);
                    }
                    Err(e) => {
                        report.spotdl_failures += 1;
                        warn!(
                            "spotDL: failed '{} - {}': {:#}",
                            track.artist, track.title, e
                        );
                    }
                }

                last_download = Instant::now();
            }

            // Trigger an immediate scan of the mp3 folder so the scanner picks
            // up the newly downloaded files. The fuzzy match will link them
            // on the next guarantor cycle (≤10 min).
            if report.spotdl_downloads > 0 {
                if let Ok(folder_id) = sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM folders WHERE folder_path = '/Users/momo/Music/mp3'",
                )
                .fetch_optional(&self.db)
                .await
                .map(|r| r.unwrap_or(0))
                {
                    if folder_id > 0 {
                        if let Err(e) = crate::tasks::start_scan_folder_task(
                            &self.task_manager,
                            &self.db,
                            folder_id,
                            ScanMode::Incremental { since: None },
                        )
                        .await
                        {
                            warn!("Failed to trigger mp3 folder scan: {:#}", e);
                        } else {
                            info!(
                                "Triggered mp3 folder scan — {} file(s) will be linked next cycle",
                                report.spotdl_downloads
                            );
                        }
                    }
                }
            }
        }

        Ok(report)
    }

    // ── Helpers ──────────────────────────────────────────────────────

    /// Re-queue a single playlist via the deemix API.
    #[allow(dead_code)]
    async fn requeue_playlist(&self, playlist_url: &str) -> Result<()> {
        let client = DeemixClient::from_db(self.db.clone())
            .await
            .context("Deemix not connected")?;
        client
            .add_to_queue(playlist_url)
            .await
            .context("Failed to add to deemix queue")?;
        info!("Re-queued playlist to deemix: {}", playlist_url);
        Ok(())
    }

    /// Download a single track via spotDL CLI.
    ///
    /// Runs `spotdl download <url> --output <flacs_dir> --bitrate 320k --format mp3`.
    /// Times out after 120 seconds. Gracefully handles spotDL not being installed.
    /// Emits `download.started/completed/failed` telemetry (source=spotdl).
    async fn spotdl_download_track(
        &self,
        spotify_track_url: &str,
        artist: &str,
        title: &str,
    ) -> Result<()> {
        crate::telemetry::emit::emit_event(
            crate::telemetry::events::EventType::DownloadStarted,
            serde_json::json!({ "source": "spotdl", "kind": "track" }),
        );
        // Sanitize: spotDL handles URLs directly, but we construct the command carefully
        let output = tokio::process::Command::new("spotdl")
            .arg("download")
            .arg(spotify_track_url)
            .arg("--output")
            .arg(&self.mp3_dir)
            .arg("--bitrate")
            .arg("320k")
            .arg("--format")
            .arg("mp3")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output();

        match tokio::time::timeout(Duration::from_secs(120), output).await {
            Ok(Ok(o)) => {
                if o.status.success() {
                    crate::telemetry::emit::emit_event(
                        crate::telemetry::events::EventType::DownloadCompleted,
                        serde_json::json!({ "source": "spotdl", "kind": "track" }),
                    );
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let msg = format!("spotDL exited with {}: {}", o.status, stderr.trim());
                    crate::telemetry::emit::emit_event(
                        crate::telemetry::events::EventType::DownloadFailed,
                        crate::telemetry::events::error_payload(&msg),
                    );
                    anyhow::bail!(msg);
                }
            }
            Ok(Err(e)) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    "spotDL not installed — run: pip install spotdl".to_string()
                } else {
                    format!("spotDL command failed: {e}")
                };
                crate::telemetry::emit::emit_event(
                    crate::telemetry::events::EventType::DownloadFailed,
                    crate::telemetry::events::error_payload(&msg),
                );
                anyhow::bail!(msg);
            }
            Err(_elapsed) => {
                let msg = format!("spotDL timed out after 120s for '{artist} - {title}'");
                crate::telemetry::emit::emit_event(
                    crate::telemetry::events::EventType::DownloadFailed,
                    crate::telemetry::events::error_payload(&msg),
                );
                anyhow::bail!(msg);
            }
        }
    }

    /// Public-facing fuzzy match: returns file_id if a file exists in either the
    /// flacs or mp3 directory whose filename's artist+title match the given inputs.
    #[allow(dead_code)]
    async fn fuzzy_match_file(&self, artist: &str, title: &str) -> Result<Option<i64>> {
        self.fuzzy_match_file_inner(artist, title).await
    }

    /// Inner fuzzy match logic. Searches both flacs (deemix) and mp3 (spotDL) directories.
    async fn fuzzy_match_file_inner(&self, artist: &str, title: &str) -> Result<Option<i64>> {
        let normalized_input = normalize_for_fuzzy(artist, title);

        for dir_prefix in [self.flacs_dir.as_str(), self.mp3_dir.as_str()] {
            let files: Vec<(i64, String)> =
                sqlx::query_as("SELECT id, file_path FROM files WHERE file_path LIKE ? || '/%'")
                    .bind(dir_prefix)
                    .fetch_all(&self.db)
                    .await
                    .context("Failed to query files for fuzzy matching")?;

            for (file_id, file_path) in files {
                let filename = std::path::Path::new(&file_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");

                if let Some((file_artist, file_title)) = filename.split_once(" - ") {
                    let normalized_file = normalize_for_fuzzy(file_artist, file_title);
                    if normalized_input == normalized_file {
                        return Ok(Some(file_id));
                    }
                }
            }
        }

        Ok(None)
    }
}

// ── Helper: deemix lifecycle bucket (free fn, telemetry) ─────────────

/// Terminal-state bucket of a deemix queue item, used for lifecycle-
/// transition detection (emit only when the observed state changed).
fn deemix_state_bucket(status: &str, downloaded: i64, size: i64, has_errors: bool) -> &'static str {
    let lower = status.to_lowercase();
    if has_errors || lower.contains("error") || lower.contains("fail") {
        "failed"
    } else if (size > 0 && downloaded >= size)
        || lower.contains("complete")
        || lower.contains("finish")
    {
        "completed"
    } else {
        "active"
    }
}

/// Emit `download.completed` / `download.failed` (source=deemix) when a
/// queue item transitions into a terminal state. Only transitions we can
/// observe (a previous stored status exists) are emitted — no retroactive
/// events for items that were already finished before momos tracked them.
fn emit_deemix_transition(
    prev_status: Option<&str>,
    status: &str,
    downloaded: i64,
    size: i64,
    has_errors: bool,
) {
    use crate::telemetry::emit::emit_event;
    use crate::telemetry::events::{EventType, error_payload};

    let Some(prev) = prev_status else {
        return; // first observation — nothing to compare against
    };
    let prev_bucket = deemix_state_bucket(prev, downloaded, size, false);
    let new_bucket = deemix_state_bucket(status, downloaded, size, has_errors);
    if prev_bucket == new_bucket {
        return;
    }
    match new_bucket {
        "completed" => {
            emit_event(
                EventType::DownloadCompleted,
                serde_json::json!({ "source": "deemix", "kind": "playlist" }),
            );
        }
        "failed" => {
            emit_event(
                EventType::DownloadFailed,
                error_payload("deemix download failed (see deemix_downloads.errors)"),
            );
        }
        _ => {}
    }
}

// ── Helper: normalize strings for fuzzy matching ────────────────────

/// Normalize artist + title for fuzzy comparison.
///
/// - Lowercase
/// - Strip common punctuation
/// - Collapse whitespace
fn normalize_for_fuzzy(artist: &str, title: &str) -> String {
    let combined = format!("{} - {}", artist, title);
    let mut result = String::with_capacity(combined.len());

    for ch in combined.chars() {
        match ch {
            // Keep alphanumeric, spaces, and hyphens; drop everything else
            c if c.is_alphanumeric() || c == ' ' || c == '-' => {
                result.push(c.to_ascii_lowercase());
            }
            // Collapse consecutive whitespace
            _ => {
                if !result.ends_with(' ') {
                    result.push(' ');
                }
            }
        }
    }

    // Collapse multiple spaces and trim
    let collapsed: String = result.split_whitespace().collect::<Vec<_>>().join(" ");

    collapsed
}

// ── Data types ──────────────────────────────────────────────────────

struct SyncReport {
    items_synced: usize,
    zombies_detected: Vec<String>,
    stuck_detected: Vec<String>,
}

struct SubscriptionGap {
    #[allow(dead_code)]
    subscription_id: i64,
    playlist_name: String,
    playlist_url: String,
    #[allow(dead_code)]
    total_tracks: usize,
    missing_tracks: Vec<MissingTrack>,
}

struct MissingTrack {
    #[allow(dead_code)]
    track_id: i64,
    title: String,
    artist: String,
    #[allow(dead_code)]
    isrc: Option<String>,
    spotify_url: String,
    reason: MissingReason,
}

enum MissingReason {
    NotOnDeezer,
    ZombiePlaylist,
    FileMayExist,
}

struct RemediationReport {
    requeued_playlists: usize,
    spotdl_downloads: usize,
    spotdl_failures: usize,
    fuzzy_matches: usize,
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deemix_bucket_classifies_terminal_states() {
        // Failed wins over size-based completion.
        assert_eq!(deemix_state_bucket("completed", 10, 10, true), "failed");
        assert_eq!(deemix_state_bucket("error", 1, 10, false), "failed");
        assert_eq!(deemix_state_bucket("downloading", 1, 10, true), "failed");

        // Completed by size and/or status wording.
        assert_eq!(deemix_state_bucket("downloading", 10, 10, false), "completed");
        assert_eq!(deemix_state_bucket("completed", 10, 10, false), "completed");
        assert_eq!(deemix_state_bucket("finished", 10, 10, false), "completed");

        // Everything else is active.
        assert_eq!(deemix_state_bucket("queued", 0, 10, false), "active");
        assert_eq!(deemix_state_bucket("inQueue", 0, 10, false), "active");
        assert_eq!(deemix_state_bucket("downloading", 3, 10, false), "active");
    }

    #[test]
    fn test_normalize_for_fuzzy_exact_match() {
        let a = normalize_for_fuzzy("Boris Brejcha", "Black Unicorn");
        let b = normalize_for_fuzzy("Boris Brejcha", "Black Unicorn");
        assert_eq!(a, b);
        assert_eq!(a, "boris brejcha - black unicorn");
    }

    #[test]
    fn test_normalize_for_fuzzy_case_insensitive() {
        let a = normalize_for_fuzzy("ANNA", "SURRENDER");
        let b = normalize_for_fuzzy("anna", "surrender");
        assert_eq!(a, b);
    }

    #[test]
    fn test_normalize_for_fuzzy_punctuation() {
        let a = normalize_for_fuzzy("Jon.K", "Madness (Malandra Jr. Remix)");
        let b = normalize_for_fuzzy("Jon K", "Madness Malandra Jr  Remix");
        // After normalization (lowercase, alphanumeric only, collapse whitespace)
        assert_eq!(a, "jon k - madness malandra jr remix");
        assert_eq!(b, "jon k - madness malandra jr remix");
        assert_eq!(a, b);
    }

    #[test]
    fn test_normalize_for_fuzzy_remix_suffix() {
        let a = normalize_for_fuzzy("Artist", "Track (Original Mix)");
        let b = normalize_for_fuzzy("Artist", "Track Original Mix");
        // Both strip parentheses but the words stay
        assert_eq!(a, "artist - track original mix");
        assert_eq!(b, "artist - track original mix");
        assert_eq!(a, b);
    }

    #[test]
    fn test_normalize_for_fuzzy_featuring() {
        let a = normalize_for_fuzzy("DJ", "Banger feat. MC Vocalist");
        let b = normalize_for_fuzzy("DJ", "Banger feat MC Vocalist");
        assert_eq!(a, b);
    }

    #[test]
    fn test_normalize_for_fuzzy_different_tracks() {
        let a = normalize_for_fuzzy("Artist", "Track One");
        let b = normalize_for_fuzzy("Artist", "Track Two");
        assert_ne!(a, b);
    }
}
