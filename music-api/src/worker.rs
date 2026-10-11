//! The pipeline worker. Flat by design: one loop, two phases per tick —
//! resolve+enqueue the pending ISRCs, then advance the in-flight downloads.
//!
//! No external queue: the SQLite `tracks.state` column *is* the queue.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::time::SystemTime;

use anyhow::Result;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::models::{Track, state};
use crate::{AppState, db, deemix, deezer, transcode};

const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "opus", "ogg"];

const LOGIN_BACKOFF_BASE: i64 = 60;
const LOGIN_BACKOFF_MAX: i64 = 1800;

/// Backoff for deemix login attempts. A dead or missing ARL must not retry —
/// or log — every tick; the delay grows 60s → 1800s and resets on success.
#[derive(Default)]
pub struct LoginBackoff {
    next_attempt: AtomicI64,
    failures: AtomicU32,
    warned_missing_arl: AtomicBool,
}

impl LoginBackoff {
    /// True while a previous failure is still cooling down.
    pub fn blocked(&self, now: i64) -> bool {
        now < self.next_attempt.load(Ordering::SeqCst)
    }

    pub fn note_success(&self) {
        self.failures.store(0, Ordering::SeqCst);
        self.next_attempt.store(0, Ordering::SeqCst);
    }

    /// Record a failure and return the delay until the next attempt.
    pub fn note_failure(&self, now: i64) -> i64 {
        let failures = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        let shift = failures.saturating_sub(1).min(5);
        let delay = (LOGIN_BACKOFF_BASE << shift).min(LOGIN_BACKOFF_MAX);
        self.next_attempt.store(now + delay, Ordering::SeqCst);
        delay
    }

    /// True exactly once — for the "ARL missing" warning.
    pub fn warn_missing_arl(&self) -> bool {
        !self.warned_missing_arl.swap(true, Ordering::SeqCst)
    }
}

