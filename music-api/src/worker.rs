//! The pipeline worker. Flat by design: one loop, two phases per tick —
//! resolve+enqueue the pending ISRCs, then advance the in-flight downloads.
//!
//! No external queue: the SQLite `tracks.state` column *is* the queue.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::models::{state, ArtistOrder, MetadataItem, Track};
use crate::{analyzer, db, deemix, deezer, tags, transcode, vault, AppState};

const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "opus", "ogg"];

/// How many ready-but-unanalysed tracks to consider per tick (bounded fan-out).
const ANALYSIS_BATCH: i64 = 25;

/// How many artist orders to submit/poll per tick (bounded fan-out).
const ARTIST_ORDER_BATCH: i64 = 10;

/// Upper bound on files scanned per artist-order ingest (bounded work).
const ARTIST_INGEST_MAX_FILES: usize = 2000;

/// A `running` artist order whose deemix item vanished but produced no files is
/// failed after this long (bounded lifetime).
const ARTIST_ORDER_STALE_SECS: i64 = 1800;

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
    // Artist orders (whole-discography downloads) run alongside the ISRC queue.
    advance_artist_orders(state).await?;
    // Analyse freshly-ready FLACs last, so a `ready` row written this tick is
    // picked up here (or on the next) without blocking downloads.
    analyze_ready(state).await?;
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

