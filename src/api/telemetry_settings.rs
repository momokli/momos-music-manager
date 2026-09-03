//! Telemetry settings — the Settings-page surface of the telemetry client
//! (snapshot push + event pipeline).
//!
//! Endpoints (all under `/api/telemetry-settings`, kept separate from the
//! `/api/telemetry/*` collector namespace of the receiver server):
//!
//! - `GET  /api/telemetry-settings/status`   — effective values + sources +
//!   last-push state + CLI-link state
//! - `POST /api/telemetry-settings/settings` — persist `[telemetry]` into
//!   config.toml (409 when a field is pinned by an env var)
//! - `POST /api/telemetry-settings/push`     — one-shot `telemetry push`
//!   (same code path as the CLI) + status record
//!
//! Precedence stays **Env > TOML > Defaults** (config.toml is the
//! persistence target — no separate UI/DB layer like the autoupdater):
//!
//! - `MOMOS_TELEMETRY_ENABLED` / `[telemetry] enabled` / default `false`
//! - `MOMOS_TELEMETRY_BASE_URL` / `[telemetry] base_url` / default none
//! - `MOMOS_TELEMETRY_TOKEN` / `[telemetry] token` / default none
//! - `MOMOS_TELEMETRY_INSTANCE` / `[telemetry] instance` / default `"macbook"`
//! - `MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS` / `[telemetry]
//!   full_db_interval_secs` / default `0` (legacy `MOMOS_TELEMETRY_INTERVAL_SECS`
//!   + `[telemetry] interval_secs` stay effective as aliases and are retired
//!   when the UI writes the explicit key)
//!
//! A field whose source is `"env"` is pinned: the UI disables the control
//! and a write attempt returns 409. Everything else is editable — the UI
//! writes straight into the `[telemetry]` section (line-preserving,
//! comments and unrelated sections survive, see
//! [`crate::config::update_telemetry_toml`]).
//!
//! The background loops (periodic full-DB push, event pipeline) read their
//! settings at startup; changes made here apply to them **after the next
//! restart** — the status response and the push button always use a fresh
//! [`ServiceCredentials::load`], so a manual "Push now" works immediately.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use sqlx::{Pool, Sqlite};

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::cli_link;
use crate::config::{
    ServiceCredentials, TelemetryTomlError, TelemetryTomlPatch, update_telemetry_toml,
};
use crate::db::settings::{self, KEY_TELEMETRY_LAST_PUSH_AT};

/// Failure of a settings write.
#[derive(Debug)]
pub(crate) enum TelemetrySettingsError {
    /// The field is pinned by an environment variable — nothing was written.
    Overridden(&'static str),
    /// config.toml could not be updated.
    Toml(TelemetryTomlError),
    /// settings-KV read failed.
    Db(sqlx::Error),
}

impl std::fmt::Display for TelemetrySettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TelemetrySettingsError::Overridden(env) => write!(
                f,
                "pinned by the environment variable {env} — change it there to edit this setting"
            ),
            TelemetrySettingsError::Toml(e) => write!(f, "{e}"),
            TelemetrySettingsError::Db(e) => write!(f, "{e}"),
        }
    }
}

/// Body of `POST /api/telemetry-settings/settings` — at least one field
/// must be present. `baseUrl`/`token`/`instance` as `""` clear the key.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySettingsRequest {
    pub enabled: Option<bool>,
    pub base_url: Option<String>,
    pub token: Option<String>,
    pub instance: Option<String>,
    pub full_db_interval_secs: Option<u64>,
}