/// Run forever, waking on the configured interval or when nudged by a new order.
pub async fn run(state: Arc<AppState>) {
    loop {
        // When paused, do no work — but keep listening for a resume notify.
        if !state.paused.load(std::sync::atomic::Ordering::Relaxed) {
            if let Err(e) = tick(&state).await {
                warn!("worker tick failed: {e:#}");
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(state.config.worker_interval) => {}
            _ = state.notify.notified() => {}
        }
    }
}

async fn tick(state: &Arc<AppState>) -> Result<()> {
    // Recover rows stranded in `downloading` by a crash/restart before doing work.
    db::recover_expired_leases(&state.pool, &retry_policy(state), db::now()).await?;
    resolve_pending(state).await?;
    advance_downloads(state).await?;
    Ok(())
}

/// Retry tuning for the durable queue, from config.
fn retry_policy(state: &AppState) -> db::RetryPolicy {
    db::RetryPolicy {
        max_attempts: state.config.queue_max_attempts,
        base_secs: state.config.queue_retry_base_secs,
        max_secs: state.config.queue_retry_max_secs,
    }
}

/// Phase A: look each pending ISRC up on Deezer and submit it to deemix.
async fn resolve_pending(state: &Arc<AppState>) -> Result<()> {
    let policy = retry_policy(state);
    let pending = db::due_tracks(&state.pool, 25, db::now()).await?;
    debug!("resolve_pending: {} due", pending.len());
    if pending.is_empty() {
        return Ok(());
    }

    // deemix needs a session before it accepts `addToQueue`. A failed login is
    // backed off so it cannot retry (or log) every tick; metadata resolution
    // below does not need deemix and still runs regardless.
    let now = db::now();
    let deemix_ready = if state.config.deemix_arl.is_empty() {
        if state.deemix_login.warn_missing_arl() {
            warn!("DEEMIX_ARL is not set — tracks stay pending until it is configured");
        }
        false
    } else if state.deemix_login.blocked(now) {
        false
    } else {
        match deemix::login(
            &state.http,
            &state.config.deemix_url,
            &state.config.deemix_arl,
        )
        .await
        {
            Ok(()) => {
                state.deemix_login.note_success();
                true
            }
            Err(e) => {
                let delay = state.deemix_login.note_failure(now);
                warn!("deemix login failed ({e:#}) — next attempt in {delay}s");
                false
            }
        }
    };

    for track in pending {
        // Non-Deezer sources (e.g. SoundCloud) are dispatched to a bounded
        // background task so the tick never blocks on a download.
        if track.source() != "deezer" {
            let lease = db::now() + state.config.download_timeout.as_secs() as i64;
            let uuid = format!("ytdlp:{}", track.isrc);
            if let Err(e) = db::mark_downloading(&state.pool, &track.isrc, &uuid, lease).await {
                warn!("{}: mark downloading failed: {e:#}", track.isrc);
                continue;
            }
            spawn_non_deezer(state.clone(), track);
            continue;
        }
        // Always resolve first: metadata is useful even when we cannot download.
        match deezer::lookup_isrc(&state.http, &state.config.deezer_base, &track.isrc).await {
            Ok(deezer::Lookup::Absent { reason }) => {
                info!("{}: absent on Deezer ({reason})", track.isrc);
                state.logs.push(
                    "warn",
                    format!("{}: absent on Deezer ({reason})", track.isrc),
                );
                db::mark_terminal(&state.pool, &track.isrc, state::ABSENT, &reason).await?;
            }
            Ok(deezer::Lookup::Found {
                deezer_id,
                title,
                artist,
                album,
            }) => {
                db::mark_resolved(
                    &state.pool,
                    &track.isrc,
                    &deezer_id,
                    &title,
                    &artist,
                    &album,
                )
                .await?;
                state.logs.push(
                    "info",
                    format!("{}: resolved — {title} / {artist}", track.isrc),
                );

                if !deemix_ready {
                    continue;
                }

                let url = deezer::deemix_track_url(&deezer_id);
                match deemix::add_to_queue(
                    &state.http,
                    &state.config.deemix_url,
                    &url,
                    state.config.deemix_bitrate,
                )
                .await
                {
                    Ok(deemix::AddOutcome::Queued { uuid }) => {
                        let uuid = uuid.unwrap_or_else(|| format!("track_{deezer_id}_1"));
                        debug!("{}: queued at deemix as {uuid}", track.isrc);
                        state
                            .logs
                            .push("info", format!("{}: queued at deemix", track.isrc));
                        let lease = db::now() + state.config.download_timeout.as_secs() as i64;
                        db::mark_downloading(&state.pool, &track.isrc, &uuid, lease).await?;
                    }
                    Ok(deemix::AddOutcome::Rejected { errid }) => {
                        let errid = errid.unwrap_or_else(|| "unknown".to_string());
                        // `CantStream` means the track is not available to this
                        // account at any quality — a permanent absence.
                        let msg = format!("deemix rejected: {errid}");
                        warn!("{}: {msg}", track.isrc);
                        state.logs.push("warn", format!("{}: {msg}", track.isrc));
                        if errid == "CantStream" {
                            db::mark_terminal(
                                &state.pool,
                                &track.isrc,
                                state::ABSENT,
                                &format!("deemix: {errid} (not streamable)"),
                            )
                            .await?;
                        } else {
                            db::schedule_retry(&state.pool, &track.isrc, &msg, &policy, db::now())
                                .await?;
                        }
                    }
                    Err(e) => {
                        warn!("{}: deemix addToQueue failed: {e:#}", track.isrc);
                        db::schedule_retry(
                            &state.pool,
                            &track.isrc,
                            &format!("deemix add: {e}"),
                            &policy,
                            db::now(),
                        )
                        .await?;
                    }
                }
            }
            Err(e) => {
                warn!("{}: deezer lookup failed: {e:#}", track.isrc);
                db::schedule_retry(
                    &state.pool,
                    &track.isrc,
                    &format!("deezer lookup: {e}"),
                    &policy,
                    db::now(),
                )
                .await?;
            }
        }
    }
    Ok(())
}

/// Dispatch a non-Deezer (SoundCloud) download onto a bounded background task.
///
/// The caller has already marked the row `downloading` with a lease, so it has
/// left the due set. Concurrency is capped by a shared semaphore; the reaper
/// re-queues the row if the lease ever lapses (crash safety).
fn spawn_non_deezer(state: Arc<AppState>, track: Track) {
    tokio::spawn(async move {
        let key = track.isrc.clone();
        // Wait for a download slot (bounded parallelism).
        let permit = match state.ytdlp_sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };
        // Refresh the lease now that we actually start (we may have queued).
        let timeout = state.config.download_timeout.as_secs() as i64;
        let _ = db::renew_lease(&state.pool, &key, db::now() + timeout).await;

        let outcome = run_non_deezer(&state, &track).await;
        if let Err(e) = outcome {
            warn!("{key}: {e:#}");
            state.logs.push("warn", format!("{key}: {e}"));
            let policy = retry_policy(&state);
            let _ =
                db::schedule_retry(&state.pool, &key, &format!("{e}"), &policy, db::now()).await;
        }
        drop(permit);
    });
}

