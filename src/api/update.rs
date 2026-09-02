//! Update controls — the Phase A+B web surface for the M6 autoupdater,
//! extended with the channel select (Phase C-1).
//!
//! Endpoints:
//! - `GET  /api/update/status`   — version/channel/enabled + last check + pending marker
//! - `POST /api/update/check`    — run a (verified) check against the *selected* channel
//! - `POST /api/update/settings` — persist the auto-update toggle and/or the update channel
//! - `POST /api/update/apply`    — manual "update now" (swap on Linux/Windows,
//!                                 verified download + instructions on macOS)
//!
//! Precedence rules (both **Env > UI (DB setting) > TOML > Default**):
//! - enabled default `true`;
//! - channel default = embedded channel of the running build (dev build →
//!   `rolling`, release build → `release`).
//! `check`/`apply` run against the effective (selected) channel; an explicit
//! cross-channel switch is not an error — the mismatch guard only fires when
//! the update source serves the *other* channel than selected.

use std::path::Path;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::autoupdate::{
    ApplyOutcome, HttpFetcher, UpdateChannel, UpdateError, UpdateSettings, UpdateStatus, swap,
    verify::Fetcher, UPDATE_CHANNEL_RELEASE, UPDATE_CHANNEL_ROLLING,
};
use crate::config::ServiceCredentials;
use crate::db::settings::{self, KEY_AUTOUPDATE_ENABLED};

// ── Response types (camelCase on the wire) ────────────────────────────────

/// `artifact` object of the status response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInfo {
    pub os_arch: String,
    pub ext: String,
}

/// Persisted JSON of the last check (`autoupdate.last_check_result`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LastCheckResult {
    /// `upToDate` | `updateAvailable` | `channelMismatch` | `disabled`
    /// | `unsupportedPlatform` | `error`
    pub state: String,
    pub available_version: Option<String>,
    pub current_version: Option<String>,
    pub artifact_name: Option<String>,
}

/// `pendingUpdate` object (from `update-state.json` next to the binary).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpdateInfo {
    pub old_version: String,
    pub new_version: String,
    pub committed: bool,
}

// ── Pure logic (unit-tested; handlers stay thin) ──────────────────────────

/// `"dev"` when the version carries a pre-release tag, else `"release"` —
/// classifies the *embedded* build channel (not the selectable update
/// channel, see [`effective_autoupdate_channel`]).
pub fn channel_for_version(version: &str) -> &'static str {
    match semver::Version::parse(version) {
        Ok(v) if !v.pre.is_empty() => "dev",
        _ => "release",
    }
}

/// Effective update channel per the precedence rule
/// **Env > UI (DB) > TOML > Default = embedded channel of the running
/// build** (dev build → `rolling`, release build → `release`).
///
/// - `MOMOS_AUTOUPDATE_CHANNEL` set *and* parseable (`"rolling"`/`"release"`)
///   wins (`"env"`);
/// - otherwise a persisted UI value `settings['autoupdate.channel']`
///   (`"ui"`);
/// - otherwise the config value — TOML `[autoupdate] channel` or the
///   embedded default (`"toml"`/`"default"`). Unparseable env values fall
///   through, mirroring the enabled-value rule.
pub async fn effective_autoupdate_channel(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<(UpdateChannel, &'static str), sqlx::Error> {
    effective_autoupdate_channel_with_env(
        config,
        db,
        std::env::var("MOMOS_AUTOUPDATE_CHANNEL").ok(),
    )
    .await
}

/// Testable core of [`effective_autoupdate_channel`] — the env value is
/// injected so the precedence matrix can be unit-tested without mutating the
/// process-global environment.
pub(crate) async fn effective_autoupdate_channel_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    env_value: Option<String>,
) -> Result<(UpdateChannel, &'static str), sqlx::Error> {
    if let Some(v) = env_value.and_then(|v| UpdateChannel::parse(&v)) {
        return Ok((v, "env"));
    }
    if let Some(raw) = settings::get_setting(db, settings::KEY_AUTOUPDATE_CHANNEL).await? {
        let channel = UpdateChannel::parse(&raw).ok_or_else(|| {
            sqlx::Error::Decode(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "invalid stored channel setting `{raw}` (expected \"rolling\" or \"release\")"
                ),
            )))
        })?;
        return Ok((channel, "ui"));
    }
    if config.autoupdate_channel_source() == "toml" {
        return Ok((config.configured_autoupdate_channel(), "toml"));
    }
    Ok((
        config.configured_autoupdate_channel(),
        "default",
    ))
}
/// Effective auto-update enablement per the precedence rule
/// **Env > UI (DB) > TOML > Default `true`**.
///
/// - `MOMOS_AUTOUPDATE_ENABLED` set *and* parseable wins (`"env"`);
/// - otherwise a persisted UI value `settings['autoupdate.enabled']` (`"ui"`);
/// - otherwise the config value — since a parseable env value was already
///   caught above, this is TOML or the built-in default (`"toml"`/`"default"`).
///   Unparseable env values keep falling through, unchanged from
///   `ServiceCredentials::load()`.
pub async fn effective_autoupdate_enabled(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<(bool, &'static str), sqlx::Error> {
    effective_autoupdate_enabled_with_env(
        config,
        db,
        std::env::var("MOMOS_AUTOUPDATE_ENABLED").ok(),
    )
    .await
}

/// Testable core of [`effective_autoupdate_enabled`] — the env value is
/// injected so the precedence matrix can be unit-tested without mutating
/// the process-global environment.
pub(crate) async fn effective_autoupdate_enabled_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    env_value: Option<String>,
) -> Result<(bool, &'static str), sqlx::Error> {
    if let Some(v) = env_value.and_then(|v| v.parse::<bool>().ok()) {
        return Ok((v, "env"));
    }
    if let Some(v) = settings::get_bool(db, KEY_AUTOUPDATE_ENABLED).await? {
        return Ok((v, "ui"));
    }
    Ok((
        config.autoupdate_enabled,
        config.autoupdate_enabled_source(),
    ))
}

/// Read the swap marker next to the running binary. On error the pending
/// update is `None` plus an error string — surfaced as `pendingUpdateError`
/// instead of a 500.
pub(crate) fn pending_update_from_marker(
    dir: &Path,
) -> (Option<PendingUpdateInfo>, Option<String>) {
    match swap::read_marker(dir) {
        Ok(Some(m)) => (
            Some(PendingUpdateInfo {
                old_version: m.old_version,
                new_version: m.new_version,
                committed: m.committed,
            }),
            None,
        ),
        Ok(None) => (None, None),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// Map an [`UpdateStatus`] to the persisted result shape.
pub(crate) fn check_result(status: &UpdateStatus, settings: &UpdateSettings) -> LastCheckResult {
    match status {
        UpdateStatus::UpToDate => LastCheckResult {
            state: "upToDate".into(),
            available_version: None,
            current_version: Some(settings.current_version.clone()),
            artifact_name: None,
        },
        UpdateStatus::UpdateAvailable(info) => LastCheckResult {
            state: "updateAvailable".into(),
            available_version: Some(info.version.clone()),
            current_version: Some(settings.current_version.clone()),
            artifact_name: Some(info.artifact_name.clone()),
        },
        UpdateStatus::ChannelMismatch {
            current_version,
            available_version,
            ..
        } => LastCheckResult {
            state: "channelMismatch".into(),
            available_version: Some(available_version.clone()),
            current_version: Some(current_version.clone()),
            artifact_name: None,
        },
        UpdateStatus::UnsupportedPlatform => LastCheckResult {
            state: "unsupportedPlatform".into(),
            available_version: None,
            current_version: Some(settings.current_version.clone()),
            artifact_name: None,
        },
        UpdateStatus::Disabled => LastCheckResult {
            state: "disabled".into(),
            available_version: None,
            current_version: Some(settings.current_version.clone()),
            artifact_name: None,
        },
    }
}

/// Result shape for a failed check (network/HTTP/signature errors).
pub(crate) fn error_result() -> LastCheckResult {
    LastCheckResult {
        state: "error".into(),
        available_version: None,
        current_version: Some(env!("MMM_VERSION").to_string()),
        artifact_name: None,
    }
}

/// Persist a last-check state (`status` = `"ok"`/`"error"`).
async fn persist_last_check(
    db: &Pool<Sqlite>,
    status: &str,
    result: &LastCheckResult,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().timestamp();
    settings::set_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_AT, &now.to_string()).await?;
    settings::set_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_STATUS, status).await?;
    settings::set_setting(
        db,
        settings::KEY_AUTOUPDATE_LAST_CHECK_RESULT,
        &serde_json::to_string(result).expect("LastCheckResult serializes"),
    )
    .await?;
    settings::set_setting(
        db,
        settings::KEY_AUTOUPDATE_LAST_CHECK_ERROR,
        error.unwrap_or(""),
    )
    .await?;
    Ok(())
}

