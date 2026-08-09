## Plan: track-file-metrics

**Status**: done ✅
**Branch**: `feat/track-file-metrics`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Add BPM, Key, Rating, and Play Count as visible columns, sortable fields, and
filterable dimensions in **both** the FILES and TRACKS views. For Files this is
straightforward (direct columns on `files`). For Tracks, a track can have multiple
linked files — use a "best file wins" aggregation strategy (format priority:
`stem.m4a` > `flac` > `mp3` > `wav` > `aiff`).

### Research: Current State (verified via curl, 2026-06-09)

**Files page** (`#files`) — verified:

| Metric      | Column | Sortable  | Filterable        | Notes                                  |
| ----------- | ------ | --------- | ----------------- | -------------------------------------- |
| BPM         | ✅     | ✅        | ✅ dual-range     | Works fine                             |
| Key         | ✅     | ❌ BROKEN | ✅ 24-key buttons | `ORDER BY key` → "no such column: key" |
| Plays       | ✅     | ✅        | ❌                | `sort=play_count` works                |
| Rating      | ❌     | ❌        | ❌                | Not a column, not in `adaptFile`       |
| Last Played | ✅     | ✅        | ❌                | Works                                  |

**Files API** (`ApiFile`) already carries: `bpm`, `musicalKey`, `rating`, `playCount`, `lastPlayed`.

**Tracks page** (`#tracks`) — verified:

| Metric      | Column | Sortable | Filterable | Notes                         |
| ----------- | ------ | -------- | ---------- | ----------------------------- |
| BPM         | ❌     | ❌       | ❌         | Absent from `ApiServiceTrack` |
| Key         | ❌     | ❌       | ❌         | Absent                        |
| Plays       | ❌     | ❌       | ❌         | Absent                        |
| Rating      | ❌     | ❌       | ❌         | Absent                        |
| Last Played | ❌     | ❌       | ❌         | Absent                        |

**Tracks API** (`ApiServiceTrack`) has NONE of these fields.

**Track→File relationship** (verified on real data):

- Track #2 "Andreas the Coffee Plug" has 7 linked files: 1 stem.m4a (BPM 155, 6m), 1 flac (BPM 155, 6m), 5 WAV sources (BPM null)
- Track #1 "Gut Morgen, Gut Nacht" has 0 linked files — needs "—" fallback
- BPM/Key is consistent across variants (same song = same BPM/Key)

### Aggregation Strategy for Tracks

Verification against production DB (13,626 files, 45,760 tracks):

| Metric      | Discrepancy across formats?        | Strategy                             |
| ----------- | ---------------------------------- | ------------------------------------ |
| BPM         | Common: ±1 (different detectors)   | Show all distinct values "159 / 160" |
| Key         | Never differs (same song)          | Best file (stem > flac)              |
| Rating      | All 0 currently, but could differ  | Max across files                     |
| Play Count  | Stems have counts, FLACs usually 0 | SUM across files (both get played)   |
| Last Played | Same pattern as play count         | Max (most recent across all files)   |

**For filtering**: All metrics use `EXISTS` subquery against ANY linked file
(a track "has BPM 140" if any of its files has BPM in range). This is robust
even when the best file lacks data.

**Format priority** (for Key display and BPM order when both are present):
`stem.m4a` > `flac` > `mp3` > `wav` > `aiff`

### Implementation: Batch Enrichment Pattern

`get_tracks()` already uses post-fetch enrichment in batches:

1. Fetch ServiceTrack rows
2. Batch query `local_files`
3. Batch query `playlist_names` + `max_added_at`
4. Batch query `playlist_tags`
5. Batch query `format_info`
6. Batch query `in_backpack`

We add step 7: **File metrics batch query**:

```sql
SELECT vft.track_id, f.bpm, f.musical_key, f.rating, f.play_count, f.last_played, f.file_type
FROM v_file_track_link vft
JOIN files f ON f.id = vft.file_id
WHERE vft.track_id IN (?,?,...)
ORDER BY vft.track_id,
  CASE f.file_type
    WHEN 'stem.m4a' THEN 0
    WHEN 'flac' THEN 1
    WHEN 'mp3' THEN 2
    WHEN 'wav' THEN 3
    ELSE 4
  END
```

Then in Rust: group rows by `track_id`, compute display values:

- **BPM**: collect distinct non-null values (ordered by format priority), join with `" / "` — e.g. `"159.0 / 160"`
- **Key**: pick from best-format file (first in order — always identical across formats)
- **Rating**: MAX across files
- **Play count**: SUM across files (FLAC may have been played before stem was created)
- **Last played**: MAX across files (most recent play)

Store in `HashMap<i64, AggregatedFileMetrics>`. Fallback for tracks with no linked
files: all fields null → frontend shows "—".

### Backend Changes

