# Plan: liked-songs-sync (Rediscovery M1)

**Status**: proposed
**Branch**: `feat/liked-songs-sync`
**Ready for review**: no
**Depends on**: nothing (independent of the other Rediscovery milestones)
**Migration needed**: yes — `032_playlist_kind.sql`
**Parent plan**: [`rediscovery-engine.md`](rediscovery-engine.md) (§ M1)
**Issue**: #50

### Description

Close the biggest data gap in the Rediscovery feature: **Liked Songs are not
synced.** `global_poller` only calls `GET /me/playlists`, and Spotify does not
return Likes there — they live behind `GET /me/tracks`.

Deliverables:

1. `/me/tracks` → a `liked` service playlist (with the **like date** as `added_at`).
2. A `playlist_kind` column so likes and our own generated packs are never counted
   as "curation".
3. `v_track_forgotten_facts` — a view exposing `playlist_count`, `last_touched_at`,
   `liked` / `liked_at` per track. This is the foundation M2 builds on.

### Ground truth (read from the code, 2026-10-01)

Things that shape the design — all verified, not assumed:

| Finding | Where | Consequence |
| --- | --- | --- |
| The poller **skips** tracks already in a playlist | `global_poller.rs` `fetch_and_store_playlist_tracks` — `if already_exists { continue }` | `added_at` is set once and never refreshed. **The liked sync must do the opposite** and always update `added_at`, because unlike→relike changes the date. |
| Step 4 marks any DB playlist absent from `/me/playlists` as "inactive" | `global_poller.rs` step 4 + `get_spotify_playlist_snapshots` (`WHERE service='spotify'`) | The `spotify:liked` row would be marked inactive and have its `snapshot_id` nulled **every cycle**. Must be excluded. |
| `ServicePlaylist` is a `FromRow` struct with explicit fields | `src/db/types.rs` | Adding `playlist_kind` to the table does **not** break existing `query_as` calls (sqlx ignores extra columns). No struct churn required. |
| Tags are auto-created from playlist names | `db::playlists::create_tags_from_playlists` + `refresh_track_tags` | The `liked` tag appears automatically — which is exactly what we want. No special-casing. |
| `add_track_to_playlist_with_added_at` already upserts `added_at` | `src/db/playlists.rs` | Reusable as-is for the liked sync. |
| No Spotify HTTP mock exists in the test suite | `tests/` (only `MockFetcher` for autoupdate) | The sync logic must be split so the **DB write path is testable without network**. |

---

## Part A — Migration `032_playlist_kind.sql`

```sql
-- Migration 032: distinguish curated playlists from the likes mirror and our own
-- generated packs. Only 'curated' playlists count towards a track's
-- "how well-used is this" score — likes are the baseline every track shares, and
-- 'generated' is our own output (counting either would make a track look used and
-- it would never resurface).

ALTER TABLE service_playlists ADD COLUMN playlist_kind TEXT NOT NULL DEFAULT 'curated';
-- values: 'curated' | 'liked' | 'generated'

-- The manual likes mirror (a real Spotify playlist named "liked"/"likes").
UPDATE service_playlists SET playlist_kind = 'liked'
 WHERE LOWER(TRIM(name)) IN ('liked', 'likes');

-- Existing generated dailies from the daily-tagging-queue feature.
UPDATE service_playlists SET playlist_kind = 'generated'
 WHERE service = 'local' AND name LIKE 'Daily-%';

CREATE INDEX idx_service_playlists_kind ON service_playlists(playlist_kind);

-- ── Forgotten-facts view ────────────────────────────────────────────────────
-- `last_touched_at` = the last time the track was added to ANY playlist
-- (curated or liked). The primary rediscovery signal.
-- The ADR-072 guard applies everywhere: tombstones only exist for archiving
-- playlists, but the guard is repeated so the view is correct standalone.
CREATE VIEW v_track_forgotten_facts AS
WITH playlist_facts AS (
    SELECT
        spt.track_id,
        SUM(CASE WHEN sp.playlist_kind = 'curated' THEN 1 ELSE 0 END) AS playlist_count,
        MAX(spt.added_at) AS last_touched_at,
        MAX(CASE WHEN sp.playlist_kind = 'liked' THEN spt.added_at END) AS liked_at
    FROM service_playlist_tracks spt
    JOIN service_playlists sp ON sp.id = spt.playlist_id
    WHERE sp.playlist_kind IN ('curated', 'liked')
      AND (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)
    GROUP BY spt.track_id
)
SELECT
    pf.track_id,
    pf.playlist_count,
    pf.last_touched_at,
    pf.liked_at,
    (pf.liked_at IS NOT NULL) AS liked
FROM playlist_facts pf;

SELECT 'Migration 032 applied: playlist_kind + v_track_forgotten_facts' as status;
```

