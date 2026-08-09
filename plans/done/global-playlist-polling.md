## Plan: global-playlist-polling

**Status**: done ✅
**Branch**: `feat/global-playlist-polling`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `007_playlist_snapshot.sql`

### Description

Add a background poller that regularly checks ALL Spotify playlists for changes using snapshot-based detection — minimal API traffic by only fetching tracks when a playlist actually changed. Complements the existing subscription poller (which only covers explicitly-subscribed playlists).

### Why

- Subscription poller only covers playlists users manually subscribe to
- Unsubscribed playlists go stale until a manual full sync
- Spotify `SimplifiedPlaylist` includes `snapshot_id` — perfect for cheap change detection
- 1 API call to check 50 playlists, full track fetch only when `snapshot_id` differs

### API traffic estimate

- 200 playlists at 50/page = 4 calls to fetch the playlist list
- Assume 5 changed in 15 min, each ~2 calls for track pages = 10 calls
- **Total: ~14 API calls every 15 min** (well within Spotify's ~180/min rate limit)

### Key differences from subscription poller

| Aspect                     | Subscription Poller           | Global Poller                                                |
| -------------------------- | ----------------------------- | ------------------------------------------------------------ |
| Scope                      | Only subscribed playlists     | **All** Spotify playlists                                    |
| Detection                  | Always fetches tracks         | **Snapshot-based** — fetches tracks only if snapshot changed |
| Frequency                  | 30s check loop, ~5min per sub | 15min global cycle                                           |
| New playlist discovery     | ❌                            | ✅                                                           |
| Deleted playlist detection | ❌                            | ✅                                                           |

### Config (config.toml, not env)

```toml
[polling]
# Interval between global playlist polling cycles (seconds), 0 = disabled
# Default: 900 (15 minutes)
global_interval_secs = 900
```

Env override still available for dev: `MOMOS_GLOBAL_POLL_INTERVAL_SECS=60`

### Migration: `migrations/007_playlist_snapshot.sql`

```sql
ALTER TABLE service_playlists ADD COLUMN snapshot_id TEXT;

SELECT 'Migration 007 applied: added snapshot_id to service_playlists' as status;
```

### Backend: `src/config.rs` — PollingConfig

Add `PollingToml` struct + `global_interval_secs` to `ServiceCredentials`:

```rust
#[derive(Debug, Clone, Deserialize)]
struct PollingToml {
    global_interval_secs: Option<u64>,  // 0 = disabled
}

// In ServiceCredentials:
pub global_poll_interval_secs: u64,  // default 900
```

Priority: env `MOMOS_GLOBAL_POLL_INTERVAL_SECS` > TOML `[polling].global_interval_secs` > default 900.

### Backend: `src/global_poller.rs` — new module

```rust
pub async fn start_global_poller(
    db: Pool<Sqlite>,
    credentials: ServiceCredentials,
    cancel_token: CancellationToken,
)
```

**Algorithm (each cycle):**

1. Sleep for `global_poll_interval_secs`
2. Create `SpotifyClient::from_stored_tokens()`
3. Fetch ALL user playlists via `GET /me/playlists` (paginated, with retry)
4. For each playlist:
   - Look up in DB by `service='spotify' AND playlist_id`
   - If not in DB → INSERT, mark as new
   - If `snapshot_id` matches DB → skip (unchanged)
   - If `snapshot_id` differs → fetch tracks (paginated, with retry), upsert new tracks, update `snapshot_id` + `last_fetched_at` + `remote_track_count`
5. Log new playlists found, new tracks added, playlists deleted from Spotify (in DB but not in API response)
6. Graceful errors: 429 → backoff + retry, auth failure → skip cycle, network error → skip cycle
7. Honor `cancel_token` for clean shutdown

### Backend: `src/spotify/models.rs` — add snapshot_id

```rust
pub struct PlaylistInfo {
    // ... existing fields ...
    pub snapshot_id: String,  // NEW
}
```

Update `impl From<&SimplifiedPlaylist>` to include `snapshot_id`.

### Backend: `src/db.rs` — new DB functions

```rust
/// Get all Spotify playlists (id, playlist_id, snapshot_id) for comparison
pub async fn get_spotify_playlist_snapshots(pool: &Pool<Sqlite>) -> Result<Vec<(i64, String, Option<String>)>>;

/// Update snapshot_id for a playlist
pub async fn update_playlist_snapshot(pool: &Pool<Sqlite>, playlist_id: &str, snapshot_id: &str) -> Result<()>;

/// Mark a service playlist as inactive (deleted from Spotify)
pub async fn mark_playlist_inactive(pool: &Pool<Sqlite>, db_id: i64) -> Result<()>;
```

### Backend: `src/main.rs` — spawn global poller

In `serve()`, after starting the subscription poller, spawn the global poller:

```rust
if credentials.global_poll_interval_secs > 0 && credentials.is_spotify_configured() {
    let global_cancel = cancel_token.clone();
    tokio::spawn(async move {
        crate::global_poller::start_global_poller(db.clone(), credentials, global_cancel).await;
    });
    info!("Global playlist poller started (interval: {}s)", credentials.global_poll_interval_secs);
} else {
    info!("Global playlist poller disabled (interval=0 or Spotify not configured)");
}
```

### Files to modify

- `migrations/007_playlist_snapshot.sql` — new migration
- `src/config.rs` — add `PollingToml` + `global_poll_interval_secs` field
- `src/global_poller.rs` — new 250-line background task module
- `src/spotify/models.rs` — add `snapshot_id` to `PlaylistInfo`
- `src/db.rs` — `get_spotify_playlist_snapshots`, `update_playlist_snapshot`, `mark_playlist_inactive`
- `src/main.rs` — spawn global poller

### Acceptance Criteria

- [x] All Spotify playlists checked every `global_poll_interval_secs` (default 900s = 15min)
- [x] Snapshot-based change detection: unchanged playlists skip track fetch entirely
- [x] New playlists (in Spotify but not DB) auto-discovered and inserted
- [x] Changed playlists: only new tracks added, existing tracks skipped
- [x] Deleted playlists (in DB but not Spotify) logged with `warn!`
- [x] New tracks found are logged with `info!` including artist + playlist name
- [x] `snapshot_id` updated in `service_playlists` after successful track sync
- [x] `last_fetched_at` and `remote_track_count` updated same as subscription poller
- [x] 429 rate limits handled with `Retry-After` backoff (reuse `extract_retry_after_secs`)
- [x] Auth failure / network error → skip cycle, retry next cycle
- [x] Cancel token honored for clean shutdown
- [x] Config via `[polling]` section in `config.toml` + env override
- [x] Graceful skip when Spotify not configured (no crash)
- [x] Backend compiles (`cargo build`)
- [x] Fresh DB: migrations 001→007 run cleanly
- [x] No regressions: subscription poller still operates independently

---

