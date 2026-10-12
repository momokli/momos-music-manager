//! Event-based telemetry: structured core events (tasks, scans, downloads,
//! errors, app updates) + UI/user-interaction events + shipped log entries
//! sent in batches to the collector.
//!
//! The `ui.*` and `log.entry` types are part of the "telemetry full
//! package" feature — see `plans/proposed/telemetry-full-package.md`
//! (decisions E1–E5) for the naming rationale and the receiver-side
//! migration `migrations/telemetry/002_ui_log_views.sql`.
//!
//! This module is the **wire format** shared between client and receiver:
//! - [`TelemetryEvent`] — one event (dedup key `event_id`, type allowlist,
//!   redacted payload).
//! - [`EventBatch`] — the HTTPS envelope posted to `POST /api/telemetry`.
//!
//! Security invariants (enforced + tested here):
//! - `event_id` / `client_id` are UUID / safe-identifier shaped.
//! - `type` is an allowlisted [`EventType`] — unknown types are rejected.
//! - payloads are JSON objects, size-capped ([`MAX_PAYLOAD_BYTES`]).
//! - `error_message` payloads are sanitized ([`sanitize_error_message`]):
//!   home-dir prefix stripped, truncated — no paths/PII.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Max events per batch (HTTP body budget, see `MAX_BATCH_BYTES`).
pub const MAX_BATCH_EVENTS: usize = 200;
/// Soft body-size cap for a batch (~1 MB per the concept doc).
pub const MAX_BATCH_BYTES: usize = 1_048_576;
/// Max serialized size of a single event payload.
pub const MAX_PAYLOAD_BYTES: usize = 4096;
/// Max length of a sanitized `error_message`.
pub const MAX_ERROR_MESSAGE_CHARS: usize = 500;
/// Max length of a shipped log message (after home-strip + truncation).
pub const MAX_LOG_MESSAGE_CHARS: usize = 1000;

/// Allowlisted event types (dotted notation on the wire).
///
/// Domains: `task.*`/`scan.*`/`download.*`/`app.*`/`error.*` are the core
/// lifecycle events; `ui.*` covers user interaction (views opened + the
/// six user-triggered actions of the full-package feature); `log.entry`
/// transports filtered + sanitized log lines. No heartbeats — see
/// `plans/proposed/telemetry-events.md` and
/// `plans/proposed/telemetry-full-package.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "task.started")]
    TaskStarted,
    #[serde(rename = "task.completed")]
    TaskCompleted,
    #[serde(rename = "task.failed")]
    TaskFailed,
    #[serde(rename = "scan.completed")]
    ScanCompleted,
    #[serde(rename = "download.started")]
    DownloadStarted,
    #[serde(rename = "download.completed")]
    DownloadCompleted,
    #[serde(rename = "download.failed")]
    DownloadFailed,
    #[serde(rename = "app.updated")]
    AppUpdated,
    #[serde(rename = "error.reported")]
    ErrorReported,
    #[serde(rename = "ui.view.opened")]
    UiViewOpened,
    #[serde(rename = "ui.action.scan_folder")]
    UiActionScanFolder,
    #[serde(rename = "ui.action.run_backup")]
    UiActionRunBackup,
    #[serde(rename = "ui.action.restore_dump")]
    UiActionRestoreDump,
    #[serde(rename = "ui.action.traktor_import")]
    UiActionTraktorImport,
    #[serde(rename = "ui.action.recompute_embeddings")]
    UiActionRecomputeEmbeddings,
    #[serde(rename = "ui.action.deemix_enqueue")]
    UiActionDeemixEnqueue,
    #[serde(rename = "log.entry")]
    LogEntry,
}