/// Status JSON — effective values (Env > TOML > Defaults) + where each
/// value comes from + last-push state + CLI-link state.
pub(crate) fn telemetry_status_json(config: &ServiceCredentials) -> serde_json::Value {
    let cli = cli_link::status();
    serde_json::json!({
        "currentVersion": env!("MMM_VERSION"),
        // Effective values + sources (env > toml > default).
        "enabled": config.telemetry_enabled,
        "enabledSource": config.telemetry_enabled_source(),
        "baseUrl": config.telemetry_base_url,
        "baseUrlSource": config.telemetry_base_url_source(),
        "token": config.telemetry_token,
        "tokenSource": config.telemetry_token_source(),
        "instance": config.telemetry_instance,
        "instanceSource": config.telemetry_instance_source(),
        "fullDbIntervalSecs": config.telemetry_effective_full_db_interval(),
        "fullDbIntervalSource": config.telemetry_interval_source(),
        // Derived, read-only.
        "eventsEndpoint": config.telemetry_events_endpoint,
        "periodicPushActive": config.telemetry_enabled
            && config.telemetry_effective_full_db_interval() > 0,
        // CLI-link state (macOS .app installs).
        "cli": {
            "supported": cli.supported,
            "reason": cli.reason,
            "linkPath": cli.link_path,
            "targetPath": cli.target_path,
        },
    })
}

/// Merge the last-push KV state into the status JSON.
async fn with_last_push(
    mut status: serde_json::Value,
    db: &Pool<Sqlite>,
) -> Result<serde_json::Value, sqlx::Error> {
    let last_push_at = settings::get_setting(db, KEY_TELEMETRY_LAST_PUSH_AT)
        .await?
        .and_then(|v| v.parse::<i64>().ok());
    let last_push_status = settings::get_setting(db, settings::KEY_TELEMETRY_LAST_PUSH_STATUS).await?;
    let last_push_error = settings::get_setting(db, settings::KEY_TELEMETRY_LAST_PUSH_ERROR)
        .await?
        .filter(|v| !v.is_empty());
    if let Some(obj) = status.as_object_mut() {
        obj.insert("lastPushAt".into(), last_push_at.into());
        obj.insert("lastPushStatus".into(), last_push_status.into());
        obj.insert("lastPushError".into(), last_push_error.into());
    }
    Ok(status)
}