/// Ensure a deemix session, honouring the login backoff. Returns whether deemix
/// is ready to accept `addToQueue`. A missing ARL warns exactly once; a failed
/// login is backed off (60s → 1800s) so it is not retried every tick.
pub async fn ensure_deemix_login(state: &Arc<AppState>) -> bool {
    let now = db::now();
    if state.config.deemix_arl.is_empty() {
        if state.deemix_login.warn_missing_arl() {
            warn!("DEEMIX_ARL is not set — tracks stay pending until it is configured");
        }
        return false;
    }
    if state.deemix_login.blocked(now) {
        return false;
    }
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
    let deemix_ready = ensure_deemix_login(state).await;

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
                artist_id,
                album_id,
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
                // Vault load: hubs, links, extids and the metadata satellite.
                // Best-effort — a vault hiccup must not stall the download.
                if let Ok(mut conn) = state.pool.acquire().await {
                    if let Err(e) = vault::ingest_deezer(
                        &mut conn,
                        &track.isrc,
                        &deezer_id,
                        &title,
                        &artist,
                        &album,
                        artist_id.as_deref(),
                        album_id.as_deref(),
                    )
                    .await
                    {
                        warn!("{}: vault ingest failed: {e:#}", track.isrc);
                    }
                }
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
    vault_download(state, &track.isrc, "ready", Some("mp3-320"), "320,128").await;
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
        vault_download(state, &track.isrc, "ready", Some("flac"), "flac,320,128").await;
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
            vault_download(state, &track.isrc, "ready", Some("mp3-320"), "320,128").await;
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
            vault_download(state, &track.isrc, "ready", Some("mp3-128"), "128").await;
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

/// Best-effort vault snapshot of the operational download state. A vault hiccup
/// must never fail the download itself.
async fn vault_download(
    state: &Arc<AppState>,
    isrc: &str,
    state_str: &str,
    source_format: Option<&str>,
    formats: &str,
) {
    if let Ok(mut conn) = state.pool.acquire().await {
        if let Err(e) = vault::load_download(
            &mut conn,
            isrc,
            state_str,
            source_format,
            formats,
            "music-api-worker",
        )
        .await
        {
            warn!("{isrc}: vault download load failed: {e:#}");
        }
    }
}

// ── Artist orders (whole-discography download) ───────────────────────────────

/// Submit an artist order to deemix (login already ensured by the caller).
/// Returns the resulting status: `running` on success, `failed` on rejection,
/// `open` when deemix is not ready (the worker retries).
pub async fn submit_artist_order(
    state: &Arc<AppState>,
    order_id: &str,
    artist_url: &str,
    deemix_ready: bool,
) -> String {
    if !deemix_ready {
        return "open".to_string();
    }
    match deemix::add_to_queue(
        &state.http,
        &state.config.deemix_url,
        artist_url,
        state.config.deemix_bitrate,
    )
    .await
    {
        Ok(deemix::AddOutcome::Queued { uuid }) => {
            let uuid = uuid.unwrap_or_else(|| format!("artist_{order_id}"));
            let _ = db::mark_artist_order_running(&state.pool, order_id, &uuid).await;
            state
                .logs
                .push("info", format!("artist order {order_id}: queued at deemix"));
            "running".to_string()
        }
        Ok(deemix::AddOutcome::Rejected { errid }) => {
            let errid = errid.unwrap_or_else(|| "unknown".to_string());
            let msg = format!("deemix rejected: {errid}");
            let _ = db::mark_artist_order_failed(&state.pool, order_id, &msg).await;
            state
                .logs
                .push("warn", format!("artist order {order_id}: {msg}"));
            "failed".to_string()
        }
        Err(e) => {
            warn!("artist order {order_id}: deemix addToQueue failed: {e:#}");
            "open".to_string()
        }
    }
}

/// Phase D: submit `open` artist orders to deemix and ingest finished ones.
async fn advance_artist_orders(state: &Arc<AppState>) -> Result<()> {
    // Submit open orders (bounded). Reuses the deemix login backoff.
    let open = db::open_artist_orders(&state.pool, ARTIST_ORDER_BATCH).await?;
    if !open.is_empty() {
        let deemix_ready = ensure_deemix_login(state).await;
        if deemix_ready {
            for order in open {
                submit_artist_order(state, &order.id, &order.artist_url, true).await;
            }
        }
    }

    // Poll running orders against the deemix queue.
    let running = db::running_artist_orders(&state.pool, ARTIST_ORDER_BATCH).await?;
    if running.is_empty() {
        return Ok(());
    }
    let queue = match deemix::queue(&state.http, &state.config.deemix_url).await {
        Ok(q) => q,
        Err(e) => {
            warn!("deemix getQueue failed: {e:#}");
            return Ok(());
        }
    };
    for order in running {
        let item = order.deemix_uuid.as_deref().and_then(|u| queue.get(u));
        match item {
            Some(item) if deemix::is_terminal_status(&item.status) => {
                if deemix::is_success_status(&item.status) {
                    // Terminal success: ingest the produced files (off the tick).
                    spawn_artist_ingest(state.clone(), order, true);
                } else {
                    db::mark_artist_order_failed(
                        &state.pool,
                        &order.id,
                        &format!("deemix status: {}", item.status),
                    )
                    .await?;
                }
            }
            Some(_) => {} // still downloading
            None => {
                // The item left the queue: it may have finished and been evicted.
                // Try to ingest; if nothing is there yet, retry until stale.
                if db::now() - order.updated_at > ARTIST_ORDER_STALE_SECS {
                    db::mark_artist_order_failed(&state.pool, &order.id, "no files found").await?;
                } else {
                    spawn_artist_ingest(state.clone(), order, false);
                }
            }
        }
    }
    Ok(())
}

/// Run the artist-order ingest on a bounded background task so the tick loop
/// stays responsive. A per-order guard prevents duplicate work while a job is
/// live. `terminal` marks the order `done` even when nothing new was ingested
/// (deemix reported success); otherwise the order stays `running` until files
/// appear or the stale check fails it.
fn spawn_artist_ingest(state: Arc<AppState>, order: ArtistOrder, terminal: bool) {
    {
        let mut live = state
            .artist_ingesting
            .lock()
            .expect("artist_ingesting mutex");
        if !live.insert(order.id.clone()) {
            return; // already ingesting this order
        }
    }
    tokio::spawn(async move {
        let _permit = state.finalize_sem.acquire().await.ok();
        match ingest_artist_downloads(&state, &order).await {
            Ok(added) if added > 0 || terminal => {
                let _ = db::mark_artist_order_done(&state.pool, &order.id, added as i64).await;
                info!("artist order {}: {} file(s) ingested", order.id, added);
                state.logs.push(
                    "info",
                    format!("artist order {}: {added} file(s) ingested", order.id),
                );
            }
            Ok(_) => {
                // Nothing (yet) — leave the order running; the stale check bounds it.
                debug!("artist order {}: no files ingested yet", order.id);
            }
            Err(e) => {
                warn!("artist order {}: ingest failed: {e:#}", order.id);
                let _ =
                    db::mark_artist_order_failed(&state.pool, &order.id, &format!("ingest: {e}"))
                        .await;
            }
        }
        state
            .artist_ingesting
            .lock()
            .expect("artist_ingesting mutex")
            .remove(&order.id);
    });
}

/// Scan the deemix download dir for files produced by this artist order and load
/// them into the vault. Bounded: at most `ARTIST_INGEST_MAX_FILES` files, and
/// only files modified at/after the order was created. Returns how many tracks
/// (with an ISRC) were ingested.
async fn ingest_artist_downloads(state: &Arc<AppState>, order: &ArtistOrder) -> Result<usize> {
    let dir = &state.config.deemix_download_dir;
    if !dir.exists() {
        return Ok(0);
    }
    let mut added = 0usize;
    let mut seen = 0usize;
    for entry in WalkDir::new(dir)
        .max_depth(6)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if seen >= ARTIST_INGEST_MAX_FILES {
            warn!(
                "artist order {}: ingest capped at {} files",
                order.id, ARTIST_INGEST_MAX_FILES
            );
            break;
        }
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
        // Only files that appeared after the order was placed.
        if let Some(secs) = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
        {
            if secs < order.created_at {
                continue;
            }
        }
        seen += 1;
        match ingest_one_file(state, order, path).await {
            Ok(true) => added += 1,
            Ok(false) => {}
            Err(e) => warn!(
                "artist order {}: {} ingest failed: {e:#}",
                order.id,
                path.display()
            ),
        }
    }
    Ok(added)
}