/// Persist an honest `disabled` last-check state (no network) — used by the
/// `serve()` startup check when the effective value is off.
pub async fn persist_disabled_check(db: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    let result = LastCheckResult {
        state: "disabled".into(),
        available_version: None,
        current_version: Some(env!("MMM_VERSION").to_string()),
        artifact_name: None,
    };
    persist_last_check(db, "ok", &result, None).await
}

/// Run `UpdateStatus::check` with the given fetcher and persist the outcome
/// (`last_check_at`/`last_check_status`/`last_check_result`/`last_check_error`).
/// Returns the freshly built status JSON. Disabled/ChannelMismatch are *not*
/// errors — they persist as `ok` with the matching state; only network/HTTP/
/// verification failures persist as `error` (still HTTP 200 for the caller).
pub(crate) async fn run_check_and_persist<F: Fetcher>(
    db: &Pool<Sqlite>,
    config: &ServiceCredentials,
    settings: &UpdateSettings,
    fetcher: &F,
) -> Result<serde_json::Value, sqlx::Error> {
    match UpdateStatus::check(settings, fetcher).await {
        Ok(status) => {
            let result = check_result(&status, settings);
            persist_last_check(db, "ok", &result, None).await?;
        }
        Err(e) => {
            tracing::warn!("autoupdate: check failed: {e}");
            persist_last_check(db, "error", &error_result(), Some(&e.to_string())).await?;
        }
    }
    build_status_json(config, db).await
}

/// Build settings for a check/apply from the config and apply the *effective*
/// values: enabled (env > UI > TOML > default true) **and** channel
/// (env > UI > TOML > default = embedded channel of the running build) —
/// `config.autoupdate_enabled`/the built-in base URL alone are not enough
/// once the UI toggle and channel select exist. `check`/`apply` therefore
/// run against the base URL of the *selected* channel.
async fn settings_for_check(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<UpdateSettings, sqlx::Error> {
    let (enabled, _source) = effective_autoupdate_enabled(config, db).await?;
    let (channel, _source) = effective_autoupdate_channel(config, db).await?;
    let mut settings = UpdateSettings::from_config(config, channel).map_err(|e| {
        sqlx::Error::Configuration(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cannot build update settings: {e}"),
        )))
    })?;
    settings.enabled = enabled;
    Ok(settings)
}

/// Full check flow used by `POST /api/update/check` and the `serve()`
/// startup check: build effective settings → run the verified check →
/// persist → return the fresh status JSON.
pub async fn perform_check<F: Fetcher>(
    db: &Pool<Sqlite>,
    config: &ServiceCredentials,
    fetcher: &F,
) -> Result<serde_json::Value, sqlx::Error> {
    let settings = settings_for_check(config, db).await?;
    run_check_and_persist(db, config, &settings, fetcher).await
}

/// Build the full status JSON (US-2 shape) from config + persisted state.
pub async fn build_status_json(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<serde_json::Value, sqlx::Error> {
    build_status_json_with_env(
        config,
        db,
        std::env::var("MOMOS_AUTOUPDATE_ENABLED").ok(),
        std::env::var("MOMOS_AUTOUPDATE_CHANNEL").ok(),
    )
    .await
}

/// Testable core of [`build_status_json`] — see
/// [`effective_autoupdate_enabled_with_env`] / [`effective_autoupdate_channel_with_env`].
pub(crate) async fn build_status_json_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    enabled_env_value: Option<String>,
    channel_env_value: Option<String>,
) -> Result<serde_json::Value, sqlx::Error> {
    let (enabled, enabled_source) =
        effective_autoupdate_enabled_with_env(config, db, enabled_env_value).await?;
    let (channel, channel_source) =
        effective_autoupdate_channel_with_env(config, db, channel_env_value).await?;

    // Best-effort settings: an unsupported platform must not break the status
    // view — base URL then comes straight from the config.
    let settings = UpdateSettings::from_config(config, channel).ok();
    let base_url = settings
        .as_ref()
        .map(|s| s.base_url.clone())
        .unwrap_or_else(|| config.autoupdate_base_url.clone());
    let artifact = settings.as_ref().map(|s| ArtifactInfo {
        os_arch: s.artifact.os_arch.clone(),
        ext: s.artifact.ext.clone(),
    });
    let platform_self_install = artifact.as_ref().map(|a| a.ext != "dmg").unwrap_or(false);

    let last_check_at = settings::get_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_AT)
        .await?
        .and_then(|v| v.parse::<i64>().ok());
    let last_check_status =
        settings::get_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_STATUS).await?;
    let last_check_error = settings::get_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_ERROR)
        .await?
        .filter(|v| !v.is_empty());
    let last_check_result: Option<LastCheckResult> =
        settings::get_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_RESULT)
            .await?
            .and_then(|v| serde_json::from_str(&v).ok());

    let (pending_update, pending_update_error) = pending_update_from_marker(&swap::exe_dir());

    let update_available = last_check_result
        .as_ref()
        .map(|r| r.state == "updateAvailable")
        .unwrap_or(false)
        || pending_update.is_some();

    let available_channels: Vec<&'static str> =
        UpdateChannel::ALL.iter().map(|c| c.as_str()).collect();

    Ok(serde_json::json!({
        "currentVersion": env!("MMM_VERSION"),
        // Effective (selected) update channel — `rolling` | `release`.
        "channel": channel.as_str(),
        "channelSource": channel_source,
        "availableChannels": available_channels,
        "baseUrl": base_url,
        "enabled": enabled,
        "enabledSource": enabled_source,
        "artifact": artifact,
        "lastCheckAt": last_check_at,
        "lastCheckStatus": last_check_status,
        "lastCheckError": last_check_error,
        "lastCheckResult": last_check_result,
        "updateAvailable": update_available,
        "pendingUpdate": pending_update,
        "pendingUpdateError": pending_update_error,
        "platformSelfInstall": platform_self_install,
    }))
}

// ── US-4: toggle + channel persistence ─────────────────────────────────────

/// Body of `POST /api/update/settings` — at least one field must be present.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    pub auto_update_enabled: Option<bool>,
    /// Update channel: `"rolling"` | `"release"`.
    pub channel: Option<String>,
}

/// Outcome of a toggle write.
#[derive(Debug)]
pub(crate) enum ToggleError {
    /// The effective value is pinned by env/TOML — nothing was written.
    Overridden(&'static str),
    Db(sqlx::Error),
}

/// Persist the UI toggle — unless env or TOML pins the value (409).
/// The env value is injected so the override matrix is unit-testable.
pub(crate) async fn settings_toggle_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    value: bool,
    env_value: Option<String>,
) -> Result<serde_json::Value, ToggleError> {
    let (_, source) = effective_autoupdate_enabled_with_env(config, db, env_value.clone())
        .await
        .map_err(ToggleError::Db)?;
    if source == "env" || source == "toml" {
        return Err(ToggleError::Overridden(source));
    }
    settings::set_bool(db, KEY_AUTOUPDATE_ENABLED, value)
        .await
        .map_err(ToggleError::Db)?;
    let (enabled, source) = effective_autoupdate_enabled_with_env(config, db, env_value)
        .await
        .map_err(ToggleError::Db)?;
    Ok(serde_json::json!({
        "autoUpdateEnabled": enabled,
        "enabled": enabled,
        "enabledSource": source,
    }))
}

