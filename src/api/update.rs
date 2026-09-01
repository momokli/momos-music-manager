//! Update controls — the Phase A+B web surface for the M6 autoupdater.
//!
//! Endpoints:
//! - `GET  /api/update/status`   — version/channel/enabled + last check + pending marker
//! - `POST /api/update/check`    — run a (verified) update check and persist the result
//! - `POST /api/update/settings` — persist the auto-update toggle (UI layer)
//! - `POST /api/update/apply`    — manual "update now" (swap on Linux/Windows,
//!                                 verified download + instructions on macOS)
//!
//! Precedence rule for the effective enabled value:
//! **Env > UI (DB setting) > TOML > Default `true`**.

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
    ApplyOutcome, HttpFetcher, UpdateError, UpdateSettings, UpdateStatus, swap, verify::Fetcher,
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

/// `"dev"` when the version carries a pre-release tag, else `"release"`.
pub fn channel_for_version(version: &str) -> &'static str {
    match semver::Version::parse(version) {
        Ok(v) if !v.pre.is_empty() => "dev",
        _ => "release",
    }
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
    Ok((config.autoupdate_enabled, config.autoupdate_enabled_source()))
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
    settings::set_setting(db, settings::KEY_AUTOUPDATE_LAST_CHECK_ERROR, error.unwrap_or(""))
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

/// Build settings for a check from the config and apply the *effective*
/// enabled value (env > UI > TOML > default) — `config.autoupdate_enabled`
/// alone is not enough once the UI toggle exists.
async fn settings_for_check(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
) -> Result<UpdateSettings, sqlx::Error> {
    let (enabled, _source) = effective_autoupdate_enabled(config, db).await?;
    let mut settings = UpdateSettings::from_config(config).map_err(|e| {
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
    )
    .await
}

/// Testable core of [`build_status_json`] — see
/// [`effective_autoupdate_enabled_with_env`].
pub(crate) async fn build_status_json_with_env(
    config: &ServiceCredentials,
    db: &Pool<Sqlite>,
    env_value: Option<String>,
) -> Result<serde_json::Value, sqlx::Error> {
    let (enabled, enabled_source) =
        effective_autoupdate_enabled_with_env(config, db, env_value).await?;

    // Best-effort settings: an unsupported platform must not break the status
    // view — base URL then comes straight from the config.
    let settings = UpdateSettings::from_config(config).ok();
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

    Ok(serde_json::json!({
        "currentVersion": env!("MMM_VERSION"),
        "channel": channel_for_version(env!("MMM_VERSION")),
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

// ── Router ─────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/update/status", get(status_handler))
        .route("/api/update/check", post(check_handler))
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
        let json = build_status_json_with_env(&config, &pool, None).await.unwrap();
        assert_eq!(json["currentVersion"], env!("MMM_VERSION"));
        assert_eq!(json["channel"], channel_for_version(env!("MMM_VERSION")));
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
        persist_last_check(&pool, "ok", &result, None).await.unwrap();
        settings::set_bool(&pool, KEY_AUTOUPDATE_ENABLED, false)
            .await
            .unwrap();

        let json = build_status_json_with_env(&config, &pool, None).await.unwrap();
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
        let json = build_status_json_with_env(&config, &pool, None).await.unwrap();
        assert_eq!(json["lastCheckStatus"], "error");
        assert_eq!(json["lastCheckResult"]["state"], "error");
        assert!(json["lastCheckError"]
            .as_str()
            .unwrap()
            .contains("network error"));
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
        let json = build_status_json_with_env(&config_with(true, false), &pool, None)
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
}