Notes:

- `SUM(CASE WHEN ... THEN 1 ELSE 0 END)` counts **rows**, not `DISTINCT playlist_id`.
  `service_playlist_tracks` has PK `(playlist_id, track_id)`, so a row per
  playlist per track is guaranteed — no duplicates to collapse.
- `MAX(CASE WHEN ... END)` returns `NULL` when the track is not liked; `liked`
  derives from that rather than a second scan.
- Name matching is deliberately not used for `generated` beyond the known
  `Daily-%` prefix. Going forward, M3 sets `playlist_kind='generated'` explicitly
  at creation — no fragile name matching.

---

## Part B — Spotify client

`src/spotify/client.rs`, next to `get_user_playlists`:

```rust
/// Liked-song count from Spotify (for removal detection).
pub async fn get_saved_tracks_total(&self) -> Result<i64> {
    self.refresh_token_if_needed().await?;
    let page = self
        .spotify
        .current_user_saved_tracks_manual(None, Some(1), Some(0))
        .await
        .context("Failed to fetch liked songs page")?;
    Ok(page.total as i64)
}

/// All liked songs, streamed newest-first.
pub async fn get_saved_tracks<'a>(
    &'a self,
) -> Result<impl tokio_stream::Stream<Item = Result<SavedTrack>> + 'a> {
    self.refresh_token_if_needed().await?;
    let stream = self.spotify.current_user_saved_tracks(None);
    Ok(stream.map(|item| match item {
        Ok(t) => Ok(t),
        Err(e) => Err(anyhow::Error::from(e).context("Spotify API error")),
    }))
}
```

Verified against rspotify 0.15.1 / rspotify-model 0.15.1:
`current_user_saved_tracks(market)` → `Paginator<SavedTrack>`;
`current_user_saved_tracks_manual(market, limit, offset)` → `Page<SavedTrack>`
(`Page.total: u32`); `SavedTrack { added_at: DateTime<Utc>, track: FullTrack }`.

`SavedTrack → TrackInfo` reuses `TrackInfo::from(&FullTrack)` (same conversion the
playlist poller uses), so the liked sync stores tracks identically.

---

## Part C — Sync logic (split for testability)

New module `src/liked_sync.rs`:

```rust
/// One liked song, already fetched from Spotify. Keeps the network out of the
/// DB path so the merge logic is testable without HTTP.
pub struct LikedItem {
    pub track: TrackInfo,
    pub added_at: i64,
}

/// Upsert the liked playlist + all memberships. Idempotent.
/// Returns the number of newly-linked tracks.
pub async fn merge_liked_items(
    db: &Pool<Sqlite>,
    items: &[LikedItem],
    liked_playlist_id: &str,   // "spotify:liked"
) -> Result<usize>;

/// Soft-delete likes that are no longer in `current_track_ids`.
/// Returns the number of rows tombstoned.
pub async fn retire_missing_likes(
    db: &Pool<Sqlite>,
    liked_playlist_id: &str,
    current_track_ids: &HashSet<String>,
) -> Result<usize>;

/// Full cycle: fetch → merge → detect removals (only when the count shrank) →
/// refresh resolved-tag tables.
pub async fn sync_liked_songs(
    db: &Pool<Sqlite>,
    spotify_client: &SpotifyClient,
) -> Result<LikedSyncStats>;
```

Behaviour:

- The playlist row is upserted with `playlist_kind='liked'`, `name='liked'`,
  `service='spotify'`, `playlist_id='spotify:liked'`.
- Every item goes through `add_track_to_playlist_with_added_at` — **including
  memberships that already exist** (unlike the playlist poller), so `added_at`
  always reflects Spotify's like date.
- `retire_missing_likes` runs **only when** `get_saved_tracks_total() <` the stored
  liked count — a shrinking total is the only cheap, reliable signal that
  something was unliked. Full diff on every cycle would be wasteful.
- Best-effort `refresh_track_tags` / `refresh_file_resolved_tags` after new
  memberships land (so a newly created `liked` tag resolves immediately).

Error handling mirrors the poller: a 429 sets `spotify_cooldown()` via
`extract_retry_after_secs` and aborts the cycle instead of retrying inline.

---

## Part D — Keep the liked row out of playlist-sync bookkeeping

`src/db/playlists.rs`:

```rust
pub async fn get_spotify_playlist_snapshots(...) {
    // add: AND playlist_kind = 'curated'
}
```

