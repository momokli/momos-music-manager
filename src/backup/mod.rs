use anyhow::{Result, anyhow};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// BackupEngine handles copying files to a remote backup destination via SSH/SCP.
/// Uses the user's ~/.ssh/config for host resolution (e.g. `backup` host).
pub struct BackupEngine {
    ssh_host: String,
}

impl BackupEngine {
    /// Create a new BackupEngine pointing at the given SSH host.
    /// The host must be resolvable via ~/.ssh/config (e.g. "backup").
    pub fn new(ssh_host: String) -> Self {
        Self { ssh_host }
    }

    /// Return the SSH host this engine connects to.
    pub fn ssh_host(&self) -> &str {
        &self.ssh_host
    }

    /// Copy a local file to the backup destination using scp.
    /// Returns (success, remote_file_size_bytes).
    /// On success, the file was copied and verified to exist remotely.
    pub async fn copy_file(&self, local_path: &Path, remote_path: &str) -> Result<(bool, i64)> {
        let local_str = local_path.to_string_lossy();
        let dest = format!("{}:{}", self.ssh_host, remote_path);

        debug!("Copying {} to {}", local_str, dest);

        // Ensure remote directory exists
        let parent = match remote_path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => {
                return Err(anyhow!(
                    "remote_path has no directory component: {}",
                    remote_path
                ));
            }
        };
        self.ensure_remote_dir(parent).await?;

