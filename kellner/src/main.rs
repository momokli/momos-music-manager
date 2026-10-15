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
mod state;
mod sync;

use std::path::Path;

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
    Backpack,
    /// Back up the local music library to music-api.
    Backup,
    /// Upload a Traktor `collection.nml` to the hub (device-token gated).
    TraktorUpload(TraktorUploadArgs),
    /// Show the state of downloads and the Backpack.
    Downloads,
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
        Command::Backpack => not_yet("backpack"),
        Command::Backup => not_yet("backup"),
        Command::TraktorUpload(args) => return traktor_upload(args),
        Command::Downloads => not_yet("downloads"),
    }

    Ok(())
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

fn traktor_upload(args: TraktorUploadArgs) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let cfg = config::load()?;
        let device = cfg.device.clone().ok_or_else(|| {
            anyhow::anyhow!("this device is not paired yet — run `kellner pair` first")
        })?;
        let hub_url = cfg.hub_url();
        let interval = args
            .interval_secs
            .or(cfg.traktor_poll_secs)
            .unwrap_or(300)
            .max(1);
        let override_nml = args.collection_nml.clone().or(cfg.traktor_nml.clone());
        let client = client::hub::HubClient::new(hub_url, device.device_token);

        loop {
            if let Err(e) = sync_once(&client, override_nml.as_deref()).await {
                eprintln!("kellner: traktor upload failed: {e:#}");
            }
            if args.once {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        }
        Ok(())
    })
}

/// One detection + change-check + (maybe) upload cycle.
async fn sync_once(client: &client::hub::HubClient, override_nml: Option<&Path>) -> Result<()> {
    let Some(path) = sync::traktor::detect(override_nml) else {
        println!(
            "kellner: Traktor collection.nml not found (pass --collection-nml or set KELLNER_TRAKTOR_NML)"
        );
        return Ok(());
    };
    let mtime = sync::traktor::mtime_nanos(&path)
        .ok_or_else(|| anyhow::anyhow!("cannot stat {}", path.display()))?;
    let state = state::load()?;
    if !state::needs_upload(&state, mtime) {
        println!("kellner: {} unchanged since last upload — nothing to do", path.display());
        return Ok(());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let stats = client
        .upload_traktor(bytes)
        .await
        .context("uploading collection.nml")?;
    let mut state = state;
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

fn default_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "kellner-device".to_string())
}

/// Placeholder for a command that is not implemented yet.
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
                Command::Backpack => "backpack",
                Command::Backup => "backup",
                Command::TraktorUpload(_) => "traktor-upload",
                Command::Downloads => "downloads",
                Command::Pair(_) => "pair",
            };
            assert_eq!(got, expected);
        }
    }
}
