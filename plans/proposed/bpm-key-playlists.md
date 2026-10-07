# Plan: bpm-key-playlists

**Status**: approved (2026-10-07)
**Branch**: `feat/bpm-key-playlists`
**Ready for review**: no
**Depends on**: nothing (reuses `playlist_kind = 'generated'`, migration 032)
**Migration needed**: yes (one additive column on `service_playlists`; next free number)

### Description

For every track in the library whose **BPM and musical key are known**, materialise a
real Spotify playlist per `(BPM, key)` combination (e.g. `124bpm // 12m`) on the user's
account, so the whole library can be browsed in Spotify grouped by BPM/key. The
playlists are marked `playlist_kind = 'generated'` — the existing "our own output"
kind — which keeps them out of usage scoring, tag creation and playlist polling.

This is a **system/presentation** feature: it must never influence the comment
write-out, tag matching, or "last touched / used" signals.

---

### Why `playlist_kind = 'generated'` (ground truth, verified 2026-10-07)

Migration 032 already added `service_playlists.playlist_kind` with values
`curated | liked | generated`, and `generated` is _already_ excluded from exactly the
surfaces the request mentions:

| Surface                          | Query                                                                            | `generated` behaviour                                            |
| -------------------------------- | -------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| Usage / "last touched"           | `v_track_forgotten_facts` (032)                                                  | **excluded** from `playlist_count`, never sets `last_touched_at` |
| Tag creation from playlists      | `get_playlists_without_tags`, `create_tags_from_playlists`, `refresh_track_tags` | **excluded**                                                     |
| Comment pipeline                 | tag resolution via playlist name-match                                           | unreachable (no tag can be created from a `generated` playlist)  |
| Global poller staleness/deletion | `get_spotify_playlist_snapshots` (`playlist_kind = 'curated'`)                   | **excluded**                                                     |

So "system playlist, not used in comment write-out or stuff like last used" **is**
`playlist_kind = 'generated'`. We reuse it; we do **not** invent an `is_system` flag.

> Caveat to fix while here: the global poller's _discovery_ step decides "new playlist"
> by absence from `db_snapshots` (curated-only), so a `generated` Spotify row that is
> already in our DB is re-fetched **every cycle**. See M2.4.

---

## Naming template

Configurable template, evaluated per `(BPM, key)`. Placeholders: `{bpm}` (integer),
`{key}` (normalised musical key, e.g. `12m`), optional `{count}`.

**Decided (2026-10-07): option B** — spaced, readable:

```
124bpm // 12m
```

For reference, the alternatives considered were:

| Option                    | Example                          | Notes                                                                     |
| ------------------------- | -------------------------------- | ------------------------------------------------------------------------- |
| A — Compact               | `124bpm//12m`                    | exact original, densest                                                   |
| B — Readable (**chosen**) | `124bpm // 12m`                  | matches your spec, easy to scan                                           |
| C — Sortable              | `070bpm // 12m`, `124bpm // 12m` | zero-pad BPM to 3 digits → groups + orders by BPM                         |
| D — Marked/clustered      | `⌁ 124bpm // 12m`                | leading marker sorts them together; visually distinct from real playlists |
| E — With count            | `124bpm // 12m (37)`             | useful, but the name churns whenever a track is added/removed             |

Recommendation: **B** (chosen), with a configurable optional `name_prefix` (default empty) so
you can switch to the clustered look later without a code change. Keep the count out of the
name (avoids renames); show it on the management page instead.

**Key normalisation**: group by canonical Camelot position+mode (via
`crate::digging::parse_camelot_key`, which maps `m → A`, `d → B`) so `12m` and `12A`
never produce two playlists; render in the stored Traktor style (`12m`/`12d`), matching
your example. (Seeds confirm the `m`-style, e.g. `4m`, `8m`, `3m`.)

---

## Data model

One additive migration (`033_…` or next free at implementation time):

```sql
ALTER TABLE service_playlists ADD COLUMN system_key TEXT;  -- e.g. 'bpm_key:124:12A', NULL for normal playlists
CREATE UNIQUE INDEX idx_service_playlists_system_key
    ON service_playlists(service, system_key) WHERE system_key IS NOT NULL;
```

- `system_key` is the stable combo id (independent of the display template).
- The Spotify playlist is the row itself (`service = 'spotify'`,
  `playlist_kind = 'generated'`, `system_key = …`, `playlist_id =` Spotify id).
- We deliberately **do not** store `service_playlist_tracks` for these rows — the
  desired membership is derived from `files` on every sync; the Spotify playlist is
  the only persisted copy. This avoids poller/tombstone interaction entirely.

`ServicePlaylist` (`FromRow`) ignores extra columns, so adding the column is source-safe
(the handful of hand-rolled `CREATE TABLE service_playlists` test fixtures must gain the
column — see Files).

---

## Milestones

