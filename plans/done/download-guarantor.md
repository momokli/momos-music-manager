## Plan: download-guarantor

**Status**: done ✅
**Branch**: `feat/download-guarantor`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Aggressive auto-remediation background task that guarantees 100% file coverage for
all subscribed Spotify playlists. Two-phase architecture:

1. **Queue Sync**: Polls the deemix-pyweb API every 5 minutes, UPSERTs real
   download status into `deemix_downloads` (fixing the currently-stale DB), detects
   zombie entries that block auto-downloads.

2. **Gap Remediation**: For every track in every subscribed playlist that has no
   linked file, tries deemix first (re-queue playlist), then falls back to spotDL
   (YouTube download via local CLI).

### Current State (from production DB analysis, 2026-07-17)

**16 subscribed playlists, 1,679 Spotify tracks:**

| Status                                        | Tracks | %     |
| --------------------------------------------- | ------ | ----- |
| Have linked files                             | 1,485  | 88.4% |
| Pipeline bugs (zombie entries, unlinked ISRC) | 107    | 6.4%  |
| Not on Deezer (need alternative source)       | 98     | 5.8%  |

**Pipeline bugs breakdown:**

| Bug                                          | Tracks                            | Root cause                                                                                                                                                                    |
| -------------------------------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Zombie `deemix_downloads` entry blocks retry | ~22 ("80s/90s")                   | Entry exists with 0/0 downloaded → `!has_deemix_entry` is false, `last_polled_at` is set, `new_tracks_found` is false → auto-download never fires                             |
| `deemix_downloads` never updated from queue  | 16 entries stuck at `queued, 0/X` | `deemix_queue_handler` uses `INSERT OR IGNORE` — new entries are backfilled but existing ones never updated. No background poller.                                            |
| Files on disk but ISRC mismatch              | ~85                               | deemix writes Deezer's ISRC; Spotify track has different ISRC → `v_file_track_link` doesn't match. Files exist in `/Users/momo/Music/flacs/` but are invisible to the system. |

**Deezer catalog gap:**

98 tracks don't exist on Deezer at all (Beatport exclusives, 90s tracks, very new releases).
Examples: Sam Paganini — Rave, GIGI D'AGOSTINO — The Riddle, venbee — not my day.

### Architecture

```
DownloadGuarantor::run()   (every 10 minutes)
│
├─ 1. QUEUE SYNC
│   ├─ GET /api/getQueue from deemix-pyweb
│   ├─ UPSERT all items into deemix_downloads
│   │   (status, track_count_downloaded, playlist_name, errors)
│   ├─ Detect zombies: entry exists, status='queued', downloaded=0
│   └─ Detect stuck: status='inQueue' for > 1 hour
│
├─ 2. GAP ANALYSIS (per subscribed playlist)
│   ├─ Query: tracks in playlist WITHOUT linked files
│   ├─ Categorize each missing track:
│   │   ├─ In deemix error list → NOT ON DEEZER → needs spotDL
│   │   ├─ Playlist is a zombie → RE-QUEUE to deemix
│   │   └─ Otherwise → FILE MAY EXIST (ISRC mismatch)
│   │       → Check filesystem for artist+title match
│   │       → If found: create file→track link directly
│   │       → If not: re-queue to deemix
│   │
│   └─ Log: "Playlist X: 5 missing (2 Deezer-gap, 3 re-queued)"
│
├─ 3. REMEDIATION
│   ├─ Re-queue: POST /api/addToQueue with playlist URL
│   │   (deemix re-scans for new tracks)
│   └─ spotDL fallback (per track, for Deezer-gap tracks):
│       spotdl download "https://open.spotify.com/track/{id}"
│           --output /Users/momo/Music/flacs
│           --bitrate 320k
│           --format mp3
│
└─ 4. FILE INGESTION (automatic, no code needed)
    ├─ Folder scanner picks up new files from flacs dir
    ├─ ISRC matching links to service_tracks via v_file_track_link
    └─ If ISRC doesn't match: fuzzy artist+title matching (new code)
```

### Design Decisions

1. **spotDL installed as system dependency** (not a separate service).
   `pip install spotdl`. Called via `std::process::Command`. No HTTP wrapper,
   no Docker container. Files go directly to `/Users/momo/Music/flacs/`.

2. **spotDL downloads are MP3** (128-192kbps from YouTube). Lower quality than
   deemix FLAC but acceptable for the 5.8% gap. The format preference system
   already prefers FLAC over MP3 when both exist.

