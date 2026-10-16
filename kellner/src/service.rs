//! OS service units — `launchd` (macOS) / `systemd` (Linux) / Task Scheduler (Windows)
//! (issue #368, plan phase P5).
//!
//! The unit *text* is produced by pure, parameter-only builders ([`launchd_plist`],
//! [`systemd_user_unit`], [`windows_task_hint`]) so it can be snapshot-tested on any
//! host. The `kellner service install|uninstall|status` logic then just resolves the
//! per-OS target path, writes/removes the file atomically, and prints the follow-up
//! commands the user still has to run (`launchctl`, `systemctl`).
//!
//! The unit runs `kellner run` (the background loop, see [`crate::sync::run`]) and
//! carries the battery-friendly launchd keys (`LowPriorityIO`, `Nice`,
//! `ProcessType=Background`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// launchd label / systemd unit name — used for both the file name and the unit.
#[allow(dead_code)] // only referenced by the macOS code path and tests.
pub const SERVICE_LABEL: &str = "de.zukkafabrik.kellner";
/// systemd user unit base name (file `kellner.service`).
#[allow(dead_code)] // only referenced by the Linux code path and tests.
pub const SYSTEMD_NAME: &str = "kellner";
/// The subcommand the service runs.
pub const SERVICE_ARGS: [&str; 1] = ["run"];

/// Parameters for the macOS LaunchAgent plist (no host lookups inside the builder).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // constructed by the macOS code path and by tests.
pub struct LaunchdParams {
    /// `Label` value, e.g. `de.zukkafabrik.kellner`.
    pub label: String,
    /// Absolute path to the `kellner` binary.
    pub program: String,
    /// Arguments passed to the program (without the program itself).
    pub args: Vec<String>,
    /// Path for captured stdout (`StandardOutPath`).
    pub stdout: String,
    /// Path for captured stderr (`StandardErrorPath`).
    pub stderr: String,
}

/// Parameters for the Linux user unit (no host lookups inside the builder).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // constructed by the Linux code path and by tests.
pub struct SystemdParams {
    /// Unit base name, e.g. `kellner` (file `kellner.service`).
    pub name: String,
    /// `Description=` text.
    pub description: String,
    /// Absolute path to the `kellner` binary.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
}

/// Build the complete macOS LaunchAgent `plist`.
///
/// Battery-friendly: `ProcessType=Background`, `Nice=10`, `LowPriorityIO=true`,
/// plus `RunAtLoad` and `KeepAlive` so the loop survives account login.
#[allow(dead_code)] // used on macOS and by tests.
pub fn launchd_plist(p: &LaunchdParams) -> String {
    let args = plist_args(&p.program, &p.args);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args}    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>Nice</key>
    <integer>10</integer>
    <key>LowPriorityIO</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>
"#,
        label = p.label,
        args = args,
        stdout = p.stdout,
        stderr = p.stderr,
    )
}

#[allow(dead_code)] // helper for `launchd_plist`.
fn plist_args(program: &str, args: &[String]) -> String {
    let mut out = format!("        <string>{}</string>\n", xml_escape(program));
    for a in args {
        out.push_str(&format!("        <string>{}</string>\n", xml_escape(a)));
    }
    out
}

#[allow(dead_code)] // helper for `launchd_plist`.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build the complete Linux **user** systemd unit.
#[allow(dead_code)] // used on Linux and by tests.
pub fn systemd_user_unit(p: &SystemdParams) -> String {
    let exec = std::iter::once(p.program.clone())
        .chain(p.args.iter().cloned())
        .map(|t| systemd_quote(&t))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\n\
         Description={description}\n\
         After=network.target\n\
         Wants=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         Nice=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        description = p.description,
        exec = exec,
    )
}

/// Quote one argv token for a systemd `ExecStart=` line.
///
/// systemd splits `ExecStart` on whitespace but honours double quotes, so an
/// absolute path containing a space (e.g. `/Users/foo bar/kellner`) would
/// otherwise be split into two arguments. Tokens made only of "safe"
/// characters stay unquoted, keeping the common case readable.
#[allow(dead_code)] // used on Linux and by tests.
fn systemd_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '@' | '+')
        });
    if safe {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Human-readable hint for the Windows Task Scheduler (no file is written).
pub fn windows_task_hint(program: &str) -> String {
    let args = SERVICE_ARGS.join(" ");
    format!(
        "Windows uses the Task Scheduler. Register the loop with e.g.:\n\
         \n\
         schtasks /Create /TN \"kellner\" /SC ONLOGON /RL LIMITED \\\n\
         \x20 /TR \"\\\"{program}\\\" {args}\"\n\
         \n\
         (kellner does not write a unit file on Windows.)",
        program = program,
        args = args,
    )
}

/// What `kellner service` should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Install,
    Uninstall,
    Status,
}