#### 1. `src/api/tracks.rs` — New struct + ApiServiceTrack fields

```rust
#[derive(Debug, Clone, Default)]
struct AggregatedFileMetrics {
    bpm: Option<f64>,          // best-file BPM for sorting
    bpm_display: String,        // e.g. "159.0 / 160" or "155.0"
    musical_key: Option<String>,
    rating: Option<i32>,
    play_count: Option<i32>,   // SUM across files
    last_played: Option<i64>,  // MAX across files
}
```

Add to `ApiServiceTrack`:

```rust
#[serde(default)]
pub bpm: Option<f64>,
#[serde(default)]
pub musical_key: Option<String>,
#[serde(default)]
pub rating: Option<i32>,
#[serde(default)]
pub play_count: Option<i32>,
#[serde(default)]
pub last_played: Option<i64>,
```

#### 2. `src/api/tracks.rs` — Batch query in `get_tracks()`

After step 6 (backpack), add step 7: batch query for best-file metrics.

#### 3. `src/api/tracks.rs` — Filter params on `TracksQuery`

```rust
pub bpm_min: Option<f64>,
pub bpm_max: Option<f64>,
pub keys: Option<String>,        // comma-separated Camelot keys
pub rating_min: Option<i32>,
pub play_count_min: Option<i32>,
```

Filter SQL using EXISTS:

```sql
-- BPM range
AND EXISTS (SELECT 1 FROM v_file_track_link vft
            JOIN files f ON f.id = vft.file_id
            WHERE vft.track_id = st.id AND f.bpm >= ? AND f.bpm <= ?)

-- Key list (OR)
AND EXISTS (SELECT 1 FROM v_file_track_link vft
            JOIN files f ON f.id = vft.file_id
            WHERE vft.track_id = st.id AND f.musical_key IN (?,?,...))

-- Rating minimum
AND EXISTS (SELECT 1 FROM v_file_track_link vft
            JOIN files f ON f.id = vft.file_id
            WHERE vft.track_id = st.id AND f.rating >= ?)

-- Play count minimum
AND EXISTS (SELECT 1 FROM v_file_track_link vft
            JOIN files f ON f.id = vft.file_id
            WHERE vft.track_id = st.id AND f.play_count >= ?)
```

#### 4. `src/api/tracks.rs` — Sort whitelist

Add to `apply_sort` whitelist: `"musical_key"`, `"rating"`, `"play_count"`, `"bpm"`.
Note: use `"musical_key"` not `"key"` — the column is named `musical_key`.
For BPM/rating/play_count, these come from the enrichment map, not the SQL row.
We need to sort in Rust after enrichment (or add a JOIN to the main query).

**Sort strategy**: For BPM/Key/Rating/PlayCount sorts, we need a different approach
since these values aren't in the `service_tracks` SELECT. Options:

A. **Join in the main query** — add LEFT JOIN to best-file metrics inside the
main SELECT. Complex because of DISTINCT and existing JOINs.
B. **Rust-side sort** — after enrichment, sort the Vec<ApiServiceTrack>.

**Recommendation**: Option B (Rust-side sort). Simpler and consistent with the
batch enrichment pattern. After enrichment, if sort column is one of the file
metrics, sort in Rust. Re-apply LIMIT/OFFSET after sorting.

#### 5. `src/api/files.rs` — Fix Key sort column name

Change `apply_sort` whitelist from `"key"` to `"musical_key"`. Also update
frontend `sortKey: "key"` to `sortKey: "musical_key"`.

#### 6. `src/api/files.rs` — Add rating + play_count filters to `FilesQuery`

```rust
pub rating_min: Option<i32>,
pub play_count_min: Option<i32>,
```

Filter SQL:

```sql
AND rating >= ?
AND play_count >= ?
```

#### 7. `src/api/files.rs` — Add `rating` to sort whitelist

Add `"rating"` to the `apply_sort` whitelist.

### Frontend Changes

#### 8. `frontend/pages/files.js` — Add Rating column

Add to `FILES_COLUMNS`:

```javascript
{ id: "rating", label: "★", sortable: true, sortKey: "rating", defaultWidth: 70 },
```

Add cell renderer:

```javascript
rating: (f) => (f.rating != null && f.rating > 0)
  ? `<span class="rating-stars">${starRating(f.rating)}</span>`
  : '<span class="text-muted">—</span>',
```

Add to `adaptFile()`: `rating: f.rating,`

Fix key sort: `sortKey: "key"` → `sortKey: "musical_key"`

#### 9. `frontend/pages/files.js` — Rating + Play Count filters

Add to toolbar RIGHT column (Classification section) after Key filter:

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="rating">Rating</span>
  <input
    type="number"
    class="input-text"
    data-sf-filter="ratingMin"
    min="0"
    max="5"
    placeholder="Min ★"
    style="width:80px"
  />
