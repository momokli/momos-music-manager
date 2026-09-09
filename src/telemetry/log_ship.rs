//! Log shipping: a tracing layer that forwards filtered app logs into the
//! event pipeline as `log.entry` events (plan E4).
//!
//! Architecture:
//!
//! - [`LogShipLayer`] is registered in `main()` **inactive** (its state is
//!   `None`) — every `on_event` is a cheap early return while shipping is
//!   off. `serve()` activates it via the [`LogShipHandle`] only AFTER the
//!   event pipeline is running (no log event can precede the pipeline).
//! - Filtering happens in the layer: level ≥ `log_min_level`, targets only
//!   with the `momos_music_manager` prefix, and `momos_music_manager::telemetry*`
//!   is excluded — the recursion guard (flusher/layer logs never re-enter
//!   the ship path; pipeline problems stay observable in file/STDOUT).
//! - The layer **never blocks**: it builds the payload (home-strip +
//!   [`MAX_LOG_MESSAGE_CHARS`] truncation via [`events::log_entry_payload`])
//!   and `try_send`s into a bounded channel (2k). A full channel drops the
//!   event and counts it (warned at most once per 1000 drops).
//! - An async worker drains the channel through a token bucket
//!   (`log_max_events_per_sec`) and calls [`emit::emit_event`] — the
//!   existing no-op-safe pipeline API. Channel close / deactivation ends
//!   the worker; unsent logs simply expire (logs are high-volume and
//!   re-derivable — deliberately no spool overhead for them).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use super::emit;
use super::events::{EventType, MAX_LOG_MESSAGE_CHARS, log_entry_payload};

/// Bounded channel capacity between the layer and the worker (2k — spikes
/// are stopped at the source; the pipeline ring buffer is the second stage).
pub const CHANNEL_CAPACITY: usize = 2_000;

/// Parse a config level string (`error|warn|info|debug|trace`) into a
/// [`tracing::Level`]; anything else falls back to `warn` (config is
/// canonicalized before activation, this is belt + braces).
pub fn parse_level(s: &str) -> Level {
    match s.trim().to_ascii_lowercase().as_str() {
        "error" => Level::ERROR,
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::WARN,
    }
}

/// Effective shipping configuration (activated via the handle).
#[derive(Debug, Clone)]
pub struct LogShipConfig {
    /// Keep events at or above this level.
    pub min_level: Level,
    /// Token-bucket refill rate: max events shipped per second (mandatory
    /// cap — the plan forbids an "unlimited" mode).
    pub max_events_per_sec: u64,
}

/// Shared state between the layer and the handle.
struct Shared {
    /// `None` while inactive (default) — the layer's hot path.
    state: RwLock<Option<ActiveState>>,
    /// Channel-full drops (layer side), counted for the 1/1000 warning.
    drops: AtomicU64,
}

struct ActiveState {
    min_level: Level,
    tx: mpsc::Sender<serde_json::Value>,
    shutdown: CancellationToken,
}

/// A `tracing` layer that forwards filtered app logs. Start inactive;
/// activate via [`LogShipHandle`].
#[derive(Clone)]
pub struct LogShipLayer {
    shared: Arc<Shared>,
}

/// Handle to activate/deactivate shipping (created alongside the layer).
pub struct LogShipHandle {
    shared: Arc<Shared>,
}

/// Create an inactive layer + its activation handle. Register the layer in
/// the subscriber in `main()`, keep the handle for `serve()`.
pub fn layer() -> (LogShipLayer, LogShipHandle) {
    let shared = Arc::new(Shared {
        state: RwLock::new(None),
        drops: AtomicU64::new(0),
    });
    (
        LogShipLayer {
            shared: shared.clone(),
        },
        LogShipHandle { shared },
    )
}