impl EventType {
    /// The dotted wire name, e.g. `task.completed`.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::TaskStarted => "task.started",
            EventType::TaskCompleted => "task.completed",
            EventType::TaskFailed => "task.failed",
            EventType::ScanCompleted => "scan.completed",
            EventType::DownloadStarted => "download.started",
            EventType::DownloadCompleted => "download.completed",
            EventType::DownloadFailed => "download.failed",
            EventType::AppUpdated => "app.updated",
            EventType::ErrorReported => "error.reported",
            EventType::UiViewOpened => "ui.view.opened",
            EventType::UiActionScanFolder => "ui.action.scan_folder",
            EventType::UiActionRunBackup => "ui.action.run_backup",
            EventType::UiActionRestoreDump => "ui.action.restore_dump",
            EventType::UiActionTraktorImport => "ui.action.traktor_import",
            EventType::UiActionRecomputeEmbeddings => "ui.action.recompute_embeddings",
            EventType::UiActionDeemixEnqueue => "ui.action.deemix_enqueue",
            EventType::LogEntry => "log.entry",
        }
    }

    /// True for the `ui.*` family (views + user-triggered actions).
    pub fn is_ui(&self) -> bool {
        matches!(
            self,
            EventType::UiViewOpened
                | EventType::UiActionScanFolder
                | EventType::UiActionRunBackup
                | EventType::UiActionRestoreDump
                | EventType::UiActionTraktorImport
                | EventType::UiActionRecomputeEmbeddings
                | EventType::UiActionDeemixEnqueue
        )
    }
}

/// One telemetry event. `event_id` is the server-side dedup key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    /// UUID v4 — dedup key, stable across retries of the same event.
    pub event_id: String,
    /// Stable per-installation client id (see `client_id.rs`).
    pub client_id: String,
    /// `env!("MMM_VERSION")`, e.g. `1.1.0-dev+4eaa1d93`.
    pub app_version: String,
    /// `macos | linux | windows`.
    pub os: String,
    /// ISO8601 UTC (client clock).
    pub ts: String,
    /// Allowlisted event type.
    #[serde(rename = "type")]
    pub r#type: EventType,
    /// Type-specific JSON object, redacted (no paths, no secrets).
    pub payload: serde_json::Value,
}

impl TelemetryEvent {
    /// Create a new event: fresh `event_id` + `ts`. Envelope fields
    /// (`client_id`, `app_version`, `os`) are filled by the emitter.
    pub fn new(r#type: EventType, payload: serde_json::Value) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            client_id: String::new(),
            app_version: String::new(),
            os: String::new(),
            ts: Utc::now().to_rfc3339(),
            r#type,
            payload,
        }
    }

    /// Fill the client envelope (called by the emitter right before queueing).
    pub fn with_envelope(mut self, client_id: &str, app_version: &str, os: &str) -> Self {
        self.client_id = client_id.to_string();
        self.app_version = app_version.to_string();
        self.os = os.to_string();
        self
    }

    /// Validate the wire format of a fully-enveloped event.
    pub fn is_valid(&self) -> bool {
        valid_uuid(&self.event_id)
            && valid_client_id(&self.client_id)
            && !self.app_version.is_empty()
            && self.app_version.len() <= 64
            && is_valid_os(&self.os)
            && parse_ts(&self.ts).is_some()
            && payload_is_valid(&self.payload)
    }
}

/// HTTPS batch envelope: one POST = one batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBatch {
    pub client_id: String,
    /// ISO8601 UTC — when the client sent this batch.
    pub sent_at: String,
    pub events: Vec<TelemetryEvent>,
}

impl EventBatch {
    pub fn new(client_id: &str, events: Vec<TelemetryEvent>) -> Self {
        Self {
            client_id: client_id.to_string(),
            sent_at: Utc::now().to_rfc3339(),
            events,
        }
    }

    /// Full validation: batch bounds, per-event wire format, unique event ids.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_client_id(&self.client_id) {
            return Err("invalid client_id".to_string());
        }
        if parse_ts(&self.sent_at).is_none() {
            return Err("invalid sent_at (expected ISO8601 UTC)".to_string());
        }
        if self.events.is_empty() {
            return Err("empty events array".to_string());
        }
        if self.events.len() > MAX_BATCH_EVENTS {
            return Err(format!(
                "batch too large: {} events (max {MAX_BATCH_EVENTS})",
                self.events.len()
            ));
        }
        let serialized = serde_json::to_vec(self).unwrap_or_default();
        if serialized.len() > MAX_BATCH_BYTES {
            return Err(format!(
                "batch too large: {} bytes (max {MAX_BATCH_BYTES})",
                serialized.len()
            ));
        }

        let mut seen = std::collections::HashSet::new();
        for event in &self.events {
            if !event.is_valid() {
                return Err(format!(
                    "invalid event {} (type={})",
                    event.event_id,
                    event.r#type.as_str()
                ));
            }
            if !seen.insert(event.event_id.clone()) {
                return Err(format!("duplicate event_id in batch: {}", event.event_id));
            }
        }
        Ok(())
    }
}