/// The actual non-Deezer acquisition, run on the background task.
async fn run_non_deezer(state: &Arc<AppState>, track: &Track) -> Result<()> {
    match track.source() {
        "soundcloud" => {
            let id = crate::models::track_external_id(&track.isrc);
            let url = crate::ytdlp::soundcloud_url(id);
            let safe = track.isrc.replace([':', '/'], "_");
            let src = crate::ytdlp::download_audio(
                &state.config.ytdlp,
                &url,
                &state.config.ytdlp_dir,
                &safe,
            )
            .await?;
            finalize_soundcloud(state, track, &src).await?;
            Ok(())
        }
        other => anyhow::bail!("unsupported source '{other}'"),
    }
}

/// Transcode a yt-dlp download into our 320/128 store and mark it ready. No
/// FLAC — the source is lossy.
async fn finalize_soundcloud(state: &Arc<AppState>, track: &Track, src: &Path) -> Result<()> {
    let cfg = &state.config;
    let base = track.isrc.replace([':', '/'], "_");
    let mp3_320 = cfg.mp3_320_dir().join(format!("{base}.mp3"));
    let mp3_128 = cfg.mp3_128_dir().join(format!("{base}.mp3"));
    tokio::fs::create_dir_all(cfg.mp3_320_dir()).await?;
    tokio::fs::create_dir_all(cfg.mp3_128_dir()).await?;
    transcode::to_mp3(&cfg.ffmpeg, src, &mp3_320, 320).await?;
    transcode::to_mp3(&cfg.ffmpeg, src, &mp3_128, 128).await?;
    db::mark_ready(
        &state.pool,
        &track.isrc,
        "mp3-320",
        None,
        Some(&mp3_320.display().to_string()),
        Some(&mp3_128.display().to_string()),
    )
    .await?;
    info!("{}: ready (soundcloud 320 + 128)", track.isrc);
    state
        .logs
        .push("info", format!("{}: ready (soundcloud)", track.isrc));
    let _ = tokio::fs::remove_file(src).await;
    Ok(())
}

/// Phase B: poll deemix for the in-flight downloads and finalise them.
async fn advance_downloads(state: &Arc<AppState>) -> Result<()> {
    let inflight = db::downloading_tracks(&state.pool).await?;
    if inflight.is_empty() {
        return Ok(());
    }

    let queue = match deemix::queue(&state.http, &state.config.deemix_url).await {
        Ok(q) => q,
        Err(e) => {
            warn!("deemix getQueue failed: {e:#}");
            return Ok(());
        }
    };

    let policy = retry_policy(state);
    let timeout = state.config.download_timeout.as_secs() as i64;
    for track in inflight {
        let item = track
            .deemix_uuid
            .as_deref()
            .and_then(|uuid| queue.get(uuid));

        match item {
            Some(item) if deemix::is_terminal_status(&item.status) => {
                if deemix::is_success_status(&item.status) {
                    // Collect + transcode off the tick loop so new orders are
                    // picked up promptly (#311).
                    spawn_finalize(state.clone(), track, true);
                } else {
                    db::schedule_retry(
                        &state.pool,
                        &track.isrc,
                        &format!("deemix status: {}", item.status),
                        &policy,
                        db::now(),
                    )
                    .await?;
                }
            }
            Some(item) => {
                // Still downloading — heartbeat the lease so the reaper leaves it
                // alone, and log progress.
                debug!(
                    "{}: downloading {} ({:.0}%)",
                    track.isrc,
                    item.title.as_deref().unwrap_or("?"),
                    item.progress.unwrap_or(0.0)
                );
                db::renew_lease(&state.pool, &track.isrc, db::now() + timeout).await?;
            }
            None => {
                // Not in the queue: it may have finished and been evicted, so try
                // to collect it. If it is genuinely gone, the reaper re-queues it
                // once the lease lapses.
                spawn_finalize(state.clone(), track, false);
            }
        }
    }
    Ok(())
}