3. **spotDL rate limiting**: Max 1 download per 2 seconds to avoid YouTube
   throttling. For 98 tracks, this means ~3 minutes total — well within the
   10-minute guarantor cycle.

4. **No new DB table needed**. Reuse `deemix_downloads` (now accurately synced).
   spotDL progress tracked via task logs in TaskManager.

5. **Fuzzy ISRC matching**: When a file exists in flacs dir but has no
   `v_file_track_link` entry, try matching by normalized artist+title
   (lowercase, remove punctuation, strip featuring/remix suffixes). If match
   found, INSERT into `file_resolved_tags` directly (bypassing the ISRC
   requirement).

6. **The queue sync fix also solves the UI problem**: The Deemix Queue page
   (`#deemix-queue`) currently shows stale data because `deemix_queue_handler`
   only backfills new entries with `INSERT OR IGNORE`. After UPSERT, it shows
   accurate download counts for all entries.

### What this does NOT do

- Does NOT change how deemix downloads work (same Docker container, same output dir)
- Does NOT change the folder scanner (it already picks up new files)
- Does NOT add a UI (existing `#deemix-queue` page now shows accurate data)
- Does NOT handle SoundCloud or YouTube playlists (Spotify subscriptions only)

### New module: `src/download_guarantor.rs`

```rust
use std::time::Duration;
use sqlx::Pool;
use crate::tasks::TaskManager;

pub struct DownloadGuarantor {
    db: Pool<Sqlite>,
    task_manager: TaskManager,
    deemix_base_url: String,  // "http://localhost:6596"
    flacs_dir: String,        // "/Users/momo/Music/flacs"
    interval: Duration,       // 10 minutes
}

impl DownloadGuarantor {
    pub fn new(db: Pool<Sqlite>, task_manager: TaskManager) -> Self;

    /// Main loop. Runs every `interval`.
    pub async fn run(&self, cancel_token: CancellationToken);

    /// ── Step 1: Sync deemix queue → deemix_downloads ──────────────
    async fn sync_queue(&self) -> Result<SyncReport>;

    /// ── Step 2: Find missing files per subscription ──────────────
    async fn analyze_gaps(&self) -> Result<Vec<SubscriptionGap>>;

    /// ── Step 3: Remediate gaps ───────────────────────────────────
    async fn remediate(&self, gaps: &[SubscriptionGap]) -> Result<RemediationReport>;

    /// ── Helpers ──────────────────────────────────────────────────
    async fn requeue_playlist(&self, playlist_url: &str) -> Result<()>;
    async fn spotdl_download_track(&self, spotify_track_id: &str, artist: &str, title: &str) -> Result<()>;
    async fn fuzzy_match_file(&self, artist: &str, title: &str) -> Result<Option<i64>>;
}

struct SyncReport {
    items_synced: usize,
    zombies_detected: Vec<String>,  // playlist names with 0 downloads
    stuck_detected: Vec<String>,    // playlist names inQueue > 1h
}

struct SubscriptionGap {
    subscription_id: i64,
    playlist_name: String,
    playlist_url: String,
    total_tracks: usize,
    missing_tracks: Vec<MissingTrack>,
}

struct MissingTrack {
    track_id: i64,
    title: String,
    artist: String,
    isrc: Option<String>,
    spotify_url: String,
    reason: MissingReason,
}

enum MissingReason {
    NotOnDeezer,        // In deemix error list — needs spotDL
    ZombiePlaylist,     // deemix entry exists but 0 downloads
    FileMayExist,       // Not in error list, file might be on disk
}

struct RemediationReport {
    requeued_playlists: usize,
    spotdl_downloads: usize,
    spotdl_failures: usize,
    fuzzy_matches: usize,
}
```

### Backend: `src/db/playlists.rs` — new functions