/// Options for one `kellner service` invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptions {
    pub action: ServiceAction,
    /// Print the unit text only; write nothing (install).
    pub print: bool,
    /// Compute but do not touch the filesystem (install/uninstall).
    pub dry_run: bool,
}

/// Resolve the per-OS service file path.
///
/// macOS: `~/Library/LaunchAgents/<label>.plist`
/// Linux: `$XDG_CONFIG_HOME/systemd/user/<name>.service` (else `~/.config/...`)
/// Windows/other: `None` (no unit file — see [`windows_task_hint`]).
pub fn unit_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(
            home.join("Library")
                .join("LaunchAgents")
                .join(format!("{SERVICE_LABEL}.plist")),
        )
    }
    #[cfg(target_os = "linux")]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(
            base.join("systemd")
                .join("user")
                .join(format!("{SYSTEMD_NAME}.service")),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// The program path to embed — the running binary, or a plain name as a fallback.
fn program_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<path-to-kellner>".to_string())
}

/// Render the unit text for the current OS (same input as what `install` writes).
pub fn render_unit() -> Result<String> {
    let program = program_path();
    #[cfg(target_os = "macos")]
    {
        let logs = log_paths();
        Ok(launchd_plist(&LaunchdParams {
            label: SERVICE_LABEL.to_string(),
            program,
            args: SERVICE_ARGS.iter().map(|s| s.to_string()).collect(),
            stdout: logs.0,
            stderr: logs.1,
        }))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(systemd_user_unit(&SystemdParams {
            name: SYSTEMD_NAME.to_string(),
            description: "kellner — local music-stack companion".to_string(),
            program,
            args: SERVICE_ARGS.iter().map(|s| s.to_string()).collect(),
        }))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(windows_task_hint(&program))
    }
}

#[cfg(target_os = "macos")]
fn log_paths() -> (String, String) {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = home.join("Library").join("Logs").join("kellner");
    (
        dir.join("kellner.out.log").to_string_lossy().into_owned(),
        dir.join("kellner.err.log").to_string_lossy().into_owned(),
    )
}

/// Follow-up command the user still has to run after `install`.
fn follow_up(path: &Path) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("launchctl load -w {}", path.display())
    }
    #[cfg(target_os = "linux")]
    {
        format!(
            "systemctl --user enable --now {}",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{SYSTEMD_NAME}.service"))
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        "see the Task Scheduler hint above".to_string()
    }
}

/// Execute one `kellner service` action.
pub fn run(opts: &ServiceOptions) -> Result<()> {
    match opts.action {
        ServiceAction::Install => install(opts),
        ServiceAction::Uninstall => uninstall(opts),
        ServiceAction::Status => status(),
    }
}

fn install(opts: &ServiceOptions) -> Result<()> {
    let text = render_unit()?;
    let Some(path) = unit_path() else {
        // Windows/other: nothing to write, just show the hint.
        println!("{text}");
        return Ok(());
    };

    if opts.print {
        print!("{text}");
        return Ok(());
    }
    if opts.dry_run {
        println!("kellner service: would write {} (dry run)", path.display());
        print!("{text}");
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    atomic_write(&path, text.as_bytes())?;
    println!("kellner service: wrote {}", path.display());
    println!("enable it with: {}", follow_up(&path));
    Ok(())
}

fn uninstall(opts: &ServiceOptions) -> Result<()> {
    let Some(path) = unit_path() else {
        println!("kellner service: no unit file on this platform");
        return Ok(());
    };
    if opts.dry_run {
        println!("kellner service: would remove {} (dry run)", path.display());
        return Ok(());
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("kellner service: removed {}", path.display());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("kellner service: nothing to remove at {}", path.display());
            Ok(())
        }
        Err(e) => Err(anyhow!(e)).with_context(|| format!("removing {}", path.display())),
    }
}

fn status() -> Result<()> {
    match unit_path() {
        Some(path) => {
            if path.exists() {
                println!("kellner service: installed at {}", path.display());
                println!("reload with: {}", follow_up(&path));
            } else {
                println!("kellner service: not installed ({} is absent)", path.display());
                println!("install with: kellner service install");
            }
        }
        None => {
            println!("kellner service: no unit file on this platform");
            println!("{}", windows_task_hint(&program_path()));
        }
    }
    Ok(())
}

