//! kellner — local, cross-platform companion for the music stack.
//!
//! The CLI surface is in place; most commands are still stubs. `pair`
//! (issue #362) is implemented: it registers this device with the hub and
//! persists the device token. kellner talks **only** to `music-api` (and the
//! hub) — never to Deezer / Spotify / SoundCloud / YouTube directly (see
//! `AGENT.md`).

mod client;
mod config;
mod pairing;
mod power;
mod service;
mod state;
mod sync;
mod tag;
mod tray;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "kellner",
    version = env!("BUILD_VERSION"),
    long_version = concat!(env!("BUILD_VERSION"), " (", env!("BUILD_GIT_DESCRIBE"), ")"),
    about = "Local companion for the music stack",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pair this device with the hub (device token).
    Pair(PairArgs),
    /// Backpack upkeep — keep the local Backpack in sync.
    Backpack(BackpackArgs),
    /// Back up the local music library to music-api.
    Backup(BackupArgs),
    /// Upload a Traktor `collection.nml` to the hub (device-token gated).
    TraktorUpload(TraktorUploadArgs),
    /// Order + download ISRCs, or show the download state without arguments.
    Downloads(DownloadsArgs),
    /// Run the background sync loop (the entry point the OS service executes).
    Run(RunArgs),
    /// Install / uninstall / inspect the launchd or systemd unit.
    Service(ServiceArgs),
}

#[derive(Debug, Args)]
struct PairArgs {
    /// Hub base URL (default: config value, else https://hub.zukkafabrik.de).
    #[arg(long)]
    hub_url: Option<String>,
    /// Device name shown in the approval prompt (default: hostname).
    #[arg(long)]
    name: Option<String>,
    /// Do not try to open a browser — just print the approval URL.
    #[arg(long)]
    no_open: bool,
    /// Register and print the approval URL, then exit (no binding, no config write).
    #[arg(long)]
    print_only: bool,
    /// Block on the confirmation Enter even without a TTY (no polling; #364).
    #[arg(long)]
    wait: bool,
    /// Re-pair even if a device identity is already stored.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct BackpackArgs {
    /// Collection directory (default: config `collection_dir`, env KELLNER_COLLECTION_DIR).
    #[arg(long)]
    root: Option<std::path::PathBuf>,
    /// Preferred delivery format: flac, 320 or 128 (default flac).
    #[arg(long)]
    format: Option<String>,
    /// Order priority (default 0).
    #[arg(long, default_value_t = 0)]
    priority: i64,
    /// Poll interval in seconds (default 5).
    #[arg(long, default_value_t = 5)]
    poll_secs: u64,
    /// Give up polling after this many seconds (default 600).
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
    /// Classify only: no order/fetch/tag/prune.
    #[arg(long)]
    dry_run: bool,
    /// Do not write the Comment tag.
    #[arg(long)]
    no_tags: bool,
}

#[derive(Debug, Args)]
struct BackupArgs {
    /// Classify only: run the presence check but upload nothing.
    #[arg(long)]
    dry_run: bool,
    /// Collection directory (default: config `collection_dir`, env KELLNER_COLLECTION_DIR).
    #[arg(long)]
    root: Option<std::path::PathBuf>,
    /// Run once and exit. Accepted for forward-compat; v1 is always a one-shot.
    #[arg(long)]
    once: bool,
}

