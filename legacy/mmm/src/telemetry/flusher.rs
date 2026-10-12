//! Async flusher: batches spooled events and POSTs them to the collector.
//!
//! Owns the event pipeline: a bounded channel (the hot path for [`emit`]
//! callers) feeding a worker that
//!
//! 1. appends every event to the durable [`EventSpool`] (crash-safe),
//! 2. holds a bounded [`EventRingBuffer`] (10k, drop-oldest + warn),
//! 3. drains batches (≤200 events / ~1 MB) and POSTs `POST /api/telemetry`,
//! 4. on 2xx ACK removes the batch from the spool,
//! 5. on transient errors retries with exponential backoff + jitter
//!    (30s → 60s → … cap 1h),
//! 6. drops batches that the server permanently rejects (4xx) after
//!    [`MAX_4XX_ATTEMPTS`] tries,
//! 7. drains remaining events on shutdown (best effort, spool is the safety net).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;
use tokio_util::sync::CancellationToken;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::buffer::EventRingBuffer;
use super::events::{EventBatch, MAX_BATCH_BYTES, MAX_BATCH_EVENTS, TelemetryEvent};
use super::spool::EventSpool;

/// Initial retry delay after the first transient failure (per concept doc).
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(30);
/// Backoff ceiling (per concept doc).
pub const MAX_BACKOFF: Duration = Duration::from_secs(3600);
/// Default ring buffer / channel capacity (events).
pub const DEFAULT_CAPACITY: usize = 10_000;
/// Re-attempt flush at most every this often while events are pending.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
/// 4xx responses are retried this many times before the batch is dropped.
pub const MAX_4XX_ATTEMPTS: u32 = 3;
/// How long shutdown tries to deliver pending events before giving up
/// (remaining events stay in the spool and are re-flushed next start).
pub const SHUTDOWN_DRAIN_BUDGET: Duration = Duration::from_secs(5);

/// Static client context stamped onto every event at emit time.
#[derive(Debug, Clone)]
pub struct PipelineEnv {
    pub client_id: String,
    pub app_version: String,
    pub os: String,
}

/// Everything the flusher needs to run.
#[derive(Debug, Clone)]
pub struct FlusherConfig {
    pub env: PipelineEnv,
    /// Full endpoint URL, e.g. `https://telemetry.example/api/telemetry`.
    pub endpoint: String,
    /// Bearer token (None → requests are sent without auth header; the
    /// receiver only accepts tokenless pushes in dev mode).
    pub token: Option<String>,
    /// Directory holding the spool file.
    pub spool_dir: PathBuf,
    /// Ring buffer / channel capacity (overridable for tests).
    pub capacity: usize,
    /// Flush cadence while events are pending (overridable for tests).
    pub flush_interval: Duration,
    /// Initial backoff (overridable for tests).
    pub initial_backoff: Duration,
}

impl FlusherConfig {
    /// Production defaults: 10k capacity, 10s flush cadence, 30s backoff.
    pub fn new(env: PipelineEnv, endpoint: String, token: Option<String>, spool_dir: PathBuf) -> Self {
        Self {
            env,
            endpoint,
            token,
            spool_dir,
            capacity: DEFAULT_CAPACITY,
            flush_interval: FLUSH_INTERVAL,
            initial_backoff: INITIAL_BACKOFF,
        }
    }
}

/// Outcome of one flush attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FlushOutcome {
    /// 2xx — batch accepted, safe to remove from the spool.
    Acknowledged,
    /// Transient (network / 5xx / 408 / 429) — retry with backoff.
    Retryable,
    /// Permanent rejection (other 4xx) — drop after a few attempts.
    Permanent,
}

fn classify_status(status: u16) -> FlushOutcome {
    match status {
        200..=299 => FlushOutcome::Acknowledged,
        // 408/429 are transient per HTTP semantics; everything else 4xx is
        // permanent (validation/auth/not-found/too-large).
        408 | 429 => FlushOutcome::Retryable,
        400..=499 => FlushOutcome::Permanent,
        _ => FlushOutcome::Retryable,
    }
}