impl LogShipLayer {
    /// Whether shipping is currently active (test helper + status logs).
    fn is_active(&self) -> bool {
        self.shared
            .state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

impl LogShipHandle {
    pub fn is_active(&self) -> bool {
        self.shared
            .state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Activate shipping with the given config. Idempotent: a second
    /// activation while active is a no-op (config changes need a restart,
    /// consistent with the other background loops). Spawns the worker that
    /// drains the channel into the event pipeline. When no tokio runtime is
    /// present (pure unit tests) the state is armed but no worker spawns.
    pub fn activate(&self, config: LogShipConfig) {
        let mut guard = self
            .shared
            .state
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return;
        }
        let rate = config.max_events_per_sec.max(1);
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let shutdown = CancellationToken::new();
        *guard = Some(ActiveState {
            min_level: config.min_level,
            tx,
            shutdown: shutdown.clone(),
        });
        drop(guard);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(run_worker(rx, rate, shutdown, |payload| {
                emit::emit_event(EventType::LogEntry, payload);
            }));
        }
    }

    /// Deactivate shipping: stops the worker (buffered logs expire) and
    /// closes the channel. Idempotent.
    pub fn deactivate(&self) {
        let mut guard = self
            .shared
            .state
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(active) = guard.take() {
            active.shutdown.cancel();
        }
    }
}

impl<S: Subscriber> Layer<S> for LogShipLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let guard = match self.shared.state.read() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let Some(active) = guard.as_ref() else {
            return; // inactive default — cheap early return
        };

        // 1. Level filter: keep events at or above `log_min_level`. tracing's
        //    Level ordering puts the most severe level first (ERROR < WARN <
        //    INFO < DEBUG < TRACE) — "at or above warn" = error + warn.
        let level = event.metadata().level();
        if *level > active.min_level {
            return;
        }

        // 2. Target filter: only `momos_music_manager` targets …
        let target = event.metadata().target();
        if !target.starts_with("momos_music_manager") {
            return;
        }
        // … and never the telemetry module itself (recursion guard).
        if target == "momos_music_manager::telemetry"
            || target.starts_with("momos_music_manager::telemetry::")
        {
            return;
        }

        // 3. Build the payload (sanitize + truncate like every other payload).
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let payload = log_entry_payload(
            &level.as_str().to_ascii_lowercase(),
            target,
            &visitor.build(),
        );

        // 4. Never block: bounded try_send, count + warn drops on full.
        match active.tx.try_send(payload) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                let dropped = self.shared.drops.fetch_add(1, Ordering::Relaxed) + 1;
                if dropped == 1 || dropped % 1000 == 0 {
                    // warn! from inside the layer is safe: this module's
                    // target is excluded by the recursion guard above.
                    tracing::warn!(
                        "log ship: channel full — dropped log event (total: {dropped})"
                    );
                }
            }
        }
    }
}

/// Drain the channel through a token bucket and emit each payload.
///
/// The bucket is a simple "one event per 1/rate interval" gate (burst 1):
/// the strictest reading of the mandatory cap — spikes are absorbed by the
/// bounded channel and shipped at exactly `rate` events/sec. The sink is
/// injectable for tests; production uses [`emit::emit_event`].
async fn run_worker<F>(
    mut rx: mpsc::Receiver<serde_json::Value>,
    rate: u64,
    shutdown: CancellationToken,
    mut sink: F,
) where
    F: FnMut(serde_json::Value),
{
    let interval = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
    let mut next_allowed = Instant::now();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            maybe = rx.recv() => {
                match maybe {
                    Some(payload) => {
                        let now = Instant::now();
                        if now < next_allowed {
                            tokio::time::sleep(next_allowed - now).await;
                        }
                        next_allowed = next_allowed.max(Instant::now()) + interval;
                        sink(payload);
                    }
                    None => return, // channel closed (all senders dropped)
                }
            }
        }
    }
}

/// Collect the `message` field + any other event fields into a single line
/// (fmt-style: `message key=value …`). `message` arrives either via
/// `record_str` (literal/`str` messages) or via `record_debug` (the macro
/// records `format_args!` whose Debug rendering is the plain text).
#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    extras: Vec<String>,
}