#[derive(Debug, Args)]
struct DownloadsArgs {
    /// ISRC to order and fetch (repeatable).
    #[arg(long = "isrc", value_name = "ISRC")]
    isrc: Vec<String>,
    /// File with ISRCs — one per line and/or comma-separated.
    #[arg(long = "isrcs", value_name = "FILE")]
    isrcs: Option<std::path::PathBuf>,
    /// Preferred delivery format: flac, 320 or 128 (default flac).
    #[arg(long)]
    format: Option<String>,
    /// Order priority (default 0).
    #[arg(long, default_value_t = 0)]
    priority: i64,
    /// Poll interval in seconds (default 5).
    #[arg(long, default_value_t = 5)]
    poll_secs: u64,
    /// Give up polling after this many seconds (default 600).
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
    /// Collection directory (default: config `collection_dir`, env KELLNER_COLLECTION_DIR).
    #[arg(long)]
    root: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Run a single pass and exit (no loop).
    #[arg(long)]
    once: bool,
    /// Base poll interval in seconds (default: config value, else 300).
    #[arg(long)]
    interval_secs: Option<u64>,
    /// Tasks to run (repeatable; default: traktor-upload).
    #[arg(long = "tasks", value_name = "TASK")]
    tasks: Vec<String>,
    /// Use a fixed interval instead of scaling it on battery.
    #[arg(long)]
    no_power_awareness: bool,
    /// Explicit `collection.nml` path (default: config value, else platform autodetect).
    #[arg(long)]
    collection_nml: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
struct ServiceArgs {
    #[command(subcommand)]
    action: ServiceCommand,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Write the unit file (macOS plist / Linux user unit) or print the Windows hint.
    Install(ServiceInstallArgs),
    /// Remove the installed unit file.
    Uninstall(ServiceUninstallArgs),
    /// Report whether a unit file is installed.
    Status,
}

#[derive(Debug, Args)]
struct ServiceInstallArgs {
    /// Print the unit text only; write nothing.
    #[arg(long)]
    print: bool,
    /// Show what would be written without touching the filesystem.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct ServiceUninstallArgs {
    /// Show what would be removed without touching the filesystem.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct TraktorUploadArgs {
    /// Explicit `collection.nml` path (default: config value, else platform autodetect).
    #[arg(long)]
    collection_nml: Option<std::path::PathBuf>,
    /// Upload at most once and exit (no watch loop).
    #[arg(long)]
    once: bool,
    /// Poll interval in seconds (default: config value, else 300).
    #[arg(long)]
    interval_secs: Option<u64>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Pair(args) => return pair(args),
        Command::Backpack(args) => return backpack(args),
        Command::Backup(args) => return backup(args),
        Command::TraktorUpload(args) => return traktor_upload(args),
        Command::Downloads(args) => return downloads(args),
        Command::Run(args) => return run_cmd(args),
        Command::Service(args) => return service_cmd(args),
    }
}

fn pair(args: PairArgs) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let mut cfg = config::load()?;
        let hub_url = args.hub_url.clone().unwrap_or_else(|| cfg.hub_url());
        let name = args.name.clone().unwrap_or_else(default_hostname);
        let opts = pairing::PairOptions {
            hub_url: hub_url.clone(),
            name,
            no_open: args.no_open,
            print_only: args.print_only,
            wait: args.wait,
            force: args.force,
            interactive: true,
        };
        let transport = pairing::HttpHubTransport::new(hub_url);
        pairing::run_pair(&mut cfg, &transport, &opts).await
    })
}

fn backpack(args: BackpackArgs) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let cfg = config::load()?;
        let device = cfg.device.clone().ok_or_else(|| {
            anyhow::anyhow!("this device is not paired yet — run `kellner pair` first")
        })?;
        let hub_url = cfg.hub_url();
        let base = cfg.music_api_url().ok_or_else(|| {
            anyhow::anyhow!(
                "music-api URL not configured — set KELLNER_MUSIC_API_URL (or `music_api_url` in config)"
            )
        })?;
        let token = cfg.music_api_token.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "music-api token not configured — set KELLNER_MUSIC_API_TOKEN (or `music_api_token` in config)"
            )
        })?;
        let root = args.root.clone().or(cfg.collection_dir.clone()).ok_or_else(|| {
            anyhow::anyhow!(
                "collection directory not configured — pass --root or set KELLNER_COLLECTION_DIR"
            )
        })?;
        if !root.is_dir() {
            anyhow::bail!("collection directory {} does not exist", root.display());
        }

        let hub = client::hub::HubClient::new(hub_url, device.device_token);
        let api = client::api::ApiClient::new(base, token);
        let opts = sync::backpack::BackpackOptions {
            root,
            format: args.format.clone(),
            priority: args.priority,
            poll_secs: args.poll_secs.max(1),
            timeout_secs: args.timeout_secs,
            dry_run: args.dry_run,
            write_tags: !args.no_tags,
        };
        let stats = sync::backpack::run_backpack(&hub, &api, &opts).await?;
        if stats.failed > 0 {
            anyhow::bail!("backpack finished with {} failure(s)", stats.failed);
        }
        if stats.pending > 0 {
            anyhow::bail!(
                "{} ISRC(s) still pending after {}s",
                stats.pending, args.timeout_secs
            );
        }
        Ok(())
    })
}