/// Read one downloaded file's tags and load it into the vault + the operational
/// `tracks` ledger. Returns `true` when a track (with an ISRC) was ingested.
async fn ingest_one_file(state: &Arc<AppState>, order: &ArtistOrder, path: &Path) -> Result<bool> {
    let Some(tags) = tags::read_tags(&state.config.ffprobe, path).await else {
        return Ok(false);
    };
    let Some(isrc) = tags.isrc.as_deref() else {
        // No ISRC — best effort only (the vault is keyed by ISRC).
        debug!(
            "artist order {}: {} has no ISRC — skipped",
            order.id,
            path.display()
        );
        return Ok(false);
    };

    // Place the file in our store so the analysis worker can read it.
    let (source_format, path_flac, path_320, path_128) =
        store_downloaded(state, isrc, path).await?;

    // Vault load: hubs, links and the metadata satellite (insert-only).
    {
        let mut conn = state.pool.acquire().await?;
        vault::ingest_file_tags(
            &mut conn,
            isrc,
            tags.title.as_deref(),
            tags.artist.as_deref(),
            tags.album.as_deref(),
            tags.duration_ms,
            "deemix",
        )
        .await?;
    }

    // Operational ledger row: `ready` with the FLAC path, so the analysis worker
    // picks it up on the next tick.
    db::ingest_ready_track(
        &state.pool,
        isrc,
        &source_format,
        path_flac.as_deref(),
        path_320.as_deref(),
        path_128.as_deref(),
    )
    .await?;
    db::add_artist_order_item(&state.pool, &order.id, isrc).await?;
    Ok(true)
}

/// Copy a downloaded file into our store (`flac`/`320`/`128`) keyed by ISRC,
/// mirroring the track finalize path. Returns
/// `(source_format, path_flac, path_320, path_128)`.
async fn store_downloaded(
    state: &Arc<AppState>,
    isrc: &str,
    src: &Path,
) -> Result<(String, Option<String>, Option<String>, Option<String>)> {
    let cfg = &state.config;
    let is_flac = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flac"))
        .unwrap_or(false);

    if is_flac {
        let dst = cfg.flac_dir().join(format!("{isrc}.flac"));
        tokio::fs::create_dir_all(cfg.flac_dir()).await?;
        tokio::fs::copy(src, &dst).await?;
        Ok((
            "flac".to_string(),
            Some(dst.display().to_string()),
            None,
            None,
        ))
    } else {
        // Lossy: classify by the actual bit rate (the ARL tier decides).
        let probed = transcode::probe_bitrate(&cfg.ffprobe, src).await;
        let is_320 = probed.map(|b| b >= 256_000).unwrap_or(true);
        let (dir, fmt) = if is_320 {
            (cfg.mp3_320_dir(), "mp3-320")
        } else {
            (cfg.mp3_128_dir(), "mp3-128")
        };
        let dst = dir.join(format!("{isrc}.mp3"));
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::copy(src, &dst).await?;
        let p = Some(dst.display().to_string());
        if is_320 {
            Ok((fmt.to_string(), None, p, None))
        } else {
            Ok((fmt.to_string(), None, None, p))
        }
    }
}