/// temp + rename so a half-written unit can never be loaded.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    tmp.set_file_name(name);
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launchd_fixture() -> LaunchdParams {
        LaunchdParams {
            label: SERVICE_LABEL.to_string(),
            program: "/usr/local/bin/kellner".to_string(),
            args: vec!["run".to_string()],
            stdout: "/Users/x/Library/Logs/kellner/kellner.out.log".to_string(),
            stderr: "/Users/x/Library/Logs/kellner/kellner.err.log".to_string(),
        }
    }

    fn systemd_fixture() -> SystemdParams {
        SystemdParams {
            name: SYSTEMD_NAME.to_string(),
            description: "kellner — local music-stack companion".to_string(),
            program: "/usr/local/bin/kellner".to_string(),
            args: vec!["run".to_string()],
        }
    }

    #[test]
    fn launchd_plist_has_battery_keys_and_args() {
        let text = launchd_plist(&launchd_fixture());
        assert!(text.starts_with("<?xml"));
        assert!(text.contains("<string>de.zukkafabrik.kellner</string>"));
        assert!(text.contains("<key>ProcessType</key>"));
        assert!(text.contains("<string>Background</string>"));
        assert!(text.contains("<key>LowPriorityIO</key>"));
        assert!(text.contains("<integer>10</integer>"));
        assert!(text.contains("<key>RunAtLoad</key>"));
        assert!(text.contains("<key>KeepAlive</key>"));
        // program + arg present in order
        assert!(text.contains("/usr/local/bin/kellner"));
        assert!(text.contains("<string>run</string>"));
    }

    #[test]
    fn launchd_escapes_xml() {
        let mut p = launchd_fixture();
        p.program = "/tmp/a&b".to_string();
        assert!(launchd_plist(&p).contains("/tmp/a&amp;b"));
    }

    #[test]
    fn systemd_unit_has_sections_and_exec() {
        let text = systemd_user_unit(&systemd_fixture());
        assert!(text.contains("[Unit]"));
        assert!(text.contains("[Service]"));
        assert!(text.contains("[Install]"));
        assert!(text.contains("ExecStart=/usr/local/bin/kellner run"));
        assert!(text.contains("Restart=on-failure"));
        assert!(text.contains("WantedBy=default.target"));
    }

    #[test]
    fn systemd_uses_network_target_not_network_online() {
        // A *user* unit cannot order against the system-wide
        // network-online.target; network.target is the correct target there.
        let text = systemd_user_unit(&systemd_fixture());
        assert!(text.contains("After=network.target"));
        assert!(text.contains("Wants=network.target"));
        assert!(!text.contains("network-online.target"));
    }

    #[test]
    fn systemd_quotes_program_path_with_spaces() {
        let mut p = systemd_fixture();
        p.program = "/Users/foo bar/kellner".to_string();
        let text = systemd_user_unit(&p);
        assert!(
            text.contains("ExecStart=\"/Users/foo bar/kellner\" run"),
            "exec line must quote the spaced path:\n{text}"
        );
    }

    #[test]
    fn systemd_quote_leaves_safe_tokens_alone() {
        assert_eq!(systemd_quote("/usr/local/bin/kellner"), "/usr/local/bin/kellner");
        assert_eq!(systemd_quote("run"), "run");
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote("say\"hi"), "\"say\\\"hi\"");
    }

    #[test]
    fn windows_hint_mentions_schtasks() {
        let hint = windows_task_hint("C:\\kellner.exe");
        assert!(hint.contains("schtasks"));
        assert!(hint.contains("run"));
    }

    #[test]
    fn render_unit_is_nonempty() {
        assert!(!render_unit().unwrap().trim().is_empty());
    }

    #[test]
    fn install_print_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // Point HOME/XDG at a temp dir so a stray write would be detectable.
        // SAFETY: tests here do not run concurrently with other env mutation.
        let guard = crate::config::test_support::env_lock();
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("cfg"));
        }
        let opts = ServiceOptions {
            action: ServiceAction::Install,
            print: true,
            dry_run: false,
        };
        run(&opts).unwrap();
        // Nothing written under the temp HOME.
        assert!(!dir.path().join("cfg").exists());
        assert!(!dir.path().join("Library").exists());
        drop(guard);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_then_uninstall_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let guard = crate::config::test_support::env_lock();
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("cfg"));
        }
        let path = unit_path().unwrap();
        assert_eq!(
            path,
            dir.path().join("cfg").join("systemd").join("user").join("kellner.service")
        );
        let install = ServiceOptions {
            action: ServiceAction::Install,
            print: false,
            dry_run: false,
        };
        run(&install).unwrap();
        assert!(path.is_file(), "install wrote the unit");
        let uninstall = ServiceOptions {
            action: ServiceAction::Uninstall,
            print: false,
            dry_run: false,
        };
        run(&uninstall).unwrap();
        assert!(!path.exists(), "uninstall removed the unit");
        drop(guard);
    }

    #[test]
    fn install_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let guard = crate::config::test_support::env_lock();
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("cfg"));
        }
        let opts = ServiceOptions {
            action: ServiceAction::Install,
            print: false,
            dry_run: true,
        };
        run(&opts).unwrap();
        assert!(!dir.path().join("cfg").exists());
        drop(guard);
    }
}