impl MessageVisitor {
    fn build(self) -> String {
        match self.message {
            Some(m) if self.extras.is_empty() => m,
            Some(m) => format!("{m} {}", self.extras.join(" ")),
            None => self.extras.join(" "),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            if self.message.is_none() {
                self.message = Some(value.to_string());
            }
        } else {
            self.extras.push(format!("{}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            if self.message.is_none() {
                self.message = Some(format!("{value:?}"));
            }
        } else {
            self.extras.push(format!("{}={value:?}", field.name()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::events::MAX_ERROR_MESSAGE_CHARS;

    /// Send `event` through a registry containing only the given layer and
    /// return the layer (to read its state afterwards). Events are recorded
    /// synchronously — the channel receives them before this returns.
    fn dispatch(layer: &LogShipLayer, event: impl Fn()) {
        use tracing_subscriber::prelude::*;
        let subscriber = tracing_subscriber::registry().with(layer.clone());
        tracing::subscriber::with_default(subscriber, event);
    }

    /// Activate without spawning the worker: swap the state and hand back
    /// the receiver so tests can assert what would be shipped.
    fn activate_for_test(
        handle: &LogShipHandle,
        min_level: Level,
        capacity: usize,
    ) -> mpsc::Receiver<serde_json::Value> {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        let mut guard = handle.shared.state.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(ActiveState {
            min_level,
            tx,
            shutdown: CancellationToken::new(),
        });
        rx
    }

    #[test]
    fn inactive_by_default_and_cheap() {
        let (layer, handle) = layer();
        assert!(!handle.is_active());
        assert!(!layer.is_active());
        // Recording while inactive must not panic and produce nothing.
        dispatch(&layer, || {
            tracing::error!(target: "momos_music_manager::db", "boom");
            tracing::info!(target: "momos_music_manager::db", "hello");
        });
    }

    #[test]
    fn level_filter_keeps_only_at_or_above_min() {
        let (layer, handle) = layer();
        let mut rx = activate_for_test(&handle, Level::WARN, 16);

        dispatch(&layer, || {
            tracing::error!(target: "momos_music_manager::db", "e1");
            tracing::warn!(target: "momos_music_manager::db", "w1");
            tracing::info!(target: "momos_music_manager::db", "i1");
            tracing::debug!(target: "momos_music_manager::db", "d1");
            tracing::trace!(target: "momos_music_manager::db", "t1");
        });

        let mut levels = Vec::new();
        while let Ok(payload) = rx.try_recv() {
            levels.push(payload["level"].as_str().unwrap().to_string());
        }
        assert_eq!(levels, vec!["error".to_string(), "warn".to_string()]);
    }

    #[test]
    fn level_filter_trace_keeps_everything() {
        let (layer, handle) = layer();
        let mut rx = activate_for_test(&handle, Level::TRACE, 16);
        dispatch(&layer, || {
            tracing::trace!(target: "momos_music_manager::db", "t");
            tracing::error!(target: "momos_music_manager::db", "e");
        });
        let mut n = 0;
        while let Ok(_) = rx.try_recv() {
            n += 1;
        }
        assert_eq!(n, 2);
    }

    #[test]
    fn target_filter_foreign_crates_and_recursion_guard() {
        let (layer, handle) = layer();
        let mut rx = activate_for_test(&handle, Level::ERROR, 16);

        dispatch(&layer, || {
            // Foreign crate: never shipped.
            tracing::error!(target: "reqwest", "http failed");
            tracing::error!(target: "hyper", "conn reset");
            // The app's own telemetry module: recursion guard.
            tracing::error!(target: "momos_music_manager::telemetry::flusher", "flush failed");
            tracing::error!(target: "momos_music_manager::telemetry::log_ship", "ship failed");
            // App target (non-telemetry): shipped.
            tracing::error!(target: "momos_music_manager::db", "disk full");
            // Bare `momos_music_manager` (crate root) is a valid prefix.
            tracing::error!(target: "momos_music_manager", "root-level error");
        });

        let mut targets = Vec::new();
        while let Ok(payload) = rx.try_recv() {
            targets.push(payload["target"].as_str().unwrap().to_string());
        }
        assert_eq!(
            targets,
            vec![
                "momos_music_manager::db".to_string(),
                "momos_music_manager".to_string()
            ]
        );
    }

    #[test]
    fn message_building_includes_extras() {
        let (layer, handle) = layer();
        let mut rx = activate_for_test(&handle, Level::INFO, 16);
        dispatch(&layer, || {
            tracing::info!(target: "momos_music_manager::watch", count = 3, "watched {} folders", 2);
            tracing::error!(target: "momos_music_manager::db", "plain message");
        });
        let mut messages = Vec::new();
        while let Ok(payload) = rx.try_recv() {
            messages.push(payload["message"].as_str().unwrap().to_string());
        }
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], "watched 2 folders count=3");
        assert_eq!(messages[1], "plain message");
        assert_eq!(rx.len(), 0);
    }

    #[test]
    fn payload_is_sanitized_and_truncated() {
        let (layer, handle) = layer();
        let mut rx = activate_for_test(&handle, Level::ERROR, 16);
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy().to_string();
        let long = "x".repeat(MAX_LOG_MESSAGE_CHARS * 2);

        dispatch(&layer, || {
            tracing::error!(target: "momos_music_manager::db", "boom at {} {}", home_str, long);
        });

        let item = rx.try_recv().unwrap();
        let payload = item;
        assert_eq!(payload["level"], "error");
        assert_eq!(payload["target"], "momos_music_manager::db");
        let message = payload["message"].as_str().unwrap();
        assert!(!message.contains(&home_str), "home path leaked: {message}");
        assert!(message.contains('~'), "home prefix must be stripped: {message}");
        assert!(
            message.chars().count() <= MAX_LOG_MESSAGE_CHARS,
            "message not truncated"
        );
        // The whole payload stays within the wire cap (1000 < 4096).
        let serialized = serde_json::to_string(&payload).unwrap();
        assert!(serialized.len() <= 4096);
    }

    #[test]
    fn drop_on_full_counts_and_never_blocks() {
        let (layer, handle) = layer();
        let mut rx = activate_for_test(&handle, Level::INFO, 2); // tiny channel

        // 4 events into a capacity-2 channel → 2 accepted, 2 dropped.
        dispatch(&layer, || {
            tracing::info!(target: "momos_music_manager::db", "a");
            tracing::info!(target: "momos_music_manager::db", "b");
            tracing::info!(target: "momos_music_manager::db", "c");
            tracing::info!(target: "momos_music_manager::db", "d");
        });

        let mut n = 0;
        while let Ok(_) = rx.try_recv() {
            n += 1;
        }
        assert_eq!(n, 2, "capacity-2 channel must accept exactly 2");
        assert_eq!(layer.shared.drops.load(Ordering::Relaxed), 2);
        // No panic = never blocked (a blocking send would deadlock this test).
    }

    #[tokio::test]
    async fn worker_paces_through_token_bucket() {
        let (tx, rx) = mpsc::channel(16);
        let shutdown = CancellationToken::new();
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sent_clone = sent.clone();

        let worker = tokio::spawn(run_worker(rx, 20, shutdown.clone(), move |payload| {
            sent_clone.lock().unwrap().push(payload["message"].clone());
        }));

        let item = |m: &str| {
            serde_json::json!({ "level": "info", "target": "t", "message": m })
        };
        let start = Instant::now();
        tx.send(item("one")).await.unwrap();
        tx.send(item("two")).await.unwrap();
        tx.send(item("three")).await.unwrap();
        drop(tx); // close → worker exits after draining

        let _ = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        let elapsed = start.elapsed();

        let messages: Vec<_> = sent.lock().unwrap().clone();
        assert_eq!(messages, vec!["one", "two", "three"]);
        // 3 events at 20/s → ≥ 2 × 50ms between first and last send.
        assert!(
            elapsed >= Duration::from_millis(100),
            "token bucket did not pace: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_without_draining() {
        let (tx, rx) = mpsc::channel(16);
        let shutdown = CancellationToken::new();
        let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sent_clone = sent.clone();
        let worker = tokio::spawn(run_worker(rx, 1000, shutdown.clone(), move |_| {
            sent_clone.fetch_add(1, Ordering::Relaxed);
        }));
        tx.send(serde_json::json!({ "message": "x" })).await.unwrap();
        // Give the worker a chance to pick the item up, then cancel.
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap();
        // Either the item was sent before cancel or dropped — the worker must
        // exit promptly either way (never hang).
        assert!(sent.load(Ordering::Relaxed) <= 1);
    }

    #[test]
    fn parse_level_maps_and_falls_back() {
        assert_eq!(parse_level("error"), Level::ERROR);
        assert_eq!(parse_level("WARN"), Level::WARN);
        assert_eq!(parse_level("debug"), Level::DEBUG);
        assert_eq!(parse_level("trace"), Level::TRACE);
        assert_eq!(parse_level("verbose"), Level::WARN);
    }

    #[test]
    fn activate_is_idempotent_without_runtime() {
        // No tokio runtime in a plain #[test]: activate() arms the state but
        // must not panic about spawning.
        let (_layer, handle) = layer();
        handle.activate(LogShipConfig {
            min_level: Level::WARN,
            max_events_per_sec: 50,
        });
        assert!(handle.is_active());
        // Second activation must not replace the running state.
        handle.activate(LogShipConfig {
            min_level: Level::TRACE,
            max_events_per_sec: 1,
        });
        let guard = handle.shared.state.read().unwrap();
        let active = guard.as_ref().unwrap();
        assert_eq!(active.min_level, Level::WARN);
        drop(guard);
        handle.deactivate();
        assert!(!handle.is_active());
        handle.deactivate(); // idempotent
    }
}