/// Exponential backoff without jitter: 30s, 60s, 120s, … capped at 1h.
pub fn backoff_base(failure_count: u32, initial: Duration) -> Duration {
    let cap = MAX_BACKOFF;
    let factor = 2u32.saturating_pow(failure_count.saturating_sub(1).min(20));
    let secs = initial.as_secs().saturating_mul(u64::from(factor));
    Duration::from_secs(secs.min(cap.as_secs().max(1)))
}

/// Apply ±50% jitter to a delay (tests assert the bounds).
pub fn with_jitter(delay: Duration) -> Duration {
    let secs = delay.as_secs_f64();
    if secs <= 0.0 {
        return delay;
    }
    let jittered = secs * rand::thread_rng().gen_range(0.5..=1.5);
    Duration::from_secs_f64(jittered)
}

/// Backoff for a failure count: base + jitter.
pub fn backoff_delay(failure_count: u32, initial: Duration) -> Duration {
    with_jitter(backoff_base(failure_count, initial))
}

/// Handle to a running event pipeline.
pub struct TelemetryPipeline {
    tx: mpsc::Sender<TelemetryEvent>,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
    spool: EventSpool,
    drops: Arc<std::sync::atomic::AtomicU64>,
}

impl TelemetryPipeline {
    /// Enqueue one event. **Never blocks, never panics**: a full or closed
    /// channel drops the event with a warning (bounded memory under load).
    pub fn emit(&self, event: TelemetryEvent) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let dropped = self
                    .drops
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                if dropped == 1 || dropped % 1000 == 0 {
                    warn!("telemetry emit: channel full — dropped event (total: {dropped})");
                }
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("telemetry emit: pipeline shut down — event dropped");
                false
            }
        }
    }

    pub fn sender(&self) -> mpsc::Sender<TelemetryEvent> {
        self.tx.clone()
    }

    pub fn spool_path(&self) -> &Path {
        self.spool.path()
    }

    /// Cancel the worker (it drains pending events and exits). Detached: does
    /// not wait for completion — use [`Self::shutdown`] to join.
    pub fn cancel(&self) {
        self.shutdown.cancel();
    }

    /// Cancel the worker and wait for it to drain + finish.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        let _ = self.handle.await;
    }
}

