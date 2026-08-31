//! M6 — Autoupdater.
//!
//! Self-update against the rolling `latest-main` release with a strict
//! verification chain (Ed25519/minisign manifest signature → per-artifact
//! SHA256 → atomic swap with backup + health-checked commit + rollback).
//!
//! See `docs/RELEASE-ROADMAP.md` (M6) for the full milestone definition.

pub mod keys;
pub mod manifest;
pub mod minisign;
pub mod platform;
pub mod swap;
pub mod verify;

pub use swap::RecoveryAction;
pub use verify::{
    ApplyOutcome, DEFAULT_BASE_URL, DEFAULT_HEALTH_GRACE_SECS, DEFAULT_RELEASE_BASE_URL,
    HttpFetcher, UpdateError, UpdateInfo, UpdateSettings, UpdateStatus,
};

/// Run the startup recovery for a pending update and return what the caller
/// should do next (see [`RecoveryAction`]).
pub fn startup_recovery() -> RecoveryAction {
    let dir = swap::exe_dir();
    let binary_name = swap::exe_name();
    match swap::recovery_action(&dir, env!("MMM_VERSION"), &binary_name) {
        Ok(action) => action,
        Err(e) => {
            tracing::warn!("autoupdate: recovery check failed: {e}");
            RecoveryAction::None
        }
    }
}

/// Perform the health-checked commit after the grace period.
///
/// Best-effort: also probes the local health endpoint once; the commit
/// succeeds if the process is still alive (i.e. the new binary is healthy).
pub async fn commit_after_grace(
    grace_secs: u64,
    health_port: Option<u16>,
) {
    tokio::time::sleep(std::time::Duration::from_secs(grace_secs)).await;

    // Optional self-probe of the health endpoint (best-effort; a running
    // process is already a strong signal).
    if let Some(port) = health_port {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            let url = format!("http://127.0.0.1:{port}/api/health");
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::info!("autoupdate: health probe OK ({url})");
                }
                Ok(resp) => {
                    tracing::warn!(
                        "autoupdate: health probe returned HTTP {} — committing anyway (process alive)",
                        resp.status()
                    );
                }
                Err(e) => {
                    tracing::warn!("autoupdate: health probe failed ({e}) — committing anyway (process alive)");
                }
            }
        }
    }

    let dir = swap::exe_dir();
    let binary_name = swap::exe_name();
    match swap::commit_update(&dir, &binary_name) {
        Ok(()) => tracing::info!("autoupdate: update committed (backup + marker removed)"),
        Err(e) => tracing::error!("autoupdate: commit failed: {e}"),
    }
}

/// Perform an auto-rollback (restore the previous binary).
pub fn perform_rollback() -> Result<(), UpdateError> {
    let dir = swap::exe_dir();
    let binary_name = swap::exe_name();
    swap::rollback(&dir, &binary_name).map_err(UpdateError::Swap)?;
    tracing::warn!("autoupdate: rolled back to previous version (restart to activate)");
    Ok(())
}