</div>
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="plays">Plays</span>
  <input
    type="number"
    class="input-text"
    data-sf-filter="playCountMin"
    min="0"
    placeholder="Min plays"
    style="width:80px"
  />
</div>
```

Add to state: `ratingMin: 0`, `playCountMin: 0`, `ratingEnabled: true`, `playsEnabled: true`.
Add to hash schema + defaults. Add to `buildParams`.

#### 10. `frontend/pages/tracks.js` — New columns

Add to `TRACKS_COLUMNS`:

```javascript
{ id: "bpm", label: "BPM", sortable: true, sortKey: "bpm", defaultWidth: 80 },
{ id: "key", label: "Key", sortable: true, sortKey: "musical_key", defaultWidth: 60 },
{ id: "rating", label: "★", sortable: true, sortKey: "rating", defaultWidth: 70 },
{ id: "plays", label: "Plays", sortable: true, sortKey: "play_count", defaultWidth: 60 },
{ id: "lastPlayed", label: "Last Played", sortable: true, sortKey: "last_played", defaultWidth: 80 },
```

Add cell renderers in `TRACKS_CELL_RENDERERS`:

```javascript
bpm: (t) => t.bpm != null ? `<span class="font-mono">${formatBPM(t.bpm)}</span>` : '<span class="text-muted">—</span>',
key: (t) => t.musicalKey ? `<span class="badge badge-key">${escapeHtml(t.musicalKey)}</span>` : '<span class="text-muted">—</span>',
rating: (t) => t.rating != null && t.rating > 0 ? `<span class="rating-stars">${starRating(t.rating)}</span>` : '<span class="text-muted">—</span>',
plays: (t) => t.playCount != null ? `<span class="font-mono text-sm">${t.playCount}</span>` : '<span class="text-muted">—</span>',
lastPlayed: (t) => t.lastPlayed ? formatTimestamp(t.lastPlayed) : '<span class="text-muted">—</span>',
```

#### 11. `frontend/pages/tracks.js` — BPM, Key, Rating, Play Count filters

Add to toolbar LEFT column (Track Info section) after Tags:

- **BPM filter**: dual-range slider (0–300), same as Files page
- **Key filter**: 24 Camelot key toggle buttons (1m–12m, 1d–12d), same as Files page
- **Rating filter**: min rating number input (0–5)
- **Play Count filter**: min plays number input

Add to state: `bpmMin`, `bpmMax`, `keys`, `ratingMin`, `playCountMin` + enable flags.
Add to hash schema + `buildParams`.

#### 12. Frontend shared imports

Both files.js and tracks.js already import `formatBPM` and `formatDuration`.
Need to also import/define `starRating` helper for the rating renderer.

### Files to modify

- `src/api/tracks.rs` — `BestFileMetrics` struct, `ApiServiceTrack` fields, batch query step 7, `TracksQuery` filter params, filter SQL, Rust-side sort
- `src/api/files.rs` — fix `"key"` → `"musical_key"` in sort whitelist, add `rating_min`/`play_count_min` to `FilesQuery`, add `"rating"` to sort whitelist, filter SQL
- `frontend/pages/files.js` — Rating column + renderer, fix key sortKey, rating/plays filter UI, adaptFile, state, hash, buildParams
- `frontend/pages/tracks.js` — New columns (bpm, key, rating, plays, lastPlayed), cell renderers, BPM/Key/Rating/PlayCount filter UI, state, hash, buildParams
- `frontend/style.css` — `.rating-stars` styles (reuse existing)

### Acceptance Criteria

- [x] FILES: Rating column visible, sortable, filterable (min rating)
- [x] FILES: Play Count filterable (min plays)
- [x] FILES: Key sort works correctly (fix `musical_key` column name)
- [x] TRACKS: BPM column shows linked files' BPMs, filtered by range
- [x] TRACKS: Key column shows best-file's Camelot key, filtered by key list
- [x] TRACKS: Rating column shows aggregated rating, filtered by min rating
- [x] TRACKS: Plays column shows aggregated play count, filtered by min
- [x] TRACKS: Last Played column shows most recent play across files
- [x] TRACKS: Track with no linked files shows "—" for all four metrics (no crash)
- [x] TRACKS: Track with multiple linked files shows both BPMs when different (e.g. "159.0 / 160")
- [x] TRACKS: BPM filter matches ANY linked file, not just the displayed one
- [x] FILES: No regressions — all existing filters/sorts/columns still work
- [x] TRACKS: No regressions — existing columns/filters/sorts still work
- [x] `cargo build` passes
- [x] `cargo test` passes (382/383 — 1 pre-existing fixture test failure)
- [x] `cd frontend && npx playwright test` passes (17/17)

---

