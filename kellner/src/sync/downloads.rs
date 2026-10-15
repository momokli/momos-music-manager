//! Download orchestration for `kellner downloads` (issue #366, leaf P2-3).
//!
//! Flow (per run): normalise the requested ISRCs → place **one** order
//! (`POST /orders`) for the ones not already fetched → poll `GET /tracks`
//! until every ISRC is terminal or `--timeout-secs` elapses → stream each
//! `ready` file (`GET /isrc/{isrc}/{format}`) and write it **flat** into the
//! collection directory (`<isrc>.<ext>`) atomically.
//!
//! Idempotency/provenance live in `state.json` ([`crate::state`]): an ISRC that
//! was already fetched is neither re-ordered nor re-downloaded, so a second run
//! performs no network work. Failures are per ISRC and tolerated (counted, the
//! rest continues), mirroring [`crate::sync::backup`].
//!
//! This is transport + command only: **no** Backpack diffing/tagging (that is
//! #367) and **no** prune.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};

use crate::client::api::{ApiClient, DELIVERY_FORMATS, TrackStatus};
use crate::state::{self, OrderRecord, SyncState};

/// Options for one `kellner downloads` run (already resolved by the caller).
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Requested ISRCs (raw; normalised inside).
    pub isrcs: Vec<String>,
    /// Preferred format (`flac` | `320` | `128`); `None` → `flac`.
    pub format: Option<String>,
    /// Order priority.
    pub priority: i64,
    /// Seconds between poll rounds.
    pub poll_secs: u64,
    /// Give up polling after this many seconds.
    pub timeout_secs: u64,
    /// Flat target directory (already validated to exist).
    pub root: PathBuf,
}

/// Tally of one download run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadStats {
    /// Distinct ISRCs requested.
    pub requested: usize,
    /// ISRCs already recorded as fetched (no network work).
    pub already_present: usize,
    /// ISRCs sent in the order.
    pub ordered: usize,
    /// Files fetched this run.
    pub fetched: usize,
    /// ISRCs that ended in a non-recoverable state.
    pub failed: usize,
    /// ISRCs still non-terminal when `--timeout-secs` elapsed.
    pub pending: usize,
}

/// Normalise raw ISRC strings: trim, drop empties, sort and deduplicate.
pub fn normalise_isrcs(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = raw
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Target file extension for a delivery format: `flac` → `.flac`, else `.mp3`.
pub fn extension_for(format: &str) -> &'static str {
    if format == "flac" { "flac" } else { "mp3" }
}