/// Build the full status response (config + last-push KV state).
pub(crate) async fn build_status_json(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<serde_json::Value, sqlx::Error> {
    with_last_push(telemetry_status_json(config), db).await
}

/// Pin pre-checks: refuse to write any field whose *effective* value is
/// pinned by an environment variable (Env > TOML > Defaults — a TOML
/// value is editable because the UI persists into exactly that file).
/// Pure (no I/O) so the override matrix is unit-testable; the env state is
/// read from the process environment like the source helpers do.
pub(crate) fn check_env_pins(
    current: &ServiceCredentials,
    patch: &TelemetryTomlPatch,
) -> Result<(), TelemetrySettingsError> {
    for (field, env, pinned) in [
        (
            "telemetry.enabled",
            "MOMOS_TELEMETRY_ENABLED",
            patch.enabled.is_some() && current.telemetry_enabled_source() == "env",
        ),
        (
            "telemetry.base_url",
            "MOMOS_TELEMETRY_BASE_URL",
            patch.base_url.is_some() && current.telemetry_base_url_source() == "env",
        ),
        (
            "telemetry.token",
            "MOMOS_TELEMETRY_TOKEN",
            patch.token.is_some() && current.telemetry_token_source() == "env",
        ),
        (
            "telemetry.instance",
            "MOMOS_TELEMETRY_INSTANCE",
            patch.instance.is_some() && current.telemetry_instance_source() == "env",
        ),
        (
            "telemetry.full_db_interval_secs",
            "MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS / MOMOS_TELEMETRY_INTERVAL_SECS",
            patch.full_db_interval_secs.is_some() && current.telemetry_interval_source() == "env",
        ),
    ] {
        if pinned {
            tracing::info!("telemetry settings: {field} pinned by {env} — write refused");
            return Err(TelemetrySettingsError::Overridden(env));
        }
    }
    Ok(())
}

/// Persist a settings patch into `[telemetry]` of config.toml (env-pinned
/// fields are refused first, see [`check_env_pins`]).
///
/// Returns the *fresh* effective status: after a successful write the
/// response re-reads config.toml + env (`ServiceCredentials::load`), so the
/// UI always sees the true Env > TOML > Defaults result.
pub(crate) fn apply_settings_patch(
    current: &ServiceCredentials,
    patch: &TelemetryTomlPatch,
) -> Result<serde_json::Value, TelemetrySettingsError> {
    // Pin pre-checks first — a request that would partially fail writes
    // nothing (a field pinned by env must not be persisted).
    check_env_pins(current, patch)?;

    let path = update_telemetry_toml(patch).map_err(TelemetrySettingsError::Toml)?;
    tracing::info!("telemetry settings: [telemetry] section updated in {}", path.display());

    // Fresh effective view (Env > TOML > Defaults over the updated file).
    let fresh = ServiceCredentials::load();
    let mut status = telemetry_status_json(&fresh);
    if let Some(obj) = status.as_object_mut() {
        obj.insert("writtenTo".into(), path.display().to_string().into());
    }
    Ok(status)
}

/// One-shot push outcome — always HTTP 200 so the Settings button can show
/// success/error inline (fetchJSON treats non-2xx as thrown errors).
async fn push_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Fresh config: the background loops may still run on the boot
    // snapshot, but the manual push must honor the *current* file/env state
    // (a push right after enabling telemetry has to work).
    let config = ServiceCredentials::load();

    let outcome = if !config.telemetry_enabled {
        Err("Telemetry is disabled — enable it in the settings above to push".to_string())
    } else {
        match crate::telemetry::push_once(&state.db, &config).await {
            Ok(()) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    };

    crate::telemetry::record_push_result(&state.db, outcome.clone()).await;

    let data = match outcome {
        Ok(()) => serde_json::json!({
            "ok": true,
            "message": "Push succeeded",
            "pushedAt": chrono::Utc::now().timestamp(),
        }),
        Err(message) => serde_json::json!({
            "ok": false,
            "message": message,
        }),
    };
    Json(ApiResponse { data }).into_response()
}

/// GET /api/telemetry-settings/status
async fn status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Fresh config so the card always shows the file state (it is the
    // persistence target of this page).
    let config = ServiceCredentials::load();
    match build_status_json(&config, &state.db).await {
        Ok(data) => Json(ApiResponse { data }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/telemetry-settings/settings
async fn settings_handler(
    State(state): State<Arc<AppState>>,
    body: Option<Json<TelemetrySettingsRequest>>,
) -> impl IntoResponse {
    let Some(Json(req)) = body else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid body — expected { \"enabled\": true|false, \"baseUrl\": \"…\", \"token\": \"…\", \"instance\": \"…\", \"fullDbIntervalSecs\": <seconds> }".into(),
            }),
        )
            .into_response();
    };
    if req.enabled.is_none()
        && req.base_url.is_none()
        && req.token.is_none()
        && req.instance.is_none()
        && req.full_db_interval_secs.is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid body — provide at least one of \"enabled\", \"baseUrl\", \"token\", \"instance\" or \"fullDbIntervalSecs\"".into(),
            }),
        )
            .into_response();
    }

    let patch = TelemetryTomlPatch {
        enabled: req.enabled,
        base_url: req.base_url,
        token: req.token,
        instance: req.instance,
        full_db_interval_secs: req.full_db_interval_secs,
    };
    // Fresh current state (env + file as of now) for the pin checks.
    let current = ServiceCredentials::load();
    match apply_settings_patch(&current, &patch) {
        Ok(data) => Json(ApiResponse { data }).into_response(),
        Err(TelemetrySettingsError::Overridden(env)) => (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: TelemetrySettingsError::Overridden(env).to_string(),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Router ─────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/telemetry-settings/status", get(status_handler))
        .route("/api/telemetry-settings/settings", post(settings_handler))
        .route("/api/telemetry-settings/push", post(push_handler))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All module tests live in ONE test fn: several of them read/mutate
    /// the process-global `MOMOS_TELEMETRY_*` environment (source helpers
    /// read it live), and parallel tests inside one binary would race.
    /// The env lock (`config::tests::ENV_LOCK`) serializes against the
    /// config.rs source-helper tests.
    #[test]
    fn telemetry_settings_matrix() {
        let _env_guard = crate::config::tests::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // ── Status JSON: defaults are all off ──
        let c = ServiceCredentials::defaults_for_test();
        let json = telemetry_status_json(&c);
        assert_eq!(json["enabled"], false);
        assert_eq!(json["enabledSource"], "default");
        assert_eq!(json["baseUrl"], serde_json::Value::Null);
        assert_eq!(json["baseUrlSource"], "default");
        assert_eq!(json["token"], serde_json::Value::Null);
        assert_eq!(json["instance"], "macbook");
        assert_eq!(json["instanceSource"], "default");
        assert_eq!(json["fullDbIntervalSecs"], 0);
        assert_eq!(json["fullDbIntervalSource"], "default");
        assert_eq!(json["periodicPushActive"], false);
        assert_eq!(json["eventsEndpoint"], serde_json::Value::Null);
        assert!(json["cli"].is_object());
        assert_eq!(json["currentVersion"], env!("MMM_VERSION"));

        // ── Status JSON: resolved endpoint + effective interval ──
        let c = {
            let mut c = ServiceCredentials::defaults_for_test();
            c.telemetry_base_url = Some("https://collector.example/".into());
            c.telemetry_events_endpoint =
                Some("https://collector.example/api/telemetry".into());
            c.telemetry_enabled = true;
            c.telemetry_full_db_interval_secs = 3600;
            c
        };
        let json = telemetry_status_json(&c);
        assert_eq!(json["baseUrl"], "https://collector.example/");
        assert_eq!(
            json["eventsEndpoint"],
            "https://collector.example/api/telemetry"
        );
        assert_eq!(json["fullDbIntervalSecs"], 3600);
        assert_eq!(json["periodicPushActive"], true);

        // ── Pin checks: nothing pinned on a clean process env ──
        let patch = TelemetryTomlPatch {
            enabled: Some(true),
            base_url: Some("https://collector.example".into()),
            token: Some("secret".into()),
            instance: Some("studio".into()),
            full_db_interval_secs: Some(3600),
            ..Default::default()
        };
        assert!(check_env_pins(&c, &patch).is_ok());
        // An empty patch passes even with env vars set (nothing to write).
        unsafe { std::env::set_var("MOMOS_TELEMETRY_ENABLED", "true") };
        assert!(check_env_pins(&c, &TelemetryTomlPatch::default()).is_ok());
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_ENABLED") };

        // ── Pin checks: env-pinned fields are refused, one by one ──
        unsafe { std::env::set_var("MOMOS_TELEMETRY_ENABLED", "true") };
        assert!(matches!(
            check_env_pins(&c, &patch),
            Err(TelemetrySettingsError::Overridden("MOMOS_TELEMETRY_ENABLED"))
        ));
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_ENABLED") };

        unsafe { std::env::set_var("MOMOS_TELEMETRY_BASE_URL", "https://env.example") };
        assert!(matches!(
            check_env_pins(&c, &patch),
            Err(TelemetrySettingsError::Overridden("MOMOS_TELEMETRY_BASE_URL"))
        ));
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_BASE_URL") };

        unsafe { std::env::set_var("MOMOS_TELEMETRY_TOKEN", "t") };
        assert!(matches!(
            check_env_pins(&c, &patch),
            Err(TelemetrySettingsError::Overridden("MOMOS_TELEMETRY_TOKEN"))
        ));
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_TOKEN") };

        unsafe { std::env::set_var("MOMOS_TELEMETRY_INSTANCE", "macbook") };
        assert!(matches!(
            check_env_pins(&c, &patch),
            Err(TelemetrySettingsError::Overridden("MOMOS_TELEMETRY_INSTANCE"))
        ));
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_INSTANCE") };

        unsafe { std::env::set_var("MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS", "3600") };
        assert!(matches!(
            check_env_pins(&c, &patch),
            Err(TelemetrySettingsError::Overridden(
                "MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS / MOMOS_TELEMETRY_INTERVAL_SECS"
            ))
        ));
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS") };

        unsafe { std::env::set_var("MOMOS_TELEMETRY_INTERVAL_SECS", "3600") };
        assert!(matches!(
            check_env_pins(&c, &patch),
            Err(TelemetrySettingsError::Overridden(
                "MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS / MOMOS_TELEMETRY_INTERVAL_SECS"
            ))
        ));
        unsafe { std::env::remove_var("MOMOS_TELEMETRY_INTERVAL_SECS") };
    }
}