One-line change, two effects:

1. Step 1 of the poller no longer treats the liked row as a playlist whose
   snapshot is missing.
2. Step 4 no longer marks it inactive every cycle.

`get_playlists_without_tags` / `create_tags_from_playlists` / `refresh_track_tags`
get `AND sp.playlist_kind != 'generated'` — the liked playlist is **not** excluded
there (its tag is wanted), but generated packs must never create tags.

---

## Part E — Manual trigger + wiring

- `POST /api/spotify/sync-liked` in `src/api/spotify_sync.rs`, following the
  existing sync handlers: returns `{"linked": n, "retired": n, "total": n}`.
  Not-configured → the same error shape the sibling handlers already return.
- The liked sync runs in the `GlobalPollCycle` path (`global_poller.rs`), after
  the playlist loop, so the 15-minute cadence covers it. Guarded by the same
  process-wide cooldown.
- `src/db/rediscovery.rs` (new): thin typed accessors over
  `v_track_forgotten_facts`, plus the file-side facts M2 needs.

```rust
#[derive(Debug, Clone, FromRow)]
pub struct TrackFacts {
    pub track_id: i64,
    pub playlist_count: i64,
    pub last_touched_at: Option<i64>,
    pub liked_at: Option<i64>,
    pub liked: bool,
    // joined from files via v_file_track_link (nullable — no file = unanalysed)
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub play_count: Option<i64>,
    pub last_played: Option<i64>,
    pub in_backpack: Option<bool>,
}

pub async fn get_track_facts(pool: &Pool<Sqlite>, track_id: i64) -> Result<Option<TrackFacts>>;
pub async fn count_touched_before(pool: &Pool<Sqlite>, days: i64) -> Result<i64>;
```

- `src/db/mod.rs`: add `pub mod rediscovery;` + `pub use rediscovery::*;`
  (matches the existing module pattern).

---

## Files to modify

| File | Change |
| --- | --- |
| `migrations/032_playlist_kind.sql` | New — column, backfill, index, view |
| `src/spotify/client.rs` | `get_saved_tracks`, `get_saved_tracks_total` |
| `src/liked_sync.rs` | New — `LikedItem`, `merge_liked_items`, `retire_missing_likes`, `sync_liked_songs` |
| `src/lib.rs` | `pub mod liked_sync;` |
| `src/global_poller.rs` | Call the liked sync after the playlist loop; task-log line |
| `src/db/playlists.rs` | `get_spotify_playlist_snapshots` → curated only; `!= 'generated'` guards |
| `src/db/rediscovery.rs` | New — `TrackFacts` + accessors |
| `src/db/mod.rs` | Register the new module |
| `src/db/types.rs` | `ServicePlaylist.playlist_kind: String` |
| `src/api/spotify_sync.rs` | `POST /api/spotify/sync-liked` |
| `src/db/testing.rs` | New scenario `liked_songs` |
| `src/api/infrastructure.rs` | Register `liked_songs` in `testing_seed_handler` |
| `tests/common/mod.rs` | `seed_liked_songs_data` |
| `tests/api_playlists.rs` | Snapshot query + `playlist_kind` tests |
| `tests/api_rediscovery.rs` | New — view semantics + liked endpoint errors |

---

## TDD: tests written first

### New seed scenario — `db::testing::seed_liked_songs_scenario`

Extends `seed_basic_scenario` (which already has playlists 1 `Groovy`, 2 `Deep Mix`
and tracks 1–3) with:

| Row | Purpose |
| --- | --- |
| Playlist 5, name `liked`, `playlist_kind='liked'`, `playlist_id='spotify:liked'` | the likes mirror |
| Playlist 6, name `Today's Selection`, `playlist_kind='generated'` | must never count or create a tag |
| Playlist 7, name `Likes`, `playlist_kind='liked'` | the manual mirror (name-variant backfill) |
| Track 1 in playlist 5, `added_at=1500000000` (old like) | old like → drives `last_touched_at` |
| Track 1 also in playlist 1, `added_at=1600000000` | `playlist_count=1`, `last_touched_at=1600000000` |
| Track 2 only in playlist 5, `added_at=1400000000` | liked, `playlist_count=0` |
| Track 3 in playlists 1+2, `added_at=1700000000` | `playlist_count=2`, not liked |

### Unit tests — `src/liked_sync.rs`

