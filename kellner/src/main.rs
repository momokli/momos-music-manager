//! kellner — local, cross-platform companion for the music stack.
//!
//! Skeleton only: the CLI surface is in place, the commands are stubs. kellner
//! talks **only** to `music-api` (never to Deezer / Spotify / SoundCloud /
//! YouTube directly) — see `AGENT.md`.

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// Backpack upkeep — keep the local Backpack in sync.
    Backpack,
    /// Back up the local music library to music-api.
    Backup,
    /// Upload a Traktor `collection.nml` to music-api.
    TraktorUpload,
    /// Show the state of downloads and the Backpack.
    Downloads,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Backpack => not_yet("backpack"),
        Command::Backup => not_yet("backup"),
        Command::TraktorUpload => not_yet("traktor-upload"),
        Command::Downloads => not_yet("downloads"),
    }

    Ok(())
}

/// Placeholder for a command that is not implemented yet.
fn not_yet(command: &str) {
    println!("kellner {command}: not yet implemented");
}