// ── Validation helpers (shared with the receiver) ─────────────────────────

pub fn valid_uuid(s: &str) -> bool {
    Uuid::parse_str(s).is_ok()
}

/// Same rule as the receiver's snapshot `instance` validation: no traversal,
/// no slashes — safe to use in DB keys.
pub fn valid_client_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn is_valid_os(s: &str) -> bool {
    matches!(s, "macos" | "linux" | "windows")
}

/// Parse an ISO8601 UTC timestamp (RFC3339, e.g. `2026-09-01T00:41:59Z`).
pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
}

fn payload_is_valid(payload: &serde_json::Value) -> bool {
    if !payload.is_object() {
        return false;
    }
    serde_json::to_vec(payload).map(|b| b.len() <= MAX_PAYLOAD_BYTES).unwrap_or(false)
}

// ── Payload hygiene ───────────────────────────────────────────────────────

/// Strip the home-dir prefix (e.g. `/Users/momo` → `~`) and truncate to
/// `max_chars` chars. Never send absolute paths / PII.
fn sanitize_message(msg: &str, max_chars: usize) -> String {
    let mut s = msg.to_string();
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy().to_string();
        if !home_str.is_empty() {
            s = s.replace(&home_str, "~");
        }
    }
    if s.chars().count() > max_chars {
        s = s.chars().take(max_chars).collect();
    }
    s
}

/// Strip the home-dir prefix (e.g. `/Users/momo` → `~`) and truncate to
/// [`MAX_ERROR_MESSAGE_CHARS`] chars. Never send absolute paths / PII.
pub fn sanitize_error_message(msg: &str) -> String {
    sanitize_message(msg, MAX_ERROR_MESSAGE_CHARS)
}

/// Build an `error.reported`-style payload from a raw error message.
pub fn error_payload(message: &str) -> serde_json::Value {
    serde_json::json!({ "error_message": sanitize_error_message(message) })
}

// ── UI / log payload helpers (full-package feature) ───────────────────────

