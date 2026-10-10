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
        if let Err(e) = tick(&state).await {
            warn!("worker tick failed: {e:#}");
        }
        tokio::select! {
            _ = tokio::time::sleep(state.config.worker_interval) => {}
            _ = state.notify.notified() => {}
        }
    }
}

async fn tick(state: &Arc<AppState>) -> Result<()> {
    resolve_pending(state).await?;
    advance_downloads(state).await?;
    resolve_url_pending(state).await?;
    Ok(())
}

/// Phase C: resolve + download pending URL tracks (YouTube / SoundCloud).
///
/// yt-dlp is synchronous, so this phase downloads inline. It is bounded per
/// tick so a large playlist order cannot starve the ISRC pipeline.
async fn resolve_url_pending(state: &Arc<AppState>) -> Result<()> {
    let pending = db::pending_url_tracks(&state.pool, 5).await?;
    if pending.is_empty() {
        return Ok(());
    }

    let format = std::env::var("YTDLP_FORMAT").unwrap_or_else(|_| "flac".to_string());
    let out_dir = state.config.data_dir.join("ytdlp-incoming");

    for track in pending {
        // Resolve metadata first (useful even if the download fails).
        match crate::ytdlp::fetch_track(&track.url).await {
            Ok(meta) => {
                db::mark_url_resolved(
                    &state.pool,
                    &track.url,
                    &meta.provider_id,
                    meta.title.as_deref().unwrap_or(""),
                    meta.artist.as_deref().unwrap_or(""),
                    meta.duration_ms,
                )
                .await?;
            }
            Err(e) => {
                warn!("{}: yt-dlp metadata failed: {e:#}", track.url);
                db::mark_url_terminal(
                    &state.pool,
                    &track.url,
                    state::FAILED,
                    &format!("metadata: {e}"),
                )
                .await?;
                continue;
            }
        }

        db::mark_url_downloading(&state.pool, &track.url).await?;

        match crate::ytdlp::download(&track.url, &out_dir, &format).await {
            Ok(src) => {
                if let Err(e) = finalize_url(state, &track, &src).await {
                    warn!("{}: finalize failed: {e:#}", track.url);
                    db::mark_url_terminal(
                        &state.pool,
                        &track.url,
                        state::FAILED,
                        &format!("{e:#}"),
                    )
                    .await?;
                }
            }
            Err(e) => {
                warn!("{}: yt-dlp download failed: {e:#}", track.url);
                db::mark_url_terminal(
                    &state.pool,
                    &track.url,
                    state::FAILED,
                    &format!("download: {e}"),
                )
                .await?;
            }
        }
    }
    Ok(())
}

/// Move a downloaded URL track into the store and derive the lossy variants.
async fn finalize_url(
    state: &Arc<AppState>,
    track: &crate::models::UrlTrack,
    src: &Path,
) -> Result<()> {
    let cfg = &state.config;
    let stem = track.provider_id.as_deref().unwrap_or("track");
    let flac_dst = cfg.flac_dir().join(format!("{stem}.flac"));
    let mp3_320 = cfg.mp3_320_dir().join(format!("{stem}.mp3"));
    let mp3_128 = cfg.mp3_128_dir().join(format!("{stem}.mp3"));

    let is_flac = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flac"))
        .unwrap_or(false);

    if is_flac {
        tokio::fs::create_dir_all(cfg.flac_dir()).await?;
        tokio::fs::copy(src, &flac_dst).await?;
        transcode::to_mp3(&cfg.ffmpeg, &flac_dst, &mp3_320, 320).await?;
        transcode::to_mp3(&cfg.ffmpeg, &flac_dst, &mp3_128, 128).await?;
        db::mark_url_ready(
            &state.pool,
            &track.url,
            "flac",
            Some(&flac_dst.display().to_string()),
            Some(&mp3_320.display().to_string()),
            Some(&mp3_128.display().to_string()),
        )
        .await?;
        info!("{}: ready (flac + 320 + 128)", track.url);
    } else {
        let probed = transcode::probe_bitrate(&cfg.ffprobe, src).await;
        let is_320 = probed.map(|b| b >= 256_000).unwrap_or(true);
        if is_320 {
            tokio::fs::create_dir_all(cfg.mp3_320_dir()).await?;
            tokio::fs::copy(src, &mp3_320).await?;
            transcode::to_mp3(&cfg.ffmpeg, &mp3_320, &mp3_128, 128).await?;
            db::mark_url_ready(
                &state.pool,
                &track.url,
                "mp3-320",
                None,
                Some(&mp3_320.display().to_string()),
                Some(&mp3_128.display().to_string()),
            )
            .await?;
            info!("{}: ready (mp3 320 + 128)", track.url);
        } else {
            tokio::fs::create_dir_all(cfg.mp3_128_dir()).await?;
            tokio::fs::copy(src, &mp3_128).await?;
            db::mark_url_ready(
                &state.pool,
                &track.url,
                "mp3-128",
                None,
                None,
                Some(&mp3_128.display().to_string()),
            )
            .await?;
            info!("{}: ready (mp3 128)", track.url);
        }
    }

    let _ = tokio::fs::remove_file(src).await;
    Ok(())
}

