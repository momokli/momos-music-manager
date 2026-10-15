//! Background sync loop + `kellner run` core (issue #368, plan phase P5).
//!
//! `kellner run` is what the launchd / systemd unit executes. It repeatedly
//! runs the selected sync tasks (currently only the Traktor upload) and then
//! sleeps — with the sleep stretched by [`crate::power::interval_for`] when the
//! host runs on battery. A failing cycle is logged and never aborts the loop;
//! `Ctrl-C` (SIGINT) stops it.
//!
//! The loop and the cycle live here (not in `main.rs`) so they can be exercised
//! by tests without the full CLI.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::client::hub::HubClient;
use crate::power::{PowerMode, interval_for};
use crate::{state, sync};

/// A sync task the loop can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    /// Detect, change-check and upload the local Traktor `collection.nml`.
    TraktorUpload,
}

impl Task {
    /// The CLI token for this task.
    pub fn as_str(self) -> &'static str {
        match self {
            Task::TraktorUpload => "traktor-upload",
        }
    }
}

/// Parse a `--tasks` list. An empty list means the default (`traktor-upload`).
///
/// Unknown names are rejected with the list of known tasks — no local
/// "active backpacks" selection lives here (that lives in the hub, #369).
pub fn parse_tasks(list: &[String]) -> Result<Vec<Task>> {
    let mut out = Vec::new();
    for raw in list {
        match raw.as_str() {
            "traktor-upload" => out.push(Task::TraktorUpload),
            other => anyhow::bail!(
                "unknown task `{other}` (known: {})",
                known_tasks().join(", ")
            ),
        }
    }
    if out.is_empty() {
        out.push(Task::TraktorUpload);
    }
    Ok(out)
}

/// All known task tokens.
pub fn known_tasks() -> [&'static str; 1] {
    ["traktor-upload"]
}

/// Everything one `kellner run` invocation needs.
pub struct RunOptions<'a> {
    pub tasks: Vec<Task>,
    /// Base poll interval before power scaling.
    pub base_interval: Duration,
    /// Run a single pass and exit.
    pub once: bool,
    /// Stretch the interval on battery (false = fixed `base_interval`).
    pub power_aware: bool,
    /// Treat the host as idle (longer intervals).
    pub idle: bool,
    pub hub: &'a HubClient,
    /// Explicit `collection.nml` override (else platform autodetect).
    pub traktor_nml: Option<PathBuf>,
    /// External stop flag (the tray's "feierabend" sets it); `None` = only Ctrl-C.
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// Run the selected tasks forever (or once), power-aware, until `Ctrl-C`
/// (or the optional `stop` flag is set).
pub async fn run(opts: RunOptions<'_>) -> Result<()> {
    loop {
        if let Some(stop) = &opts.stop
            && stop.load(std::sync::atomic::Ordering::SeqCst)
        {
            break;
        }
        for task in &opts.tasks {
            let result = match task {
                Task::TraktorUpload => traktor_cycle(opts.hub, opts.traktor_nml.as_deref()).await,
            };
            if let Err(e) = result {
                // A failed cycle must never kill the loop.
                eprintln!("kellner: task `{}` failed: {e:#}", task.as_str());
            }
        }

        if opts.once {
            break;
        }

        let wait = if opts.power_aware {
            let mode = PowerMode::detect();
            interval_for(mode, opts.base_interval, opts.idle)
        } else {
            opts.base_interval
        };

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = tokio::signal::ctrl_c() => {
                println!("kellner: shutdown requested — stopping");
                break;
            }
        }
    }
    Ok(())
}

/// One detection + change-check + (maybe) upload cycle for `collection.nml`.
pub async fn traktor_cycle(client: &HubClient, override_nml: Option<&Path>) -> Result<()> {
    let Some(path) = sync::traktor::detect(override_nml) else {
        println!(
            "kellner: Traktor collection.nml not found (pass --collection-nml or set KELLNER_TRAKTOR_NML)"
        );
        return Ok(());
    };
    let mtime = sync::traktor::mtime_nanos(&path)
        .ok_or_else(|| anyhow::anyhow!("cannot stat {}", path.display()))?;
    let mut state = state::load()?;
    if !state::needs_upload(&state, mtime) {
        println!(
            "kellner: {} unchanged since last upload — nothing to do",
            path.display()
        );
        return Ok(());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let stats = client
        .upload_traktor(bytes)
        .await
        .context("uploading collection.nml")?;
    state.last_upload_mtime = Some(mtime);
    state::save(&state)?;
    println!(
        "kellner: uploaded {} ({} entries, {} matched, {} playlists)",
        path.display(),
        stats.entries,
        stats.matched,
        stats.playlists
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_defaults_to_traktor_upload() {
        assert_eq!(parse_tasks(&[]).unwrap(), vec![Task::TraktorUpload]);
    }

    #[test]
    fn explicit_traktor_upload_parses() {
        let tasks = parse_tasks(&["traktor-upload".to_string()]).unwrap();
        assert_eq!(tasks, vec![Task::TraktorUpload]);
    }

    #[test]
    fn unknown_task_is_rejected() {
        let err = parse_tasks(&["backpack".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unknown task"));
    }

    #[test]
    fn known_tasks_lists_traktor_upload() {
        assert!(known_tasks().contains(&"traktor-upload"));
    }

    #[tokio::test]
    async fn once_returns_without_sleeping() {
        // With `--once` the loop runs a single pass and returns immediately
        // (no 60s sleep). On a host without a Traktor collection the pass is a
        // no-op, so no network is touched.
        let opts = RunOptions {
            tasks: vec![Task::TraktorUpload],
            base_interval: Duration::from_secs(60),
            once: true,
            power_aware: false,
            idle: false,
            hub: &crate::client::hub::HubClient::new("http://127.0.0.1:1", "token"),
            traktor_nml: None,
            stop: None,
        };
        let start = std::time::Instant::now();
        run(opts).await.unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "--once must not sleep the 60s interval"
        );
    }
}