### M1 — Derivation + naming core (`src/bpm_key.rs`, pure, unit-tested)

- `struct BpmKey { bpm: i64, key: String /* canonical, e.g. "12A" */ }`
- `fn from_file(bpm: Option<f64>, musical_key: Option<&str>) -> Option<BpmKey>` —
  rounds BPM to nearest integer, parses/normalises the key; returns `None` if either is
  missing/blank/invalid.
- `fn format_name(template: &str, prefix: &str, bpm: i64, key: &str) -> String`.
- `src/db/bpm_key.rs` — derivation query:

```sql
SELECT CAST(ROUND(f.bpm) AS INTEGER) AS bpm,
       f.musical_key                  AS raw_key,
       COUNT(DISTINCT f.id)           AS file_count,
       GROUP_CONCAT(DISTINCT 'spotify:track:' || st.service_id) AS uris
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_tracks st ON st.id = v.track_id AND st.service = 'spotify'
WHERE f.bpm IS NOT NULL
  AND f.musical_key IS NOT NULL AND TRIM(f.musical_key) <> ''
GROUP BY bpm, raw_key
```

(Final grouping by _canonical_ key happens in Rust after the row scan so `m`/`A`
variants collapse; URIs are de-duplicated per group.)

- Unit tests: rounding, m/d↔A/B normalisation, missing values, template variants,
  `(128.0 flac, 128.5 stem)` producing separate buckets with the same URI de-duped.

### M2 — Reconcile + push to Spotify

- `src/bpm_key/sync.rs`:
  - For each desired group `(bpm,key)` with `file_count >= min_tracks`:
    find an existing row by `system_key`; else `create_playlist` (private by default);
    then `replace_playlist_items` (mirror semantics, already chunked ≤100 in
    `SpotifyClient`).
  - Empty/vanished groups: **keep** them by default (no destructive deletes); the
    `strict` mode (row removal + Spotify unfollow) stays **off**.
  - Sequential with the existing rate-limit cooldown (ADR-061); abort the run on 429,
    leave the rest for the next run (idempotent).
- New `TaskType::SyncBpmKeyPlaylists { strict: bool }`, conflict key
  `"bpm_key_sync"`, label "Sync BPM//key playlists", registered in
  `task_type_conflict_key`, `task_type_display`, `task_type_label`.
- API (`src/api/bpm_key.rs` → wired into the router):
  - `GET /api/bpm-key-playlists/preview` → derived groups: `{bpm,key,name,fileCount,trackCount,exists,spotifyUrl}`
  - `POST /api/bpm-key-playlists/sync` → starts the task, returns `{taskId}`
  - `GET /api/bpm-key-playlists` → the `generated` system rows
- Settings (`src/db/settings.rs` keys, surfaced through the existing settings API):
  `enabled`, `nameTemplate` (default `{bpm}bpm // {key}`), `namePrefix`, `minTracks`
  (**default 1**), `public` (**default false → private**), `keyStyle` (`m`-style vs
  Camelot, default `m`-style), `strict` (default off), `scheduleEnabled` (default off),
  `scheduleIntervalSecs`.
- **M2.4 — poller fix**: in `global_poller::run_poll_cycle`, build a second map of
  **all known** `(service='spotify')` playlist ids with their `playlist_kind`; skip
  syncing any playlist whose DB row is non-`curated` (currently such rows have no
  snapshot and fall into the "New playlist" branch, re-fetching every cycle). Keep
  genuinely-unknown playlists in the discovery path. Cover with a test.

- **M2.5 — auto-trigger on metadata availability**: when `enabled`, enqueue a
  `SyncBpmKeyPlaylists` task after the operations that can populate `files.bpm` /
  `files.musical_key` — folder scans (`start_scan_folder_task` / `scan_folder`) and
  Traktor import (`TaskType::TraktorImport`). The task's unique conflict key
  (`bpm_key_sync`) coalesces bursts, so a scan+import storm collapses into one run.
  An optional scheduled interval (`scheduleEnabled` + `scheduleIntervalSecs`, default
  **off**) reuses the same enqueue path. No polling of Spotify is involved.

### M3 — Frontend + tests

- New Tools page `#bpm-key-playlists` (`frontend/pages/bpm-key-playlists.js` +
  `PAGE_MAP` in `app.js` + `NAV_SECTIONS`/`TOOLS_ITEMS` in `shared/nav.js`):
  preview table (BPM, key, name, counts, exists), settings form, "Sync to Spotify"
  button driving the task; poll the tasks API for progress.
- `frontend/pages/playlists.js` / API: add a `system` query param
  (`exclude` default | `include` | `only`) to `PlaylistsQuery` + handler, plus a
  `data-kind` badge; hide `generated` rows by default.
- Playwright `frontend/tests/bpm-key-playlists.spec.js`: seed (new
  `bpm_key_playlists` scenario in `src/db/testing.rs`) → preview renders expected
  groups → toggle reveals `generated` rows in the playlists page → no `pageerror`.
