# Plan: spotify-api-diet

**Status**: approved (2026-10-08)
**Branch**: `feat/spotify-api-diet`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Cut steady-state Spotify API traffic from **O(library size)** to **O(new items)**.
The app currently re-fetches every liked track on every 15-minute poll cycle, which
exhausts Spotify's app-level rate limit and produces the chronic multi-hour
`Retry-After` 429s.

### Findings (verified in the codebase, 2026-10-08)

| Source | Cadence | ~Requests/day |
|--------|---------|---------------|
| **Liked `/me/tracks` full re-fetch** | every poll cycle (15 min) | **~100,000** |
| Global poller playlist listing (`/me/playlists`, 50/page × ~501) | every 15 min | ~1,050 |
| Subscription poller (only *due* subs) | 30 s tick | low (currently ~1 sub) |
| BPM//key system-playlist reconcile | manual / one-off | bursty (~2×N) |
| OAuth token refresh | hourly-ish | negligible |

- `liked_sync::sync_liked_songs` calls `get_saved_tracks()` → `current_user_saved_tracks(None)`
  = **50/page**; with ~52,376 likes that is **~1,048 requests per pass**.
- `global_poller::run_poll_cycle` runs it **exactly once per cycle** (Step 4b); the cycle
  interval defaults to **900 s**.
- The design is *deliberately* full-refresh ("an already-present membership is **not**
  skipped — the like date is refreshed on every pass"), which is what makes it O(library).

### Milestones

**M1 — Incremental liked sync (the big win)** — `src/liked_sync.rs`
- `/me/tracks` is ordered newest-first; a **re-like** moves an item to the front with a
  newer `added_at`. So page forward and **stop** once a full page of items is already
  known locally **with the same `added_at`** → steady state ≈ 1–3 requests.
- Only upsert new/changed memberships (no 52k writes/cycle).
- **Retire** (unlike) only when `total` shrank, and only then pay for a full scan (rare).
- Keep relike semantics; drop the per-cycle full cost.

**M2 — Cadence** — `src/global_poller.rs`, `src/db/settings.rs`
- Liked sync gets its **own interval** (`spotify.liked_sync_interval_secs`, default 3600 s)
  instead of running every poll cycle. Gate in Step 4b, stamp `…last_at` on success.

**M3 — Visibility** — `src/spotify/metrics.rs` (new), `src/global_poller.rs`, `src/poller.rs`,
`src/api/spotify_sync.rs`
- Process-wide per-source request counters (`liked_sync`, `playlist_list`, `playlist_tracks`,
  `subscriptions`, `bpm_key_sync`, `auth`, `other`); read via
  `GET /api/services/spotify/metrics` and in the poll-cycle summary log.

**M4 — BPM//key burst hygiene** — `src/bpm_key/sync.rs`, `src/tasks/mod.rs`
- Fix the stuck-`Running` bug: the early-abort/cancel paths `return` before the finalization
  block, so the task never reaches `Completed` and its `bpm_key_sync` conflict key then blocks
  all future syncs. Finalize on every exit path.
- Pace reconcile calls with a small inter-bucket delay (resumable, cooldown-aware).

### Acceptance Criteria

- [ ] `cargo build`, `cargo test`, `cd frontend && npx playwright test` all green
- [ ] Liked sync steady state issues ~1–3 requests (early-stop), not ~1,048
- [ ] New likes are picked up; relikes refresh `added_at`; unlikes still retire (on shrink)
- [ ] `GET /api/services/spotify/metrics` returns per-source counters
- [ ] `sync_bpm_key_playlists` reaches `Completed` on a rate-limited abort
- [ ] No `cargo fmt` churn

### Non-goals

- Changing relike semantics (kept — just made cheap).
- The `min_tracks` / playlist-volume of the BPM//key feature itself.
- Reducing the subscription-poller tick (low impact at current scale).