fn backup(args: BackupArgs) -> Result<()> {
    let _ = args.once; // v1 is a one-shot; the flag is accepted for the future loop.
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let cfg = config::load()?;
        let base = cfg.music_api_url().ok_or_else(|| {
            anyhow::anyhow!(
                "music-api URL not configured — set KELLNER_MUSIC_API_URL (or `music_api_url` in config)"
            )
        })?;
        let token = cfg.music_api_token.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "music-api token not configured — set KELLNER_MUSIC_API_TOKEN (or `music_api_token` in config)"
            )
        })?;
        let root = args
            .root
            .clone()
            .or(cfg.collection_dir.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "collection directory not configured — pass --root or set KELLNER_COLLECTION_DIR"
                )
            })?;
        if !root.is_dir() {
            anyhow::bail!("collection directory {} does not exist", root.display());
        }

        let client = client::api::ApiClient::new(base, token);
        let opts = sync::backup::BackupOptions {
            root,
            dry_run: args.dry_run,
        };
        let stats = sync::backup::run_backup(&client, &opts).await?;
        if stats.failed > 0 {
            anyhow::bail!("backup finished with {} failure(s)", stats.failed);
        }
        Ok(())
    })
}

fn downloads(args: DownloadsArgs) -> Result<()> {
    // Collect ISRCs from --isrc and --isrcs <file> (lines and/or commas).
    let mut isrcs: Vec<String> = args.isrc.clone();
    if let Some(file) = &args.isrcs {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("reading ISRC file {}", file.display()))?;
        for line in text.lines() {
            for part in line.split(',') {
                let t = part.trim();
                if !t.is_empty() {
                    isrcs.push(t.to_string());
                }
            }
        }
    }

    // US4: no ISRCs → read-only status report, no network call.
    if isrcs.is_empty() {
        let state = state::load()?;
        sync::downloads::report_status(&state);
        return Ok(());
    }

    let preferred = args.format.clone();
    let priority = args.priority;
    let poll_secs = args.poll_secs.max(1);
    let timeout_secs = args.timeout_secs;
    let root_arg = args.root.clone();

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let cfg = config::load()?;
        let base = cfg.music_api_url().ok_or_else(|| {
            anyhow::anyhow!(
                "music-api URL not configured — set KELLNER_MUSIC_API_URL (or `music_api_url` in config)"
            )
        })?;
        let token = cfg.music_api_token.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "music-api token not configured — set KELLNER_MUSIC_API_TOKEN (or `music_api_token` in config)"
            )
        })?;
        let root = root_arg.or(cfg.collection_dir.clone()).ok_or_else(|| {
            anyhow::anyhow!(
                "collection directory not configured — pass --root or set KELLNER_COLLECTION_DIR"
            )
        })?;
        if !root.is_dir() {
            anyhow::bail!("collection directory {} does not exist", root.display());
        }

        let client = client::api::ApiClient::new(base, token);
        let opts = sync::downloads::DownloadOptions {
            isrcs,
            format: preferred,
            priority,
            poll_secs,
            timeout_secs,
            root,
        };
        let stats = sync::downloads::run_downloads(&client, &opts).await?;
        if stats.failed > 0 {
            anyhow::bail!("downloads finished with {} failure(s)", stats.failed);
        }
        if stats.pending > 0 {
            anyhow::bail!(
                "{} ISRC(s) still pending after {}s",
                stats.pending, timeout_secs
            );
        }
        Ok(())
    })
}