- Extend `generate_fixtures`/`testing.rs` + register the seed in
  `testing_seed_handler` (`src/api/infrastructure.rs`).

---

## Files to modify (indicative)

| File                                                                     | Change                                                                                                                                    |
| ------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `migrations/03X_*.sql`                                                   | `system_key` column + partial unique index                                                                                                |
| `src/bpm_key.rs` (new)                                                   | pure grouping/naming + unit tests                                                                                                         |
| `src/db/bpm_key.rs` (new)                                                | derivation + system-playlist CRUD                                                                                                         |
| `src/bpm_key/sync.rs` (new)                                              | reconcile/push worker                                                                                                                     |
| `src/tasks/mod.rs`                                                       | new `TaskType` + worker + labels/conflict key                                                                                             |
| `src/api/bpm_key.rs` (new)                                               | preview/sync/list handlers + router                                                                                                       |
| `src/api/playlists.rs`                                                   | `system` filter in `PlaylistsQuery`/handler; include `playlist_kind` in the `Playlist` DTO                                                |
| `src/api/playlists.rs`                                                   | `push_playlist_to_spotify`: set `playlist_kind = 'generated'` on the inserted Spotify mirror for generated locals (small correctness fix) |
| `src/global_poller.rs`                                                   | skip known non-`curated` playlists during discovery (M2.4)                                                                                |
| `src/db/settings.rs`                                                     | new setting keys                                                                                                                          |
| `src/db/testing.rs` + `src/api/infrastructure.rs`                        | seed scenario + registration                                                                                                              |
| `frontend/pages/bpm-key-playlists.js` (new) + `app.js` + `shared/nav.js` | page + routing/nav                                                                                                                        |
| `frontend/pages/playlists.js`                                            | `system` filter UI + badge                                                                                                                |
| `frontend/tests/bpm-key-playlists.spec.js` (new), `playlists.spec.js`    | Playwright                                                                                                                                |
| `tests/api_bpm_key_playlists.rs` (new)                                   | integration tests                                                                                                                         |
| `docs/DECISIONS.md`, `README.md`, `CHANGELOG.md`                         | ADR (next free) + docs at release                                                                                                         |
| test fixtures with hand-rolled `service_playlists`                       | add `system_key` column: `src/backpack.rs`, `src/db/{tracks,storage}.rs`, `src/db/{music_api,dynamic_bundles}.rs`                         |

---

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes (incl. the migration-integrity test and the new
      `tests/api_bpm_key_playlists.rs`)
- [ ] `cd frontend && npx playwright test` passes (incl. the new spec)
- [ ] Preview returns the exact groups for the seed (BPM rounded, key normalised,
      URIs de-duped, missing BPM/key excluded)
- [ ] A flac+stem pair disagreeing on BPM yields a single bucket (representative-file
      rule)
- [ ] Synced playlists land as `service='spotify'`, `playlist_kind='generated'`,
      `public=false`, with a `system_key`; re-running sync with no library change
      performs no duplicate creations and mirrors the item set
- [ ] Enabling the feature and running a scan/Traktor import auto-enqueues exactly one
      sync task (burst-coalesced via the conflict key)
- [ ] `generated` playlists never appear in `v_track_forgotten_facts`,
      `get_playlists_without_tags`, `create_tags_from_playlists`, or
      `get_spotify_playlist_snapshots` (regression tests)
- [ ] Global poller does not re-fetch known `generated` playlists each cycle
      (M2.4 test)
- [ ] `cargo llvm-cov --fail-under-lines 75` holds

---

## Decisions (locked 2026-10-07)

1. **Template** — B: `124bpm // 12m` (`namePrefix` configurable, default empty).
2. **Key style in the name** — Traktor `m`/`d` (e.g. `12m`), matching your example and
   the seeds. Grouping is by canonical Camelot internally; `keyStyle` can flip the
   rendering to `A`/`B`.
3. **BPM** — stored as float, **rounded to nearest integer** for the bucket.
4. **min_tracks** — **1**.
5. **Visibility** — **private**.
6. **Empty groups** — **keep** (no delete/unfollow; `strict` off).
7. **Trigger** — a **task**: manual button/API plus **auto-enqueue when BPM/key metadata
   becomes available** (post-scan, post-Traktor-import). Optional schedule, **off by
   default**.

### Still open (default chosen — say the word to change)

- **Variant files**: a track with a `128.0` flac + `128.5` stem would otherwise land in
  two buckets. Default: derive **per Spotify track**, pick one representative file
  (prefer non-stem, then lowest id) → one bucket per track. Flip to per-file if you'd
  rather see both.

### Out of scope

- Backfilling/renaming existing user playlists.
- Grouping by BPM _ranges_ or harmonic-compatible key neighbourhoods (single keys only).
- Any change to the comment format or the tag/backpack pipeline.
