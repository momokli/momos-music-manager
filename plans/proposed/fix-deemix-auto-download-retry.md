## Plan: fix-deemix-auto-download-retry

**Status**: proposed
**Branch**: `fix/deemix-auto-download-retry`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Fix the subscription poller's auto-deemix-download so it retries when the first
attempt fails (e.g., ARL expired). Currently the auto-download only triggers on
first poll (`last_polled_at IS NULL`) or when new tracks are found
(`new_tracks_found`). If the first poll's auto-download fails (dead ARL),
`last_polled_at` gets set and every subsequent poll hits the snapshot-based
early-return — the auto-download code is **never reached again**.

### Root Cause

The snapshot early-return at `poller.rs:265` (`return Ok(())`) skips the
auto-download block at line 446 entirely. Meanwhile, the main poller loop
(line 127) unconditionally sets `last_polled_at` after every poll — even
failed ones. So:

1. First poll: ARL dead → `ensure_queued()` fails → no `deemix_downloads` entry
2. `last_polled_at` gets set (line 127, runs always)
3. Next poll: snapshot unchanged → early-return at line 265 → auto-download never reached

This does NOT affect the "new tracks" path: if Spotify adds tracks, the
snapshot changes → track fetch runs → `new_tracks_found = true` → auto-download
re-triggers correctly. The bug is specifically about the **initial** download
failing and never getting a retry.

### Fix

**Step 1**: Change the snapshot early-return to set a flag instead of returning.
The track fetch gets skipped (saving API calls), but execution continues past it
to the auto-download block.

**Step 2**: Expand the auto-download condition to also trigger when no
`deemix_downloads` entry exists for the playlist.

**Step 3**: Add a lightweight DB helper to check for an existing
`deemix_downloads` entry.

### Before / After (pseudocode)

```rust
// ── BEFORE ──
snapshot_check() {
    if unchanged { return Ok(()); }  // ← auto-download NEVER reached
}
track_fetch_loop();                  // ← skipped by early return
auto_download {                      // ← skipped by early return
    if first_poll || new_tracks { ... }
}

// ── AFTER ──
let mut skip_tracks = false;
snapshot_check() {
    if unchanged { skip_tracks = true; }  // ← flag, don't return
}
if !skip_tracks { track_fetch_loop(); }   // ← still skipped when unchanged
auto_download {                            // ← ALWAYS reached now
    if first_poll || new_tracks || !has_deemix_entry { ... }
}
```

### Files to modify

| File                  | Change                                                                        |
| --------------------- | ----------------------------------------------------------------------------- |
| `src/poller.rs`       | Restructure snaphot early-return + expand auto-download condition (~50 lines) |
| `src/db/playlists.rs` | Add `has_deemix_download_entry()` helper (~10 lines)                          |

### `src/db/playlists.rs` — new function

```rust
/// Check if a deemix_downloads entry exists for the given Spotify playlist URL.
pub async fn has_deemix_download_entry(
    pool: &Pool<Sqlite>,
    spotify_playlist_url: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM deemix_downloads WHERE spotify_playlist_url = ?"
    )
    .bind(spotify_playlist_url)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}
```

### `src/poller.rs` — changes

**1. Snapshot block (lines 242-290)**: Replace `return Ok(())` at line 265 with
`skip_track_fetch = true`. Keep the remote-count update (still useful).

**2. Track fetch loop (line 292)**: Wrap in `if !skip_track_fetch { ... }`.

**3. Auto-download block (lines 446-492)**:

- Build the deemix URL BEFORE the condition (move line 450-451 up)
- Add a DB check: `let has_entry = db::has_deemix_download_entry(db, &url).await.unwrap_or(false);`
- Change condition from:
  `if subscription.last_polled_at.is_none() || new_tracks_found`
  to:
  `if subscription.last_polled_at.is_none() || new_tracks_found || !has_entry`

### Why `ensure_queued` being called more often is safe

`ensure_queued` is idempotent for active items:

- Item in queue + status `queued`/`downloading`/`processing` → returns `Ok(())` immediately (no-op)
- Item in queue + terminal status → calls `retry_download()` (re-scan for new tracks — correct)
- Item NOT in queue → calls `add_to_queue()`

The extra DB query (`has_deemix_download_entry`) is a cheap indexed lookup
(`idx_deemix_downloads_url`), so running it every poll cycle is negligible.

### Acceptance Criteria

- [ ] Dead-ARL first poll: auto-download fails, `last_polled_at` set, no `deemix_downloads` entry
- [ ] Second poll (ARL now fresh): snapshot unchanged → `skip_track_fetch=true` → track fetch skipped → auto-download **reached** → `!has_entry` triggers `ensure_queued()` → entry created ✅
- [ ] Third poll (entry exists, no new tracks): snapshot unchanged → skip tracks → auto-download reached → all conditions false → skipped (no-op) ✅
- [ ] New tracks arrive: snapshot changes → `skip_track_fetch=false` → tracks fetched → `new_tracks_found=true` → `ensure_queued()` → `retry_download()` (re-scan) ✅
- [ ] Normal first-poll (ARL fresh from start): behaves exactly as before ✅
- [ ] Subscription without any linked `service_playlist_id`: falls through to track fetch as before ✅
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing tests)

---