/// [`settings_toggle_with_env`] with the process env.
pub(crate) async fn settings_toggle(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    value: bool,
) -> Result<serde_json::Value, ToggleError> {
    settings_toggle_with_env(
        config,
        db,
        value,
        std::env::var("MOMOS_AUTOUPDATE_ENABLED").ok(),
    )
    .await
}

/// Delete the persisted last-check cache (`autoupdate.last_check_*`) — used
/// after a channel switch: a check result from the previous channel must not
/// be presented as the state of the new one.
pub(crate) async fn clear_last_check_cache(db: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    for key in [
        settings::KEY_AUTOUPDATE_LAST_CHECK_AT,
        settings::KEY_AUTOUPDATE_LAST_CHECK_STATUS,
        settings::KEY_AUTOUPDATE_LAST_CHECK_RESULT,
        settings::KEY_AUTOUPDATE_LAST_CHECK_ERROR,
    ] {
        settings::delete_setting(db, key).await?;
    }
    Ok(())
}

/// Persist the UI channel — unless env or TOML pins the value (409). A
/// successful switch clears the last-check cache (see
/// [`clear_last_check_cache`]). The env value is injected so the override
/// matrix is unit-testable.
pub(crate) async fn settings_channel_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    channel: UpdateChannel,
    env_value: Option<String>,
) -> Result<serde_json::Value, ToggleError> {
    let (_, source) = effective_autoupdate_channel_with_env(config, db, env_value.clone())
        .await
        .map_err(ToggleError::Db)?;
    if source == "env" || source == "toml" {
        return Err(ToggleError::Overridden(source));
    }
    settings::set_setting(db, settings::KEY_AUTOUPDATE_CHANNEL, channel.as_str())
        .await
        .map_err(ToggleError::Db)?;
    if let Err(e) = clear_last_check_cache(db).await {
        tracing::warn!("autoupdate: clearing last-check cache after channel switch failed: {e}");
    }
    let (channel, source) = effective_autoupdate_channel_with_env(config, db, env_value)
        .await
        .map_err(ToggleError::Db)?;
    Ok(serde_json::json!({
        "channel": channel.as_str(),
        "channelSource": source,
    }))
}

/// [`settings_channel_with_env`] with the process env.
pub(crate) async fn settings_channel(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    channel: UpdateChannel,
) -> Result<serde_json::Value, ToggleError> {
    settings_channel_with_env(
        config,
        db,
        channel,
        std::env::var("MOMOS_AUTOUPDATE_CHANNEL").ok(),
    )
    .await
}

// ── US-5: manual "update now" ─────────────────────────────────────────────

/// Outcome of an apply attempt — status code + wire error message.
#[derive(Debug)]
pub(crate) enum UpdateApplyError {
    Db(sqlx::Error),
    Failed { status: StatusCode, message: String },
}

/// Map an [`ApplyOutcome`] to the wire shape
/// (`installed` → restartNeeded, `downloaded` → path + instructions).
pub(crate) fn apply_outcome_json(outcome: &ApplyOutcome) -> serde_json::Value {
    match outcome {
        ApplyOutcome::Installed {
            new_version,
            old_version,
        } => serde_json::json!({
            "outcome": "installed",
            "newVersion": new_version,
            "oldVersion": old_version,
            "restartNeeded": true,
        }),
        ApplyOutcome::DownloadedOnly { path, version } => serde_json::json!({
            "outcome": "downloaded",
            "path": path.to_string_lossy(),
            "version": version,
        }),
    }
}

/// Map an [`UpdateError`] to (status, message) — CLI-style channel-mismatch
/// explanation (mirrors main.rs `update apply`).
pub(crate) fn apply_error_json(err: &UpdateError) -> (StatusCode, String) {
    match err {
        UpdateError::Disabled => (
            StatusCode::CONFLICT,
            "autoupdate is disabled — enable it in the Settings page or via config.toml".into(),
        ),
        UpdateError::NoUpdate => (StatusCode::NOT_FOUND, "no update available".into()),
        UpdateError::ChannelMismatch {
            channel,
            available_version,
            current_version,
        } => {
            let tracks = if *channel == UPDATE_CHANNEL_ROLLING {
                "rolling dev builds of main (latest-main)"
            } else {
                "stable semver releases (releases/latest)"
            };
            let published = if available_version.contains("-dev+") {
                "a dev build"
            } else {
                "a stable release"
            };
            let text = format!(
                "channel mismatch: update channel is '{channel}', but the update source serves {published} v{available_version} instead of {tracks} (current build: v{current_version}). Pick the matching channel in the Settings page or fix the update source (base_url)."
            );
            (StatusCode::CONFLICT, text)
        }
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    }
}

/// Full apply flow: effective settings → `UpdateStatus::apply` (check +
/// download + verify + swap / DMG download) → refresh the persisted
/// last-check state like US-3. Failure states (disabled, no update, channel
/// mismatch) are also persisted so the status view stays honest.
pub(crate) async fn update_apply<F: Fetcher>(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    fetcher: &F,
) -> Result<serde_json::Value, UpdateApplyError> {
    let settings = settings_for_check(config, db)
        .await
        .map_err(UpdateApplyError::Db)?;
    update_apply_with_settings(&settings, config, db, fetcher).await
}

/// Testable core of [`update_apply`] — settings injected so tests can use
/// the MockFetcher's test keypair (`verify::tests::test_settings`).
pub(crate) async fn update_apply_with_settings<F: Fetcher>(
    settings: &UpdateSettings,
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    fetcher: &F,
) -> Result<serde_json::Value, UpdateApplyError> {
    match UpdateStatus::apply(settings, fetcher).await {
        Ok(outcome) => {
            if let Err(e) = run_check_and_persist(db, config, settings, fetcher).await {
                tracing::warn!("autoupdate: last-check refresh after apply failed: {e}");
            }
            Ok(apply_outcome_json(&outcome))
        }
        Err(err) => {
            // Persist an honest last-check state for each failure class.
            match &err {
                UpdateError::Disabled => {
                    let _ = persist_disabled_check(db).await;
                }
                UpdateError::NoUpdate => {
                    let result = LastCheckResult {
                        state: "upToDate".into(),
                        available_version: None,
                        current_version: Some(settings.current_version.clone()),
                        artifact_name: None,
                    };
                    let _ = persist_last_check(db, "ok", &result, None).await;
                }
                UpdateError::ChannelMismatch {
                    available_version,
                    current_version,
                    ..
                } => {
                    let result = LastCheckResult {
                        state: "channelMismatch".into(),
                        available_version: Some(available_version.clone()),
                        current_version: Some(current_version.clone()),
                        artifact_name: None,
                    };
                    let _ = persist_last_check(db, "ok", &result, None).await;
                }
                other => {
                    let _ =
                        persist_last_check(db, "error", &error_result(), Some(&other.to_string()))
                            .await;
                }
            }
            let (status, message) = apply_error_json(&err);
            Err(UpdateApplyError::Failed { status, message })
        }
    }
}

// ── Handlers ───────────────────────────────────────────────────────────────

