//! macOS Launch Agent management (launchd plist).
//!
//! Provides `install`, `uninstall`, and `status` subcommands for managing a
//! `com.momo.music-manager` launchd agent that auto-starts the server on login.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Label used for the launchd plist — must match the filename.
const LABEL: &str = "com.momo.music-manager";

/// Filename for the plist inside `~/Library/LaunchAgents/`.
const PLIST_FILENAME: &str = "com.momo.music-manager.plist";

/// Relative path (under `~/Library/Logs/`) for service logs.
const LOG_SUBDIR: &str = "momos-music-manager";

/// Working directory used in the plist.
const WORK_DIR: &str = "/usr/local/var/momos-music-manager";

// ── Path helpers ───────────────────────────────────────────────────────────

/// `~/Library/LaunchAgents/com.momo.music-manager.plist`
fn plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join("Library/LaunchAgents").join(PLIST_FILENAME))
}

/// `~/Library/Logs/momos-music-manager/`
fn log_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join("Library/Logs").join(LOG_SUBDIR))
}

/// Resolve the absolute path to the running binary.
fn binary_path() -> Result<String> {
    let path = std::env::current_exe()
        .context("Could not determine binary path — is the binary compiled?")?;
    Ok(path.to_string_lossy().to_string())
}

// ── Plist rendering ────────────────────────────────────────────────────────

/// Build the plist XML content with placeholders filled in.
fn render_plist(bin: &str, log: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>serve</string>
        <string>--host</string>
        <string>127.0.0.1</string>
        <string>--port</string>
        <string>3000</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>{log}/stderr.log</string>
    <key>WorkingDirectory</key>
    <string>{work_dir}</string>
</dict>
</plist>
"#,
        label = LABEL,
        bin = bin,
        log = log,
        work_dir = WORK_DIR,
    )
}

// ── launchctl helpers ──────────────────────────────────────────────────────

/// Get the current user's UID by running `id -u`.
fn current_uid() -> Result<u32> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("Failed to run id -u")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let uid_str = stdout.trim();
    uid_str
        .parse::<u32>()
        .with_context(|| format!("Failed to parse UID from: {uid_str:?}"))
}

/// Run `launchctl bootstrap` (macOS 11+) or fall back to `launchctl load`.
fn launchctl_bootstrap(plist: &PathBuf) -> Result<()> {
    let uid = current_uid()?;

    // Try `launchctl bootstrap gui/$UID` first (macOS 11+ Big Sur+).
    let bootstrap_result = std::process::Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{}", uid)])
        .arg(plist)
        .output();

    match bootstrap_result {
        Ok(output) if output.status.success() => {
            return Ok(());
        }
        Ok(output) => {
            // If bootstrap failed (e.g. on older macOS), fall through to load.
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("launchctl bootstrap failed (will try load): {stderr}");
        }
        Err(e) => {
            eprintln!("Could not run launchctl bootstrap: {e} (will try load)");
        }
    }

    // Fallback: `launchctl load -w` (pre-macOS 11).
    let load_output = std::process::Command::new("launchctl")
        .args(["load", "-w"])
        .arg(plist)
        .output()
        .with_context(|| "Failed to run launchctl load (fallback)")?;

    if !load_output.status.success() {
        let stderr = String::from_utf8_lossy(&load_output.stderr);
        anyhow::bail!("launchctl load failed: {stderr}");
    }

    Ok(())
}

/// Run `launchctl bootout` (macOS 11+) or fall back to `launchctl unload`.
fn launchctl_bootout(label: &str) -> Result<()> {
    let uid = current_uid()?;

    // Try `launchctl bootout gui/$UID/label` first (macOS 11+).
    let bootout_result = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{}/{}", uid, label)])
        .output();

    match bootout_result {
        Ok(output) if output.status.success() => {
            return Ok(());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // If service wasn't loaded, that's fine too
            if stderr.contains("inactive") || stderr.contains("Could not find") {
                return Ok(());
            }
            eprintln!("launchctl bootout failed (will try unload): {stderr}");
        }
        Err(e) => {
            eprintln!("Could not run launchctl bootout: {e} (will try unload)");
        }
    }

    // Fallback: `launchctl unload -w` (pre-macOS 11).
    let plist = plist_path()?;
    let unload_output = std::process::Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(&plist)
        .output()
        .with_context(|| "Failed to run launchctl unload (fallback)")?;

    if !unload_output.status.success() {
        let stderr = String::from_utf8_lossy(&unload_output.stderr);
        // If it's already unloaded, that's fine
        if !stderr.contains("inactive") && !stderr.contains("Could not find") {
            anyhow::bail!("launchctl unload failed: {stderr}");
        }
    }

    Ok(())
}

