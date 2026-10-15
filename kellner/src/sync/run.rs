//! Background sync loop + `kellner run` core (issue #368, plan phase P5).
//!
//! `kellner run` is what the launchd / systemd unit executes. It repeatedly
//! runs the selected sync tasks (currently only the Traktor upload) and then
//! sleeps — with the sleep stretched by [`crate::power::interval_for`] when the
//! host runs on battery. A failing cycle is logged and never aborts the loop;
//! `Ctrl-C` (SIGINT), a `SIGTERM` (how systemd stops a user unit) or the tray's
//! `stop` flag stop it — the sleep itself is interruptible, so none of these
//! waits out the current interval.
//!
//! The loop and the cycle live here (not in `main.rs`) so they can be exercised
//! by tests without the full CLI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::client::hub::HubClient;
use crate::power::{MIN_INTERVAL, PowerMode, interval_for};
use crate::{state, sync};

/// How often the loop re-checks the `stop` flag while sleeping.
const STOP_POLL: Duration = Duration::from_millis(500);

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
    /// External stop flag (the tray's "feierabend" sets it); `None` = only signals.
    pub stop: Option<Arc<AtomicBool>>,
}

/// Run the selected tasks forever (or once), power-aware, until `Ctrl-C`
/// (or the optional `stop` flag is set).
pub async fn run(opts: RunOptions<'_>) -> Result<()> {
    loop {
        if let Some(stop) = &opts.stop
            && stop.load(Ordering::SeqCst)
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
            // Apply the same hard floor `interval_for` enforces, so
            // `--no-power-awareness --interval-secs 1` cannot spin below it.
            opts.base_interval.max(MIN_INTERVAL)
        };

        tokio::select! {
            stopped = sleep_or_stop(wait, opts.stop.as_ref()) => {
                if stopped {
                    println!("kellner: shutdown requested — stopping");
                    break;
                }
            }
            _ = shutdown_signal() => {
                println!("kellner: shutdown requested — stopping");
                break;
            }
        }
    }
    Ok(())
}

/// Sleep for `wait`, returning `true` early if `stop` becomes set.
///
/// The flag is polled every [`STOP_POLL`] so the tray's "feierabend" takes
/// effect promptly; checking it only at the loop head (the previous behaviour)
/// let a quit hang for a whole power-scaled interval. Without a flag this is a
/// plain sleep that always returns `false`.
async fn sleep_or_stop(wait: Duration, stop: Option<&Arc<AtomicBool>>) -> bool {
    let Some(flag) = stop else {
        tokio::time::sleep(wait).await;
        return false;
    };
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        tokio::time::sleep(STOP_POLL.min(deadline - now)).await;
    }
}

/// Resolve on SIGINT (Ctrl-C) or, on Unix, SIGTERM — how `systemctl --user
/// stop kellner` terminates a unit by default.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
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

    #[tokio::test]
    async fn stop_flag_interrupts_the_sleep() {
        // The tray's "feierabend" sets this flag; the loop must react promptly
        // instead of waiting out the whole (here hour-long) interval.
        let stop = Arc::new(AtomicBool::new(false));
        let setter = stop.clone();
        let opts = RunOptions {
            tasks: vec![Task::TraktorUpload],
            base_interval: Duration::from_secs(3600),
            once: false,
            power_aware: false,
            idle: false,
            hub: &crate::client::hub::HubClient::new("http://127.0.0.1:1", "token"),
            traktor_nml: None,
            stop: Some(stop),
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            setter.store(true, Ordering::SeqCst);
        });
        let start = std::time::Instant::now();
        run(opts).await.unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "stop flag must interrupt the 3600s sleep promptly"
        );
    }
}