/// GET /api/update/status
async fn status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match build_status_json(&state.config, &state.db).await {
        Ok(data) => Json(ApiResponse { data }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/update/check — run a verified check, persist the result and
/// return the fresh status JSON. Network/HTTP errors are *not* 5xx: they
/// come back as HTTP 200 with `lastCheckStatus: "error"` (fetchJSON
/// convention — a failed check is a valid UI state).
async fn check_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let fetcher = HttpFetcher::new();
    match perform_check(&state.db, &state.config, &fetcher).await {
        Ok(data) => Json(ApiResponse { data }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/update/settings — persist the auto-update toggle. 409 when the
/// effective value is pinned by env or TOML (nothing is written); 400 on a
/// missing/invalid body.
/// POST /api/update/settings — persist the auto-update toggle and/or the
/// update channel (`{"autoUpdateEnabled": bool, "channel": "rolling"|"release"}`;
/// at least one field required). 409 when a field is pinned by env or TOML
/// (nothing is written); 400 on a missing/invalid body or channel value.
async fn settings_handler(
    State(state): State<Arc<AppState>>,
    body: Option<Json<UpdateSettingsRequest>>,
) -> impl IntoResponse {
    let Some(Json(req)) = body else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid body — expected { \"autoUpdateEnabled\": true|false, \"channel\": \"rolling\"|\"release\" }".into(),
            }),
        )
            .into_response();
    };
    if req.auto_update_enabled.is_none() && req.channel.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid body — provide at least one of \"autoUpdateEnabled\" or \"channel\"".into(),
            }),
        )
            .into_response();
    }
    let wanted_channel = match req.channel.as_deref() {
        Some(value) => match UpdateChannel::parse(value) {
            Some(channel) => Some(channel),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!(
                            "invalid channel \"{value}\" — expected \"{UPDATE_CHANNEL_ROLLING}\" or \"{UPDATE_CHANNEL_RELEASE}\""
                        ),
                    }),
                )
                    .into_response();
            }
        },
        None => None,
    };

    // Pin pre-checks first so a request that would partially fail writes
    // nothing (a field pinned by env/TOML must not be persisted).
    if req.auto_update_enabled.is_some() {
        let (_, source) = match effective_autoupdate_enabled(&state.config, &state.db).await {
            Ok(v) => v,
            Err(e) => return internal_error(e).into_response(),
        };
        if source == "env" || source == "toml" {
            return pinned_conflict("autoupdate.enabled", source, "MOMOS_AUTOUPDATE_ENABLED", "[autoupdate] enabled", "toggle").into_response();
        }
    }
    if wanted_channel.is_some() {
        let (_, source) = match effective_autoupdate_channel(&state.config, &state.db).await {
            Ok(v) => v,
            Err(e) => return internal_error(e).into_response(),
        };
        if source == "env" || source == "toml" {
            return pinned_conflict("autoupdate.channel", source, "MOMOS_AUTOUPDATE_CHANNEL", "[autoupdate] channel", "dropdown").into_response();
        }
    }

    // Persist whatever was requested — pre-checks above guarantee no 409.
    let mut data = serde_json::json!({});
    if let Some(value) = req.auto_update_enabled {
        match settings_toggle(&state.config, &state.db, value).await {
            Ok(toggle_data) => data = toggle_data,
            // Race only; pre-check already passed.
            Err(ToggleError::Overridden(source)) => {
                return pinned_conflict("autoupdate.enabled", source, "MOMOS_AUTOUPDATE_ENABLED", "[autoupdate] enabled", "toggle").into_response();
            }
            Err(ToggleError::Db(e)) => return internal_error(e).into_response(),
        }
    }
    if let Some(channel) = wanted_channel {
        match settings_channel(&state.config, &state.db, channel).await {
            Ok(channel_data) => {
                for (k, v) in channel_data.as_object().expect("channel response is an object") {
                    data[k] = v.clone();
                }
            }
            Err(ToggleError::Overridden(source)) => {
                return pinned_conflict("autoupdate.channel", source, "MOMOS_AUTOUPDATE_CHANNEL", "[autoupdate] channel", "dropdown").into_response();
            }
            Err(ToggleError::Db(e)) => return internal_error(e).into_response(),
        }
    }

    // Merged response: always report both effective values + sources so the
    // UI can update its state from a single response.
    if req.auto_update_enabled.is_none() {
        let (enabled, enabled_source) =
            match effective_autoupdate_enabled(&state.config, &state.db).await {
                Ok(v) => v,
                Err(e) => return internal_error(e).into_response(),
            };
        data["autoUpdateEnabled"] = enabled.into();
        data["enabled"] = enabled.into();
        data["enabledSource"] = enabled_source.into();
    }
    if wanted_channel.is_none() {
        let (channel, channel_source) =
            match effective_autoupdate_channel(&state.config, &state.db).await {
                Ok(v) => v,
                Err(e) => return internal_error(e).into_response(),
            };
        data["channel"] = channel.as_str().into();
        data["channelSource"] = channel_source.into();
    }
    Json(ApiResponse { data }).into_response()
}

/// 409 body for a setting pinned by env/TOML.
fn pinned_conflict(
    setting: &str,
    source: &str,
    env_name: &str,
    toml_name: &str,
    control: &str,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: match source {
                "env" => format!(
                    "{setting} is pinned by the environment variable {env_name} — change it there to edit this {control}"
                ),
                _ => format!(
                    "{setting} is pinned by {toml_name} in config.toml — edit the file to change this {control}"
                ),
            },
        }),
    )
}

/// POST /api/update/apply — manual "update now": check + verified download
/// + atomic swap (Linux/Windows) or verified DMG download + instructions
/// (macOS). 409 when disabled or on channel mismatch, 404 when no update is
/// available, 500 for other failures.
async fn apply_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let fetcher = HttpFetcher::new();
    match update_apply(&state.config, &state.db, &fetcher).await {
        Ok(data) => Json(ApiResponse { data }).into_response(),
        Err(UpdateApplyError::Db(e)) => internal_error(e).into_response(),
        Err(UpdateApplyError::Failed { status, message }) => {
            (status, Json(ErrorResponse { error: message })).into_response()
        }
    }
}