| # | Test | Proves |
| --- | --- | --- |
| 1 | `merge_inserts_new_likes_with_spotify_added_at` | `added_at` comes from Spotify, not `now()` |
| 2 | `merge_updates_added_at_for_existing_like` | unlike→relike refreshes the date (differs from the playlist poller) |
| 3 | `merge_is_idempotent` | running twice changes no row counts |
| 4 | `merge_creates_the_liked_playlist_as_kind_liked` | `playlist_kind='liked'`, `playlist_id='spotify:liked'`, `name='liked'` |
| 5 | `retire_missing_likes_soft_deletes_only_absent_tracks` | removed like → `deleted_at IS NOT NULL`, others untouched |
| 6 | `retire_missing_likes_keeps_track_in_other_playlists` | the track's `Groovy` membership survives |
| 7 | `reliked_track_is_reactivated` | `deleted_at` cleared back to `NULL` |

### Unit tests — `src/db/rediscovery.rs`

| # | Test | Proves |
| --- | --- | --- |
| 8 | `playlist_count_ignores_liked_and_generated` | tracks 1/2/3 report 1/0/2 |
| 9 | `last_touched_at_spans_curated_and_liked` | track 1 → `1600000000` (curated wins over the older like) |
| 10 | `liked_flag_and_liked_at` | track 2 liked, track 3 not |
| 11 | `archived_playlist_tombstones_still_count` | ADR-072 guard: `archive_deleted=1` + `deleted_at` still counts |
| 12 | `non_archived_tombstones_do_not_count` | a plain `deleted_at` row is excluded |

### Integration tests — `tests/api_playlists.rs`

| # | Test | Proves |
| --- | --- | --- |
| 13 | `snapshot_query_excludes_liked_and_generated` | only curated rows returned |
| 14 | `create_tags_skips_generated_but_includes_liked` | a `liked` tag is created; no `Today's Selection` tag |
| 15 | `liked_name_variants_backfill_to_kind_liked` | `liked` **and** `Likes` → `'liked'` |
| 16 | `daily_playlists_backfill_to_generated` | `Daily-...` → `'generated'` |

### Integration tests — `tests/api_rediscovery.rs`

| # | Test | Proves |
| --- | --- | --- |
| 17 | `sync_liked_without_spotify_configured_returns_error` | endpoint error path (mirrors `spotify_sync_playlists_error`) |
| 18 | `forgotten_facts_view_is_queryable_over_the_api` | the M2 surface exists and returns the seeded shape |

### Migration integrity

`tests/migration_integrity.rs` already runs all migrations end-to-end; 032 must
pass it unmodified (no test change needed, but it is the gate).

---

## Acceptance criteria

- [ ] `cargo build` passes with no new warnings
- [ ] `cargo test` passes (all existing + the 18 new tests)
- [ ] Migration 032 runs cleanly on a fresh DB (`001 → 032`) **and** on a copy of `app.db`
- [ ] `/me/tracks` sync stores likes with Spotify's `added_at`
- [ ] Re-running the sync is idempotent; unlike is tombstoned; relike reactivates
- [ ] The liked playlist is never marked inactive by the global poller
- [ ] `liked` becomes a tag; `Today's Selection` / `Daily-*` do not
- [ ] `playlist_count` counts curated only
- [ ] `npx playwright test` still passes (no frontend change expected; the smoke
      suite guards against regressions in playlists/tracks pages)

---

## Out of scope

- The candidates query and its facets — that is M2.
- The daily rotation and naming — M3.
- Backfilling likes from the **manual** mirror playlist's contents into the like
  table directly; the mirror is only reclassified, the `/me/tracks` sync is the
  source of truth. (Open question in the parent plan: retire the mirror once the
  sync is trusted.)
- Spotify listening history — does not exist via the API.

---

## Agent decomposition (2 agents, zero file conflicts)

| Agent | Files | Work |
| --- | --- | --- |
| **A** — data | `migrations/032_playlist_kind.sql`, `src/db/playlists.rs`, `src/db/rediscovery.rs`, `src/db/mod.rs`, `src/db/types.rs`, `src/db/testing.rs`, `src/api/infrastructure.rs`, `tests/common/mod.rs`, `tests/api_playlists.rs`, `tests/api_rediscovery.rs` | Migration + view + accessors + guards + seed + tests 8–18 |
| **B** — sync | `src/spotify/client.rs`, `src/liked_sync.rs`, `src/lib.rs`, `src/global_poller.rs`, `src/api/spotify_sync.rs` | Client methods, merge/retire logic, wiring, endpoint + tests 1–7 |

Agent B depends on Agent A's `playlist_kind` column and seed scenario. Interface
first: agree the `LikedItem`/`TrackFacts` shapes and the `liked_songs` scenario
contract before either writes code; B's unit tests may stub the playlist row
until A's migration lands.