fn traktor_upload(args: TraktorUploadArgs) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let cfg = config::load()?;
        let device = cfg.device.clone().ok_or_else(|| {
            anyhow::anyhow!("this device is not paired yet — run `kellner pair` first")
        })?;
        let hub = client::hub::HubClient::new(cfg.hub_url(), device.device_token);
        let interval = args
            .interval_secs
            .or(cfg.traktor_poll_secs)
            .unwrap_or(300)
            .max(1);
        let override_nml = args.collection_nml.clone().or(cfg.traktor_nml.clone());
        let opts = sync::run::RunOptions {
            tasks: vec![sync::run::Task::TraktorUpload],
            base_interval: Duration::from_secs(interval),
            once: args.once,
            power_aware: false,
            idle: false,
            hub: &hub,
            traktor_nml: override_nml,
            stop: None,
        };
        sync::run::run(opts).await
    })
}

/// `kellner run` — the background loop the OS service executes.
fn run_cmd(args: RunArgs) -> Result<()> {
    #[cfg(feature = "tray")]
    if !args.once {
        return run_with_tray(args);
    }

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let cfg = config::load()?;
        let device = cfg.device.clone().ok_or_else(|| {
            anyhow::anyhow!("this device is not paired yet — run `kellner pair` first")
        })?;
        let tasks = sync::run::parse_tasks(&args.tasks)?;
        let base = args
            .interval_secs
            .or(cfg.traktor_poll_secs)
            .unwrap_or(300)
            .max(1);
        let override_nml = args.collection_nml.clone().or(cfg.traktor_nml.clone());
        let hub = client::hub::HubClient::new(cfg.hub_url(), device.device_token);
        let opts = sync::run::RunOptions {
            tasks,
            base_interval: Duration::from_secs(base),
            once: args.once,
            power_aware: !args.no_power_awareness,
            idle: false,
            hub: &hub,
            traktor_nml: override_nml,
            stop: None,
        };
        sync::run::run(opts).await
    })
}

/// `kellner service …` — writes/inspects the launchd / systemd unit.
fn service_cmd(args: ServiceArgs) -> Result<()> {
    let opts = match args.action {
        ServiceCommand::Install(a) => service::ServiceOptions {
            action: service::ServiceAction::Install,
            print: a.print,
            dry_run: a.dry_run,
        },
        ServiceCommand::Uninstall(a) => service::ServiceOptions {
            action: service::ServiceAction::Uninstall,
            print: false,
            dry_run: a.dry_run,
        },
        ServiceCommand::Status => service::ServiceOptions {
            action: service::ServiceAction::Status,
            print: false,
            dry_run: false,
        },
    };
    service::run(&opts)
}

/// With the `tray` feature, run the sync loop on a worker thread and the
/// menu-bar widget on the main thread; "feierabend" stops both.
#[cfg(feature = "tray")]
fn run_with_tray(args: RunArgs) -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let stop = Arc::new(AtomicBool::new(false));
    let stop_loop = stop.clone();
    let worker = std::thread::spawn(move || -> Result<()> {
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async move {
            let cfg = config::load()?;
            let device = cfg.device.clone().ok_or_else(|| {
                anyhow::anyhow!("this device is not paired yet — run `kellner pair` first")
            })?;
            let tasks = sync::run::parse_tasks(&args.tasks)?;
            let base = args
                .interval_secs
                .or(cfg.traktor_poll_secs)
                .unwrap_or(300)
                .max(1);
            let override_nml = args.collection_nml.clone().or(cfg.traktor_nml.clone());
            let hub = client::hub::HubClient::new(cfg.hub_url(), device.device_token);
            let opts = sync::run::RunOptions {
                tasks,
                base_interval: Duration::from_secs(base),
                once: false,
                power_aware: !args.no_power_awareness,
                idle: false,
                hub: &hub,
                traktor_nml: override_nml,
                stop: Some(stop_loop),
            };
            sync::run::run(opts).await
        })
    });

    let stop_tray = stop.clone();
    let on_reauth = || {
        if let Err(e) = re_auth() {
            eprintln!("kellner: re-auth failed: {e:#}");
        }
    };
    let tray_result = tray::run(stop_tray, on_reauth);
    stop.store(true, Ordering::SeqCst);
    let _ = worker.join();
    tray_result
}