/// Check whether the launch agent is loaded via `launchctl print`.
fn launchctl_is_loaded(label: &str) -> Result<bool> {
    let uid = match current_uid() {
        Ok(uid) => uid,
        Err(_) => return Ok(false),
    };
    let output = std::process::Command::new("launchctl")
        .args(["print", &format!("gui/{}/{}", uid, label)])
        .output()
        .with_context(|| "Failed to run launchctl print")?;

    Ok(output.status.success())
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Install the launch agent plist and load it into launchd.
///
/// 1. Creates `~/Library/Logs/momos-music-manager/` (if missing).
/// 2. Writes the plist to `~/Library/LaunchAgents/com.momo.music-manager.plist`.
/// 3. Runs `launchctl bootstrap` (or `launchctl load` on older macOS).
pub fn install() -> Result<()> {
    let plist = plist_path()?;
    let log = log_dir()?;
    let bin = binary_path()?;

    // Create log directory
    std::fs::create_dir_all(&log)
        .with_context(|| format!("Failed to create log directory: {}", log.display()))?;

    // Create parent directory for plist (LaunchAgents should exist, but be safe)
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Write plist
    let plist_content = render_plist(&bin, &log.to_string_lossy());
    std::fs::write(&plist, &plist_content)
        .with_context(|| format!("Failed to write plist: {}", plist.display()))?;

    println!("📄 Wrote plist to {}", plist.display());
    println!("   Binary:   {bin}");
    println!("   Log dir:  {}", log.display());
    println!("   Work dir: {WORK_DIR}");

    // Load into launchd
    launchctl_bootstrap(&plist)?;

    println!("✅ Launch agent installed and loaded.");
    println!("   The server will auto-start on login and restart on crash.");
    println!();
    println!("   Run `cargo run -- service-status` to check status.");
    println!("   Run `cargo run -- uninstall-launch-agent` to remove.");

    Ok(())
}

/// Unload and remove the launch agent.
///
/// 1. Runs `launchctl bootout` (or `launchctl unload`).
/// 2. Deletes the plist file.
pub fn uninstall() -> Result<()> {
    let plist = plist_path()?;

    // Unload from launchd
    launchctl_bootout(LABEL)?;

    // Remove plist file
    if plist.exists() {
        std::fs::remove_file(&plist)
            .with_context(|| format!("Failed to remove plist: {}", plist.display()))?;
        println!("🗑️  Removed plist: {}", plist.display());
    } else {
        println!("ℹ️  Plist not found (already removed): {}", plist.display());
    }

    println!("✅ Launch agent uninstalled.");

    Ok(())
}

/// Print a human-readable status of the launch agent.
pub fn status() -> Result<String> {
    let plist = plist_path()?;
    let loaded = launchctl_is_loaded(LABEL).unwrap_or(false);

    let plist_exists = plist.exists();

    let mut lines = Vec::new();

    lines.push(format!("Launch Agent: {LABEL}"));
    lines.push(format!("  Plist path: {}", plist.display()));
    lines.push(format!(
        "  Plist file: {}",
        if plist_exists {
            "✅ present"
        } else {
            "❌ missing"
        }
    ));
    lines.push(format!(
        "  Loaded:     {}",
        if loaded {
            "✅ loaded in launchd"
        } else {
            "❌ not loaded"
        }
    ));

    if plist_exists {
        // Read and show the plist content (abbreviated)
        if let Ok(content) = std::fs::read_to_string(&plist) {
            // Extract binary path from plist for convenience
            if let Some(bin_line) = content
                .lines()
                .skip_while(|l| !l.contains("ProgramArguments"))
                .nth(2)
            {
                let bin = bin_line
                    .trim()
                    .trim_start_matches("<string>")
                    .trim_end_matches("</string>");
                lines.push(format!("  Binary:     {bin}"));
            }
        }
    }

    Ok(lines.join("\n"))
}