/// Validate a view id (SPA page id, e.g. `dashboard`, `tag-categories`):
/// lowercase alphanumeric + hyphen, 1–64 chars — safe to store/group in DB
/// keys, no traversal or query noise.
pub fn valid_view_id(view: &str) -> bool {
    !view.is_empty()
        && view.len() <= 64
        && view
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Build a `ui.view.opened` payload. `None` for invalid view ids — the
/// emitter then skips the event entirely (never send garbage on the wire).
pub fn view_payload(view: &str) -> Option<serde_json::Value> {
    if !valid_view_id(view) {
        return None;
    }
    Some(serde_json::json!({ "view": view }))
}

/// Build a `ui.action.*` payload from the handler outcome. Only sync
/// errors go into `error_message` (sanitized like `error_payload`);
/// lifecycle errors keep flowing through `task.failed`/`error.reported`.
pub fn action_payload(ok: bool, error_message: Option<&str>) -> serde_json::Value {
    match error_message {
        Some(msg) => serde_json::json!({
            "ok": ok,
            "error_message": sanitize_error_message(msg),
        }),
        None => serde_json::json!({ "ok": ok }),
    }
}

/// Build a `log.entry` payload: `level` (error/warn/info/debug/trace as
/// traced by the app), the originating `target` (module path) and the
/// `message` — home-prefix-stripped + truncated to
/// [`MAX_LOG_MESSAGE_CHARS`].
pub fn log_entry_payload(level: &str, target: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "level": level,
        "target": target,
        "message": sanitize_message(message, MAX_LOG_MESSAGE_CHARS),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_event() -> TelemetryEvent {
        TelemetryEvent::new(EventType::TaskCompleted, serde_json::json!({"task_type": "scan_folder"}))
            .with_envelope("client-123", "1.1.0-dev+abc123", "macos")
    }

    #[test]
    fn event_type_wire_names_are_dotted() {
        assert_eq!(EventType::TaskStarted.as_str(), "task.started");
        assert_eq!(EventType::TaskCompleted.as_str(), "task.completed");
        assert_eq!(EventType::TaskFailed.as_str(), "task.failed");
        assert_eq!(EventType::ScanCompleted.as_str(), "scan.completed");
        assert_eq!(EventType::DownloadStarted.as_str(), "download.started");
        assert_eq!(EventType::DownloadCompleted.as_str(), "download.completed");
        assert_eq!(EventType::DownloadFailed.as_str(), "download.failed");
        assert_eq!(EventType::AppUpdated.as_str(), "app.updated");
        assert_eq!(EventType::ErrorReported.as_str(), "error.reported");
        assert_eq!(EventType::UiViewOpened.as_str(), "ui.view.opened");
        assert_eq!(EventType::UiActionScanFolder.as_str(), "ui.action.scan_folder");
        assert_eq!(EventType::UiActionRunBackup.as_str(), "ui.action.run_backup");
        assert_eq!(EventType::UiActionRestoreDump.as_str(), "ui.action.restore_dump");
        assert_eq!(EventType::UiActionTraktorImport.as_str(), "ui.action.traktor_import");
        assert_eq!(
            EventType::UiActionRecomputeEmbeddings.as_str(),
            "ui.action.recompute_embeddings"
        );
        assert_eq!(
            EventType::UiActionDeemixEnqueue.as_str(),
            "ui.action.deemix_enqueue"
        );
        assert_eq!(EventType::LogEntry.as_str(), "log.entry");
    }

    #[test]
    fn ui_family_flag() {
        assert!(EventType::UiViewOpened.is_ui());
        assert!(EventType::UiActionScanFolder.is_ui());
        assert!(EventType::UiActionRunBackup.is_ui());
        assert!(EventType::UiActionRestoreDump.is_ui());
        assert!(EventType::UiActionTraktorImport.is_ui());
        assert!(EventType::UiActionRecomputeEmbeddings.is_ui());
        assert!(EventType::UiActionDeemixEnqueue.is_ui());
        // Core + log types are not ui.*
        assert!(!EventType::LogEntry.is_ui());
        assert!(!EventType::TaskStarted.is_ui());
        assert!(!EventType::ScanCompleted.is_ui());
    }

    #[test]
    fn ui_and_log_types_roundtrip_serialization() {
        for (r#type, payload) in [
            (EventType::UiViewOpened, serde_json::json!({"view": "dashboard"})),
            (EventType::UiActionScanFolder, serde_json::json!({"ok": true})),
            (EventType::UiActionRunBackup, serde_json::json!({"ok": false, "error_message": "no path"})),
            (EventType::UiActionRestoreDump, serde_json::json!({"ok": true})),
            (EventType::UiActionTraktorImport, serde_json::json!({"ok": true})),
            (EventType::UiActionRecomputeEmbeddings, serde_json::json!({"ok": true})),
            (EventType::UiActionDeemixEnqueue, serde_json::json!({"ok": false})),
            (EventType::LogEntry, serde_json::json!({"level": "warn", "target": "mmm::db", "message": "disk full"})),
        ] {
            let event = TelemetryEvent::new(r#type, payload).with_envelope("client-1", "1.3.0-test", "linux");
            let json = serde_json::to_string(&event).unwrap();
            let back: TelemetryEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back.r#type, r#type);
            assert!(back.is_valid());
            assert!(json.contains(&format!("\"type\":\"{}\"", r#type.as_str())));
        }
    }

    #[test]
    fn unknown_event_type_is_still_rejected() {
        // Existing unknown core-ish type.
        let json = r#"{"event_id":"00000000-0000-4000-8000-000000000000","client_id":"c","app_version":"1","os":"macos","ts":"2026-09-01T00:00:00Z","type":"ui.clicked","payload":{}}"#;
        assert!(serde_json::from_str::<TelemetryEvent>(json).is_err());
        // New-style unknowns are rejected too (no open `ui.action` string).
        let json = r#"{"event_id":"00000000-0000-4000-8000-000000000000","client_id":"c","app_version":"1","os":"macos","ts":"2026-09-01T00:00:00Z","type":"ui.action.delete_everything","payload":{}}"#;
        assert!(serde_json::from_str::<TelemetryEvent>(json).is_err());
        let json = r#"{"event_id":"00000000-0000-4000-8000-000000000000","client_id":"c","app_version":"1","os":"macos","ts":"2026-09-01T00:00:00Z","type":"log.error","payload":{}}"#;
        assert!(serde_json::from_str::<TelemetryEvent>(json).is_err());
    }

    #[test]
    fn is_valid_accepts_new_full_events() {
        let e = TelemetryEvent::new(EventType::LogEntry, log_entry_payload("warn", "mmm::db", "hi"))
            .with_envelope("client-1", "1.3.0-test", "linux");
        assert!(e.is_valid());
        let e = TelemetryEvent::new(EventType::UiViewOpened, serde_json::json!({"view": "files"}))
            .with_envelope("client-1", "1.3.0-test", "linux");
        assert!(e.is_valid());
    }

    #[test]
    fn roundtrip_serialization() {
        let event = full_event();
        let json = serde_json::to_string(&event).unwrap();
        let back: TelemetryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_id, event.event_id);
        assert_eq!(back.r#type, EventType::TaskCompleted);
        assert_eq!(back.payload["task_type"], "scan_folder");
        assert!(json.contains("\"type\":\"task.completed\""));
    }

    #[test]
    fn unknown_event_type_is_rejected() {
        let json = r#"{"event_id":"00000000-0000-4000-8000-000000000000","client_id":"c","app_version":"1","os":"macos","ts":"2026-09-01T00:00:00Z","type":"ui.clicked","payload":{}}"#;
        assert!(serde_json::from_str::<TelemetryEvent>(json).is_err());
    }

    #[test]
    fn view_payload_accepts_valid_ids() {
        for view in ["dashboard", "tag-categories", "dynamic-bundles", "file-detail", "a1"] {
            let p = view_payload(view).unwrap();
            assert_eq!(p["view"], view);
        }
    }

    #[test]
    fn view_payload_rejects_invalid_ids() {
        for view in ["", "Files", "files?x=1", "../files", "files space", "über", "a".repeat(65).as_str()] {
            assert!(view_payload(view).is_none(), "{view:?} must be rejected");
        }
    }

    #[test]
    fn valid_view_id_rules() {
        assert!(valid_view_id("dashboard"));
        assert!(valid_view_id("tag-categories"));
        assert!(valid_view_id("0abc"));
        assert!(!valid_view_id(""));
        assert!(!valid_view_id("Dashboard"));
        assert!(!valid_view_id("a/b"));
        assert!(!valid_view_id("has_space"));
        assert!(!valid_view_id("has_underscore"));
        assert!(!valid_view_id(&"x".repeat(65)));
    }

    #[test]
    fn action_payload_ok_and_error() {
        let p = action_payload(true, None);
        assert_eq!(p["ok"], true);
        assert!(p.get("error_message").is_none());

        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy().to_string();
        let p = action_payload(false, Some(&format!("boom at {home_str}/x")));
        assert_eq!(p["ok"], false);
        let msg = p["error_message"].as_str().unwrap();
        assert!(!msg.contains(&home_str));
        assert!(msg.contains("~/x"));
    }

    #[test]
    fn action_payload_truncates_error_message() {
        let long = "e".repeat(MAX_ERROR_MESSAGE_CHARS * 2);
        let p = action_payload(false, Some(&long));
        assert_eq!(
            p["error_message"].as_str().unwrap().chars().count(),
            MAX_ERROR_MESSAGE_CHARS
        );
    }

    #[test]
    fn log_entry_payload_shape_and_strip() {
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy().to_string();
        let p = log_entry_payload("warn", "momos_music_manager::db", &format!("failed at {home_str}/Music/x.flac"));
        assert_eq!(p["level"], "warn");
        assert_eq!(p["target"], "momos_music_manager::db");
        let msg = p["message"].as_str().unwrap();
        assert!(!msg.contains(&home_str));
        assert!(msg.contains("~/Music/x.flac"));
    }

    #[test]
    fn log_entry_payload_truncates_message() {
        let long = "m".repeat(MAX_LOG_MESSAGE_CHARS * 2);
        let p = log_entry_payload("info", "t", &long);
        assert_eq!(
            p["message"].as_str().unwrap().chars().count(),
            MAX_LOG_MESSAGE_CHARS
        );
    }

    #[test]
    fn log_entry_payload_keeps_short_messages() {
        let p = log_entry_payload("error", "t", "disk full");
        assert_eq!(p["message"], "disk full");
    }

    #[test]
    fn log_entry_payload_caps_are_independent_of_error_messages() {
        // error_message cap (500) must not leak into log entries (1000).
        let mid = "m".repeat(700);
        let p = log_entry_payload("warn", "t", &mid);
        assert_eq!(p["message"].as_str().unwrap().chars().count(), 700);
    }

    #[test]
    fn new_event_has_unique_ids_and_current_ts() {
        let a = TelemetryEvent::new(EventType::TaskStarted, serde_json::json!({}));
        let b = TelemetryEvent::new(EventType::TaskStarted, serde_json::json!({}));
        assert_ne!(a.event_id, b.event_id);
        assert!(parse_ts(&a.ts).is_some());
        assert_eq!(a.client_id, "");
    }

    #[test]
    fn is_valid_accepts_full_event() {
        assert!(full_event().is_valid());
    }

    #[test]
    fn is_valid_rejects_bad_fields() {
        let mut e = full_event();
        e.event_id = "not-a-uuid".to_string();
        assert!(!e.is_valid());

        let mut e = full_event();
        e.client_id = "../etc/passwd".to_string();
        assert!(!e.is_valid());

        let mut e = full_event();
        e.os = "beos".to_string();
        assert!(!e.is_valid());

        let mut e = full_event();
        e.ts = "yesterday".to_string();
        assert!(!e.is_valid());

        let mut e = full_event();
        e.payload = serde_json::json!([1, 2, 3]); // must be an object
        assert!(!e.is_valid());
    }

    #[test]
    fn is_valid_rejects_oversized_payload() {
        let mut e = full_event();
        let big = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        e.payload = serde_json::json!({ "blob": big });
        assert!(!e.is_valid());
    }

    #[test]
    fn batch_validate_rejects_duplicate_event_ids() {
        let ev = full_event();
        let dup = ev.clone();
        let batch = EventBatch::new("client-123", vec![ev, dup]);
        let err = batch.validate().unwrap_err();
        assert!(err.contains("duplicate event_id"), "got: {err}");
    }

    #[test]
    fn batch_validate_rejects_oversized_batch() {
        let events: Vec<TelemetryEvent> = (0..MAX_BATCH_EVENTS + 1)
            .map(|_| full_event())
            .collect();
        let batch = EventBatch::new("client-123", events);
        let err = batch.validate().unwrap_err();
        assert!(err.contains("batch too large"), "got: {err}");
    }

    #[test]
    fn batch_validate_rejects_empty_and_bad_envelope() {
        let batch = EventBatch::new("client-123", vec![]);
        assert!(batch.validate().unwrap_err().contains("empty"));

        let batch = EventBatch::new("bad client!", vec![full_event()]);
        assert!(batch.validate().unwrap_err().contains("client_id"));

        let mut batch = EventBatch::new("client-123", vec![full_event()]);
        batch.sent_at = "not-a-date".to_string();
        assert!(batch.validate().unwrap_err().contains("sent_at"));
    }

    #[test]
    fn batch_validate_ok() {
        let batch = EventBatch::new("client-123", vec![full_event()]);
        assert!(batch.validate().is_ok());
    }

    #[test]
    fn sanitize_strips_home_prefix() {
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy().to_string();
        let msg = format!("failed to open {home_str}/Music/file.flac");
        let cleaned = sanitize_error_message(&msg);
        assert!(!cleaned.contains(&home_str));
        assert!(cleaned.contains("~/Music/file.flac"));
    }

    #[test]
    fn sanitize_truncates_long_messages() {
        let msg = "e".repeat(MAX_ERROR_MESSAGE_CHARS * 2);
        let cleaned = sanitize_error_message(&msg);
        assert_eq!(cleaned.chars().count(), MAX_ERROR_MESSAGE_CHARS);
    }

    #[test]
    fn sanitize_keeps_short_messages() {
        let msg = "disk full";
        assert_eq!(sanitize_error_message(msg), msg);
    }

    #[test]
    fn error_payload_sanitizes() {
        let payload = error_payload("boom at /Users/x");
        assert!(payload["error_message"].as_str().unwrap().starts_with("boom at"));
    }

    #[test]
    fn valid_client_id_rules() {
        assert!(valid_client_id("3f2a1b2c-1234-5678-9abc-def012345678"));
        assert!(valid_client_id("music-server_1"));
        assert!(!valid_client_id(""));
        assert!(!valid_client_id("a/b"));
        assert!(!valid_client_id("../etc"));
        assert!(!valid_client_id("has space"));
    }

    #[test]
    fn parse_ts_accepts_rfc3339_utc() {
        assert!(parse_ts("2026-09-01T00:41:59Z").is_some());
        assert!(parse_ts("2026-09-01T02:41:59+02:00").is_some());
        assert!(parse_ts("01.09.2026").is_none());
    }
}