/// Phase A: look each pending ISRC up on Deezer and submit it to deemix.
async fn resolve_pending(state: &Arc<AppState>) -> Result<()> {
    let pending = db::pending_tracks(&state.pool, 25).await?;
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
        // Always resolve first: metadata is useful even when we cannot download.
        match deezer::lookup_isrc(&state.http, &state.config.deezer_base, &track.isrc).await {
            Ok(deezer::Lookup::Absent {
                reason,
                title,
                artist,
            }) => {
                // Fallback: Deezer knows the track but cannot stream it — try
                // yt-dlp (YouTube search) before declaring it absent.
                if let (Some(t), Some(a)) = (title.as_deref(), artist.as_deref()) {
                    if !t.is_empty() && !a.is_empty() && try_fallback(state, &track, a, t).await? {
                        continue;
                    }
                }
                info!("{}: absent on Deezer ({reason})", track.isrc);
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
                        db::mark_downloading(&state.pool, &track.isrc, &uuid).await?;
                    }
                    Ok(deemix::AddOutcome::Rejected { errid }) => {
                        let errid = errid.unwrap_or_else(|| "unknown".to_string());
                        // `CantStream` means the track is not available to this
                        // account at any quality — a permanent absence.
                        let (st, msg) = if errid == "CantStream" {
                            (state::ABSENT, format!("deemix: {errid} (not streamable)"))
                        } else {
                            (state::FAILED, format!("deemix rejected: {errid}"))
                        };
                        warn!("{}: {msg}", track.isrc);
                        db::mark_terminal(&state.pool, &track.isrc, st, &msg).await?;
                    }
                    Err(e) => warn!("{}: deemix addToQueue failed: {e:#}", track.isrc),
                }
            }
            Err(e) => warn!("{}: deezer lookup failed: {e:#}", track.isrc),
        }
    }
    Ok(())
}

/// Fallback for a Deezer-absent track: search YouTube via yt-dlp, download the
/// best match, and run it through the normal finalize path. Returns `true` when
/// the track ended up `ready`.
async fn try_fallback(
    state: &Arc<AppState>,
    track: &Track,
    artist: &str,
    title: &str,
) -> Result<bool> {
    let format = std::env::var("YTDLP_FORMAT").unwrap_or_else(|_| "flac".to_string());
    let out_dir = state.config.data_dir.join("ytdlp-incoming");
    let query = format!("{artist} - {title}");

    let src = match crate::ytdlp::search(&query, &out_dir, &format).await {
        Ok(p) => p,
        Err(e) => {
            warn!("{}: yt-dlp fallback failed: {e:#}", track.isrc);
            return Ok(false);
        }
    };

    // Copy into the deemix download dir under the name `finalize` expects, so we
    // reuse the store/transcode path unchanged.
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("flac");
    let dst = state
        .config
        .deemix_download_dir
        .join(format!("{artist} - {title}.{ext}"));
    tokio::fs::create_dir_all(&state.config.deemix_download_dir).await?;
    tokio::fs::copy(&src, &dst).await?;
    let _ = tokio::fs::remove_file(&src).await;

    // Persist the metadata so `finalize` can locate the file, then finalize.
    db::mark_resolved(&state.pool, &track.isrc, "", title, artist, "").await?;
    let resolved = db::get_track(&state.pool, &track.isrc)
        .await?
        .unwrap_or_else(|| track.clone());
    if finalize(state, &resolved).await? {
        info!("{}: ready via yt-dlp fallback", track.isrc);
        Ok(true)
    } else {
        Ok(false)
    }
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

    let timeout = state.config.download_timeout.as_secs() as i64;
    for track in inflight {
        let item = track
            .deemix_uuid
            .as_deref()
            .and_then(|uuid| queue.get(uuid));

        match item {
            Some(item) if deemix::is_terminal_status(&item.status) => {
                if deemix::is_success_status(&item.status) {
                    if !finalize(state, &track).await? {
                        warn!(
                            "{}: deemix reported '{}' but no file was found in {}",
                            track.isrc,
                            item.status,
                            state.config.deemix_download_dir.display()
                        );
                        db::mark_terminal(
                            &state.pool,
                            &track.isrc,
                            state::FAILED,
                            "file not found after download",
                        )
                        .await?;
                    }
                } else {
                    db::mark_terminal(
                        &state.pool,
                        &track.isrc,
                        state::FAILED,
                        &format!("deemix status: {}", item.status),
                    )
                    .await?;
                }
            }
            Some(item) => {
                // Still downloading — only give up after the timeout.
                debug!(
                    "{}: downloading {} ({:.0}%)",
                    track.isrc,
                    item.title.as_deref().unwrap_or("?"),
                    item.progress.unwrap_or(0.0)
                );
                if db::now() - track.updated_at > timeout {
                    db::mark_terminal(&state.pool, &track.isrc, state::FAILED, "download timeout")
                        .await?;
                }
            }
            None => {
                // Not in the queue: it may have finished and been evicted, so
                // try to collect it before declaring a timeout.
                if !finalize(state, &track).await? && db::now() - track.updated_at > timeout {
                    db::mark_terminal(
                        &state.pool,
                        &track.isrc,
                        state::FAILED,
                        "disappeared from deemix queue",
                    )
                    .await?;
                }
            }
        }
    }
    Ok(())
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