// ── Phase C: BPM/key analysis (music-api as analysis authority) ─────────────

/// Hand ready-but-unanalysed FLACs to the analyzer. Disabled when no
/// `ANALYZER_URL` is configured. A dead analyzer is backed off (see
/// `analysis_backoff`) so it is not retried — or logged — every tick.
async fn analyze_ready(state: &Arc<AppState>) -> Result<()> {
    if !analyzer::enabled(state.config.analyzer_url.as_deref()) {
        return Ok(());
    }
    if state.analysis_backoff.blocked(db::now()) {
        return Ok(());
    }
    let candidates = db::ready_flac_unaanalyzed(&state.pool, ANALYSIS_BATCH).await?;
    for track in candidates {
        spawn_analysis(state.clone(), track);
    }
    Ok(())
}

/// Dispatch one analysis onto a bounded background task. A per-ISRC guard stops
/// duplicate work while a job is live (the tick may re-select a row before the
/// `essentia` metadata row lands).
fn spawn_analysis(state: Arc<AppState>, track: Track) {
    {
        let mut live = state.analyzing.lock().expect("analyzing mutex");
        if !live.insert(track.isrc.clone()) {
            return; // already being analysed
        }
    }
    tokio::spawn(async move {
        let _permit = state.analysis_sem.acquire().await.ok();
        match analyze_track(&state, &track).await {
            Ok(true) => state.analysis_backoff.note_success(),
            // The analyzer answered but had nothing usable; leave the row
            // unanalysed so a later tick tries again.
            Ok(false) => {}
            Err(e) => {
                warn!("{}: analysis failed: {e:#}", track.isrc);
                state
                    .logs
                    .push("warn", format!("{}: analysis failed: {e}", track.isrc));
                let delay = state.analysis_backoff.note_failure(db::now());
                debug!("analyzer backed off for {delay}s");
            }
        }
        state
            .analyzing
            .lock()
            .expect("analyzing mutex")
            .remove(&track.isrc);
    });
}

/// Analyse one track's FLAC and store the result as `source='essentia'`.
/// Returns `true` when the analyzer produced at least one field and a metadata
/// row was written. Public so tests can drive a single analysis without the
/// background/backoff machinery.
pub async fn analyze_track(state: &Arc<AppState>, track: &Track) -> Result<bool> {
    let Some(base) = state.config.analyzer_url.as_deref() else {
        return Ok(false);
    };
    let Some(path) = track.path_flac.as_deref() else {
        return Ok(false);
    };

    let analysis = analyzer::analyze(&state.http, base, path).await?;
    if analysis.is_empty() {
        warn!("{}: analyzer returned no fields for {path}", track.isrc);
        state.logs.push(
            "warn",
            format!("{}: analyzer returned no fields", track.isrc),
        );
        return Ok(false);
    }

    // The vault is the data home: write the analysis satellite (insert-only,
    // hash_diff-gated, one row per analyzer).
    {
        let mut conn = state.pool.acquire().await?;
        let track_hk = vault::load_track(&mut conn, &track.isrc, "music-api-worker").await?;
        vault::load_analysis(
            &mut conn,
            &track_hk,
            "essentia",
            "music-api-worker",
            analysis.bpm,
            analysis.key.as_deref(),
            analysis.camelot.as_deref(),
            None,
        )
        .await?;
    }

    // Compat projection: keep `track_metadata` in step so the legacy read
    // surface (`/metadata`) and the idempotency check keep working unchanged.
    // Non-null wins; a partial result never wipes a value the hub already pushed.
    let item = MetadataItem {
        isrc: track.isrc.clone(),
        bpm: analysis.bpm,
        key: analysis.key,
        camelot: analysis.camelot,
        title: None,
        artist: None,
        album: None,
        genre: None,
        source: Some("essentia".to_string()),
    };
    db::upsert_metadata(&state.pool, std::slice::from_ref(&item)).await?;
    info!(
        "{}: analysed (bpm {:?}, key {:?}, camelot {:?})",
        track.isrc, item.bpm, item.key, item.camelot
    );
    state
        .logs
        .push("info", format!("{}: analysed by essentia", track.isrc));
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