/// Pick the format to fetch: the preferred one when available, otherwise the
/// first servable format in [`DELIVERY_FORMATS`] order (flac→320→128).
///
/// An empty `available` list (server omitted `formats`) falls back to the
/// preferred format, letting the server answer `404` if it cannot serve it.
pub fn choose_format(preferred: &str, available: &[String]) -> Option<String> {
    if available.is_empty() {
        return Some(preferred.to_string());
    }
    if available.iter().any(|f| f == preferred) {
        return Some(preferred.to_string());
    }
    DELIVERY_FORMATS
        .iter()
        .find(|f| available.iter().any(|a| a == *f))
        .map(|f| f.to_string())
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run one download cycle. Returns the tally; the caller decides the exit code
/// from [`DownloadStats::failed`] / [`DownloadStats::pending`].
pub async fn run_downloads(client: &ApiClient, opts: &DownloadOptions) -> Result<DownloadStats> {
    let isrcs = normalise_isrcs(&opts.isrcs);
    if isrcs.is_empty() {
        anyhow::bail!("no ISRCs given — pass --isrc <ISRC> and/or --isrcs <file>");
    }
    let preferred = opts.format.clone().unwrap_or_else(|| "flac".to_string());

    let mut state = state::load()?;
    let mut stats = DownloadStats {
        requested: isrcs.len(),
        ..DownloadStats::default()
    };

    // Skip anything already fetched (idempotency, #366 AK3.5).
    let mut to_fetch: Vec<String> = Vec::new();
    for isrc in &isrcs {
        if state.is_fetched(isrc) {
            stats.already_present += 1;
        } else {
            to_fetch.push(isrc.clone());
        }
    }

    if to_fetch.is_empty() {
        report(&stats, opts);
        return Ok(stats);
    }

    // US1: one order for everything still missing.
    let order = client
        .order(&to_fetch, opts.priority)
        .await
        .context("placing music-api order failed")?;
    stats.ordered = to_fetch.len();
    state.record_order(OrderRecord {
        order_id: order.order_id.clone(),
        isrcs: to_fetch.clone(),
        priority: opts.priority,
        created_at: now_epoch_secs(),
        states: BTreeMap::new(),
    });
    state::save(&state)?;
    println!(
        "kellner: downloads: order {} accepted ({} ISRC(s), status {})",
        order.order_id, order.count, order.status
    );

    // US2: poll until terminal or timeout.
    let (ready, failed, still_pending, states) = poll_until_ready(client, &to_fetch, opts).await?;
    if let Some(rec) = state.orders.last_mut() {
        rec.states = states;
    }
    state::save(&state)?;

    for isrc in &failed {
        stats.failed += 1;
        eprintln!("kellner: downloads: {isrc} failed");
    }
    stats.pending = still_pending.len();

    // US3: fetch + flat file for every ready ISRC.
    for status in &ready {
        match fetch_one(client, opts, &mut state, &status.isrc, &preferred, &status.formats, &order.order_id)
            .await
        {
            Ok(path) => {
                stats.fetched += 1;
                println!(
                    "kellner: downloads: {} -> {}",
                    status.isrc,
                    path.display()
                );
            }
            Err(e) => {
                stats.failed += 1;
                eprintln!("kellner: downloads: fetching {} failed: {e:#}", status.isrc);
            }
        }
    }

    report(&stats, opts);
    Ok(stats)
}

/// Poll `GET /tracks` until every ISRC is terminal or the timeout elapses.
///
/// Returns `(ready rows, failed ISRCs, still-pending ISRCs, last states map)`.
async fn poll_until_ready(
    client: &ApiClient,
    isrcs: &[String],
    opts: &DownloadOptions,
) -> Result<(Vec<TrackStatus>, Vec<String>, Vec<String>, BTreeMap<String, String>)> {
    let deadline = Instant::now() + Duration::from_secs(opts.timeout_secs);
    let poll = Duration::from_secs(opts.poll_secs.max(1));

    loop {
        let resp = client
            .tracks(isrcs)
            .await
            .context("polling music-api tracks failed")?;

        let mut ready: Vec<TrackStatus> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut waiting: Vec<String> = Vec::new();
        let mut states: BTreeMap<String, String> = BTreeMap::new();
        for t in &resp.tracks {
            let label = t
                .state
                .clone()
                .unwrap_or_else(|| if t.unknown { "unknown".into() } else { "pending".into() });
            states.insert(t.isrc.clone(), label);
            if t.is_ready() {
                ready.push(t.clone());
            } else if t.is_failed() {
                failed.push(t.isrc.clone());
            } else {
                waiting.push(t.isrc.clone());
            }
        }
        // ISRCs the server did not echo at all stay pending.
        for isrc in isrcs {
            if !states.contains_key(isrc) {
                waiting.push(isrc.clone());
                states.insert(isrc.clone(), "pending".into());
            }
        }

        if waiting.is_empty() || Instant::now() >= deadline {
            return Ok((ready, failed, waiting, states));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Choose format, fetch the bytes and store them flat + atomically.
async fn fetch_one(
    client: &ApiClient,
    opts: &DownloadOptions,
    state: &mut SyncState,
    isrc: &str,
    preferred: &str,
    available: &[String],
    order_id: &str,
) -> Result<PathBuf> {
    let format = choose_format(preferred, available)
        .ok_or_else(|| anyhow!("no servable format offered (have {available:?})"))?;
    let bytes = client
        .fetch_file(isrc, &format)
        .await
        .with_context(|| format!("fetching {isrc} ({format})"))?;
    let path = write_atomic(&opts.root, isrc, &format, &bytes)?;
    state.record_download(isrc, path.clone(), &format, order_id, now_epoch_secs());
    state::save(state)?;
    Ok(path)
}

/// Write `bytes` to `<root>/<isrc>.<ext>` via a temporary file + `rename`, so a
/// failed write never leaves a partial file at the final path (AK3.3).
pub fn write_atomic(root: &Path, isrc: &str, format: &str, bytes: &[u8]) -> Result<PathBuf> {
    let final_path = root.join(format!("{isrc}.{}", extension_for(format)));
    let tmp = root.join(format!(
        ".{}.{}.tmp",
        isrc,
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("finalising {}", final_path.display()));
    }
    Ok(final_path)
}

fn report(stats: &DownloadStats, opts: &DownloadOptions) {
    println!(
        "kellner: downloads: {} requested, {} already present, {} fetched, {} failed, {} pending (root {})",
        stats.requested,
        stats.already_present,
        stats.fetched,
        stats.failed,
        stats.pending,
        opts.root.display()
    );
}

/// Read-only status report (US4): no network call, purely `state.json`.
pub fn report_status(state: &SyncState) {
    println!(
        "kellner: downloads: {} order(s), {} fetched track(s)",
        state.orders.len(),
        state.downloads.len()
    );
    for order in &state.orders {
        println!(
            "  order {} ({} ISRC(s), priority {}, {} states)",
            order.order_id,
            order.isrcs.len(),
            order.priority,
            order.states.len()
        );
        for (isrc, status) in &order.states {
            println!("    {isrc}: {status}");
        }
    }
    for (isrc, rec) in &state.downloads {
        println!(
            "  fetched {isrc}: {} [{}] order {}",
            rec.path.display(),
            rec.format,
            rec.order_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_trims_sorts_dedupes() {
        let got = normalise_isrcs(&[
            " b ".into(),
            "a".into(),
            "".into(),
            "b".into(),
            "  ".into(),
        ]);
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn extension_maps_flac_else_mp3() {
        assert_eq!(extension_for("flac"), "flac");
        assert_eq!(extension_for("320"), "mp3");
        assert_eq!(extension_for("128"), "mp3");
    }

    #[test]
    fn format_prefers_requested_then_falls_back() {
        assert_eq!(choose_format("flac", &["flac".into(), "320".into()]), Some("flac".to_string()));
        // No flac → next in preference order.
        assert_eq!(choose_format("flac", &["128".into(), "320".into()]), Some("320".to_string()));
        assert_eq!(choose_format("320", &["128".into()]), Some("128".to_string()));
        // Nothing servable offered.
        assert_eq!(choose_format("flac", &["ogg".into()]), None);
        // Empty list falls back to the preference.
        assert_eq!(choose_format("flac", &[]), Some("flac".to_string()));
    }

    #[test]
    fn write_atomic_places_flat_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_atomic(dir.path(), "DEABC1234567", "flac", b"hello").unwrap();
        assert_eq!(path, dir.path().join("DEABC1234567.flac"));
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        // No temp leftovers.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[test]
    fn track_status_classification() {
        let ready = TrackStatus {
            isrc: "A".into(),
            state: Some("ready".into()),
            ..Default::default()
        };
        assert!(ready.is_ready() && !ready.is_failed());

        let pending = TrackStatus {
            isrc: "A".into(),
            state: Some("pending".into()),
            ..Default::default()
        };
        assert!(!pending.is_ready() && !pending.is_failed());

        let unknown = TrackStatus {
            isrc: "A".into(),
            unknown: true,
            ..Default::default()
        };
        assert!(unknown.is_failed());

        let failed = TrackStatus {
            isrc: "A".into(),
            state: Some("failed".into()),
            error: Some("nope".into()),
            ..Default::default()
        };
        assert!(failed.is_failed());
    }
}