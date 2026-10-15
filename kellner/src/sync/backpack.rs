//! Backpack upkeep for `kellner backpack` (issue #367, leaves P2-5/P2-6/P3-*).
//!
//! Flow (per run):
//! 1. read the owner's active backpacks from the hub (`GET /api/hub/backpacks`);
//! 2. export each active backpack (`GET /api/hub/backpacks/{id}/export`) and
//!    union their tracks by ISRC into the *desired* set;
//! 3. order + poll + fetch the ISRCs not yet on disk ([`crate::sync::downloads`]
//!    helpers, so behaviour matches `kellner downloads`), writing them **flat**
//!    into the collection root (`<isrc>.<ext>`);
//! 4. render + write the resolved Comment tag (deterministic; unchanged render
//!    ⇒ no rewrite) and record the canonical content hash in `state.json`;
//! 5. **soft-prune**: membership ISRCs no longer in any active backpack are moved
//!    to `<root>/.kellner-trash/` — never hard-deleted, and user `downloads`
//!    entries (issue #366) are never touched.
//!
//! Failures are per ISRC and tolerated (counted, the run continues); the caller
//! decides the exit code from [`BackpackStats::failed`] / [`BackpackStats::pending`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};

use crate::client::api::{ApiClient, TrackStatus};
use crate::client::hub::HubClient;
use crate::state::{self, MembershipRecord, SyncState};
use crate::sync::backup::canonicalise;
use crate::sync::downloads::{choose_format, write_atomic};
use crate::tag::comment::{render_comment, write_comment};

/// Subdirectory (under the collection root) that receives soft-pruned files.
pub const TRASH_DIR: &str = ".kellner-trash";

/// Options for one `kellner backpack` run (already resolved by the caller).
#[derive(Debug, Clone)]
pub struct BackpackOptions {
    /// Flat collection directory (already validated to exist).
    pub root: PathBuf,
    /// Preferred delivery format (`flac` | `320` | `128`); `None` → `flac`.
    pub format: Option<String>,
    /// Order priority.
    pub priority: i64,
    /// Seconds between poll rounds.
    pub poll_secs: u64,
    /// Give up polling after this many seconds.
    pub timeout_secs: u64,
    /// Classify only: no order/fetch/write/prune.
    pub dry_run: bool,
    /// Write the Comment tag (default true; `--no-tags` disables).
    pub write_tags: bool,
}

/// Tally of one backpack run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackpackStats {
    /// Backpacks the hub reported.
    pub backpacks: usize,
    /// Active backpacks exported.
    pub active: usize,
    /// Distinct desired ISRCs across all active backpacks.
    pub desired: usize,
    /// ISRCs sent in the order (missing from state **and** disk).
    pub ordered: usize,
    /// Files fetched this run.
    pub fetched: usize,
    /// Files whose Comment tag was (re)written this run.
    pub tagged: usize,
    /// Membership entries soft-moved to `.kellner-trash/`.
    pub pruned: usize,
    /// ISRCs that ended in a non-recoverable state.
    pub failed: usize,
    /// ISRCs still non-terminal when `--timeout-secs` elapsed.
    pub pending: usize,
}

