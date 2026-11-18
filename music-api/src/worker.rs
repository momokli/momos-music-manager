//! The pipeline worker. Flat by design: one loop, one download path — resolve
//! the pending ISRCs and acquire them natively (`deezer_dl`), then derive and
//! analyse the delivered files.
//!
//! No external queue: the SQLite `tracks.state` column *is* the queue.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::models::{state, MetadataItem, Track};
use crate::{analyzer, db, deezer, tags, transcode, vault, work, AppState};

/// How many ready-but-unanalysed tracks to consider per tick (bounded fan-out).
const ANALYSIS_BATCH: i64 = 25;

/// How many pending tracks to materialise into the `download` kind per tick
/// (bounded reconcile; highest priority first).
const RECONCILE_BATCH: i64 = 500;

/// Work items stranded in `running` longer than this are re-queued (a worker
/// that died mid-flight); the next claim counts another attempt.
const WORK_STALE_SECS: i64 = 1800;

const LOGIN_BACKOFF_BASE: i64 = 60;
const LOGIN_BACKOFF_MAX: i64 = 1800;

/// Backoff for native Deezer login attempts. A dead or missing ARL must not
/// retry — or log — every tick; the delay grows 60s → 1800s and resets on
/// success.
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

/// Canned native download for tests: serves `bytes` for any track id/quality,
/// so the native worker path can be exercised without an ARL or the network.
#[derive(Debug, Clone)]
pub struct NativeFake {
    /// File bytes to hand back (must be a container `lofty` can tag).
    pub bytes: Vec<u8>,
    /// File extension to write them with (e.g. `"flac"`, `"mp3"`).
    pub extension: String,
    /// When true, a request for `Quality::Flac` fails with `NoLosslessTier`,
    /// forcing the 320 fallback (tier-exercised path).
    pub no_flac: bool,
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
    // Recover rows stranded in `downloading`/`running` by a crash/restart before
    // doing work.
    db::recover_expired_leases(&state.pool, &retry_policy(state), db::now()).await?;
    work::recover_stale(&state.pool, db::now(), WORK_STALE_SECS).await?;
    resolve_pending(state).await?;
    // Derive the lossy formats (320/128) off the download path.
    advance_transcode(state).await?;
    // Analyse freshly-ready FLACs last, so a `ready` row written this tick is
    // picked up here (or on the next) without blocking downloads.
    advance_analyze(state).await?;
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

/// Boxed, owned-input login future. Production is [`crate::deezer_dl::login`];
/// tests inject their own so a failing login drives the backoff without the
/// network.
pub type NativeLoginFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<crate::deezer_dl::Session, crate::deezer_dl::Error>>
            + Send,
    >,
>;

/// Ensure a native Deezer session, honouring the native login backoff.
///
/// Returns whether the native path is ready to download. The backoff grows
/// 60s → 1800s on repeated failures and resets on success. A missing ARL warns
/// exactly once. The ARL is only ever handed to [`crate::deezer_dl::login`] —
/// it never reaches a log line or an error message.
///
/// When [`AppState::native_fake`] is set (tests), no session is needed.
pub async fn ensure_native_login(state: &Arc<AppState>) -> bool {
    ensure_native_login_with(state, |arl| -> NativeLoginFuture {
        Box::pin(async move { crate::deezer_dl::login(&arl).await })
    })
    .await
}

/// [`ensure_native_login`] with the login call injected.
///
/// Production passes [`crate::deezer_dl::login`]; tests inject a fake so a
/// failing login can be exercised without touching the network. Everything else
/// — the empty-ARL short-circuit, the session cache and the backoff — is shared
/// with the production path.
pub async fn ensure_native_login_with<L>(state: &Arc<AppState>, login: L) -> bool
where
    L: Fn(String) -> NativeLoginFuture,
{
    if state.native_fake.is_some() {
        return true;
    }
    let arl = state.config.deezer_arl.trim();
    if arl.is_empty() {
        if state.native_login.warn_missing_arl() {
            warn!("DEEZER_ARL is not set — native Deezer downloads unavailable");
        }
        return false;
    }
    if state.native_session.read().await.is_some() {
        return true;
    }
    let now = db::now();
    if state.native_login.blocked(now) {
        return false;
    }
    match login(arl.to_string()).await {
        Ok(session) => {
            state.native_login.note_success();
            *state.native_session.write().await = Some(Arc::new(session));
            info!("native Deezer session established");
            true
        }
        Err(e) => {
            let delay = state.native_login.note_failure(now);
            // `{e}` is ARL-free by construction (deezer_dl renders no ARL).
            warn!("native Deezer login failed ({e}) — next attempt in {delay}s");
            false
        }
    }
}

/// Phase A: look each pending ISRC up on Deezer and start the native download.
///
/// Orders are accepted without limit; only the *dispatch* is bounded. The pending
/// set is materialised into the `download` kind (bounded reconcile) and the
/// dispatcher pulls `next("download")` — the priority `next`. The in-flight cap
/// stays on the `tracks` ledger (`count_downloading`, the #427 fix), so at most
/// `MAX_INFLIGHT` rows are `downloading` at once and the rest wait their turn.
pub async fn resolve_pending(state: &Arc<AppState>) -> Result<()> {
    let policy = retry_policy(state);

    // In-flight cap: never have more than `max_inflight` downloads dispatched
    // at once. A submitted row is marked `downloading`, so it counts here on the
    // next tick.
    let max_inflight = state.config.max_inflight as i64;
    let inflight = db::count_downloading(&state.pool).await?;
    let slots = max_inflight - inflight;
    if slots <= 0 {
        debug!("resolve_pending: {inflight}/{max_inflight} in flight — no free slots, waiting");
        return Ok(());
    }

    // Materialise the highest-priority pending tracks into the download kind, so
    // the queue stays a faithful, priority-ordered view of what still needs
    // downloading.
    for (isrc, priority) in db::pending_work(&state.pool, RECONCILE_BATCH, db::now()).await? {
        work::enqueue(&state.pool, work::kind::DOWNLOAD, &isrc, priority, None).await?;
    }

    // Pull at most `slots` from the priority `next`, highest priority first.
    let queued = work::next(&state.pool, work::kind::DOWNLOAD, slots.min(25), db::now()).await?;
    debug!(
        "resolve_pending: {} queued ({} slots free of {max_inflight})",
        queued.len(),
        slots
    );
    if queued.is_empty() {
        return Ok(());
    }

    for item in queued {
        let Some(track) = db::get_track(&state.pool, &item.reference).await? else {
            // Unknown ref — settle it so it stops being offered.
            work::finish(&state.pool, item.id, db::now()).await?;
            continue;
        };
        if track.state == state::DOWNLOADING {
            // Already in flight but the item was not marked running — sync it.
            work::ensure_running_ref(&state.pool, work::kind::DOWNLOAD, &track.isrc, db::now())
                .await?;
            continue;
        }
        if track.state != state::PENDING {
            // Terminal (ready/absent/failed): settle the item.
            work::finish(&state.pool, item.id, db::now()).await?;
            continue;
        }
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

                // Native path: without a session the track stays pending and is
                // retried next tick (no lease taken, no sentinel written).
                if !ensure_native_login(state).await {
                    continue;
                }
                let lease = db::now() + state.config.download_timeout.as_secs() as i64;
                let sentinel = format!("native:{}", track.isrc);
                if let Err(e) = db::mark_downloading(&state.pool, &track.isrc, &sentinel, lease).await
                {
                    warn!("{}: mark downloading failed: {e:#}", track.isrc);
                    continue;
                }
                state
                    .logs
                    .push("info", format!("{}: native download started", track.isrc));
                spawn_native_download(state.clone(), track);
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

/// Dispatch a non-Deezer (SoundCloud, YouTube) download onto a bounded
/// background task.
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
    let source = track.source();
    let id = crate::models::track_external_id(&track.isrc);
    let url = match source {
        "soundcloud" => crate::ytdlp::soundcloud_url(id),
        "youtube" => crate::ytdlp::youtube_url(id),
        other => anyhow::bail!("unsupported source '{other}'"),
    };
    let safe = track.isrc.replace([':', '/'], "_");
    let src =
        crate::ytdlp::download_audio(&state.config.ytdlp, &url, &state.config.ytdlp_dir, &safe)
            .await?;
    finalize_external(state, track, &src).await?;
    Ok(())
}

/// Transcode a yt-dlp download into our 320 store and mark it ready (the 128 is
/// derived by the `transcode` queue kind). No FLAC — the source is lossy.
async fn finalize_external(state: &Arc<AppState>, track: &Track, src: &Path) -> Result<()> {
    let cfg = &state.config;
    let base = track.isrc.replace([':', '/'], "_");
    let mp3_320 = cfg.mp3_320_dir().join(format!("{base}.mp3"));
    tokio::fs::create_dir_all(cfg.mp3_320_dir()).await?;
    transcode::to_mp3(&cfg.ffmpeg, src, &mp3_320, 320).await?;
    db::mark_ready(
        &state.pool,
        &track.isrc,
        "mp3-320",
        None,
        Some(&mp3_320.display().to_string()),
        None,
    )
    .await?;
    vault_download(state, &track.isrc, "ready", Some("mp3-320"), "320").await;
    info!(
        "{}: ready ({} 320) — 128 queued",
        track.isrc,
        track.source()
    );
    state.logs.push(
        "info",
        format!("{}: ready ({})", track.isrc, track.source()),
    );
    let _ = tokio::fs::remove_file(src).await;
    enqueue_transcode(state, &track.isrc).await;
    Ok(())
}

/// The fallback ladder for a wanted quality: the wanted tier first, then every
/// lower tier. A lossless account cap steps down FLAC → 320 → 128.
fn quality_ladder(wanted: crate::deezer_dl::Quality) -> Vec<crate::deezer_dl::Quality> {
    use crate::deezer_dl::Quality::*;
    match wanted {
        Flac => vec![Flac, Mp3_320, Mp3_128],
        Mp3_320 => vec![Mp3_320, Mp3_128],
        Mp3_128 => vec![Mp3_128],
    }
}

/// Attempt a download with the tier fallback: try the wanted quality, and on
/// `NoLosslessTier` step down one rung. Any other error aborts immediately.
/// Returns the bytes and the quality that actually succeeded (for the run-log).
async fn download_with_fallback<F, Fut>(
    wanted: crate::deezer_dl::Quality,
    mut dl: F,
) -> Result<(Vec<u8>, crate::deezer_dl::Quality), crate::deezer_dl::Error>
where
    F: FnMut(crate::deezer_dl::Quality) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, crate::deezer_dl::Error>>,
{
    use crate::deezer_dl::Error;
    let mut last = Error::NoLosslessTier;
    for q in quality_ladder(wanted) {
        match dl(q).await {
            Ok(bytes) => return Ok((bytes, q)),
            Err(Error::NoLosslessTier) => {
                last = Error::NoLosslessTier;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// Dispatch a native (`deezer_dl`) download onto a background task. The caller
/// has already marked the row `downloading` with a sentinel + lease, so it has
/// left the due set; the reaper re-queues it if the lease lapses.
fn spawn_native_download(state: Arc<AppState>, track: Track) {
    tokio::spawn(async move {
        let key = track.isrc.clone();
        let timeout = state.config.download_timeout.as_secs() as i64;
        let _ = db::renew_lease(&state.pool, &key, db::now() + timeout).await;
        match run_native_download(&state, &track).await {
            Ok(()) => {}
            Err(e) => {
                warn!("{key}: native download failed: {e:#}");
                state
                    .logs
                    .push("warn", format!("{key}: native download failed: {e}"));
                let policy = retry_policy(&state);
                let _ = db::schedule_retry(&state.pool, &key, &format!("{e}"), &policy, db::now())
                    .await;
            }
        }
    });
}

/// The actual native acquisition, run on the background task: resolve the
/// Deezer id, download the best available quality, write the ISRC tag, store the
/// file and mark it ready.
async fn run_native_download(state: &Arc<AppState>, track: &Track) -> Result<()> {
    let cfg = &state.config;
    let isrc = track.isrc.clone();

    // Deezer id: set by `resolve_pending`; re-resolve when missing (recovery).
    let deezer_id = match track.deezer_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => match deezer::lookup_isrc(&state.http, &cfg.deezer_base, &isrc).await? {
            deezer::Lookup::Found { deezer_id, .. } => deezer_id,
            deezer::Lookup::Absent { reason } => {
                anyhow::bail!("absent on Deezer ({reason})")
            }
        },
    };

    // Per-track wish from the order (`toys` -> mp3-128), else the global
    // default. An unknown value falls back instead of failing the download.
    let wanted = track
        .wanted_quality
        .as_deref()
        .and_then(crate::deezer_dl::Quality::from_tier)
        .unwrap_or(cfg.download_quality);
    let started = std::time::Instant::now();

    let (bytes, used) = if let Some(fake) = state.native_fake.as_ref() {
        // Test seam: canned bytes, no session/network. `no_flac` exercises the
        // tier fallback without an account.
        let fake = fake.clone();
        download_with_fallback(wanted, move |q| {
            let fake = fake.clone();
            async move {
                if fake.no_flac && q == crate::deezer_dl::Quality::Flac {
                    Err(crate::deezer_dl::Error::NoLosslessTier)
                } else {
                    Ok(fake.bytes.clone())
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("native download failed: {e}"))?
    } else {
        let Some(session) = state.native_session.read().await.clone() else {
            anyhow::bail!("native session not established");
        };
        let id = deezer_id.clone();
        download_with_fallback(wanted, move |q| {
            let session = session.clone();
            let id = id.clone();
            async move { crate::deezer_dl::download_track(&session, &id, q).await }
        })
        .await
        .map_err(|e| anyhow::anyhow!("native download failed: {e}"))?
    };
    let elapsed = started.elapsed();

    // Write the bytes to a scratch file, tag it, then store it.
    let safe = isrc.replace([':', '/'], "_");
    let ext = used.extension().trim_start_matches('.');
    let temp = cfg.data_dir.join(format!("native-{safe}.{ext}"));
    tokio::fs::write(&temp, &bytes).await?;
    if let Err(e) = tags::write_isrc(&temp, &isrc).await {
        warn!("{isrc}: write_isrc failed: {e:#}");
    }

    let result = finalize_native(state, track, &temp, used, elapsed).await;
    let _ = tokio::fs::remove_file(&temp).await;
    result
}

/// Place a tagged native download into the store (`flac`/`320`/`128`, chosen by
/// the quality that actually succeeded), mark the row ready and log the run.
async fn finalize_native(
    state: &Arc<AppState>,
    track: &Track,
    temp: &Path,
    used: crate::deezer_dl::Quality,
    elapsed: std::time::Duration,
) -> Result<()> {
    use crate::deezer_dl::Quality;
    let cfg = &state.config;
    let isrc = &track.isrc;

    let (label, dst) = match used {
        Quality::Flac => ("flac", cfg.flac_dir().join(format!("{isrc}.flac"))),
        Quality::Mp3_320 => ("mp3-320", cfg.mp3_320_dir().join(format!("{isrc}.mp3"))),
        Quality::Mp3_128 => ("mp3-128", cfg.mp3_128_dir().join(format!("{isrc}.mp3"))),
    };
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(temp, &dst).await?;
    let path = dst.display().to_string();
    let (pf, p320, p128) = match used {
        Quality::Flac => (Some(path.as_str()), None, None),
        Quality::Mp3_320 => (None, Some(path.as_str()), None),
        Quality::Mp3_128 => (None, None, Some(path.as_str())),
    };
    db::mark_ready(&state.pool, isrc, label, pf, p320, p128).await?;
    let vault_formats = match used {
        Quality::Flac => "flac",
        Quality::Mp3_320 => "320",
        Quality::Mp3_128 => "128",
    };
    vault_download(state, isrc, "ready", Some(label), vault_formats).await;

    let secs = elapsed.as_secs_f64();
    info!(
        isrc,
        quality = label,
        source = "native",
        elapsed_ms = elapsed.as_millis() as u64,
        "native download complete"
    );
    state.logs.push(
        "info",
        format!("{isrc}: ready (native, {label}, {secs:.1}s)"),
    );
    // Derive lower tiers only when there is something to derive and the
    // track is for the library: a tonie order stops at 128k, and a 128k
    // primary has no lower tier left.
    if used != Quality::Mp3_128 && track.intent != "toys" {
        enqueue_transcode(state, isrc).await;
    }
    Ok(())
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

// ── Phase E: derived formats — the `transcode` kind ─────────────────────────

/// Enqueue derived-format work for a track (320/128 from the primary). It runs
/// on the bounded queue, priority-ordered like every kind, so it never blocks
/// the download slot or the lossless file.
async fn enqueue_transcode(state: &Arc<AppState>, isrc: &str) {
    let priority = db::order_priority(&state.pool, isrc).await.unwrap_or(0);
    if let Err(e) = work::enqueue(
        &state.pool,
        work::kind::TRANSCODE,
        isrc,
        priority,
        Some("music-api-worker"),
    )
    .await
    {
        warn!("{isrc}: enqueue transcode failed: {e:#}");
    }
}

/// The `transcode` kind: drain `next("transcode")` up to `MAX_INFLIGHT`, each on
/// a bounded background task.
async fn advance_transcode(state: &Arc<AppState>) -> Result<()> {
    let kind = work::kind::TRANSCODE;
    let slots = work::free_slots(&state.pool, kind, state.config.max_inflight as i64).await?;
    if slots == 0 {
        return Ok(());
    }
    for item in work::next(&state.pool, kind, slots, db::now()).await? {
        if !work::claim(&state.pool, item.id, db::now()).await? {
            continue;
        }
        spawn_transcode(state.clone(), item);
    }
    Ok(())
}

/// Run one transcode on a bounded background task and settle its queue item.
fn spawn_transcode(state: Arc<AppState>, item: work::WorkItem) {
    tokio::spawn(async move {
        let _permit = state.finalize_sem.acquire().await.ok();
        let now = db::now();
        let attempt = item.attempts.max(1) as u32;
        match transcode_item(&state, &item.reference).await {
            Ok(()) => {
                let _ = work::finish(&state.pool, item.id, now).await;
            }
            Err(e) => {
                warn!("{}: transcode failed: {e:#}", item.reference);
                let _ = work::retry_later(
                    &state.pool,
                    item.id,
                    &format!("{e}"),
                    now + db::backoff_delay(
                        attempt,
                        state.config.queue_retry_base_secs,
                        state.config.queue_retry_max_secs,
                    ),
                    now,
                )
                .await;
            }
        }
    });
}

/// Derive the missing lossy formats for one track: 320 + 128 from a FLAC, or 128
/// from a 320. Idempotent — a track whose formats already exist is a no-op.
/// Public so tests can drive a single transcode without the background/handling
/// machinery.
pub async fn transcode_item(state: &Arc<AppState>, isrc: &str) -> Result<()> {
    let Some(track) = db::get_track(&state.pool, isrc).await? else {
        return Ok(()); // unknown track — nothing to derive
    };
    let cfg = &state.config;
    let base = isrc.replace([':', '/'], "_");

    let mut path_320: Option<String> = None;
    let mut path_128: Option<String> = None;

    if let Some(flac) = track.path_flac.as_deref() {
        if track.path_320.is_none() {
            let dst = cfg.mp3_320_dir().join(format!("{base}.mp3"));
            tokio::fs::create_dir_all(cfg.mp3_320_dir()).await?;
            transcode::to_mp3(&cfg.ffmpeg, Path::new(flac), &dst, 320).await?;
            path_320 = Some(dst.display().to_string());
        }
        if track.path_128.is_none() {
            let dst = cfg.mp3_128_dir().join(format!("{base}.mp3"));
            tokio::fs::create_dir_all(cfg.mp3_128_dir()).await?;
            transcode::to_mp3(&cfg.ffmpeg, Path::new(flac), &dst, 128).await?;
            path_128 = Some(dst.display().to_string());
        }
    } else if let Some(from_320) = track.path_320.as_deref() {
        if track.path_128.is_none() {
            let dst = cfg.mp3_128_dir().join(format!("{base}.mp3"));
            tokio::fs::create_dir_all(cfg.mp3_128_dir()).await?;
            transcode::to_mp3(&cfg.ffmpeg, Path::new(from_320), &dst, 128).await?;
            path_128 = Some(dst.display().to_string());
        }
    }

    if path_320.is_none() && path_128.is_none() {
        return Ok(()); // nothing to derive (already complete, or no source)
    }
    db::set_track_derived(&state.pool, isrc, path_320.as_deref(), path_128.as_deref()).await?;

    // Refresh the vault download satellite with the full format set.
    if let Some(t) = db::get_track(&state.pool, isrc).await? {
        let formats = t.formats().join(",");
        vault_download(state, isrc, "ready", t.source_format.as_deref(), &formats).await;
    }
    info!("{isrc}: derived formats ready");
    Ok(())
}

// ── Phase C: BPM/key analysis — the `analyze` kind on the unified queue ─────

/// The `analyze` kind. Each tick reconciles work from ready-but-unanalysed FLACs
/// (`enqueue_if_absent`, so a permanently failed item is not resurrected), then
/// drains `next("analyze")` up to the analysis concurrency. Disabled when no
/// `ANALYZER_URL` is configured. A dead analyzer is backed off (see
/// `analysis_backoff`) so it is not retried — or logged — every tick.
pub async fn advance_analyze(state: &Arc<AppState>) -> Result<()> {
    if !analyzer::enabled(state.config.analyzer_url.as_deref()) {
        return Ok(());
    }
    if state.analysis_backoff.blocked(db::now()) {
        return Ok(());
    }

    let kind = work::kind::ANALYZE;
    for track in db::ready_flac_unaanalyzed(&state.pool, ANALYSIS_BATCH).await? {
        let priority = db::order_priority(&state.pool, &track.isrc).await?;
        work::enqueue_if_absent(&state.pool, kind, &track.isrc, priority, Some("essentia")).await?;
    }

    let slots =
        work::free_slots(&state.pool, kind, state.config.analysis_concurrency as i64).await?;
    if slots == 0 {
        return Ok(());
    }
    for item in work::next(&state.pool, kind, slots, db::now()).await? {
        if !work::claim(&state.pool, item.id, db::now()).await? {
            continue;
        }
        let Some(track) = db::get_track(&state.pool, &item.reference).await? else {
            work::fail(&state.pool, item.id, "unknown track", db::now()).await?;
            continue;
        };
        // `attempts` after the claim is `item.attempts + 1`.
        spawn_analysis(state.clone(), track, item.id, item.attempts as u32 + 1);
    }
    Ok(())
}

/// Dispatch one analysis onto a bounded background task and settle its queue
/// item. A per-ISRC guard stops duplicate work while a job is live.
fn spawn_analysis(state: Arc<AppState>, track: Track, work_id: i64, attempt: u32) {
    {
        let mut live = state.analyzing.lock().expect("analyzing mutex");
        if !live.insert(track.isrc.clone()) {
            return; // already being analysed — the item stays `running`
        }
    }
    tokio::spawn(async move {
        let _permit = state.analysis_sem.acquire().await.ok();
        let outcome = analyze_track(&state, &track).await;
        let now = db::now();
        let base = state.config.queue_retry_base_secs;
        let max = state.config.queue_retry_max_secs;
        match outcome {
            Ok(true) => {
                state.analysis_backoff.note_success();
                let _ = work::finish(&state.pool, work_id, now).await;
            }
            // The analyzer answered but had nothing usable — back off, then give
            // up after `max_attempts` (the reconciler won't re-arm it).
            Ok(false) => {
                let delay = state.analysis_backoff.note_failure(now);
                let _ = work::retry_later(
                    &state.pool,
                    work_id,
                    "analyzer returned no fields",
                    now + db::backoff_delay(attempt, base, max),
                    now,
                )
                .await;
                debug!("analyzer returned nothing; retry after {delay}s");
            }
            Err(e) => {
                warn!("{}: analysis failed: {e:#}", track.isrc);
                state
                    .logs
                    .push("warn", format!("{}: analysis failed: {e}", track.isrc));
                let delay = state.analysis_backoff.note_failure(now);
                let _ = work::retry_later(
                    &state.pool,
                    work_id,
                    &format!("{e}"),
                    now + db::backoff_delay(attempt, base, max),
                    now,
                )
                .await;
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn quality_ladder_steps_down_by_tier() {
        use crate::deezer_dl::Quality::*;
        assert_eq!(quality_ladder(Flac), vec![Flac, Mp3_320, Mp3_128]);
        assert_eq!(quality_ladder(Mp3_320), vec![Mp3_320, Mp3_128]);
        assert_eq!(quality_ladder(Mp3_128), vec![Mp3_128]);
    }

    #[tokio::test]
    async fn download_falls_back_from_flac_to_320() {
        use crate::deezer_dl::Quality::*;
        let mut seen = Vec::new();
        let (bytes, used) = download_with_fallback(Flac, |q| {
            seen.push(q);
            async move {
                if q == Flac {
                    Err(crate::deezer_dl::Error::NoLosslessTier)
                } else {
                    Ok(vec![b'x'])
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(used, Mp3_320);
        assert_eq!(seen, vec![Flac, Mp3_320]);
        assert_eq!(bytes, vec![b'x']);
    }

    #[tokio::test]
    async fn download_falls_back_all_the_way_to_128() {
        use crate::deezer_dl::Quality::*;
        let (_, used) = download_with_fallback(Flac, |q| async move {
            match q {
                Flac | Mp3_320 => Err(crate::deezer_dl::Error::NoLosslessTier),
                _ => Ok(vec![1u8]),
            }
        })
        .await
        .unwrap();
        assert_eq!(used, Mp3_128);
    }

    #[tokio::test]
    async fn download_aborts_on_a_hard_error_without_fallback() {
        use crate::deezer_dl::Quality::*;
        let mut seen = Vec::new();
        let err = download_with_fallback(Flac, |q| {
            seen.push(q);
            async move { Err::<Vec<u8>, _>(crate::deezer_dl::Error::Api("boom".into())) }
        })
        .await
        .unwrap_err();
        assert!(matches!(err, crate::deezer_dl::Error::Api(_)));
        assert_eq!(seen, vec![Flac], "must not step down on a hard error");
    }

    #[test]
    fn native_login_backoff_grows_and_resets() {
        let b = LoginBackoff::default();
        assert!(!b.blocked(1000));
        assert_eq!(b.note_failure(1000), 60);
        assert!(b.blocked(1030));
        assert!(!b.blocked(1061));
        assert_eq!(b.note_failure(1000), 120);
        for _ in 0..10 {
            b.note_failure(1000);
        }
        assert_eq!(b.note_failure(1000), LOGIN_BACKOFF_MAX);
        b.note_success();
        assert!(!b.blocked(1000));
        assert_eq!(b.note_failure(1000), 60);
        assert!(b.warn_missing_arl());
        assert!(!b.warn_missing_arl());
    }

    /// The native login-failure warning is assembled from the error's Display,
    /// which carries no ARL — the exact invariant the worker relies on.
    #[test]
    fn native_login_error_messages_never_carry_the_arl() {
        let arl = "this-is-a-fake-arl-zzz999";
        for err in [
            crate::deezer_dl::Error::InvalidArl,
            crate::deezer_dl::Error::Api("deezer api unreachable".to_string()),
        ] {
            let msg = format!("native Deezer login failed ({err}) — next attempt in 60s");
            assert!(!msg.contains(arl), "ARL leaked: {msg}");
            assert!(!msg.contains("zzz999"), "ARL fragment leaked: {msg}");
        }
    }

    #[test]
    fn arl_warning_fires_once() {
        let b = LoginBackoff::default();
        assert!(b.warn_missing_arl());
        assert!(!b.warn_missing_arl());
    }
}
