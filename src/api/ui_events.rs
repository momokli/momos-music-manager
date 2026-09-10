//! Internal UI-events surface (telemetry full package, plan E2/E4).
//!
//! Two kinds of producers feed `ui.*` events into the existing pipeline:
//!
//! - **Client-side view opens**: the SPA posts `{ "type": "ui.view.opened",
//!   "payload": { "view": "<page_id>" } }` to `POST /api/ui-events`
//!   (fire-and-forget from `frontend/shared/ui-events.js`). The endpoint
//!   maps ONLY `ui.*` wire types onto [`EventType`] and feeds the pipeline.
//!   Response is **always 204** — off flag, no pipeline, unknown type or
//!   invalid payload never produce a 4xx (the JS hook must not create an
//!   error flood; symmetric with the no-op philosophy of `emit()`).
//! - **Server-side action emission**: the six user-triggered POST handlers
//!   (scan folder, run backup, restore dump, traktor import, recompute
//!   embeddings, deemix enqueue) call [`emit_action`] exactly once per
//!   request, with `ok` taken from their response decision.
//!
//! Both paths are gated by the boot-time `[telemetry] ui_events_enabled`
//! flag (changes apply after the next restart, like the other background
//! loops); `emit_event` is a no-op while the pipeline is not running.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::post,
};
use std::sync::Arc;

use crate::AppState;
use crate::telemetry::emit;
use crate::telemetry::events::{EventType, action_payload, view_payload};

/// Map a wire-type string onto [`EventType`]; `None` for unknown types or
/// anything outside the `ui.*` family (the endpoint must never accept
/// `log.entry` — that path is owned by the log-shipping layer).
pub fn event_type_from_wire(type_str: &str) -> Option<EventType> {
    let value = serde_json::Value::String(type_str.to_string());
    let event_type: EventType = serde_json::from_value(value).ok()?;
    if event_type.is_ui() {
        Some(event_type)
    } else {
        None
    }
}

/// Gate shared by the endpoint + handler emissions: UI events only flow
/// when the user opted in via `[telemetry] ui_events_enabled` (boot-time
/// config — changes apply after the next restart). `emit_event` itself
/// covers the "pipeline not running" case (process-wide no-op).
fn ui_events_allowed(state: &AppState) -> bool {
    state.config.telemetry_ui_events_enabled
}

/// Emit one `ui.action.*` event from a user-triggered handler (exactly one
/// call per request, after the response decision). No-op when the flag is
/// off or no pipeline is running — never blocks, never fails the request.
pub(crate) fn emit_action(
    state: &AppState,
    r#type: EventType,
    ok: bool,
    error_message: Option<&str>,
) {
    debug_assert!(r#type.is_ui() && r#type != EventType::UiViewOpened);
    if !ui_events_allowed(state) {
        return;
    }
    emit::emit_event(r#type, action_payload(ok, error_message));
}

/// POST /api/ui-events — fire-and-forget view/action ingestion. Always 204
/// (the raw body is read manually so malformed JSON can never produce a
/// 4xx from an extractor rejection).
async fn ui_events_handler(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
) -> StatusCode {
    // Off flag → silent 204 (never 4xx).
    if !ui_events_allowed(&state) {
        return StatusCode::NO_CONTENT;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), 64 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::NO_CONTENT,
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return StatusCode::NO_CONTENT;
    };
    let Some(type_str) = json.get("type").and_then(|v| v.as_str()) else {
        return StatusCode::NO_CONTENT;
    };
    let Some(event_type) = event_type_from_wire(type_str) else {
        return StatusCode::NO_CONTENT;
    };
    let Some(payload) = json.get("payload") else {
        return StatusCode::NO_CONTENT;
    };
    if !payload.is_object() {
        return StatusCode::NO_CONTENT;
    }
    // `ui.view.opened` payloads carry a validated view id — invalid ids are
    // dropped silently (garbage never reaches the wire).
    if event_type == EventType::UiViewOpened {
        let Some(view) = payload.get("view").and_then(|v| v.as_str()) else {
            return StatusCode::NO_CONTENT;
        };
        if view_payload(view).is_none() {
            return StatusCode::NO_CONTENT;
        }
    }
    // No pipeline → emit is a no-op; the response stays 204 either way.
    emit::emit_event(event_type, payload.clone());
    StatusCode::NO_CONTENT
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/ui-events", post(ui_events_handler))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-mapping accepts the whole `ui.*` family, nothing else.
    #[test]
    fn wire_mapping_accepts_only_ui_types() {
        assert_eq!(
            event_type_from_wire("ui.view.opened"),
            Some(EventType::UiViewOpened)
        );
        assert_eq!(
            event_type_from_wire("ui.action.scan_folder"),
            Some(EventType::UiActionScanFolder)
        );
        assert_eq!(
            event_type_from_wire("ui.action.run_backup"),
            Some(EventType::UiActionRunBackup)
        );
        assert_eq!(
            event_type_from_wire("ui.action.restore_dump"),
            Some(EventType::UiActionRestoreDump)
        );
        assert_eq!(
            event_type_from_wire("ui.action.traktor_import"),
            Some(EventType::UiActionTraktorImport)
        );
        assert_eq!(
            event_type_from_wire("ui.action.recompute_embeddings"),
            Some(EventType::UiActionRecomputeEmbeddings)
        );
        assert_eq!(
            event_type_from_wire("ui.action.deemix_enqueue"),
            Some(EventType::UiActionDeemixEnqueue)
        );
        // Unknown + non-ui types are rejected (no open strings, no log.entry
        // through the user-facing endpoint).
        assert_eq!(event_type_from_wire("ui.action.delete_everything"), None);
        assert_eq!(event_type_from_wire("log.entry"), None);
        assert_eq!(event_type_from_wire("task.completed"), None);
        assert_eq!(event_type_from_wire("bogus"), None);
    }
}
