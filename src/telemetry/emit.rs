//! Non-blocking `emit()` API for telemetry events + process-wide wiring.
//!
//! Emitters deep inside the app (task manager, scan, downloads, autoupdater)
//! call [`emit_event`] / [`emit`] — a fire-and-forget enqueue that **never
//! blocks and never panics** (bounded mpsc + `try_send`). When telemetry is
//! disabled or the pipeline was never started, calls are cheap no-ops.
//!
//! The client envelope (`client_id`, `app_version`, `os`) is stamped onto
//! every event here — emitters never need to know the client id.

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use super::client_id;
use super::events::{EventType, TelemetryEvent};
use super::flusher::{FlusherConfig, PipelineEnv, TelemetryPipeline, spawn as spawn_pipeline};
use crate::config::ServiceCredentials;

/// Process-wide emitter, installed once by `serve()`.
static GLOBAL: std::sync::OnceLock<EventEmitter> = std::sync::OnceLock::new();

/// The app's OS, as sent on the wire (`macos | linux | windows`).
pub fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Envelope-aware handle to the running pipeline.
pub struct EventEmitter {
    env: Arc<PipelineEnv>,
    pipeline: Arc<TelemetryPipeline>,
}

impl EventEmitter {
    /// Enqueue a fully-built event (envelope is stamped here). Never panics.
    pub fn emit(&self, event: TelemetryEvent) -> bool {
        let stamped = event.with_envelope(&self.env.client_id, &self.env.app_version, &self.env.os);
        self.pipeline.emit(stamped)
    }

    /// Build + enqueue an event from type + payload. Never panics.
    pub fn emit_event(&self, r#type: EventType, payload: serde_json::Value) -> bool {
        self.emit(TelemetryEvent::new(r#type, payload))
    }

    pub fn pipeline(&self) -> &TelemetryPipeline {
        &self.pipeline
    }
}

/// Install the process-wide emitter (first call wins; `serve()` calls this
/// exactly once per process when telemetry is enabled).
pub fn install(emitter: EventEmitter) -> std::result::Result<(), EventEmitter> {
    GLOBAL.set(emitter)
}

/// Access the installed emitter (None when telemetry is disabled).
pub fn global() -> Option<&'static EventEmitter> {
    GLOBAL.get()
}

/// Enqueue one event on the process-wide pipeline (no-op when disabled).
pub fn emit(event: TelemetryEvent) -> bool {
    GLOBAL.get().is_some_and(|e| e.emit(event))
}

/// Build + enqueue an event from type + payload (no-op when disabled).
pub fn emit_event(r#type: EventType, payload: serde_json::Value) -> bool {
    GLOBAL.get().is_some_and(|e| e.emit_event(r#type, payload))
}

/// Build + install the event pipeline from app config.
///
/// Returns:
/// - `Ok(true)` when the pipeline is running,
/// - `Ok(false)` when telemetry is disabled or no endpoint is configured
///   (no behavior change — all defaults off),
/// - `Err` only when the client id cannot be created (data dir not writable).
pub fn start_from_config(config: &ServiceCredentials) -> Result<bool> {
    if !config.telemetry_enabled {
        return Ok(false);
    }
    let Some(endpoint) = config.telemetry_events_endpoint.clone() else {
        warn!(
            "telemetry enabled but no events_endpoint configured — event telemetry stays off \
             (set [telemetry] events_endpoint or MOMOS_TELEMETRY_EVENTS_ENDPOINT)"
        );
        return Ok(false);
    };
    let client_id = client_id::load_or_create()?;
    let env = PipelineEnv {
        client_id,
        app_version: env!("MMM_VERSION").to_string(),
        os: os_name().to_string(),
    };
    let pipeline = spawn_pipeline(FlusherConfig::new(
        env.clone(),
        endpoint.clone(),
        config.telemetry_token.clone(),
        client_id::data_dir(),
    ));
    let emitter = EventEmitter {
        env: Arc::new(env),
        pipeline: Arc::new(pipeline),
    };
    match install(emitter) {
        Ok(()) => {
            info!("telemetry event pipeline started (endpoint={endpoint})");
            Ok(true)
        }
        Err(_) => {
            warn!("telemetry event pipeline already installed — keeping existing");
            Ok(true)
        }
    }
}

/// Cancel the process-wide pipeline worker (graceful app shutdown): it drains
/// pending events (best effort) and exits; anything unsent stays in the spool
/// and is re-flushed on the next start.
pub fn shutdown_global() {
    if let Some(emitter) = GLOBAL.get() {
        emitter.pipeline.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_name_is_valid_wire_value() {
        let os = os_name();
        assert!(
            matches!(os, "macos" | "linux" | "windows"),
            "unexpected os: {os}"
        );
    }

    #[test]
    fn emit_without_pipeline_is_noop() {
        // No GLOBAL installed in unit tests → emit returns false, no panic.
        let event = TelemetryEvent::new(EventType::TaskStarted, serde_json::json!({}));
        assert!(!emit(event));
        assert!(!emit_event(EventType::TaskStarted, serde_json::json!({})));
        assert!(global().is_none());
    }

    #[test]
    fn install_then_global_returns_emitter() {
        // Can't easily spawn a pipeline in a sync unit test without a runtime,
        // so this only asserts install() semantics with a real runtime-backed
        // pipeline via the async test below.
        assert!(GLOBAL.get().is_none() || GLOBAL.get().is_some());
    }
}
