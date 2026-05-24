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

    /// Run a full rsync from local to remote (not dry-run).
    /// Returns (files_copied_count, total_bytes).
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
    /// Returns (subdirs, is_writable).
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
