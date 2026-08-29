//! Resolve external CLI tools that the app shells out to.
//!
//! When run as a macOS GUI app (launched from Finder/Dock), the process inherits
//! a minimal `PATH` of `/usr/bin:/bin:/usr/sbin:/sbin` — it does **not** include
//! Homebrew's `/opt/homebrew/bin`. Spawning `metaflac`, `exiftool`, `ffmpeg` or
//! `ffprobe` via `Command::new` then fails with
//! `No such file or directory (os error 2)` even though the tools are installed.
//!
//! This module resolves known tools to an absolute path when they live in a
//! common install location, falling back to the bare name otherwise (so the
//! spawn still reports a clear error if the tool is genuinely absent).

use std::path::Path;

/// Candidate directories searched for external tools, in priority order.
/// Covers Homebrew (Apple Silicon + Intel), MacPorts, and their sbin variants.
const TOOL_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/opt/local/sbin",
];

/// Resolve `name` to an absolute path if it exists in a known location,
/// otherwise return the bare `name`.
///
/// Paths that already contain a separator (i.e. are already absolute or
/// relative to cwd) are returned unchanged.
pub fn resolve_tool(name: &str) -> String {
    if name.contains('/') {
        return name.to_string();
    }

    for dir in TOOL_DIRS {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    name.to_string()
}
