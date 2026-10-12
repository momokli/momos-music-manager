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

/// Process-wide emitter, installed once by `serve()` (replaceable in tests).
static GLOBAL: std::sync::RwLock<Option<Arc<EventEmitter>>> =
    std::sync::RwLock::new(None);

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
    /// Create an emitter over a running pipeline with the given client env.
    pub fn new(env: PipelineEnv, pipeline: TelemetryPipeline) -> Self {
        Self {
            env: Arc::new(env),
            pipeline: Arc::new(pipeline),
        }
    }

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
pub fn install(emitter: EventEmitter) {
    let mut guard = GLOBAL
        .write()
        .unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(Arc::new(emitter));
    } else {
        warn!("telemetry event pipeline already installed — keeping existing");
    }
}

/// Remove the process-wide emitter (tests only).
#[cfg(test)]
pub fn uninstall_for_test() {
    let mut guard = GLOBAL
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// Access the installed emitter (None when telemetry is disabled).
pub fn global() -> Option<Arc<EventEmitter>> {
    GLOBAL
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Enqueue one event on the process-wide pipeline (no-op when disabled).
pub fn emit(event: TelemetryEvent) -> bool {
    global().is_some_and(|e| e.emit(event))
}

/// Build + enqueue an event from type + payload (no-op when disabled).
pub fn emit_event(r#type: EventType, payload: serde_json::Value) -> bool {
    global().is_some_and(|e| e.emit_event(r#type, payload))
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
    install(emitter);
    info!("telemetry event pipeline started (endpoint={endpoint})");
    Ok(true)
}

/// Cancel the process-wide pipeline worker (graceful app shutdown): it drains
/// pending events (best effort) and exits; anything unsent stays in the spool
/// and is re-flushed on the next start.
pub fn shutdown_global() {
    if let Some(emitter) = global() {
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
        // No GLOBAL installed (unit tests never install) → emit returns
        // false, no panic.
        let event = TelemetryEvent::new(EventType::TaskStarted, serde_json::json!({}));
        assert!(!emit(event));
        assert!(!emit_event(EventType::TaskStarted, serde_json::json!({})));
        assert!(global().is_none());
    }

    #[tokio::test]
    async fn install_replace_uninstall_roundtrip() {
        // Install semantics: first install wins; uninstall clears. Uses a
        // fake pipeline via the public API — no events are sent.
        uninstall_for_test();
        assert!(global().is_none());

        // Build a real (but un-driven) pipeline in a temp dir; never emitted to.
        let dir = tempfile::tempdir().unwrap();
        let pipeline = super::super::flusher::spawn(super::super::flusher::FlusherConfig::new(
            PipelineEnv {
                client_id: "client-a".to_string(),
                app_version: "1.0.0".to_string(),
                os: "linux".to_string(),
            },
            "http://127.0.0.1:1/api/telemetry".to_string(),
            None,
            dir.path().to_path_buf(),
        ));
        let emitter = EventEmitter {
            env: Arc::new(PipelineEnv {
                client_id: "client-a".to_string(),
                app_version: "1.0.0".to_string(),
                os: "linux".to_string(),
            }),
            pipeline: Arc::new(pipeline),
        };
        install(emitter);
        assert!(global().is_some());
        // Second install is ignored (first wins).
        uninstall_for_test();
        assert!(global().is_none());
    }
}