        let output = Command::new("rsync")
            .arg("-a") // archive mode: preserve timestamps, permissions, etc.
            .arg("--rsh=ssh")
            .arg(&*local_str)
            .arg(&dest)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("rsync failed for {}: {}", local_str, stderr);
            return Ok((false, 0));
        }

        // Verify remote file exists and get its size
        match self.remote_file_size(remote_path).await? {
            Some(size) if size > 0 => {
                info!("Successfully copied {} ({}) to backup", local_str, size);
                Ok((true, size))
            }
            _ => {
                warn!(
                    "File {} copied but verification failed on remote",
                    local_str
                );
                Ok((false, 0))
            }
        }
    }

    /// Verify a file exists on the backup destination with matching size.
    /// Returns true if the file exists and size matches.
    pub async fn verify_file(&self, remote_path: &str, expected_size: i64) -> Result<bool> {
        match self.remote_file_size(remote_path).await? {
            Some(size) => Ok(size == expected_size),
            None => Ok(false),
        }
    }

    /// Get the size of a file on the backup destination.
    /// Returns None if the file doesn't exist.
    pub async fn remote_file_size(&self, remote_path: &str) -> Result<Option<i64>> {
        let output = Command::new("ssh")
            .arg(&self.ssh_host)
            .arg("stat")
            .arg("-f")
            .arg("%z")
            .arg(remote_path)
            .output()
            .await?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let size = stdout.trim().parse::<i64>().ok();
        Ok(size)
    }

    /// List files in a remote directory (filenames only, one per line).
    /// If max_depth > 1, also traverses subdirectories up to that depth.
    pub async fn list_remote_files(&self, remote_dir: &str) -> Result<Vec<String>> {
        self.list_remote_files_with_depth(remote_dir, 1).await
    }

    /// List remote files with a specific max depth (like find -maxdepth).
    /// Returns just base filenames (no path), matching the flat ls -1 output format.
    pub async fn list_remote_files_with_depth(
        &self,
        remote_dir: &str,
        max_depth: u32,
    ) -> Result<Vec<String>> {
        // Use find to get full paths, then strip to base filenames in Rust.
        // BusyBox (Synology) doesn't support -printf, and -exec basename may fail.
        let output = Command::new("ssh")
            .arg(&self.ssh_host)
            .arg("find")
            .arg(remote_dir)
            .arg("-maxdepth")
            .arg(max_depth.to_string())
            .arg("-type")
            .arg("f")
            .output()
            .await?;

        if !output.status.success() {
            // Fall back to flat ls -1
            let output = Command::new("ssh")
                .arg(&self.ssh_host)
                .arg("ls")
                .arg("-1")
                .arg(remote_dir)
                .output()
                .await?;
            if !output.status.success() {
                return Ok(Vec::new());
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Ok(stdout.lines().map(|s| s.to_string()).collect());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Strip paths: /volume1/media/stems/subdir/file.wav -> file.wav
        Ok(stdout
            .lines()
            .filter_map(|line| {
                let path = std::path::Path::new(line);
                path.file_name().map(|n| n.to_string_lossy().to_string())
            })
            .collect())
    }

    /// List remote files with full relative paths (not stripped to basenames).
    /// Returns paths relative to the remote_base directory.
    /// Used for backup discovery where we need to reconstruct local file paths.
    pub async fn list_remote_files_full(
        &self,
        remote_dir: &str,
        max_depth: u32,
    ) -> Result<Vec<String>> {
        // Use find to get full paths, then strip the remote_dir prefix to get relative paths.
        let output = Command::new("ssh")
            .arg(&self.ssh_host)
            .arg("find")
            .arg(remote_dir)
            .arg("-maxdepth")
            .arg(max_depth.to_string())
            .arg("-type")
            .arg("f")
            .output()
            .await?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Strip remote_dir prefix: /volume1/media/stems/subdir/file.wav -> subdir/file.wav
        let base = remote_dir.trim_end_matches('/');
        Ok(stdout
            .lines()
            .filter_map(|line| {
                let stripped = line.strip_prefix(base)?.strip_prefix('/').unwrap_or(line);
                if stripped.is_empty() {
                    None
                } else {
                    Some(stripped.to_string())
                }
            })
            .collect())
    }

    /// Run rsync in dry-run mode to show what would be transferred.
    /// Returns a list of file paths that would be copied.
    pub async fn dry_run_sync(&self, local_dir: &str, remote_dir: &str) -> Result<Vec<String>> {
        let dest = format!("{}:{}", self.ssh_host, remote_dir);

        let output = Command::new("rsync")
            .arg("-rauv")
            .arg("--dry-run")
            .arg(local_dir)
            .arg(&dest)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("rsync dry-run failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Skip summary lines and only return filenames
        Ok(stdout
            .lines()
            .filter(|l| {
                !l.starts_with("sending")
                    && !l.starts_with("sent")
                    && !l.contains("/")
                    && !l.is_empty()
            })
            .map(|s| s.to_string())
            .collect())
    }

    /// Ensure a remote directory exists (creates if needed).
    async fn ensure_remote_dir(&self, remote_dir: &str) -> Result<()> {
        let output = Command::new("ssh")
            .arg(&self.ssh_host)
            .arg("mkdir")
            .arg("-p")
            .arg(remote_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Failed to create remote directory {}: {}",
                remote_dir,
                stderr
            ));
        }
        Ok(())
    }

    /// Copy a batch of files using rsync with `--files-from` + `--ignore-existing`.
    ///
    /// Unlike `run_sync` (which scans ALL files in a directory), this method takes an explicit
    /// list of relative paths and only copies those — no stat/scan overhead for already-backed-up files.
    /// `--ignore-existing` avoids remote stat calls entirely: we already know these files don't exist
    /// remotely (from the reconcile step), so just copy them.
    ///
    /// Returns `Ok(())` on success (rsync exit code 0). On failure, returns the rsync error.
    pub async fn copy_batch(
        &self,
        local_dir: &str,
        remote_base: &str,
        rel_paths: &[String],
    ) -> Result<()> {
        // Write relative paths to a temp file for --files-from
        let temp_dir = std::env::temp_dir();
        let temp_file_path = temp_dir.join(format!("mmm_backup_{}.txt", std::process::id()));
        {
            // Scope for file handle — explicit close before rsync
            let content = rel_paths.join("\n");
            std::fs::write(&temp_file_path, &content)
                .map_err(|e| anyhow!("Failed to write --files-from temp file: {}", e))?;
        }

        let dest = format!("{}:{}", self.ssh_host, remote_base);
        // Trailing slash means "copy contents", not the directory itself
        let local_with_slash = format!("{}/", local_dir.trim_end_matches('/'));

        debug!(
            "Copying batch of {} files from {} to {} via --files-from",
            rel_paths.len(),
            local_with_slash,
            dest
        );

        let output = Command::new("rsync")
            .arg("-a")
            .arg("--ignore-existing")
            .arg("--rsh=ssh")
            .arg("--files-from")
            .arg(temp_file_path.to_string_lossy().as_ref())
            .arg(&local_with_slash)
            .arg(&dest)
            .output()
            .await?;

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_file_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("rsync --files-from failed: {}", stderr));
        }

        Ok(())
    }

    /// Run a full rsync from local to remote (not dry-run).
    /// Returns (files_copied_count, total_bytes).
    ///
    /// DEPRECATED: Use `copy_batch` instead for targeted backups.
    pub async fn run_sync(&self, local_dir: &str, remote_dir: &str) -> Result<(usize, i64)> {
        let dest = format!("{}:{}", self.ssh_host, remote_dir);

        let output = Command::new("rsync")
            .arg("-rauvP")
            .arg(local_dir)
            .arg(&dest)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("rsync failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let file_count = stdout
            .lines()
            .filter(|l| l.contains(".wav") || l.contains(".flac") || l.contains(".m4a"))
            .count();
        let total_bytes = stdout
            .lines()
            .filter_map(|l| {
                if l.starts_with("sent ") {
                    l.split_whitespace()
                        .nth(1)
                        .and_then(|s| s.replace(',', "").parse::<i64>().ok())
                } else {
                    None
                }
            })
            .next()
            .unwrap_or(0);

        Ok((file_count, total_bytes))
    }

    /// Test SSH connectivity to the host.
    /// Returns true if the host is reachable via SSH.
    pub async fn test_host(&self) -> Result<bool> {
        let output = Command::new("ssh")
            .arg("-o")
            .arg("ConnectTimeout=5")
            .arg(&self.ssh_host)
            .arg("exit")
            .arg("0")
            .output()
            .await?;
        Ok(output.status.success())
    }

    /// Explore a remote directory: list subdirectories and check if writable.
    /// Returns `(subdirs, is_writable)`.
    pub async fn explore_dir(&self, remote_dir: &str) -> Result<(Vec<String>, bool)> {
        // List subdirectories only (ending with /)
        let list_output = Command::new("ssh")
            .arg(&self.ssh_host)
            .arg("ls")
            .arg("-1p")
            .arg(remote_dir)
            .output()
            .await?;

        let dirs = if list_output.status.success() {
            let stdout = String::from_utf8_lossy(&list_output.stdout);
            stdout
                .lines()
                .filter(|l| l.ends_with('/'))
                .map(|l| l.trim_end_matches('/').to_string())
                .collect()
        } else {
            Vec::new()
        };

        // Check writability by touching a test file
        let test_path = format!("{}/.mmm_writable_test", remote_dir.trim_end_matches('/'));
        let write_output = Command::new("ssh")
            .arg(&self.ssh_host)
            .arg("touch")
            .arg(&test_path)
            .output()
            .await?;

        let writable = write_output.status.success();

        // Clean up test file
        if writable {
            let _ = Command::new("ssh")
                .arg(&self.ssh_host)
                .arg("rm")
                .arg("-f")
                .arg(&test_path)
                .output()
                .await;
        }

        Ok((dirs, writable))
    }
}

