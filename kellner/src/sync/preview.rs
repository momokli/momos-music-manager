//! `kellner comment --preview` — offline, read-only Comment preview (issue #671, C5).
//!
//! Reads the owner's active backpacks from the hub exactly like `kellner backpack`
//! (shared [`crate::sync::backpack::collect_desired`]), renders the Comment for the
//! first `--limit` tracks through the **same** [`comment_template::render`] call the
//! sync path uses, and prints `"{title}  ->  {comment}"`.
//!
//! Strictly read-only: no order, no fetch, no file write, no `state.json` mutation.
//! Only the two hub GETs (`list_backpacks` + `export_backpack`) are issued; the
//! music-api URL/token and the collection root are never touched.

use anyhow::Result;

use crate::client::hub::HubClient;
use crate::sync::backpack::collect_desired;
use comment_template::{TrackFacts, render};

/// Tally of one preview run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewStats {
    /// Backpacks the hub reported (active or not).
    pub backpacks: usize,
    /// Active backpacks exported.
    pub active: usize,
    /// Distinct desired ISRCs across all active backpacks.
    pub desired: usize,
    /// Lines actually printed (bounded by `--limit`).
    pub shown: usize,
}

/// Print the first `limit` desired tracks and their rendered Comment. Never
/// mutates local or remote state.
pub async fn run_preview(hub: &HubClient, limit: usize) -> Result<PreviewStats> {
    let ds = collect_desired(hub).await?;
    let mut stats = PreviewStats {
        backpacks: ds.backpacks,
        active: ds.active,
        desired: ds.tracks.len(),
        shown: 0,
    };

    if ds.active == 0 {
        println!("kellner: comment --preview: no active backpack");
        return Ok(stats);
    }
    if ds.tracks.is_empty() {
        println!(
            "kellner: comment --preview: 0 track(s) in {} active backpack(s)",
            ds.active
        );
        return Ok(stats);
    }

    // BTreeMap iteration is ascending by ISRC — the same order `retag_existing`
    // walks, so `--limit` selects the same first N as a sync would tag first.
    for dev in ds.tracks.values().take(limit) {
        let comment = render(
            &dev.comment_template,
            &TrackFacts {
                types: vec![],
                tags: dev.tags.clone(),
                source_ids: vec![],
                bpm: dev.bpm,
                key: dev.key.clone(),
                energy: dev.energy,
            },
        );
        println!("{}  ->  {}", dev.title, comment);
        stats.shown += 1;
    }
    Ok(stats)
}