/// Run `finalize` (file copy + transcode) on a background task so the worker's
/// tick loop stays responsive (#311). Concurrency is bounded by
/// `finalize_sem`; a per-ISRC guard prevents double work while a job is live.
///
/// `retry_if_missing` schedules a retry when deemix already reported success but
/// the file is not (yet) on disk.
fn spawn_finalize(state: Arc<AppState>, track: Track, retry_if_missing: bool) {
    {
        let mut live = state.finalizing.lock().expect("finalizing mutex");
        if !live.insert(track.isrc.clone()) {
            return; // already being finalized
        }
    }
    tokio::spawn(async move {
        let _permit = state.finalize_sem.acquire().await.ok();
        match finalize(&state, &track).await {
            Ok(true) => {}
            Ok(false) => {
                if retry_if_missing {
                    warn!(
                        "{}: deemix reported success but no file was found in {}",
                        track.isrc,
                        state.config.deemix_download_dir.display()
                    );
                    let policy = retry_policy(&state);
                    let _ = db::schedule_retry(
                        &state.pool,
                        &track.isrc,
                        "file not found after download",
                        &policy,
                        db::now(),
                    )
                    .await;
                }
            }
            Err(e) => warn!("{}: finalize failed: {e:#}", track.isrc),
        }
        state
            .finalizing
            .lock()
            .expect("finalizing mutex")
            .remove(&track.isrc);
    });
}

/// Collect a finished download into our store, transcode the lossy variants and
/// mark the track ready. Returns `false` when the file is not there yet.
async fn finalize(state: &Arc<AppState>, track: &Track) -> Result<bool> {
    let Some(artist) = track.artist.as_deref() else {
        return Ok(false);
    };
    let Some(title) = track.title.as_deref() else {
        return Ok(false);
    };

    let Some(src) = find_downloaded(&state.config.deemix_download_dir, artist, title) else {
        return Ok(false);
    };

    let cfg = &state.config;
    let flac_dst = cfg.flac_dir().join(format!("{}.flac", track.isrc));
    let mp3_320 = cfg.mp3_320_dir().join(format!("{}.mp3", track.isrc));
    let mp3_128 = cfg.mp3_128_dir().join(format!("{}.mp3", track.isrc));

    let is_flac = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flac"))
        .unwrap_or(false);

    if is_flac {
        tokio::fs::create_dir_all(cfg.flac_dir()).await?;
        tokio::fs::copy(&src, &flac_dst).await?;
        transcode::to_mp3(&cfg.ffmpeg, &flac_dst, &mp3_320, 320).await?;
        transcode::to_mp3(&cfg.ffmpeg, &flac_dst, &mp3_128, 128).await?;

        db::mark_ready(
            &state.pool,
            &track.isrc,
            "flac",
            Some(&flac_dst.display().to_string()),
            Some(&mp3_320.display().to_string()),
            Some(&mp3_128.display().to_string()),
        )
        .await?;
        info!("{}: ready (flac + 320 + 128)", track.isrc);
        state
            .logs
            .push("info", format!("{}: ready (flac)", track.isrc));
    } else {
        // deemix fell back (no lossless available). The delivered MP3 may be
        // 320 *or* 128 — the ARL's Deezer tier decides — so classify it by what
        // it actually is rather than assuming.
        let probed = transcode::probe_bitrate(&cfg.ffprobe, &src).await;
        let is_320 = probed.map(|b| b >= 256_000).unwrap_or(true);

        if is_320 {
            tokio::fs::create_dir_all(cfg.mp3_320_dir()).await?;
            tokio::fs::copy(&src, &mp3_320).await?;
            transcode::to_mp3(&cfg.ffmpeg, &mp3_320, &mp3_128, 128).await?;

            db::mark_ready(
                &state.pool,
                &track.isrc,
                "mp3-320",
                None,
                Some(&mp3_320.display().to_string()),
                Some(&mp3_128.display().to_string()),
            )
            .await?;
            info!("{}: ready (mp3 320 + 128, no FLAC available)", track.isrc);
        } else {
            tokio::fs::create_dir_all(cfg.mp3_128_dir()).await?;
            tokio::fs::copy(&src, &mp3_128).await?;

            db::mark_ready(
                &state.pool,
                &track.isrc,
                "mp3-128",
                None,
                None,
                Some(&mp3_128.display().to_string()),
            )
            .await?;
            info!(
                "{}: ready (mp3 128 only — Deezer tier/availability caps quality)",
                track.isrc
            );
        }
    }

    // The dedicated deemix download dir is ours — drop the source once copied.
    let _ = tokio::fs::remove_file(&src).await;
    Ok(true)
}