/// Extract the parent directory from a remote path, for use with `mkdir -p`.
pub fn remote_path_parent(remote_path: &str) -> Result<String> {
    match remote_path.rsplit_once('/') {
        Some((dir, _)) => Ok(dir.to_string()),
        None => Err(anyhow!(
            "remote_path has no directory component: {}",
            remote_path
        )),
    }
}

/// Strip a base directory prefix from a full path to get a relative path.
pub fn strip_remote_prefix(full_path: &str, base: &str) -> Option<String> {
    let base = base.trim_end_matches('/');
    let stripped = full_path.strip_prefix(base)?;
    let path = stripped.strip_prefix('/').unwrap_or(stripped);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Extract the basename (filename) from a path string.
pub fn path_basename(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
}

/// Parse a remote file size from stdout (e.g. `stat -f "%z"` output).
pub fn parse_remote_file_size(stdout: &str) -> Option<i64> {
    stdout.trim().parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_engine_creation() {
        let engine = BackupEngine::new("backup".to_string());
        assert_eq!(engine.ssh_host(), "backup");
    }

    #[test]
    fn test_backup_engine_custom_host() {
        let engine = BackupEngine::new("my-nas.local".to_string());
        assert_eq!(engine.ssh_host(), "my-nas.local");
    }

    #[test]
    fn test_remote_path_parent_valid() {
        let parent = remote_path_parent("/volume1/media/stems/file.flac").unwrap();
        assert_eq!(parent, "/volume1/media/stems");
    }

    #[test]
    fn test_remote_path_parent_root() {
        let parent = remote_path_parent("/file.flac").unwrap();
        assert_eq!(parent, "");
    }

    #[test]
    fn test_remote_path_parent_no_slash() {
        let result = remote_path_parent("justafilename");
        assert!(result.is_err());
    }

    #[test]
    fn test_strip_remote_prefix_basic() {
        let result = strip_remote_prefix(
            "/volume1/media/stems/subdir/file.wav",
            "/volume1/media/stems",
        );
        assert_eq!(result, Some("subdir/file.wav".to_string()));
    }

    #[test]
    fn test_strip_remote_prefix_no_match() {
        let result = strip_remote_prefix("/other/path/file.wav", "/volume1/media/stems");
        assert_eq!(result, None);
    }

    #[test]
    fn test_strip_remote_prefix_exact_match() {
        let result = strip_remote_prefix("/volume1/media/stems", "/volume1/media/stems");
        assert_eq!(result, None);
    }

    #[test]
    fn test_path_basename_regular() {
        let result = path_basename("/volume1/media/stems/file.wav");
        assert_eq!(result, Some("file.wav".to_string()));
    }

    #[test]
    fn test_path_basename_root() {
        let result = path_basename("/");
        assert_eq!(result, None);
    }

    #[test]
    fn test_path_basename_just_filename() {
        let result = path_basename("file.wav");
        assert_eq!(result, Some("file.wav".to_string()));
    }

    #[test]
    fn test_parse_remote_file_size_valid() {
        let result = parse_remote_file_size("12345\n");
        assert_eq!(result, Some(12345));
    }

    #[test]
    fn test_parse_remote_file_size_invalid() {
        let result = parse_remote_file_size("not a number");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_remote_file_size_empty() {
        let result = parse_remote_file_size("");
        assert_eq!(result, None);
    }
}
