//! Lightweight, process-wide Spotify request counters.
//!
//! Purpose: make the API load **visible** — which subsystem issues the most
//! requests — without an external metrics stack. Counters are *approximate*
//! request counts aggregated at the call sites (paged streams are estimated
//! from the number of items consumed and the page size). They are never reset
//! by the app; call [`reset`] in tests.
//!
//! Read them via `GET /api/services/spotify/metrics` or the global-poller cycle
//! summary log line.

use std::sync::atomic::{AtomicU64, Ordering};

/// Which part of the app a Spotify request belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `/me/tracks` liked-songs sync (full or incremental pass).
    LikedSync,
    /// Paging the user's playlists (`/me/playlists`).
    PlaylistList,
    /// Fetching a playlist's tracks (`/playlists/{id}/tracks`) or a single playlist.
    PlaylistTracks,
    /// Subscription poller fetches.
    Subscriptions,
    /// BPM//key system-playlist reconcile.
    BpmKeySync,
    /// OAuth token refresh (accounts endpoint).
    Auth,
    /// Anything else.
    Other,
}

impl Source {
    /// Stable label used in the JSON snapshot / log summaries.
    pub fn label(self) -> &'static str {
        match self {
            Source::LikedSync => "liked_sync",
            Source::PlaylistList => "playlist_list",
            Source::PlaylistTracks => "playlist_tracks",
            Source::Subscriptions => "subscriptions",
            Source::BpmKeySync => "bpm_key_sync",
            Source::Auth => "auth",
            Source::Other => "other",
        }
    }

    /// Every source, in a stable order (for snapshots).
    pub const ALL: [Source; 7] = [
        Source::LikedSync,
        Source::PlaylistList,
        Source::PlaylistTracks,
        Source::Subscriptions,
        Source::BpmKeySync,
        Source::Auth,
        Source::Other,
    ];
}

static LIKED_SYNC: AtomicU64 = AtomicU64::new(0);
static PLAYLIST_LIST: AtomicU64 = AtomicU64::new(0);
static PLAYLIST_TRACKS: AtomicU64 = AtomicU64::new(0);
static SUBSCRIPTIONS: AtomicU64 = AtomicU64::new(0);
static BPM_KEY_SYNC: AtomicU64 = AtomicU64::new(0);
static AUTH: AtomicU64 = AtomicU64::new(0);
static OTHER: AtomicU64 = AtomicU64::new(0);

fn cell(src: Source) -> &'static AtomicU64 {
    match src {
        Source::LikedSync => &LIKED_SYNC,
        Source::PlaylistList => &PLAYLIST_LIST,
        Source::PlaylistTracks => &PLAYLIST_TRACKS,
        Source::Subscriptions => &SUBSCRIPTIONS,
        Source::BpmKeySync => &BPM_KEY_SYNC,
        Source::Auth => &AUTH,
        Source::Other => &OTHER,
    }
}

/// Add 1 to `src`.
pub fn record(src: Source) {
    add(src, 1);
}

/// Add `n` to `src`.
pub fn add(src: Source, n: u64) {
    if n == 0 {
        return;
    }
    cell(src).fetch_add(n, Ordering::Relaxed);
}

/// Current value of `src`.
pub fn get(src: Source) -> u64 {
    cell(src).load(Ordering::Relaxed)
}

/// `(label, count)` for every source, in [`Source::ALL`] order.
pub fn snapshot() -> Vec<(&'static str, u64)> {
    Source::ALL.iter().map(|s| (s.label(), get(*s))).collect()
}

/// Total across every source.
pub fn total() -> u64 {
    Source::ALL.iter().map(|s| get(*s)).sum()
}

/// Reset every counter to zero (tests / diagnostics).
pub fn reset() {
    for s in Source::ALL {
        cell(s).store(0, Ordering::Relaxed);
    }
}

/// One-line summary for logs, e.g. `liked_sync=1048 playlist_list=11 …`.
pub fn summary() -> String {
    snapshot()
        .into_iter()
        .map(|(name, n)| format!("{name}={n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_add_and_snapshot() {
        reset();
        record(Source::LikedSync);
        record(Source::LikedSync);
        add(Source::PlaylistList, 11);
        assert_eq!(get(Source::LikedSync), 2);
        assert_eq!(get(Source::PlaylistList), 11);
        assert_eq!(total(), 13);

        let snap = snapshot();
        assert_eq!(snap[0], ("liked_sync", 2));
        assert_eq!(snap[1], ("playlist_list", 11));
        assert!(summary().contains("liked_sync=2"));

        reset();
        assert_eq!(total(), 0);
    }

    #[test]
    fn add_zero_is_a_noop() {
        reset();
        add(Source::Other, 0);
        assert_eq!(get(Source::Other), 0);
    }
}