```rust
/// UPSERT a deemix_downloads row from actual queue data.
/// Unlike the existing INSERT OR IGNORE in deemix_queue_handler,
/// this updates existing rows with real download counts.
pub async fn upsert_deemix_download(
    pool: &Pool<Sqlite>,
    spotify_url: &str,
    playlist_name: &str,
    status: &str,
    track_count_total: i64,
    track_count_downloaded: i64,
    error_json: Option<&str>,
) -> Result<()>;

/// Get all deemix_downloads entries with zero downloads (zombies).
pub async fn get_zombie_deemix_entries(
    pool: &Pool<Sqlite>,
) -> Result<Vec<(i64, String)>>;

/// Find a file in the flacs dir by fuzzy artist+title match.
/// Returns file_id if found, None otherwise.
pub async fn find_file_by_artist_title(
    pool: &Pool<Sqlite>,
    artist: &str,
    title: &str,
) -> Result<Option<i64>>;

/// Create a direct file→track link bypassing the ISRC requirement.
/// Used for fuzzy-matched files where ISRCs differ between
/// Deezer and Spotify.
pub async fn link_file_to_track_direct(
    pool: &Pool<Sqlite>,
    file_id: i64,
    track_id: i64,
) -> Result<()>;
```

### Backend: `src/api/deemix_api.rs` — fix `deemix_queue_handler`

Change `INSERT OR IGNORE` to an UPSERT that updates existing rows:

```rust
// Before (line 186-198):
let _ = sqlx::query(
    "INSERT OR IGNORE INTO deemix_downloads
     (spotify_playlist_url, playlist_name, status, track_count_total,
      track_count_downloaded, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)"
)

// After:
let _ = sqlx::query(
    "INSERT INTO deemix_downloads
     (spotify_playlist_url, playlist_name, status, track_count_total,
      track_count_downloaded, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(spotify_playlist_url) DO UPDATE SET
         playlist_name = excluded.playlist_name,
         status = excluded.status,
         track_count_total = excluded.track_count_total,
         track_count_downloaded = excluded.track_count_downloaded,
         updated_at = excluded.updated_at"
)
```

This single change makes the Deemix Queue page show accurate data whenever
it's loaded — the handler already fetches the live queue and now updates
existing rows instead of ignoring them.

### Backend: `src/poller.rs` — fix auto-download condition

Add a check for zombie entries (track_count_downloaded == 0):

```rust
// Line 537: extend condition
let has_zero_downloads = if has_deemix_entry {
    sqlx::query_scalar::<_, i64>(
        "SELECT track_count_downloaded FROM deemix_downloads
         WHERE spotify_playlist_url = ?"
    )
    .bind(&deemix_url)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or(-1) == 0
} else {
    false
};

if subscription.last_polled_at.is_none()
    || new_tracks_found
    || !has_deemix_entry
    || has_zero_downloads  // ← NEW
{
    // ... auto-download ...
}
```

### Backend: `src/main.rs` — spawn DownloadGuarantor

In `serve()`, after spawning the maintainer and auto-backup poller:

```rust
// Download Guarantor: ensures 100% file coverage for subscriptions
if credentials.is_spotify_configured() {
    let guarantor = crate::download_guarantor::DownloadGuarantor::new(
        state.db.clone(),
        state.task_manager.clone(),
    );
    let cancel = cancel_token.clone();
    tokio::spawn(async move {
        guarantor.run(cancel).await;
    });
    info!("Download Guarantor started");
}
```

### Dependencies

- **spotDL**: Must be installed (`pip install spotdl`). Added as a system
  requirement, not a Cargo dependency. If not installed, the guarantor
  logs a warning and skips spotDL downloads (deemix still works).

### Files to create

| File                        | Description                                                                                       |
| --------------------------- | ------------------------------------------------------------------------------------------------- |
| `src/download_guarantor.rs` | ~300 lines — main module: queue sync, gap analysis, remediation, spotDL CLI calls, fuzzy matching |

### Files to modify