/// One desired track unioned across the active backpacks.
#[derive(Debug, Clone, PartialEq)]
struct Desired {
    tags: Vec<String>,
    bpm: Option<f64>,
    key: Option<String>,
    energy: Option<f64>,
    backpack_id: i64,
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Extensions checked for an already-present flat file.
const EXTENSIONS: &[&str] = &["flac", "mp3", "m4a"];

/// The flat file for `isrc` already on disk, if any.
fn existing_file(root: &Path, isrc: &str) -> Option<PathBuf> {
    EXTENSIONS
        .iter()
        .map(|ext| root.join(format!("{isrc}.{ext}")))
        .find(|p| p.is_file())
}

/// Run one backpack sync cycle.
pub async fn run_backpack(
    hub: &HubClient,
    api: &ApiClient,
    opts: &BackpackOptions,
) -> Result<BackpackStats> {
    let preferred = opts.format.clone().unwrap_or_else(|| "flac".to_string());
    let mut state = state::load()?;
    let mut stats = BackpackStats::default();

    // US1: read the owner's backpacks, then export each active one.
    let backpacks = hub
        .list_backpacks()
        .await
        .context("listing hub backpacks failed")?;
    stats.backpacks = backpacks.len();

    let mut desired: BTreeMap<String, Desired> = BTreeMap::new();
    let mut active_sorted: Vec<_> = backpacks.iter().filter(|b| b.is_active).collect();
    active_sorted.sort_by_key(|b| b.id);
    for bp in &active_sorted {
        stats.active += 1;
        let export = hub
            .export_backpack(bp.id)
            .await
            .with_context(|| format!("exporting backpack {} failed", bp.id))?;
        for t in export.tracks {
            let isrc = t.isrc.trim().to_string();
            if isrc.is_empty() {
                stats.failed += 1;
                eprintln!("kellner: backpack: backpack {} has a track without ISRC — skipped", bp.id);
                continue;
            }
            // Union by ISRC: the first (lowest backpack id) wins, deterministically.
            desired.entry(isrc).or_insert_with(|| Desired {
                tags: t.tags,
                bpm: t.bpm,
                key: t.key,
                energy: t.energy,
                backpack_id: bp.id,
            });
        }
    }
    stats.desired = desired.len();

    // US4: prune membership for tracks no longer in any active backpack.
    prune(&mut state, &desired, opts, &mut stats)?;

    // US2: diff the desired set against state + disk.
    let missing: Vec<String> = desired
        .keys()
        .filter(|isrc| !state.is_backpack_member(isrc) && existing_file(&opts.root, isrc).is_none())
        .cloned()
        .collect();

    if opts.dry_run {
        println!(
            "kellner: backpack --dry-run: {} backpack(s) ({} active), {} desired ISRC(s), {} missing",
            stats.backpacks, stats.active, stats.desired, missing.len()
        );
        for isrc in &missing {
            println!("kellner: backpack --dry-run: would order+fetch {isrc}");
        }
        return Ok(stats);
    }

    if missing.is_empty() {
        report(&stats, opts);
        return Ok(stats);
    }

    // US2a: one order for everything still missing.
    let order = api
        .order(&missing, opts.priority)
        .await
        .context("placing music-api order failed")?;
    stats.ordered = missing.len();
    println!(
        "kellner: backpack: order {} accepted ({} ISRC(s))",
        order.order_id, order.count
    );

    // US2b: poll until terminal or timeout.
    let (ready, failed, still_pending) = poll_until_ready(api, &missing, opts).await?;
    for isrc in &failed {
        stats.failed += 1;
        eprintln!("kellner: backpack: {isrc} failed");
    }
    stats.pending = still_pending.len();

    // US3: fetch + tag + record membership for every ready ISRC.
    for status in &ready {
        match fetch_and_tag(api, opts, &mut state, &status.isrc, &preferred, &status.formats, &desired)
            .await
        {
            Ok((path, tagged)) => {
                stats.fetched += 1;
                if tagged {
                    stats.tagged += 1;
                }
                println!("kellner: backpack: {} -> {}", status.isrc, path.display());
            }
            Err(e) => {
                stats.failed += 1;
                eprintln!("kellner: backpack: fetching {} failed: {e:#}", status.isrc);
            }
        }
    }

    report(&stats, opts);
    Ok(stats)
}

/// Soft-move membership entries whose ISRC left every active backpack.
fn prune(
    state: &mut SyncState,
    desired: &BTreeMap<String, Desired>,
    opts: &BackpackOptions,
    stats: &mut BackpackStats,
) -> Result<()> {
    let stale: Vec<String> = state
        .backpack_membership
        .keys()
        .filter(|isrc| !desired.contains_key(*isrc))
        .cloned()
        .collect();
    if stale.is_empty() {
        return Ok(());
    }

    let trash = opts.root.join(TRASH_DIR);
    for isrc in &stale {
        let rec = state
            .backpack_membership
            .get(isrc)
            .cloned()
            .expect("just listed");
        if opts.dry_run {
            println!("kellner: backpack --dry-run: would soft-move {isrc} -> {}", rec.path.display());
            continue;
        }
        if rec.path.exists() {
            std::fs::create_dir_all(&trash)
                .with_context(|| format!("creating {}", trash.display()))?;
            let dest = unique_trash_path(&trash, &rec.path);
            if let Err(e) = std::fs::rename(&rec.path, &dest) {
                stats.failed += 1;
                eprintln!(
                    "kellner: backpack: soft-moving {} failed: {e:#}",
                    rec.path.display()
                );
                continue;
            }
            println!(
                "kellner: backpack: pruned {isrc} -> {}",
                dest.display()
            );
        }
        // Drop the membership record either way (the file may already be gone).
        state.backpack_membership.remove(isrc);
        stats.pruned += 1;
    }
    if !opts.dry_run {
        state::save(state)?;
    }
    Ok(())
}

/// A collision-safe destination under the trash directory (never overwrites).
fn unique_trash_path(trash: &Path, src: &Path) -> PathBuf {
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mut candidate = trash.join(&name);
    let mut n = 1u32;
    while candidate.exists() {
        candidate = trash.join(format!("{name}.{n}"));
        n += 1;
    }
    candidate
}

/// Poll `GET /tracks` until every ISRC is terminal or the timeout elapses.
///
/// Returns `(ready rows, failed ISRCs, still-pending ISRCs)`.
async fn poll_until_ready(
    api: &ApiClient,
    isrcs: &[String],
    opts: &BackpackOptions,
) -> Result<(Vec<TrackStatus>, Vec<String>, Vec<String>)> {
    let deadline = Instant::now() + Duration::from_secs(opts.timeout_secs);
    let poll = Duration::from_secs(opts.poll_secs.max(1));

    loop {
        let resp = api
            .tracks(isrcs)
            .await
            .context("polling music-api tracks failed")?;

        let mut ready: Vec<TrackStatus> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut waiting: Vec<String> = Vec::new();
        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        for t in &resp.tracks {
            seen.insert(t.isrc.clone(), ());
            if t.is_ready() {
                ready.push(t.clone());
            } else if t.is_failed() {
                failed.push(t.isrc.clone());
            } else {
                waiting.push(t.isrc.clone());
            }
        }
        for isrc in isrcs {
            if !seen.contains_key(isrc) {
                waiting.push(isrc.clone());
            }
        }

        if waiting.is_empty() || Instant::now() >= deadline {
            return Ok((ready, failed, waiting));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Choose a format, fetch the bytes flat, tag the Comment and record membership.
///
/// Membership is recorded **only** after a successful fetch + tag; partial or
/// failed work is never recorded as complete. Returns `(path, tag_changed)`.
#[allow(clippy::too_many_arguments)]
async fn fetch_and_tag(
    api: &ApiClient,
    opts: &BackpackOptions,
    state: &mut SyncState,
    isrc: &str,
    preferred: &str,
    available: &[String],
    desired: &BTreeMap<String, Desired>,
) -> Result<(PathBuf, bool)> {
    let format = choose_format(preferred, available)
        .ok_or_else(|| anyhow!("no servable format offered (have {available:?})"))?;
    let bytes = api
        .fetch_file(isrc, &format)
        .await
        .with_context(|| format!("fetching {isrc} ({format})"))?;
    let path = write_atomic(&opts.root, isrc, &format, &bytes)?;

    let dev = desired
        .get(isrc)
        .ok_or_else(|| anyhow!("{isrc} vanished from the desired set"))?;
    let rendered = render_comment(
        &dev.tags,
        dev.bpm,
        dev.key.as_deref(),
        dev.energy,
        true,
    );
    let tagged = if opts.write_tags {
        write_comment(&path, &rendered)
            .with_context(|| format!("writing comment to {}", path.display()))?
    } else {
        false
    };

    // Canonical content hash (Comment cleared) — the same key the backup uses.
    let (content_hash, _bytes) = canonicalise(&path)
        .with_context(|| format!("canonicalising {}", path.display()))?;

    state.record_membership(
        isrc,
        MembershipRecord {
            path: path.clone(),
            backpack_id: dev.backpack_id,
            tags_rendered: if opts.write_tags { rendered } else { String::new() },
            content_hash,
            downloaded_at: now_epoch_secs(),
        },
    );
    state::save(state)?;
    Ok((path, tagged))
}

fn report(stats: &BackpackStats, opts: &BackpackOptions) {
    println!(
        "kellner: backpack: {} backpack(s) ({} active), {} desired, {} ordered, {} fetched, {} tagged, {} pruned, {} failed, {} pending (root {})",
        stats.backpacks,
        stats.active,
        stats.desired,
        stats.ordered,
        stats.fetched,
        stats.tagged,
        stats.pruned,
        stats.failed,
        stats.pending,
        opts.root.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_file_finds_common_extensions() {
        let dir = tempfile::tempdir().unwrap();
        assert!(existing_file(dir.path(), "X").is_none());
        std::fs::write(dir.path().join("X.flac"), b"x").unwrap();
        assert_eq!(existing_file(dir.path(), "X"), Some(dir.path().join("X.flac")));
        std::fs::write(dir.path().join("Y.mp3"), b"x").unwrap();
        assert_eq!(existing_file(dir.path(), "Y"), Some(dir.path().join("Y.mp3")));
    }

    #[test]
    fn unique_trash_path_never_collides() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join(TRASH_DIR);
        std::fs::create_dir_all(&trash).unwrap();
        let src = PathBuf::from("/music/A.flac");
        let first = unique_trash_path(&trash, &src);
        assert_eq!(first, trash.join("A.flac"));
        std::fs::write(&first, b"x").unwrap();
        let second = unique_trash_path(&trash, &src);
        assert_eq!(second, trash.join("A.flac.1"));
        assert_ne!(first, second);
    }
}