// ── Router ─────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/update/status", get(status_handler))
        .route("/api/update/check", post(check_handler))
        .route("/api/update/settings", post(settings_handler))
        .route("/api/update/apply", post(apply_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autoupdate::verify::tests::MockFetcher;
    use sqlx::SqlitePool;

    async fn test_pool() -> Pool<Sqlite> {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn config_with(enabled: bool, has_toml: bool) -> ServiceCredentials {
        let mut c = ServiceCredentials::defaults_for_test();
        c.autoupdate_enabled = enabled;
        c.autoupdate_has_toml = has_toml;
        c
    }

    // ── Precedence matrix: env × ui × toml → effective value + source ──

    #[tokio::test]
    async fn precedence_default_true_without_anything() {
        let pool = test_pool().await;
        let config = config_with(true, false);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (true, "default")
        );
    }

    #[tokio::test]
    async fn precedence_toml_false_wins_over_default() {
        let pool = test_pool().await;
        let config = config_with(false, true);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (false, "toml")
        );
    }

    #[tokio::test]
    async fn precedence_ui_false_wins_over_toml() {
        let pool = test_pool().await;
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();
        let config = config_with(true, true);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (false, "ui")
        );
    }

    #[tokio::test]
    async fn precedence_ui_true_wins_over_toml_false() {
        let pool = test_pool().await;
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, true)
            .await
            .unwrap();
        let config = config_with(false, true);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (true, "ui")
        );
    }

    #[tokio::test]
    async fn precedence_env_true_wins_over_everything() {
        let pool = test_pool().await;
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();
        let config = config_with(false, true);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, Some("true".into()))
                .await
                .unwrap(),
            (true, "env")
        );
    }

    #[tokio::test]
    async fn precedence_env_false_wins_over_everything() {
        let pool = test_pool().await;
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, true)
            .await
            .unwrap();
        let config = config_with(true, true);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, Some("false".into()))
                .await
                .unwrap(),
            (false, "env")
        );
    }

    #[tokio::test]
    async fn precedence_unparseable_env_falls_through_to_ui() {
        let pool = test_pool().await;
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();
        let config = config_with(true, false);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, Some("garbage".into()))
                .await
                .unwrap(),
            (false, "ui")
        );
    }

    #[tokio::test]
    async fn precedence_unparseable_env_falls_through_to_toml() {
        let pool = test_pool().await;
        let config = config_with(false, true);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, Some("garbage".into()))
                .await
                .unwrap(),
            (false, "toml")
        );
    }

    #[tokio::test]
    async fn precedence_unparseable_env_falls_through_to_default() {
        let pool = test_pool().await;
        let config = config_with(true, false);
        assert_eq!(
            effective_autoupdate_enabled_with_env(&config, &pool, Some("garbage".into()))
                .await
                .unwrap(),
            (true, "default")
        );
    }

    // ── Channel detection ──────────────────────────────────────────────

    #[test]
    fn channel_detection_dev_and_release() {
        assert_eq!(channel_for_version("1.1.0-dev+abc1234"), "dev");
        assert_eq!(channel_for_version("1.1.0"), "release");
        assert_eq!(channel_for_version("0.9.0-beta.1"), "dev");
        assert_eq!(channel_for_version("not-a-version"), "release");
    }

    // ── Channel precedence matrix: env × ui × toml × default ──────────

    /// Config with a `[autoupdate] channel` toml value (enabled defaults).
    fn channel_config(toml_channel: Option<&str>) -> ServiceCredentials {
        let mut c = config_with(true, false);
        c.autoupdate_channel_toml = toml_channel.map(str::to_string);
        c
    }

    /// Default channel of the running build in tests (Cargo version without
    /// pre-release → release).
    fn embedded_default_channel() -> UpdateChannel {
        UpdateChannel::for_version(env!("MMM_VERSION"))
    }

    #[tokio::test]
    async fn channel_default_follows_embedded_build_channel() {
        let pool = test_pool().await;
        let config = channel_config(None);
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (embedded_default_channel(), "default")
        );
    }

    #[tokio::test]
    async fn channel_toml_wins_over_default() {
        let pool = test_pool().await;
        let config = channel_config(Some("rolling"));
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (UpdateChannel::Rolling, "toml")
        );
    }

    #[tokio::test]
    async fn channel_ui_wins_over_toml() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "release")
            .await
            .unwrap();
        let config = channel_config(Some("rolling"));
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, None)
                .await
                .unwrap(),
            (UpdateChannel::Release, "ui")
        );
    }

    #[tokio::test]
    async fn channel_env_wins_over_everything() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "release")
            .await
            .unwrap();
        let config = channel_config(Some("release"));
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, Some("rolling".into()))
                .await
                .unwrap(),
            (UpdateChannel::Rolling, "env")
        );
    }

    #[tokio::test]
    async fn channel_unparseable_env_falls_through_to_ui() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "rolling")
            .await
            .unwrap();
        let config = channel_config(None);
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, Some("banana".into()))
                .await
                .unwrap(),
            (UpdateChannel::Rolling, "ui")
        );
    }

    #[tokio::test]
    async fn channel_unparseable_env_falls_through_to_toml() {
        let pool = test_pool().await;
        let config = channel_config(Some("rolling"));
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, Some("banana".into()))
                .await
                .unwrap(),
            (UpdateChannel::Rolling, "toml")
        );
    }

    #[tokio::test]
    async fn channel_unparseable_env_falls_through_to_default() {
        let pool = test_pool().await;
        let config = channel_config(None);
        assert_eq!(
            effective_autoupdate_channel_with_env(&config, &pool, Some("banana".into()))
                .await
                .unwrap(),
            (embedded_default_channel(), "default")
        );
    }

    #[tokio::test]
    async fn channel_invalid_db_value_is_an_error() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "banana")
            .await
            .unwrap();
        let config = channel_config(None);
        assert!(effective_autoupdate_channel_with_env(&config, &pool, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn status_json_reflects_persisted_channel_and_source() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "rolling")
            .await
            .unwrap();
        let config = channel_config(None);
        let json = build_status_json_with_env(&config, &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["channel"], "rolling");
        assert_eq!(json["channelSource"], "ui");
        assert_eq!(json["availableChannels"], serde_json::json!(["release", "rolling"]));
        // Base URL follows the selected channel (config carries only the
        // untouched built-in default).
        assert_eq!(
            json["baseUrl"],
            crate::autoupdate::DEFAULT_BASE_URL
        );
    }

    #[tokio::test]
    async fn status_json_base_url_follows_selected_release_channel() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "release")
            .await
            .unwrap();
        let config = channel_config(None);
        let json = build_status_json_with_env(&config, &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["channel"], "release");
        assert_eq!(json["baseUrl"], crate::autoupdate::DEFAULT_RELEASE_BASE_URL);
    }

    #[tokio::test]
    async fn status_json_env_channel_and_source() {
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "release")
            .await
            .unwrap();
        let config = channel_config(None);
        let json = build_status_json_with_env(&config, &pool, None, Some("rolling".into()))
            .await
            .unwrap();
        assert_eq!(json["channel"], "rolling");
        assert_eq!(json["channelSource"], "env");
        assert_eq!(json["baseUrl"], crate::autoupdate::DEFAULT_BASE_URL);
    }

    // ── Channel persistence (settings_channel_*) ──────────────────────

    #[tokio::test]
    async fn channel_write_persists_and_clears_stale_check_cache() {
        let pool = test_pool().await;
        let config = channel_config(None);
        // Simulate a stale check from the previous channel.
        let stale = LastCheckResult {
            state: "channelMismatch".into(),
            available_version: Some("2.0.0".into()),
            current_version: Some("1.1.0".into()),
            artifact_name: None,
        };
        persist_last_check(&pool, "ok", &stale, None).await.unwrap();

        let json = settings_channel_with_env(&config, &pool, UpdateChannel::Rolling, None)
            .await
            .unwrap();
        assert_eq!(json["channel"], "rolling");
        assert_eq!(json["channelSource"], "ui");
        assert_eq!(
            settings::get_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL)
                .await
                .unwrap()
                .as_deref(),
            Some("rolling")
        );
        // Stale last-check state of the previous channel is cleared.
        assert!(settings::get_setting(&pool, settings::KEY_AUTOUPDATE_LAST_CHECK_STATUS)
            .await
            .unwrap()
            .is_none());
        assert!(settings::get_setting(&pool, settings::KEY_AUTOUPDATE_LAST_CHECK_RESULT)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn channel_write_rejected_when_env_overrides_and_nothing_written() {
        let pool = test_pool().await;
        let config = channel_config(None);

        let err = settings_channel_with_env(&config, &pool, UpdateChannel::Rolling, Some("release".into()))
            .await
            .unwrap_err();
        match err {
            ToggleError::Overridden("env") => {}
            other => panic!("expected env override, got {other:?}"),
        }
        assert_eq!(
            settings::get_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn channel_write_rejected_when_toml_overrides_and_nothing_written() {
        let pool = test_pool().await;
        let config = channel_config(Some("rolling"));

        let err = settings_channel_with_env(&config, &pool, UpdateChannel::Release, None)
            .await
            .unwrap_err();
        match err {
            ToggleError::Overridden("toml") => {}
            other => panic!("expected toml override, got {other:?}"),
        }
        assert_eq!(
            settings::get_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn channel_write_allowed_with_unparseable_env() {
        let pool = test_pool().await;
        let config = channel_config(None);
        let json =
            settings_channel_with_env(&config, &pool, UpdateChannel::Rolling, Some("x".into()))
                .await
                .unwrap();
        assert_eq!(json["channel"], "rolling");
        assert_eq!(json["channelSource"], "ui");
    }

    // ── check/apply against the *selected* channel ────────────────────

    #[tokio::test]
    async fn settings_for_check_uses_ui_selected_channel_and_its_base_url() {
        // The UI selected `rolling` (the running build is a release build,
        // embedded default would be release): settings_for_check must build
        // rolling settings whose base URL is the rolling feed. (The full
        // verified fetch cannot run with MockFetcher fixtures here — those
        // are signed with the test key while perform_check uses the embedded
        // release key; the chain itself is covered by the injected-settings
        // tests below and in verify.rs.)
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "rolling")
            .await
            .unwrap();
        let config = config_with(true, false);

        let settings = settings_for_check(&config, &pool).await.unwrap();
        assert_eq!(settings.channel, UpdateChannel::Rolling);
        assert_eq!(settings.base_url, crate::autoupdate::DEFAULT_BASE_URL);
        assert!(settings.enabled);
    }

    #[tokio::test]
    async fn settings_for_check_default_channel_is_embedded_build_channel() {
        // No UI/env/toml value → the release build (embedded default) tracks
        // the release feed.
        let pool = test_pool().await;
        let config = config_with(true, false);

        let settings = settings_for_check(&config, &pool).await.unwrap();
        assert_eq!(settings.channel, UpdateChannel::Release);
        assert_eq!(settings.base_url, crate::autoupdate::DEFAULT_RELEASE_BASE_URL);
    }

    #[tokio::test]
    async fn settings_for_check_respects_config_toml_channel() {
        let pool = test_pool().await;
        let config = channel_config(Some("rolling"));

        let settings = settings_for_check(&config, &pool).await.unwrap();
        assert_eq!(settings.channel, UpdateChannel::Rolling);
        assert_eq!(settings.base_url, crate::autoupdate::DEFAULT_BASE_URL);
    }

    #[tokio::test]
    async fn settings_for_check_explicit_base_url_override_wins_over_channel() {
        // An explicit base URL override (≠ the built-in rolling default) is
        // kept even when the channel would point elsewhere — the mismatch
        // guard then reports an inconsistent source instead of silently
        // checking the wrong feed.
        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "rolling")
            .await
            .unwrap();
        let mut config = config_with(true, false);
        config.autoupdate_base_url = "https://mirror.example.invalid/custom".into();

        let settings = settings_for_check(&config, &pool).await.unwrap();
        assert_eq!(settings.channel, UpdateChannel::Rolling);
        assert_eq!(settings.base_url, "https://mirror.example.invalid/custom");
    }

    #[tokio::test]
    async fn check_persists_channel_mismatch_when_source_serves_other_channel() {
        // Rolling channel (selected in the UI) whose source serves a stable
        // release → inconsistent source → check persists channelMismatch
        // (not an error), the status view keeps reporting the channel.
        use crate::autoupdate::verify::tests::{MockFetcher, signed_fixture, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        settings::set_setting(&pool, settings::KEY_AUTOUPDATE_CHANNEL, "rolling")
            .await
            .unwrap();
        let config = config_with(true, false);
        let mut settings = test_settings("linux-x64", "tar.gz", "1.1.0");
        settings.channel = UpdateChannel::Rolling;

        let sha = "ab".repeat(32);
        let manifest = format!(
            "{sha}  momos-music-manager-2.0.0-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        );
        let mut files = HashMap::new();
        signed_fixture(&mut files, &manifest, "2.0.0", None);
        let fetcher = MockFetcher::new(files);

        let json = run_check_and_persist(&pool, &config, &settings, &fetcher)
            .await
            .unwrap();
        assert_eq!(json["channel"], "rolling");
        assert_eq!(json["channelSource"], "ui");
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "channelMismatch");
        assert_eq!(json["lastCheckResult"]["availableVersion"], "2.0.0");
    }

    #[tokio::test]
    async fn update_apply_cross_channel_after_ui_switch() {
        // UI switched a release build to `rolling`: apply must install the
        // dev binary from the rolling feed (explicit cross-channel switch).
        use crate::autoupdate::verify::tests::{MockFetcher, signed_fixture, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        let config = config_with(true, false);

        let archive = {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            use std::io::Write;
            let mut builder = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "momos-music-manager", b"dev-bin" as &[u8])
                .unwrap();
            builder.finish().unwrap();
            let tar_bytes = builder.into_inner().unwrap();
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap()
        };
        let sha = crate::autoupdate::verify::hex_digest(&archive);
        let manifest = format!(
            "{sha}  momos-music-manager-2.0.0-dev+abc-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        );
        let mut files = HashMap::new();
        signed_fixture(&mut files, &manifest, "2.0.0-dev+abc", Some(archive));
        let fetcher = MockFetcher::new(files);

        let mut settings = test_settings("linux-x64", "tar.gz", "1.1.0");
        settings.channel = UpdateChannel::Rolling; // explicit switch
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("momos-music-manager");
        std::fs::write(&bin_path, b"old").unwrap();
        settings.install_dir = Some(dir.path().to_path_buf());

        let json = update_apply_with_settings(&settings, &config, &pool, &fetcher)
            .await
            .unwrap();
        assert_eq!(json["outcome"], "installed");
        assert_eq!(json["newVersion"], "2.0.0-dev+abc");
        assert_eq!(json["oldVersion"], "1.1.0");
        assert_eq!(std::fs::read(&bin_path).unwrap(), b"dev-bin");
        // Honest refresh of the last-check state (on the new channel).
        let status = build_status_json(&config, &pool).await.unwrap();
        assert_eq!(status["lastCheckResult"]["state"], "updateAvailable");
    }

    // ── Marker handling ────────────────────────────────────────────────

    #[test]
    fn marker_absent_means_no_pending_update() {
        let dir = tempfile::tempdir().unwrap();
        let (pending, error) = pending_update_from_marker(dir.path());
        assert!(pending.is_none());
        assert!(error.is_none());
    }

    #[test]
    fn marker_present_is_surfaced() {
        let dir = tempfile::tempdir().unwrap();
        swap::write_marker(
            dir.path(),
            &swap::UpdateMarker {
                old_version: "1.0.1".into(),
                new_version: "1.1.0".into(),
                created_at_unix: 0,
                start_count: 0,
                committed: false,
            },
        )
        .unwrap();
        let (pending, error) = pending_update_from_marker(dir.path());
        assert!(error.is_none());
        let pending = pending.unwrap();
        assert_eq!(pending.old_version, "1.0.1");
        assert_eq!(pending.new_version, "1.1.0");
        assert!(!pending.committed);
    }

    #[test]
    fn corrupt_marker_yields_error_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(swap::MARKER_FILE), b"{not json").unwrap();
        let (pending, error) = pending_update_from_marker(dir.path());
        assert!(pending.is_none());
        assert!(error.is_some());
    }

    // ── Status JSON from persisted state ───────────────────────────────

    #[tokio::test]
    async fn status_json_fresh_db_defaults() {
        let pool = test_pool().await;
        let config = config_with(true, false);
        let json = build_status_json_with_env(&config, &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["currentVersion"], env!("MMM_VERSION"));
        // Channel defaults to the embedded channel of the running build and
        // both selectable channels are advertised.
        let expected_channel =
            crate::autoupdate::UpdateChannel::for_version(env!("MMM_VERSION"));
        assert_eq!(json["channel"], expected_channel.as_str());
        assert_eq!(json["channelSource"], "default");
        assert_eq!(json["availableChannels"], serde_json::json!(["release", "rolling"]));
        assert_eq!(json["enabled"], true);
        assert_eq!(json["enabledSource"], "default");
        assert!(json["lastCheckAt"].is_null());
        assert!(json["lastCheckStatus"].is_null());
        assert!(json["lastCheckResult"].is_null());
        assert_eq!(json["updateAvailable"], false);
        assert!(json["pendingUpdate"].is_null());
        assert!(json["pendingUpdateError"].is_null());
        // Artifact + platformSelfInstall reflect the *runtime* platform.
        if let Some(artifact) = json["artifact"].as_object() {
            let ext = artifact["ext"].as_str().unwrap();
            assert_eq!(json["platformSelfInstall"], ext != "dmg");
        }
    }

    #[tokio::test]
    async fn status_json_reflects_persisted_last_check() {
        let pool = test_pool().await;
        let config = config_with(true, false);

        let result = LastCheckResult {
            state: "updateAvailable".into(),
            available_version: Some("2.0.0".into()),
            current_version: Some("1.0.1".into()),
            artifact_name: Some("momos-music-manager-2.0.0-linux-x64.tar.gz".into()),
        };
        persist_last_check(&pool, "ok", &result, None)
            .await
            .unwrap();
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();

        let json = build_status_json_with_env(&config, &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "updateAvailable");
        assert_eq!(json["lastCheckResult"]["availableVersion"], "2.0.0");
        assert!(json["lastCheckAt"].is_i64());
        assert_eq!(json["updateAvailable"], true);
        assert_eq!(json["enabled"], false);
        assert_eq!(json["enabledSource"], "ui");
    }

    #[tokio::test]
    async fn status_json_error_check_surfaces_error_and_not_update() {
        let pool = test_pool().await;
        let config = config_with(true, false);
        persist_last_check(
            &pool,
            "error",
            &error_result(),
            Some("network error fetching https://example.invalid: timeout"),
        )
        .await
        .unwrap();
        let json = build_status_json_with_env(&config, &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["lastCheckStatus"], "error");
        assert_eq!(json["lastCheckResult"]["state"], "error");
        assert!(
            json["lastCheckError"]
                .as_str()
                .unwrap()
                .contains("network error")
        );
        assert_eq!(json["updateAvailable"], false);
    }

    // ── Check persistence (MockFetcher + signed fixture) ──────────────

    #[tokio::test]
    async fn check_persists_update_available_with_signed_fixture() {
        use crate::autoupdate::verify::tests::{MockFetcher, signed_fixture, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        let config = config_with(true, false);

        let sha = "ab".repeat(32); // any 64-hex digest; check() does not download
        let manifest = format!(
            "{sha}  momos-music-manager-2.0.0-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        );
        let mut files = HashMap::new();
        signed_fixture(&mut files, &manifest, "2.0.0", None);
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");

        let json = run_check_and_persist(&pool, &config, &settings, &fetcher)
            .await
            .unwrap();
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "updateAvailable");
        assert_eq!(json["lastCheckResult"]["availableVersion"], "2.0.0");
        assert_eq!(
            json["lastCheckResult"]["artifactName"],
            "momos-music-manager-2.0.0-linux-x64.tar.gz"
        );
        assert_eq!(json["updateAvailable"], true);
        assert!(json["lastCheckAt"].is_i64());
        // Persisted roundtrip: the raw KV holds the serialized result.
        let raw = settings::get_setting(&pool, settings::KEY_AUTOUPDATE_LAST_CHECK_RESULT)
            .await
            .unwrap()
            .unwrap();
        let parsed: LastCheckResult = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.state, "updateAvailable");
    }

    #[tokio::test]
    async fn check_persists_ok_when_disabled() {
        use crate::autoupdate::verify::tests::{MockFetcher, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        let config = config_with(true, false);
        let mut settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        settings.enabled = false;
        let fetcher = MockFetcher::new(HashMap::new());

        let json = run_check_and_persist(&pool, &config, &settings, &fetcher)
            .await
            .unwrap();
        // Disabled is NOT an error: status ok, state disabled, no network hit.
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "disabled");
        assert!(json["lastCheckError"].is_null());
    }

    #[tokio::test]
    async fn check_persists_error_on_broken_fixture() {
        use crate::autoupdate::verify::tests::{MockFetcher, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        let config = config_with(true, false);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        // Empty fetcher → 404 on SHA256SUMS → UpdateError::HttpStatus.
        let fetcher = MockFetcher::new(HashMap::new());

        let json = run_check_and_persist(&pool, &config, &settings, &fetcher)
            .await
            .unwrap();
        assert_eq!(json["lastCheckStatus"], "error");
        assert_eq!(json["lastCheckResult"]["state"], "error");
        assert!(json["lastCheckError"].as_str().unwrap().contains("404"));
        assert_eq!(json["updateAvailable"], false);
    }

    #[tokio::test]
    async fn perform_check_respects_ui_toggle() {
        use crate::autoupdate::verify::tests::{MockFetcher, signed_fixture, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        // UI toggle off → effective disabled → check must not even hit the
        // fetcher (which would 404 on a fresh DB).
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();
        let config = config_with(true, false);

        let sha = "ab".repeat(32);
        let manifest = format!(
            "{sha}  momos-music-manager-2.0.0-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        );
        let mut files = HashMap::new();
        signed_fixture(&mut files, &manifest, "2.0.0", None);
        let fetcher = MockFetcher::new(files);

        let json = perform_check(&pool, &config, &fetcher).await.unwrap();
        assert_eq!(json["enabled"], false);
        assert_eq!(json["enabledSource"], "ui");
        assert_eq!(json["lastCheckResult"]["state"], "disabled");
        assert_eq!(json["lastCheckStatus"], "ok");
    }

    #[tokio::test]
    async fn persist_disabled_check_writes_ok_state() {
        let pool = test_pool().await;
        persist_disabled_check(&pool).await.unwrap();
        let json = build_status_json_with_env(&config_with(true, false), &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "disabled");
        assert!(json["lastCheckAt"].is_i64());
    }

    // ── Check result mapping ───────────────────────────────────────────

    #[test]
    fn check_result_maps_all_states() {
        use crate::autoupdate::minisign::MinisignPublicKey;
        let settings = UpdateSettings {
            base_url: "https://example.invalid/latest-main".into(),
            channel: UpdateChannel::Release,
            enabled: true,
            health_grace_secs: 5,
            current_version: "1.0.1".into(),
            artifact: crate::autoupdate::verify::PlatformArtifact {
                os_arch: "linux-x64".into(),
                ext: "tar.gz".into(),
                binary_name: "momos-music-manager".into(),
            },
            pubkey: MinisignPublicKey::from_blob(crate::autoupdate::keys::PUBLIC_KEY_B64)
                .expect("embedded key parses"),
            install_dir: None,
        };
        let info = crate::autoupdate::UpdateInfo {
            version: "2.0.0".into(),
            artifact_name: "momos-music-manager-2.0.0-linux-x64.tar.gz".into(),
            sha256: "abc".into(),
            url: "https://example.invalid/x".into(),
        };

        let r = check_result(&UpdateStatus::UpToDate, &settings);
        assert_eq!(r.state, "upToDate");
        assert!(r.available_version.is_none());

        let r = check_result(&UpdateStatus::UpdateAvailable(info), &settings);
        assert_eq!(r.state, "updateAvailable");
        assert_eq!(r.available_version.as_deref(), Some("2.0.0"));

        let r = check_result(
            &UpdateStatus::ChannelMismatch {
                channel: "release",
                current_version: "1.0.1".into(),
                available_version: "2.0.0".into(),
            },
            &settings,
        );
        assert_eq!(r.state, "channelMismatch");
        assert_eq!(r.available_version.as_deref(), Some("2.0.0"));

        let r = check_result(&UpdateStatus::UnsupportedPlatform, &settings);
        assert_eq!(r.state, "unsupportedPlatform");

        let r = check_result(&UpdateStatus::Disabled, &settings);
        assert_eq!(r.state, "disabled");

        assert_eq!(error_result().state, "error");
    }

    // ── US-4: toggle persistence ────────────────────────────────────────

    #[tokio::test]
    async fn toggle_writes_ui_value_and_responds_ui_source() {
        let pool = test_pool().await;
        let config = config_with(true, false);

        let json = settings_toggle_with_env(&config, &pool, false, None)
            .await
            .unwrap();
        assert_eq!(json["autoUpdateEnabled"], false);
        assert_eq!(json["enabled"], false);
        assert_eq!(json["enabledSource"], "ui");
        assert_eq!(
            settings::get_bool(&pool, KEY_AUTOUPDATE_ENABLED)
                .await
                .unwrap(),
            Some(false)
        );
    }

    #[tokio::test]
    async fn toggle_roundtrip_and_status_reflects_persisted_value() {
        let pool = test_pool().await;
        let config = config_with(true, false);

        settings_toggle_with_env(&config, &pool, true, None)
            .await
            .unwrap();
        assert_eq!(
            settings::get_bool(&pool, KEY_AUTOUPDATE_ENABLED)
                .await
                .unwrap(),
            Some(true)
        );
        // The status view must now source the effective value from the DB.
        let json = build_status_json_with_env(&config, &pool, None, None)
            .await
            .unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["enabledSource"], "ui");

        settings_toggle_with_env(&config, &pool, false, None)
            .await
            .unwrap();
        assert_eq!(
            settings::get_bool(&pool, KEY_AUTOUPDATE_ENABLED)
                .await
                .unwrap(),
            Some(false)
        );
    }

    #[tokio::test]
    async fn toggle_rejected_when_env_overrides_and_nothing_written() {
        let pool = test_pool().await;
        let config = config_with(true, false);

        let err = settings_toggle_with_env(&config, &pool, false, Some("true".into()))
            .await
            .unwrap_err();
        match err {
            ToggleError::Overridden("env") => {}
            other => panic!("expected env override, got {other:?}"),
        }
        assert_eq!(
            settings::get_setting(&pool, KEY_AUTOUPDATE_ENABLED)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn toggle_rejected_when_toml_overrides_and_nothing_written() {
        let pool = test_pool().await;
        let config = config_with(true, true); // [autoupdate] enabled in config.toml

        let err = settings_toggle_with_env(&config, &pool, false, None)
            .await
            .unwrap_err();
        match err {
            ToggleError::Overridden("toml") => {}
            other => panic!("expected toml override, got {other:?}"),
        }
        assert_eq!(
            settings::get_setting(&pool, KEY_AUTOUPDATE_ENABLED)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn toggle_allowed_with_unparseable_env() {
        // Unparseable env falls through (unchanged behavior) — the UI layer
        // stays writable when nothing pins the value.
        let pool = test_pool().await;
        let config = config_with(true, false);

        let json = settings_toggle_with_env(&config, &pool, false, Some("banana".into()))
            .await
            .unwrap();
        assert_eq!(json["enabled"], false);
        assert_eq!(json["enabledSource"], "ui");
    }

    // ── US-5: apply mapping + flow ──────────────────────────────────────

    #[test]
    fn apply_outcome_mapping_installed_and_downloaded() {
        let installed = apply_outcome_json(&ApplyOutcome::Installed {
            new_version: "2.0.0".into(),
            old_version: "1.1.0".into(),
        });
        assert_eq!(installed["outcome"], "installed");
        assert_eq!(installed["newVersion"], "2.0.0");
        assert_eq!(installed["oldVersion"], "1.1.0");
        assert_eq!(installed["restartNeeded"], true);

        let downloaded = apply_outcome_json(&ApplyOutcome::DownloadedOnly {
            path: "/Users/x/Downloads/momos-music-manager-2.0.0-macos-universal.dmg".into(),
            version: "2.0.0".into(),
        });
        assert_eq!(downloaded["outcome"], "downloaded");
        assert_eq!(
            downloaded["path"],
            "/Users/x/Downloads/momos-music-manager-2.0.0-macos-universal.dmg"
        );
        assert_eq!(downloaded["version"], "2.0.0");
    }

    #[test]
    fn apply_error_mapping_covers_all_classes() {
        let (status, msg) = apply_error_json(&UpdateError::Disabled);
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(msg.contains("disabled"));

        let (status, msg) = apply_error_json(&UpdateError::NoUpdate);
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(msg, "no update available");

        let (status, msg) = apply_error_json(&UpdateError::ChannelMismatch {
            channel: "rolling",
            current_version: "1.1.0-dev+abc1234".into(),
            available_version: "2.0.0".into(),
        });
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(msg.contains("channel mismatch"));
        assert!(msg.contains("rolling"));
        assert!(msg.contains("stable release"));

        let (status, msg) = apply_error_json(&UpdateError::ChannelMismatch {
            channel: "release",
            current_version: "1.1.0".into(),
            available_version: "2.0.0-dev+abc1234".into(),
        });
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(msg.contains("release"));
        assert!(msg.contains("dev build"));

        let (status, _) = apply_error_json(&UpdateError::ChecksumMismatch);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn apply_rejected_when_ui_toggle_off() {
        use crate::autoupdate::verify::tests::MockFetcher;
        use std::collections::HashMap;

        let pool = test_pool().await;
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();
        let config = config_with(true, false);
        // Empty fetcher: a disabled apply must never hit the network.
        let fetcher = MockFetcher::new(HashMap::new());

        let err = update_apply(&config, &pool, &fetcher).await.unwrap_err();
        match err {
            UpdateApplyError::Failed { status, .. } => {
                assert_eq!(status, StatusCode::CONFLICT);
            }
            other => panic!("expected disabled conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_no_update_persists_uptodate_state() {
        use crate::autoupdate::verify::tests::{MockFetcher, signed_fixture, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        let config = config_with(true, false);
        // Test settings carry the MockFetcher's keypair; current 1.1.0 is a
        // release build, same as env!("MMM_VERSION") in test builds.
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0");

        // Manifest only lists an *older* release → fetch_update_info → Ok(None)
        // → NoUpdate (no download, no swap — safe to run anywhere).
        let sha = "ab".repeat(32);
        let manifest = format!(
            "{sha}  momos-music-manager-1.0.0-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        );
        let mut files = HashMap::new();
        signed_fixture(&mut files, &manifest, "1.0.0", None);
        let fetcher = MockFetcher::new(files);

        let err = update_apply_with_settings(&settings, &config, &pool, &fetcher)
            .await
            .unwrap_err();
        match err {
            UpdateApplyError::Failed { status, message } => {
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert_eq!(message, "no update available");
            }
            other => panic!("expected no-update 404, got {other:?}"),
        }
        // The honest last-check state was persisted (up to date).
        let json = build_status_json(&config, &pool).await.unwrap();
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "upToDate");
    }

    #[tokio::test]
    async fn apply_channel_mismatch_persists_state_and_maps_conflict() {
        use crate::autoupdate::verify::tests::{MockFetcher, signed_fixture, test_settings};
        use std::collections::HashMap;

        let pool = test_pool().await;
        let config = config_with(true, false);
        // Current is a release build; the manifest lists a *dev* build →
        // channel mismatch, no download.
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0");

        let sha = "ab".repeat(32);
        let manifest = format!(
            "{sha}  momos-music-manager-2.0.0-dev+abc-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        );
        let mut files = HashMap::new();
        signed_fixture(&mut files, &manifest, "2.0.0-dev+abc", None);
        let fetcher = MockFetcher::new(files);

        let err = update_apply_with_settings(&settings, &config, &pool, &fetcher)
            .await
            .unwrap_err();
        match err {
            UpdateApplyError::Failed { status, message } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert!(message.contains("channel mismatch"));
            }
            other => panic!("expected channel-mismatch conflict, got {other:?}"),
        }
        let json = build_status_json(&config, &pool).await.unwrap();
        assert_eq!(json["lastCheckStatus"], "ok");
        assert_eq!(json["lastCheckResult"]["state"], "channelMismatch");
        assert_eq!(json["lastCheckResult"]["availableVersion"], "2.0.0-dev+abc");
    }
}