/// Spawn the flusher worker. Loads the spool into the ring buffer first.
pub fn spawn(config: FlusherConfig) -> TelemetryPipeline {
    let (tx, rx) = mpsc::channel(config.capacity.max(1));
    let shutdown = CancellationToken::new();
    let spool = EventSpool::in_data_dir(&config.spool_dir);
    let drops = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let handle = tokio::spawn(run_worker(
        config,
        rx,
        shutdown.clone(),
        spool.clone(),
        drops.clone(),
    ));
    TelemetryPipeline {
        tx,
        shutdown,
        handle,
        spool,
        drops,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_worker(
    config: FlusherConfig,
    mut rx: mpsc::Receiver<TelemetryEvent>,
    shutdown: CancellationToken,
    spool: EventSpool,
    _drops: Arc<std::sync::atomic::AtomicU64>,
) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("telemetry flusher: HTTP client build failed ({e}) — events stay spooled");
            // Keep draining into the spool until shutdown so nothing is lost,
            // but never attempt HTTP.
            drain_until_shutdown(rx, spool, shutdown).await;
            return;
        }
    };

    // Startup: spool → ring buffer.
    let mut buffer = EventRingBuffer::new(config.capacity.max(1));
    let loaded = spool.load().await;
    if !loaded.is_empty() {
        info!("telemetry flusher: loaded {} event(s) from spool", loaded.len());
    }
    for event in loaded {
        buffer.push(event);
    }
    // If the spool outgrew the ring buffer, evicted (oldest) events must not
    // linger in the file — keep the file in sync with the buffer.
    compact_spool_to_buffer(&spool, &buffer).await;
    let mut last_dropped_seen = buffer.dropped_total();

    let http_client = &client;
    let mut last_flush: Option<Instant> = None;
    let mut next_flush_allowed: Option<Instant> = None;
    let mut failure_count: u32 = 0;
    let mut permanent_failures: u32 = 0;
    let mut channel_closed = false;
    let mut shutdown_at: Option<Instant> = None;

    loop {
        // 1. Drain the channel into spool + buffer (never stall on I/O errors).
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    if let Err(e) = spool.append(&[event.clone()]).await {
                        warn!("telemetry spool append failed: {e}");
                    }
                    buffer.push(event);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    channel_closed = true;
                    break;
                }
            }
        }
        // Keep the spool in sync when the ring buffer evicted oldest events
        // (drop-oldest policy): evicted events must not pile up in the file.
        if buffer.dropped_total() != last_dropped_seen {
            compact_spool_to_buffer(&spool, &buffer).await;
            last_dropped_seen = buffer.dropped_total();
        }

        // 2. Flush decision.
        let now = Instant::now();
        let backoff_ok = next_flush_allowed.is_none_or(|t| now >= t);
        let interval_ok = last_flush.is_none_or(|t| now.duration_since(t) >= config.flush_interval);
        let wants_flush = !buffer.is_empty()
            && (buffer.len() >= MAX_BATCH_EVENTS || interval_ok || shutdown.is_cancelled());
        let give_up = shutdown_at.is_some_and(|t| now.duration_since(t) >= SHUTDOWN_DRAIN_BUDGET);

        if backoff_ok && wants_flush && !give_up {
            let batch = buffer.drain_batch(MAX_BATCH_EVENTS, MAX_BATCH_BYTES);
            last_flush = Some(now);
            match send_batch(http_client, &config, &batch).await {
                FlushOutcome::Acknowledged => {
                    failure_count = 0;
                    permanent_failures = 0;
                    next_flush_allowed = None;
                    info!(
                        "telemetry flush ok: {} event(s) acknowledged",
                        batch.len()
                    );
                    compact_spool_to_buffer(&spool, &buffer).await;
                }
                FlushOutcome::Permanent => {
                    permanent_failures = permanent_failures.saturating_add(1);
                    if permanent_failures >= MAX_4XX_ATTEMPTS {
                        failure_count = 0;
                        permanent_failures = 0;
                        next_flush_allowed = None;
                        warn!(
                            "telemetry flush: collector permanently rejected {} event(s) \
                             {MAX_4XX_ATTEMPTS}x — dropping batch",
                            batch.len()
                        );
                        compact_spool_to_buffer(&spool, &buffer).await;
                    } else {
                        failure_count = failure_count.saturating_add(1);
                        // 4xx batches are retried a few times (backoff) before
                        // being dropped — maybe the server state changed.
                        let delay = if shutdown.is_cancelled() {
                            Duration::from_millis(250)
                        } else {
                            backoff_delay(failure_count, config.initial_backoff)
                        };
                        next_flush_allowed = Some(Instant::now() + delay);
                        warn!(
                            "telemetry flush rejected (4xx, attempt {permanent_failures}/\
                             {MAX_4XX_ATTEMPTS}) — retrying in {delay:?}"
                        );
                        buffer.prepend(batch);
                    }
                }
                FlushOutcome::Retryable => {
                    failure_count = failure_count.saturating_add(1);
                    permanent_failures = 0;
                    // During shutdown drain, retry quickly (250ms) until the
                    // drain budget runs out; otherwise exponential backoff.
                    let delay = if shutdown.is_cancelled() {
                        Duration::from_millis(250)
                    } else {
                        backoff_delay(failure_count, config.initial_backoff)
                    };
                    next_flush_allowed = Some(Instant::now() + delay);
                    warn!(
                        "telemetry flush failed (attempt {failure_count}) — retrying in {delay:?} \
                         ({} event(s) pending)",
                        buffer.len()
                    );
                    buffer.prepend(batch);
                }
            }
            continue;
        }

        if shutdown.is_cancelled() && buffer.is_empty() {
            info!("telemetry flusher: shutdown complete");
            break;
        }
        if give_up {
            warn!("telemetry flusher: shutdown drain budget exceeded — events stay spooled");
            break;
        }
        if channel_closed && buffer.is_empty() && !shutdown.is_cancelled() {
            info!("telemetry flusher: channel closed, queue empty — exiting");
            break;
        }

        // 3. Wait for work (event, shutdown, or a tick to re-check the flush
        //    interval). Ticks also let a due backoff re-arm the flush above.
        //    Once shutdown started we only tick (the drain retries every 250ms
        //    via `next_flush_allowed`) — avoids a busy loop on the ready
        //    cancellation future.
        if shutdown_at.is_some() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        } else {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    info!("telemetry flusher: shutdown requested — draining");
                    shutdown_at = Some(Instant::now());
                }
                maybe = rx.recv() => {
                    match maybe {
                        Some(event) => {
                            if let Err(e) = spool.append(&[event.clone()]).await {
                                warn!("telemetry spool append failed: {e}");
                            }
                            buffer.push(event);
                        }
                        None => channel_closed = true,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
    }
}