/// Locate the file deemix wrote for `artist - title`.
///
/// Exact stem match first, then a looser "contains both" match. Newest wins.
pub fn find_downloaded(dir: &Path, artist: &str, title: &str) -> Option<PathBuf> {
    if !dir.exists() {
        return None;
    }
    let target = normalize_filename(&format!("{artist} - {title}"));
    let artist_n = normalize_filename(artist);
    let title_n = normalize_filename(title);

    let mut exact: Option<(SystemTime, PathBuf)> = None;
    let mut loose: Option<(SystemTime, PathBuf)> = None;

    for entry in WalkDir::new(dir)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let stem_n = normalize_filename(stem);
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if stem_n == target {
            if exact.as_ref().is_none_or(|(t, _)| mtime > *t) {
                exact = Some((mtime, path.to_path_buf()));
            }
        } else if !artist_n.is_empty()
            && !title_n.is_empty()
            && stem_n.contains(&artist_n)
            && stem_n.contains(&title_n)
            && loose.as_ref().is_none_or(|(t, _)| mtime > *t)
        {
            loose = Some((mtime, path.to_path_buf()));
        }
    }

    exact.or(loose).map(|(_, p)| p)
}

/// Lowercase, strip everything that is not alphanumeric.
fn normalize_filename(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn login_backoff_grows_and_resets() {
        let b = LoginBackoff::default();
        assert!(!b.blocked(1000));

        assert_eq!(b.note_failure(1000), 60);
        assert!(b.blocked(1030));
        assert!(!b.blocked(1061));

        assert_eq!(b.note_failure(1000), 120);
        // Capped at 30 minutes.
        for _ in 0..10 {
            b.note_failure(1000);
        }
        assert_eq!(b.note_failure(1000), LOGIN_BACKOFF_MAX);

        b.note_success();
        assert!(!b.blocked(1000));
        assert_eq!(b.note_failure(1000), 60);
    }

    #[test]
    fn arl_warning_fires_once() {
        let b = LoginBackoff::default();
        assert!(b.warn_missing_arl());
        assert!(!b.warn_missing_arl());
    }

    #[test]
    fn normalises_filenames() {
        // Non-ASCII and punctuation are stripped, case is folded.
        assert_eq!(normalize_filename("Ünïcode - Tr!ck (Mix)"), "ncodetrckmix");
        assert_eq!(normalize_filename("A - B"), "ab");
    }

    #[test]
    fn finds_exact_match() {
        let dir = std::env::temp_dir().join(format!("music-api-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Molchat Doma - Sudno.flac"), b"x").unwrap();

        let found = find_downloaded(&dir, "Molchat Doma", "Sudno");
        assert_eq!(
            found.unwrap().file_name().unwrap(),
            "Molchat Doma - Sudno.flac"
        );

        assert!(find_downloaded(&dir, "Nobody", "Nothing").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