| File                    | Change                                                                                                                                  |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`           | Add `pub mod download_guarantor;` + spawn in `serve()` (~15 lines)                                                                      |
| `src/poller.rs`         | Extend auto-download condition with `has_zero_downloads` check (~15 lines)                                                              |
| `src/api/deemix_api.rs` | Change `INSERT OR IGNORE` → `UPSERT` in `deemix_queue_handler` (~15 lines)                                                              |
| `src/db/playlists.rs`   | Add `upsert_deemix_download()`, `get_zombie_deemix_entries()`, `find_file_by_artist_title()`, `link_file_to_track_direct()` (~80 lines) |
| `AGENT.md`              | Update plan status, bump "Last Updated"                                                                                                 |

### Acceptance Criteria

**Queue Sync:**

- [ ] `GET /api/getQueue` data UPSERTed into `deemix_downloads` on every cycle
- [ ] `deemix_queue_handler` uses UPSERT (not INSERT OR IGNORE) — loading the
      Deemix Queue page immediately shows accurate download counts
- [ ] Zombie entries (track_count_downloaded=0) detected and logged
- [ ] Stuck items (inQueue > 1h) detected and logged

**Gap Analysis:**

- [ ] Every subscribed playlist checked for tracks without linked files
- [ ] Missing tracks categorized: NotOnDeezer / ZombiePlaylist / FileMayExist
- [ ] Gap report logged with per-playlist counts

**Remediation:**

- [ ] Zombie playlists re-queued to deemix via `POST /api/addToQueue`
- [ ] Deezer-gap tracks downloaded via spotDL CLI (`spotdl download <url>`)
- [ ] spotDL downloads go to `/Users/momo/Music/flacs/`
- [ ] spotDL rate limited: max 1 download per 2 seconds
- [ ] Fuzzy artist+title matching finds unlinked files in flacs dir
- [ ] Fuzzy-matched files linked to tracks via direct INSERT (bypassing ISRC)

**Auto-download fix:**

- [ ] Zombie entries trigger auto-download ("80s/90s" gets re-queued)
- [ ] No regression: existing auto-download logic still works

**End-to-end:**

- [ ] After guarantor runs once, all fixable gaps are resolved
- [ ] Subscription coverage improves from 88.4% → ≥94.8% after first cycle
- [ ] spotDL fills the remaining 5.8% Deezer gap
- [ ] Subsequent cycles are idempotent (no double-downloads)
- [ ] Guarantor logs every action at `info!` level
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing tests)
- [ ] Graceful degradation: if spotDL not installed, deemix still works
- [ ] Cancel token honored for clean shutdown

### Agent Decomposition (4 agents, zero file conflicts)

| Agent | Files                                    | Work                                                                                                                                                                                         |
| ----- | ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A** | `src/download_guarantor.rs` (new)        | Core module: struct, `run()` loop, `sync_queue()`, `analyze_gaps()`, `remediate()`, spotDL CLI calls, fuzzy matching (~300 lines)                                                            |
| **B** | `src/db/playlists.rs`, `src/main.rs`     | DB functions: `upsert_deemix_download()`, `get_zombie_deemix_entries()`, `find_file_by_artist_title()`, `link_file_to_track_direct()` + module declaration + spawn in `serve()` (~100 lines) |
| **C** | `src/api/deemix_api.rs`, `src/poller.rs` | API fix: `INSERT OR IGNORE` → `UPSERT` in `deemix_queue_handler` + auto-download condition fix in poller (~30 lines)                                                                         |
| **D** | `AGENT.md`                               | Plan status update, bump "Last Updated"                                                                                                                                                      |

**Write scope verification — zero overlap:**

- Agent A: new file only
- Agent B: `src/db/playlists.rs`, `src/main.rs`
- Agent C: `src/api/deemix_api.rs`, `src/poller.rs`
- Agent D: `AGENT.md` only

All 4 agents can run in parallel.

### Testing strategy

**Unit tests** (`src/download_guarantor.rs` `#[cfg(test)]`):

- `test_fuzzy_match_exact` — exact artist+title match
- `test_fuzzy_match_case_insensitive` — different casing
- `test_fuzzy_match_punctuation` — special characters
- `test_fuzzy_match_remix_suffix` — "Track (Remix)" matches "Track"
- `test_spotdl_command_format` — correct CLI arguments assembled
- `test_rate_limit_spacing` — downloads spaced ≥2s apart

**Integration tests** (`tests/api_storage.rs` — extend existing):

- `deemix_queue_handler_upserts` — verify UPSERT updates existing rows
- `download_guarantor_reports_gaps` — gap analysis returns correct data

**Manual verification** (after deploy):

```bash
# Check queue sync is working
sqlite3 library.db "SELECT playlist_name, status, track_count_downloaded FROM deemix_downloads"

# After guarantor cycle, check coverage improved
curl -s localhost:3000/api/tasks | jq '.data[] | select(.type == "DownloadGuarantor")'

# Verify 80s/90s was re-queued
sqlite3 library.db "SELECT * FROM deemix_downloads WHERE spotify_playlist_url LIKE '%2gy2iH2%'"
```

### Out of scope (v2)

- spotDL as a managed service (Docker container or HTTP wrapper)
- SoundCloud or YouTube playlist downloads
- Per-track download progress UI
- Automatic spotDL installation (user runs `pip install spotdl`)
- Download quality selection per playlist
- Bandcamp or Beatport as download sources

---