/// Keep draining into the spool when no HTTP client is available.
async fn drain_until_shutdown(
    mut rx: mpsc::Receiver<TelemetryEvent>,
    spool: EventSpool,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            maybe = rx.recv() => {
                match maybe {
                    Some(event) => {
                        if let Err(e) = spool.append(&[event]).await {
                            warn!("telemetry spool append failed: {e}");
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

/// Rewrite the spool file so it contains exactly the buffered events
/// (removes acknowledged / dropped / evicted events).
async fn compact_spool_to_buffer(spool: &EventSpool, buffer: &EventRingBuffer) {
    let keep: HashSet<String> = buffer.iter().map(|e| e.event_id.clone()).collect();
    if let Err(e) = spool.retain_ids(&keep).await {
        warn!("telemetry spool compaction failed: {e}");
    }
}

/// POST one batch; returns the outcome without panicking.
async fn send_batch(
    client: &reqwest::Client,
    config: &FlusherConfig,
    batch: &[TelemetryEvent],
) -> FlushOutcome {
    if batch.is_empty() {
        return FlushOutcome::Acknowledged;
    }
    let envelope = EventBatch::new(&config.env.client_id, batch.to_vec());
    if let Err(reason) = envelope.validate() {
        warn!("telemetry flush: invalid batch ({reason}) — dropping");
        return FlushOutcome::Permanent;
    }
    let mut req = client.post(&config.endpoint).json(&envelope);
    if let Some(token) = &config.token {
        req = req.bearer_auth(token);
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                return FlushOutcome::Acknowledged;
            }
            warn!(
                "telemetry flush: collector responded {status} — {}",
                resp.text().await.unwrap_or_default().chars().take(300).collect::<String>()
            );
            classify_status(status)
        }
        Err(e) => {
            warn!("telemetry flush: request failed: {e}");
            FlushOutcome::Retryable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::events::EventType;

    fn ev(id: &str) -> TelemetryEvent {
        let mut e = TelemetryEvent::new(EventType::TaskCompleted, serde_json::json!({}));
        e.event_id = id.to_string();
        e
    }

    #[test]
    fn classify_status_groups() {
        assert_eq!(classify_status(202), FlushOutcome::Acknowledged);
        assert_eq!(classify_status(200), FlushOutcome::Acknowledged);
        assert_eq!(classify_status(400), FlushOutcome::Permanent);
        assert_eq!(classify_status(401), FlushOutcome::Permanent);
        assert_eq!(classify_status(404), FlushOutcome::Permanent);
        assert_eq!(classify_status(408), FlushOutcome::Retryable);
        assert_eq!(classify_status(429), FlushOutcome::Retryable);
        assert_eq!(classify_status(500), FlushOutcome::Retryable);
        assert_eq!(classify_status(503), FlushOutcome::Retryable);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let base = Duration::from_secs(30);
        assert_eq!(backoff_base(1, base), Duration::from_secs(30));
        assert_eq!(backoff_base(2, base), Duration::from_secs(60));
        assert_eq!(backoff_base(3, base), Duration::from_secs(120));
        assert_eq!(backoff_base(8, base), Duration::from_secs(3600));
        // no overflow explosion
        assert_eq!(backoff_base(100, base), Duration::from_secs(3600));
    }

    #[test]
    fn jitter_stays_within_half_and_half_again() {
        let base = Duration::from_secs(100);
        for _ in 0..200 {
            let j = with_jitter(base);
            let j = j.as_secs_f64();
            assert!((50.0..=150.0).contains(&j), "jittered delay {j}s out of range");
        }
    }

    #[test]
    fn jitter_zero_is_zero() {
        assert_eq!(with_jitter(Duration::ZERO), Duration::ZERO);
    }
}