/// Re-run pairing (the tray's "re-auth" action): same flow as `kellner pair --force`.
#[cfg(feature = "tray")]
fn re_auth() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let mut cfg = config::load()?;
        let hub_url = cfg.hub_url();
        let opts = pairing::PairOptions {
            hub_url: hub_url.clone(),
            name: default_hostname(),
            no_open: false,
            print_only: false,
            wait: true,
            force: true,
            interactive: true,
        };
        let transport = pairing::HttpHubTransport::new(hub_url);
        pairing::run_pair(&mut cfg, &transport, &opts).await
    })
}

fn default_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "kellner-device".to_string())
}

/// Placeholder for a command that is not implemented yet.
#[allow(dead_code)]
fn not_yet(command: &str) {
    println!("kellner {command}: not yet implemented");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_pair_flags() {
        let cli = Cli::try_parse_from([
            "kellner",
            "pair",
            "--hub-url",
            "https://hub.example.test",
            "--name",
            "my-mac",
            "--no-open",
            "--print-only",
            "--wait",
        ])
        .unwrap();

        match cli.command {
            Command::Pair(args) => {
                assert_eq!(args.hub_url.as_deref(), Some("https://hub.example.test"));
                assert_eq!(args.name.as_deref(), Some("my-mac"));
                assert!(args.no_open);
                assert!(args.print_only);
                assert!(args.wait);
                assert!(!args.force);
            }
            other => panic!("expected Pair, got {other:?}"),
        }
    }

    #[test]
    fn existing_stubs_unchanged() {
        for (argv, expected) in [
            ("backpack", "backpack"),
            ("backup", "backup"),
            ("traktor-upload", "traktor-upload"),
            ("downloads", "downloads"),
        ] {
            let cli = Cli::try_parse_from(["kellner", argv]).unwrap();
            let got = match cli.command {
                Command::Backpack(_) => "backpack",
                Command::Backup(_) => "backup",
                Command::TraktorUpload(_) => "traktor-upload",
                Command::Downloads(_) => "downloads",
                Command::Pair(_) => "pair",
                Command::Run(_) => "run",
                Command::Service(_) => "service",
            };
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn cli_parses_backup_flags() {
        let cli = Cli::try_parse_from(["kellner", "backup", "--dry-run", "--root", "/music", "--once"])
            .unwrap();
        match cli.command {
            Command::Backup(args) => {
                assert!(args.dry_run);
                assert_eq!(args.root.as_deref(), Some(std::path::Path::new("/music")));
                assert!(args.once);
            }
            other => panic!("expected Backup, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_run_flags() {
        let cli = Cli::try_parse_from([
            "kellner",
            "run",
            "--once",
            "--interval-secs",
            "120",
            "--tasks",
            "traktor-upload",
            "--no-power-awareness",
        ])
        .unwrap();
        match cli.command {
            Command::Run(args) => {
                assert!(args.once);
                assert_eq!(args.interval_secs, Some(120));
                assert_eq!(args.tasks, vec!["traktor-upload".to_string()]);
                assert!(args.no_power_awareness);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_service_install_flags() {
        let cli =
            Cli::try_parse_from(["kellner", "service", "install", "--print", "--dry-run"]).unwrap();
        match cli.command {
            Command::Service(args) => match args.action {
                ServiceCommand::Install(a) => {
                    assert!(a.print);
                    assert!(a.dry_run);
                }
                other => panic!("expected Install, got {other:?}"),
            },
            other => panic!("expected Service, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_service_uninstall_and_status() {
        let uninstall = Cli::try_parse_from(["kellner", "service", "uninstall", "--dry-run"]).unwrap();
        match uninstall.command {
            Command::Service(args) => match args.action {
                ServiceCommand::Uninstall(a) => assert!(a.dry_run),
                other => panic!("expected Uninstall, got {other:?}"),
            },
            other => panic!("expected Service, got {other:?}"),
        }

        let status = Cli::try_parse_from(["kellner", "service", "status"]).unwrap();
        match status.command {
            Command::Service(args) => {
                assert!(matches!(args.action, ServiceCommand::Status));
            }
            other => panic!("expected Service, got {other:?}"),
        }
    }
}
