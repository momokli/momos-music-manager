# Momo's Music Manager — Agent Guidance

> **Last Updated**: 2026-05-22 — v0.3.2

---

# Section 1: Agent Reference

This section is **static** — it's the system prompt for any agent working on this project.

---

## Project Context

Music library management for DJs. Rust backend (Axum/SQLx/SQLite) + modular SPA frontend (vanilla JS, ES modules).
Single developer, no production data, no backward compatibility needed.

---

## Key Principles

### Workflow

1. **`main` is always clean** — never commit directly to `main`. Every change goes through a feature branch or the review staging branch.
2. **Feature branches** — `feat/short-description` or `fix/short-description`, branched from `main`.
3. **Staging branch** — `review/all-features` collects feature branches for a release. Feature branches are merged into it (not rebased). Small features or cleanup can be committed directly on it.
4. **Plan first** — every task starts with a Plan entry in Section 2 of this file. User reviews the plan, then agents are spawned.
5. **Additive migrations** — never modify `001_initial_schema.sql`. New schema changes get a new migration file. Before release, consolidate same-release migrations into the earliest one (e.g. `003_*.sql` → merged into `002_*.sql` if both are new in this release).

### Release Process

When bundling features for a release:

1. **Collect branches** — merge all `feat/*` and `fix/*` branches into `review/all-features`. Commit any remaining work directly on it.
2. **Consolidate migrations** — merge same-release migration files into the earliest new one. Only one net-new migration per release.
3. **Write CHANGELOG** — create/update `CHANGELOG.md` from `git diff main..review/all-features`. Group by Added / Changed / Fixed.
4. **Update ADRs** — add an ADR per feature in `docs/DECISIONS.md` (ADR-### format, date, status, context, decision, consequences).
5. **Update README** — new pages, new endpoints, new migrations, updated project structure.
6. **Update AGENT.md** — mark all plans as done, bump "Last Updated" date.
7. **Verify** — `cargo build` must pass. Delete `app.db*` and test migrations from scratch.
8. **Rebase onto main** — `git rebase main` on `review/all-features`, then `git checkout main && git merge --ff-only review/all-features`. Linear history preserved.
9. **Tag** — `git tag v0.X.0` on `main`.

### Architecture

6. **Schema**: 13 tables — `tag_categories`, `tags`, `service_tracks`, `service_playlists`, `service_playlist_tracks`, `files`, `service_config`, `folders`, `subscriptions`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities`, `tag_parents` (plus views: `unified_tracks`, `v_file_track_link`, `v_tag_playlist`, `v_file_tags`, `v_subscriptions`, `v_tag_categories`, `v_tags_with_categories`, `v_resolved_tags`, `v_file_resolved_tags`)
7. **Separate Types**: `File` (local files with BPM/Key) vs `ServiceTrack` (service entries, no BPM/Key) — linked via `v_file_track_link` view
8. **Tags = Playlists**: Via name matching (case-insensitive). Setlist is default category.
9. **Comment Format**: `[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}` — e.g. `[PMV] build jazzy warehouse sp:xxx`
10. **Service IDs**: Direct columns on `files` (`spotify_id`, `soundcloud_id`, `youtube_id`)
11. **Key Matching**: Rust-only (Camelot wheel, no DB table)
12. **Task Manager**: In-memory task tracking — 4 operation types (ServiceSync, WriteComment, RecomputeEmbeddings, ScanFolder)
13. **Sync State**: In-memory `TaskManager` — tasks auto-pruned 5 min after completion
14. **Config Priority** (highest wins): Env vars > `~/.config/momos-music-manager/config.toml` > built-in defaults
15. **Server-Side Filtering**: All filters must be server-side on paginated pages. Client-side filtering after pagination breaks page counts.

---

## Config (config.toml)

Service secrets live in `~/.config/momos-music-manager/config.toml`:

```toml
[spotify]
client_id     = "your_spotify_client_id"
client_secret = "your_spotify_client_secret"
redirect_uri  = "http://localhost:3000/callback"

[soundcloud]
api_key  = "your_soundcloud_api_key"
user_id  = "your_soundcloud_user_id"

[youtube]
api_key      = "your_youtube_api_key"
playlist_id  = "your_youtube_playlist_id"
```

**Override with env vars** — a `.env` file in the project root or exporting
`SPOTIFY_CLIENT_ID=...` directly in the shell will override the TOML values.

Dev-only env vars (not in config.toml):

- `DATABASE_URL` — default `sqlite:app.db`

---

## Dev Commands

```bash
# Start backend
cargo run -- serve --host 127.0.0.1 --port 3000

# Start frontend (separate terminal)
cd frontend && python3 -m http.server 8000

# Kill everything
./kill-all.sh

# Scan single file for metadata debugging
cargo run -- scan-file /path/to/file.stem.m4a

# Delete old DBs + restart (only when messing with migrations during dev)
rm -f app.db && cargo run -- serve --host 127.0.0.1 --port 3000

# Dump DB to JSON
cargo run -- dump

# Restore DB from JSON dump
cargo run -- restore

# Import Traktor collection.nml
cargo run -- serve  # then use the Traktor import page in the frontend
```

---

## Important Gotchas

- **Migrations are additive** — never edit `001_initial_schema.sql`. Create `002_xxx.sql` etc.
- **To reset a dirty migration state**: delete `app.db` and re-run — migrations run 001→002→003 from scratch.
- **Frontend is an SPA** — modular vanilla JS with ES modules in `frontend/`. Hash-based router (`app.js`), shared modules in `shared/`, pages in `pages/`. Serve embedded via `rust-embed`, no separate dev server needed.
- **digging.html** is a standalone HTML page (not part of the SPA) for the digging/curation workflow
- **Playlist subscriptions** poll every 30s in the background — managed in `poller.rs`
- **No SoundCloud/YouTube OAuth yet** — framework is ready, actual flow not implemented
- **Docker** was removed — will be recreated later. Use `cargo run` for now.

---

## Tag Categories (Defaults)

| Category | Prefix | Icon          | Sort |
| -------- | ------ | ------------- | ---- |
| Setlist  | S      | fa-list-music | 0    |
| Phase    | P      | fa-layers     | 1    |
| Mood     | M      | fa-heart      | 2    |
| Vibe     | V      | fa-sparkles   | 3    |
| Merkmal  | E      | fa-hashtag    | 4    |

---

## Source Modules

```
src/
├── main.rs              # CLI, router, server start
├── api.rs               # All API endpoints
├── config.rs            # Config.toml + env var loading
├── db.rs                # Database queries, scanning, comment computation
├── comment.rs           # Comment parsing/generation
├── audio_extensions.rs  # AudioExtension enum
├── digging.rs           # Curator/session-builder for track discovery
├── dump.rs              # DB dump/restore (JSON)
├── embeddings.rs        # Semantic tag embeddings (candle/ML)
├── poller.rs            # Playlist subscription background poller
├── spotify/
│   ├── mod.rs
│   ├── client.rs        # Spotify OAuth client
│   ├── models.rs        # PlaylistInfo, TrackInfo
│   └── sync_worker.rs   # Background sync worker
├── tasks/
│   └── mod.rs           # TaskManager (generic) + task workers
├── traktor.rs           # Traktor collection.nml parser
└── watch.rs             # Folder watcher (optional, not auto-started)
```

---

## Frontend Pages (SPA)

| Route              | Module                              | Description                        |
| ------------------ | ----------------------------------- | ---------------------------------- |
| `#dashboard`       | `frontend/pages/dashboard.js`       | Stats cards + recent activity      |
| `#files`           | `frontend/pages/files.js`           | Local files table + comment status |
| `#tracks`          | `frontend/pages/tracks.js`          | Service tracks table               |
| `#playlists`       | `frontend/pages/playlists.js`       | All playlists                      |
| `#services`        | `frontend/pages/services.js`        | Service status/config              |
| `#tags`            | `frontend/pages/tags.js`            | Tags table                         |
| `#tag-categories`  | `frontend/pages/tag-categories.js`  | Tag categories                     |
| `#folders`         | `frontend/pages/folders.js`         | Folder management                  |
| `#tasks`           | `frontend/pages/tasks.js`           | Task manager UI                    |
| `#auto-categorize` | `frontend/pages/auto-categorize.js` | AI tag categorization wizard       |
| `#traktor`         | `frontend/pages/traktor-import.js`  | Traktor collection import          |
| `#data`            | `frontend/pages/data.js`            | Import/export database             |
| `#tag-curation`    | `frontend/pages/tag-curation.js`    | Tag parent curation workflow       |
| `digging.html`     | (standalone HTML)                   | Curator/session-builder page       |

---

## Docs

- `docs/ARCHITECTURE.md` — System design
- `docs/DECISIONS.md` — ADRs
- `docs/COMMENT_SYSTEM.md` — Comment format spec
- `docs/TASK_MANAGER.md` — Task manager details
- `docs/FRONTEND_BUILD_PLAN.md` — SPA migration history (mostly historical, some details outdated)
- `docs/FRONTEND_NEXT_PLAN.md` — Remaining frontend work (partially done)
- `CHANGELOG.md` — Release changelog

---

## Handover

1. Document progress and decisions in `docs/DECISIONS.md`
2. Leave TODO comments in code
3. Ensure backend compiles (`cargo build`) before handing over
4. Test with `curl` commands first, then frontend

---

---

# Section 2: Active Plans

This section is **dynamic** — plans are appended, updated, and checked off as work progresses.

**Lifecycle**: `proposed` → `approved` → `in-progress` → `done`

---

## Plan: tags-filter-box

**Status**: done ✅
**Branch**: `feat/tags-filter-box`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Retrofit the Tags page filter toolbar into the canonical 2-column filter-panel pattern (same as Playlists/File pages). Replace the flat toolbar with a collapsible filter-panel containing category selector + search + New Tag button.

### Files to modify

- `frontend/pages/tags.js`

### Acceptance Criteria

- [ ] Filter-panel with collapsible toggle (localStorage persistence)
- [ ] 2-col grid: Category filter (multi-select buttons) | Search + New Tag button
- [ ] Toggleable data-filter labels with generic toggle handler
- [ ] No regressions to existing sort/pagination/hash-sync/column-config
- [ ] Compile check: no backend changes needed

---

## Plan: modifier-column-layout

**Status**: done ✅ (already on `main` — implemented in prior work)
**Branch**: `feat/modifier-column-layout`
**Depends on**: nothing
**Migration needed**: no

### Description

Add the "Modify Column Layout" toggle button to all CRUD pages (files, tracks, playlists, tags). When active: column headers become draggable (reorder), resize handles appear, and a "Done" button replaces the toggle. Reuses existing `column-config.js` wiring.

### Files to modify

- `frontend/pages/files.js`
- `frontend/pages/tracks.js`
- `frontend/pages/playlists.js`
- `frontend/pages/tags.js`
- `frontend/shared/column-config.js` (minor — ensure `wireColumnResize` / `wireColumnDragReorder` are exported and usable)

### Acceptance Criteria

- [ ] `state.layoutMode` added to all 4 pages
- [ ] Toggle button in each page's stats row: "Modify Column Layout" ↔ "Done"
- [ ] `.layout-mode` CSS class on `<body>` enables resize handles + drag
- [ ] Reordering persists (column config saved on "Done")
- [ ] Resize persists
- [ ] No regressions to existing sort/pagination/hash-sync
- [ ] Compile check: no backend changes needed

---

## Plan: column-resize-pixel

**Status**: done ✅
**Branch**: `feat/column-resize-pixel`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Fix column resize feedback loop by switching from percentage-based to pixel-based sizing in `column-config.js`. Replace `width: XX%` with `width: XXpx`, clamp 30–500px, use new localStorage key (`columnConfig_v2_` prefix) to avoid stale percentage data.

### Files to modify

- `frontend/shared/column-config.js`
- `frontend/style.css`

### Acceptance Criteria

- [ ] `wireColumnResize()` uses pixel math instead of percentage
- [ ] `renderColumnHeaders()` outputs `style="width:XXpx;min-width:30px;max-width:XXpx"`
- [ ] `loadColumnConfig()` uses key `columnConfig_v2_{page}`
- [ ] Default widths scaled from % to px (e.g. 18% → 180px)
- [ ] Dragging resizes smoothly without feedback loop
- [ ] Compile check: no backend changes needed

---

## Plan: import-export-ui

**Status**: done ✅
**Branch**: `feat/import-export-ui`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Add web UI wrapping CLI `dump`/`restore` commands. Backend: `GET /api/dump` (download JSON) + `POST /api/restore?confirm=true` (upload JSON). Frontend: new `#data` page with Export section (download button) + Import section (file upload → preview → confirm → restore).

### Files to modify

- `src/api.rs` — add `dump_handler` and `restore_handler` endpoints
- `frontend/pages/data.js` — new page module (canonical pattern, no table/pagination)
- `frontend/app.js` — register `"data": "data"` in PAGE_MAP
- `frontend/shared/nav.js` — add Import/Export entry under TOOLS_ITEMS

### Acceptance Criteria

- [ ] `GET /api/dump` returns JSON download with `Content-Disposition` header
- [ ] `POST /api/restore?confirm=true` accepts multipart upload, restores DB
- [ ] `POST /api/restore` without `confirm=true` returns 400
- [ ] Frontend Export: fetch + trigger browser download, loading spinner
- [ ] Frontend Import: file picker → preview (row counts per table, timestamp) → confirm → restore → redirect to dashboard
- [ ] Warning banner on import section: "⚠️ This will replace ALL existing data"
- [ ] Destructive button styled red
- [x] Backend compiles (`cargo build`)
- [ ] Tested with `curl` first

---

## Plan: server-side-filtering

**Status**: done ✅
**Branch**: `feat/server-side-filtering`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Move client-side filters on Tracks and Files pages to server-side to fix pagination bugs. On Tracks: add `services`, `fileTypes`, `fileTypeAgg` to `TracksQuery`. Remove PMV filter from Tracks (dead code — service tracks have no comment data). On Files: audit and move PMV/file-type/comment-status filters to `FilesQuery`.

### Files to modify

- `src/api.rs` — extend `TracksQuery` / `FilesQuery` with new filter params
- `frontend/pages/tracks.js` — move filters to `buildParams()`, remove `applyClientFilters` blocks
- `frontend/pages/files.js` — move filters to `buildParams()`, remove `applyClientFilters` blocks

### Acceptance Criteria

- [ ] `services=spotify,soundcloud` param filters tracks server-side via SQL `IN`
- [ ] `fileTypes=flac,stem.m4a` param filters tracks via `v_file_track_link` EXISTS join
- [ ] `fileTypeAgg=any|none` param toggles has-file/has-no-file filters
- [ ] PMV filter row removed from Tracks page (dead code — no comment data)
- [ ] Files page filters (PMV, file type, comment status) moved to server
- [ ] Pagination works correctly when filters are active (total count matches filtered set)
- [ ] No `applyClientFilters` remains in either page (or only for truly client-only concerns)
- [x] Backend compiles (`cargo build`)

---

## Plan: cleanup-feat-current-wip

**Status**: done ✅
**Branch**: `feat/current-wip`
**Migration needed**: no

### Description

Preserved all unstaged work from the old `main` into a feature branch for cherry-picking later.

### Files modified

All 27 files that had uncommitted changes + deleted modules (`src/scan_cache.rs`, `src/spotify/replay.rs`).

### Completed

- [x] `git stash --include-untracked` on `main`
- [x] `git stash branch feat/current-wip` — creates branch + applies stash
- [x] `git add -A && git commit -m "wip: current working state before workflow reset"`
- [x] `git checkout main` — working tree clean, ready for new feature branches

---

---

## Plan: incremental-folder-scan

**Status**: done ✅
**Branch**: `feat/incremental-folder-scan`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Add incremental "Quick Scan" mode to folder scanning — only process files whose `mtime` is newer than `folders.last_scanned`. Also activate the dormant `FolderWatcher` on server startup so it polls active folders every 5 minutes.

### Backend: db.rs

1. Add `ScanMode` enum: `Full | Incremental { since: Option<i64> }`
2. `scan_directory_with_config` accepts `scan_mode`, skips files with `mtime <= since` in walk loop
3. `scan_folder` accepts `ScanMode`, passes folder's `last_scanned` as `since` when Incremental

### Backend: api.rs + tasks/mod.rs

4. `scan_folder_handler` accepts `?mode=incremental|full` query param (default: `incremental`)
5. `start_scan_folder_task` passes `ScanMode` through to `scan_directory_with_config`

### Backend: watch.rs + main.rs

6. In `serve()`, create `FolderWatcher` + call `.start()` so it polls active folders
7. `scan_active_folders` uses `ScanMode::Incremental` (polling should be lightweight)

### Frontend: folders.js

8. Replace single rescan button with two: Quick Scan (fa-bolt, yellow) and Full Rescan (fa-sync)
9. `scanFolder(id, btnEl, mode)` sends `?mode=` query param

### Files to modify

- `src/db.rs` — ScanMode + scan_directory_with_config + scan_folder
- `src/api.rs` — scan_folder_handler query param
- `src/tasks/mod.rs` — start_scan_folder_task ScanMode
- `src/watch.rs` — scan_active_folders uses Incremental
- `src/main.rs` — start FolderWatcher
- `frontend/pages/folders.js` — two buttons + mode param

### Acceptance Criteria

- [x] Quick Scan skips files with mtime ≤ folder.last_scanned
- [x] Fresh folder (last_scanned = NULL) does full scan regardless of mode
- [x] Full scan preserves current behavior
- [x] FolderWatcher starts at boot, polls active folders every 5 min
- [x] FolderWatcher uses incremental mode for its polls
- [x] Two buttons in UI: Quick Scan (bolt) + Full Rescan (sync)
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

## Plan: tracks-filter-overhaul

**Status**: proposed
**Branch**: `feat/tracks-filter-overhaul`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `002_track_tags_view.sql`

### Description

Overhaul the Tracks page filter toolbar to match the Files page canonical pattern. Add five filter dimensions: Service (fix existing), Tags, PMV, Type, and Date. The toolbar follows the same 2-column filter-panel layout as Files (File Info left / Classification right). All filters are server-side.

### Current State

- Toolbar has only Service icon buttons + search. No Tags, PMV, Type, or Date filters.
- Backend `TracksQuery` supports: `services`, `file_types`, `file_type_agg`, but no `tags`, `pmv_categories`, `pmv_aggregate`, or date fields.
- **Service filter bug**: `wireToolbarEvents` updates `state.selectedServices` and fetches, but never toggles button active CSS classes — toolbar is rendered once and not updated. Buttons don't reflect current state visually.
- `buildParams` already sends `fileTypes`/`fileTypeAgg` to backend but state/UI never expose these.

### Migration 002 (`migrations/002_track_tags_view.sql`)

New view to encapsulate the track→tag→category resolution chain:

```sql
-- v_track_tags: Resolves every service track's tags through its playlists
-- Chain: service_playlist_tracks → service_playlists → tags → tag_categories
-- Used by Tags, PMV, and any other track-tag-filter queries
CREATE VIEW v_track_tags AS
SELECT DISTINCT
    spt.track_id,
    t.id AS tag_id,
    t.name AS tag_name,
    tc.id AS category_id,
    tc.name AS category_name,
    tc.prefix,
    tc.is_default
FROM service_playlist_tracks spt
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id;
```

This puts all business logic (name matching, category resolution) in the view. The Rust query code only filters on the view's columns.

### Backend Changes (`src/api.rs`)

Extend `TracksQuery` with new params:

- `tags: Option<String>` — comma-separated tag names, filter via `v_track_tags.tag_name IN (...)`
- `pmv_categories: Option<String>` — comma-separated categories (p,m,v), filter via `v_track_tags.prefix IN (...)`
- `pmv_aggregate: Option<String>` — `full`/`partial`/`none`, filter by PMV coverage via `v_track_tags`
- `imported_after_days: Option<i64>` — tracks imported within last N days
- `imported_before_days: Option<i64>` — tracks imported before N days ago
- `added_after_days: Option<i64>` — tracks with latest playlist add within last N days
- `added_before_days: Option<i64>` — tracks with latest playlist add before N days ago

Modify `get_tracks()` and `get_tracks_count()` to apply these filters via SQL.

#### Tags filter SQL (using `v_track_tags`)

```sql
AND EXISTS (
  SELECT 1 FROM v_track_tags vtt
  WHERE vtt.track_id = st.id AND vtt.tag_name IN (?,?,...)
)
```

#### PMV filter SQL — categories (using `v_track_tags`)

```sql
AND EXISTS (
  SELECT 1 FROM v_track_tags vtt
  WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN (?,?,...)
)
```

#### PMV aggregate SQL (using `v_track_tags`)

- `full`: track has tags in all three PMV categories:
  ```sql
  AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'p')
  AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'm')
  AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'v')
  ```
- `partial`: track has at least one PMV tag: same as categories with p,m,v
- `none`: `NOT EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ('p','m','v'))`

#### Date filter SQL

- `imported_after_days`: `st.imported_at >= unixepoch('now', '-N days')`
- `imported_before_days`: `st.imported_at <= unixepoch('now', '-N days')`
- `added_after_days` / `added_before_days`: subquery on `MAX(spt.added_at)`
  ```sql
  AND (SELECT MAX(spt4.added_at) FROM service_playlist_tracks spt4
       WHERE spt4.track_id = st.id) >= unixepoch('now', '-N days')
  ```

### Frontend Changes (`frontend/pages/tracks.js`)

#### 1. Fix Service filter

- After clicking a service button, toggle its `.active` class directly in `wireToolbarEvents` instead of relying on toolbar re-render.
- Also add `updateFilterUI()` helper (like Files) to sync button states on init.

#### 2. Restructure toolbar layout

Match the 2-column pattern from Files:

- **Left column** (Track Info): Tags filter (typeahead + chips), Date filter
- **Right column** (Classification): Service, PMV, Type

Render toolbar HTML with:

- Filter-panel header: search + toggle button
- Filter-panel-body with scrollable 2-col grid
- Each filter row: toggleable label + filter controls
- Enable/disable flags in state (`tagEnabled`, `serviceEnabled`, `pmvEnabled`, `typeEnabled`, `dateEnabled`)

#### 3. Tags filter (like Files)

- Typeahead input (`#tracks-tag-search`) with dropdown populated from `/api/tags`
- Tag chips container showing selected tags
- Click to add/remove tags
- Generic toggle handler via `data-filter="tag"`
- Wire tag search debounce + dropdown selection

#### 4. PMV filter (like Files)

- 3 category buttons: P, M, V (multi-select)
- Separator + 3 aggregate buttons: Full, Partial, None (single-select, mutually exclusive with categories)
- Same interaction: picking categories clears aggregate, picking aggregate clears categories

#### 5. Type filter (like Files PMV layout)

- 4 specific type buttons: FLAC, MP3, Stem, WAV (multi-select)
- Separator + 2 aggregate buttons: Some (has any file), None (has no file)
- Same mutual-exclusion pattern

#### 6. Date filter (new)

- Two rows: one for Imported, one for Latest Added
- Each row: mode selector (Since / Before) | number input | unit selector (days / weeks / months)
- Convert weeks/months to days client-side before sending
- Send as `importedAfterDays`, `importedBeforeDays`, `addedAfterDays`, `addedBeforeDays`

#### 7. State management

Add to state:

```javascript
selectedTags: [],        // array of tag name strings
pmvCategories: [],       // ['p','m','v']
pmvAggregate: '',        // 'full'|'partial'|'none'|''
fileTypes: [],           // ['flac','mp3','stem.m4a','wav']
fileTypeAgg: '',         // 'any'|'none'|''
importedMode: '',        // 'since'|'before'|''
importedNum: null,       // number
importedUnit: 'days',    // 'days'|'weeks'|'months'
addedMode: '',           // 'since'|'before'|''
addedNum: null,          // number
addedUnit: 'days',       // 'days'|'weeks'|'months'
// Enable flags
tagEnabled: true,
serviceEnabled: true,
pmvEnabled: true,
typeEnabled: true,
dateEnabled: true,
```

#### 8. Hash sync

Extend `updateHash` defaults to include all new filter params (with empty defaults).

#### 9. `buildParams`

Add all new filter params to the query string:

- `tags` from `selectedTags`
- `pmvCategories`, `pmvAggregate`
- `fileTypes`, `fileTypeAgg`
- `importedAfterDays`, `importedBeforeDays`, `addedAfterDays`, `addedBeforeDays` (computed from mode/num/unit)

### Files to modify

- `migrations/002_track_tags_view.sql` — new view for track→tag→category resolution
- `src/api.rs` — extend `TracksQuery`, update `get_tracks()` and `get_tracks_count()`
- `frontend/pages/tracks.js` — full toolbar/filter overhaul

### Acceptance Criteria

- [ ] Service filter buttons toggle active class correctly on click
- [ ] Tags typeahead filters tracks by playlist tag membership (server-side)
- [ ] PMV category buttons filter by playlist tag category (server-side)
- [ ] PMV aggregate: Full/Partial/None work correctly
- [ ] Type filter: FLAC, MP3, Stem, WAV buttons + Some/None aggregate (server-side, reuses existing backend)
- [ ] Date filter: Since/Before + number + unit for both Imported and Latest Added (server-side)
- [ ] All filters have toggleable labels with localStorage-persisted collapse
- [ ] Pagination works correctly with all filter combinations (count query matches filtered set)
- [ ] Hash URL syncs all filter state
- [ ] 2-column filter-panel layout matches Files page
- [ ] No regressions: sort, pagination, column config, layout mode, playlist scoping
- [x] Backend compiles (`cargo build`)
- [ ] Test with `curl` first

---

## Plan: auto-deemix-subscriptions

**Status**: done ✅
**Branch**: `feat/auto-deemix-subscriptions`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

When the subscription poller discovers new tracks (or polls for the first time),
automatically trigger a deemix download via `ensure_queued()` — which checks the
live deemix queue and uses `retry_download` (UUID-based re-scan) if already
queued, or `add_to_queue` if new. Also inserts into `deemix_downloads` for
immediate UI status.

### Files modified

- `src/deemix/client.rs` — added `from_db()` constructor and `ensure_queued()` method
- `src/poller.rs` — auto-download trigger on first poll (`last_polled_at IS NULL`) and new tracks
- `src/api.rs` — delegated `load_deemix_client_from_db()` to `DeemixClient::from_db()`
- `frontend/pages/playlists.js` — updated subscribe button tooltip

### Acceptance Criteria

- [x] First poll ever triggers full deemix download (like manual 🔄 restart)
- [x] New tracks found triggers re-scan via `retry_download`
- [x] Already-queued playlists use UUID-based retry, not duplicate add
- [x] `deemix_downloads` table updated after auto-download for immediate UI
- [x] Graceful skip when deemix not configured (debug log)
- [x] Push-button manual re-download still works unchanged
- [x] Backend compiles (`cargo build`)

---

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

## Completed Archives

- **Phase 1** (Files Page) — Reference blueprint complete ✅
- **Phase 2** (Tracks Page) — Complete, awaiting server-side filter fix ✅
- **Phase 3** (Playlists Page) — Complete with tag lookup optimization ✅
- **Phase 4** (Tags Page) — Complete ✅
- **Phase 8.1** (Playlists filter box) — Complete ✅

---

## Plan: tracks-bulk-comments

**Status**: done ✅
**Branch**: `feat/tracks-bulk-comments`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Add multi-select checkboxes to the Tracks page with an ACTIONS panel containing a "WRITE COMMENTS (X)" button. X is the number of selected tracks whose linked files have outdated comments (need a comment string update).

### Backend Changes

- New `POST /api/tracks/needs-comment-count` endpoint — takes `{ trackIds: [1,2,3] }`, finds linked files via `v_file_track_link`, computes which need updates, returns `{ totalTracks, tracksNeedingUpdate, filesNeedingUpdate }`
- New `POST /api/tracks/write-comments` endpoint — takes track IDs, finds linked files needing updates, queues write-comment task via `start_write_comment_task`

### Frontend Changes

- `frontend/shared/actions-panel.js` — updated with configurable button rendering and selection count badge
- `frontend/shared/crud.js` — added `Set` instance skip in `updateHash` to avoid serialization issues
- `frontend/pages/tracks.js` — checkbox column (select-all + per-row), selection state (`selectedTrackIds` Set), `computeNeedsCount` to query backend, `writeCommentsForSelected` to trigger bulk writes, `updateSelectionUI` to keep panel in sync
- `frontend/style.css` — `.col-checkbox` styles for checkbox column

### Acceptance Criteria

- [x] Checkbox column with select-all in header
- [x] Selection persists across page navigation (Set-based)
- [x] Actions panel shows selection count badge
- [x] "WRITE COMMENTS (X)" button shows count of selected tracks needing updates
- [x] Clicking button queues write-comment task for linked files
- [x] Toast notifications for success/error/up-to-date
- [x] Selection cleared after successful write
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

## Plan: files-bulk-comments

**Status**: done ✅
**Branch**: `feat/files-bulk-comments`
**Ready for review**: yes
**Depends on**: `feat/tracks-bulk-comments` (already merged into `review/all-features`)
**Migration needed**: no

### Description

Port the checkbox-selection + "WRITE COMMENTS (X)" bulk-action pattern from Tracks to Files. On the Files page: multi-select checkboxes, an ACTIONS panel button showing how many selected files actually need a comment update (have a `needsUpdate` delta), click to queue write-comment tasks for all selected files needing updates.

### What exists already

- Per-row "write-comment" button (pencil icon) — calls `POST /api/files/{id}/write-comment`
- Actions panel skeleton in `init()` — div#files-sel-count badge, refresh button, `wireActionsRefresh` import
- `POST /api/files/write-comments` (filter-based: linked_only/tags/non_default_only) + `GET /api/files/needs-update-count` (same filters)
- File data model already has `needsUpdate` (bool), `comment`, `commentTarget`, `diffOld`, `diffNew` — comment diff is already rendered per-row
- Shared `actions-panel.js` already supports configurable buttons + selection count badge
- `.col-checkbox` CSS already exists (from tracks)

### What's missing (files-specific)

#### Backend

Unlike tracks, files don't need a join — they ARE the comment-bearing entity. So the endpoints are simpler:

1. **`POST /api/files/needs-comment-count`** — takes `{ fileIds: [1,2,3] }`, fetches those files, runs `compute_target_comment` for each, returns `{ totalFiles, filesNeedingUpdate }`
2. **`POST /api/files/write-comments-by-ids`** — takes `{ fileIds: [1,2,3] }`, fetches files, filters to those needing updates, calls `start_write_comment_task`, returns `{ taskId, fileCount }`

Router additions (in `src/api.rs`, near existing file routes):

- `.route("/api/files/needs-comment-count", post(files_needs_comment_count_by_ids_handler))`
- `.route("/api/files/write-comments-by-ids", post(files_write_comments_by_ids_handler))`

#### Frontend (`frontend/pages/files.js`)

Same pattern as tracks, adapted to the files page structure:

1. **State**: add `selectedFileIds: new Set()`, `needsCommentCount: 0`
2. **renderBody**: prepend checkbox `<th>` + `<td>` to each row (outside column-config system — same as tracks)
3. **renderEmptyBody**: add checkbox header + increment colspan
4. **wireContentEvents**: wire select-all + individual row checkboxes (same logic as tracks)
5. **init**: replace inline actions panel HTML with `renderActionsPanel([{ id: "write-comments", label: "WRITE COMMENTS", ... }])` — same call as tracks
6. **init**: wire `#files-actions-write-comments` button to `writeCommentsForSelected(container, state)`
7. **add helpers**: `updateSelectionUI`, `computeNeedsCount`, `writeCommentsForSelected` — same pattern as tracks but using `/api/files/needs-comment-count` and `/api/files/write-comments-by-ids`
8. **fetchAndRender**: call `updateSelectionUI` after each render
9. **handle null signal**: `if (signal && signal.aborted) return;` — already present in tracks, replicate in files

#### Key differences from tracks

| Aspect                      | Tracks                                                     | Files                                                                              |
| --------------------------- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Endpoint entity             | tracks → joined to files via `v_file_track_link`           | files directly                                                                     |
| needs-update count response | `{ totalTracks, tracksNeedingUpdate, filesNeedingUpdate }` | `{ totalFiles, filesNeedingUpdate }`                                               |
| needsUpdate field           | computed server-side per-request                           | already in API response (`needsUpdate`), but still verify server-side for accuracy |
| Import/state                | `showToast`, `updateSelectionCount` already imported       | `showToast` already imported, `updateSelectionCount` needs adding                  |
| renderBody params           | `(data, state)` — already has `state`                      | `(data, state)` — already has `state`                                              |

#### Potential client-side optimization

Files already return `needsUpdate` from the API. We _could_ compute X client-side (count `selectedFileIds ∩ files.where(f => f.needsUpdate)`), avoiding the `/api/files/needs-comment-count` round-trip. But the server-side check is more accurate (recomputes target comment fresh), so stick with the backend endpoint for consistency with tracks.

### Files to modify

- `src/api.rs` — add `FilesBulkRequest` struct + 2 handlers + 2 routes
- `frontend/pages/files.js` — checkbox column, selection state, actions panel wiring, helper functions

### Acceptance Criteria

- [ ] Checkbox column with select-all in header
- [ ] Selection persists across page navigation (Set-based)
- [ ] Actions panel shows selection count badge
- [ ] "WRITE COMMENTS (X)" button shows count of selected files needing updates (X = files with comment delta)
- [ ] Clicking button queues write-comment task for selected files that need updates
- [ ] Toast notifications for success/error/up-to-date
- [ ] Selection cleared after successful write
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

## Plan: spotify-rate-limit-retry

**Status**: done ✅
**Branch**: `feat/spotify-rate-limit-retry`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Parse Spotify's `Retry-After` header from 429 responses and add retry logic with backoff to the sync worker. Currently 429s just fail immediately — all playlist syncs in a batch fire in a tight loop with no delay or retry.

### Technical detail

rspotify's error chain is:

```
rspotify::ClientError::Http(Box<rspotify_http::ReqwestError>)
  → ReqwestError::StatusCode(reqwest::Response)
    → response.status() == 429
    → response.headers().get("retry-after") → "30"
```

We can downcast to `reqwest::Response` to read the header. This is already possible because rspotify uses the `reqwest` backend.

### Changes

#### `src/spotify/sync_worker.rs`

1. **New function** `extract_retry_after_secs(err: &anyhow::Error) -> Option<u64>`:
   - Walk `err.chain()` looking for `rspotify::ClientError`
   - Downcast `ClientError::Http` → `ReqwestError::StatusCode(response)`
   - Check `response.status() == 429`, parse `retry-after` header
   - Return seconds as `Option<u64>`

2. **Modify `sync_playlist_list`**: between playlist syncs, add a 300ms `tokio::sleep` to stay under Spotify's soft rate limit (~3 req/s).

3. **Modify `sync_tracks_for_playlist`**: wrap the `get_playlist` call (the first API call that triggers 429) in a retry loop:

   ```
   for attempt in 0..3:
     match client.get_playlist(id):
       Ok(p) → break
       Err(e) if is_429(e) → sleep extract_retry_after(e) or default 5s, continue
       Err(e) → bail (not a rate limit)
   ```

   Same for `get_playlist_tracks`.

4. **Modify `sync_playlists_only`**: same retry pattern for the playlist fetch loop.

5. **Logging**: emit `warn!` with the `Retry-After` duration when backing off.

### Files to modify

- `src/spotify/sync_worker.rs` — retry helper + retry loops + inter-call sleep

### Acceptance Criteria

- [ ] 429 responses with `Retry-After` header are caught and the worker sleeps the specified duration before retrying
- [ ] Max 3 retries per playlist, then moves on (no infinite loops)
- [ ] 300ms delay between successful playlist syncs to avoid hitting the limit
- [ ] Non-429 errors still fail immediately
- [x] Backend compiles (`cargo build`)
- [ ] Batch sync runs without `429 Too Many Requests` failures (tested against Spotify)

---

## Plan: tag-parents

**Status**: done ✅
**Branch**: `feat/tag-parents`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — merged into `002_playlist_fetch_tracking.sql`

### Description

Allow Setlist-category tags (long playlist names) to have "parent" tags that replace them in file comments. A Setlist tag like `Dark Techno/2026/Hardtechno/...` resolves to parent tags `dark` (Mood), `techno` (Vibe), `hard` (Merkmal). Comments use the parent tag names and categories instead of the long original. Only Setlist tags can have parents; P/M/V/E tags cannot.

### Schema

- **`tag_parents`** table: `(id, tag_id, parent_tag_id, created_at)` with UNIQUE(tag_id, parent_tag_id)
- **`v_resolved_tags`** view: for each tag, returns parent tags if they exist, otherwise the tag itself
- **`v_file_resolved_tags`** view: like `v_file_tags` but resolves through `v_resolved_tags`

### Backend Changes

- **`src/db.rs`**: `get_tag_parents()`, `get_tag_children()`, `set_tag_parents()` (with validation: Setlist-only, no self-ref, parents must exist)
- **`src/db.rs`**: `compute_target_comment()` now queries `v_file_resolved_tags` instead of `v_file_tags`
- **`src/api.rs`**: `GET /api/tags/{id}/parents`, `PUT /api/tags/{id}/parents`, `GET /api/tags/{id}/children`

### Frontend Changes

- **`frontend/pages/tags.js`**: Edit modal shows "Parent Tags" section for Setlist tags with typeahead search, chip management, and save

### Acceptance Criteria

- [x] Setlist tags can be assigned parent tags via API and frontend
- [x] Non-Setlist tags rejected with clear error
- [x] Self-reference prevented
- [x] Non-existent parent tags rejected
- [x] `compute_target_comment` uses resolved parent tags (names + categories)
- [x] Comment PMV indicators reflect parent tag categories
- [x] Tags without parents work as before (backward compatible)
- [x] Backend compiles (`cargo build`)
- [x] Migration runs cleanly
- [x] Tested with curl

---

## Plan: tag-curation-page

**Status**: done ✅
**Branch**: `feat/tag-curation-page`
**Ready for review**: yes
**Depends on**: `feat/tag-parents`
**Migration needed**: no

### Description

A dedicated curation workflow page for going through Setlist tags and assigning parent tags efficiently. Combines a sequential workflow (prev/next through the queue) with a browsable table to jump around, plus smart search that can add existing tags or create-and-add new ones inline.

### Backend Changes

- **`src/db.rs`**: `get_curation_queue()` — returns Setlist tags with parent counts, file counts, and full parent tag details as JSON. Filterable by search, has_parents (yes/no/any), sortable by name/length/files/parents.
- **`src/api.rs`**: `GET /api/tags/curation-queue` endpoint with `CurationQueueQuery` params

### Frontend Changes

- **`frontend/pages/tag-curation.js`** — new 950-line page module with:
  - Top nav bar: prev/next with progress bar (keyboard shortcuts ←/→ or p/n)
  - Tag card: big tag name, metadata
  - Parent tags editor: chips with remove, typeahead search with "Add" button, inline "Create & Add" popover (category picker → create → add as parent)
  - Browse All: collapsible mini table of Setlist tags with search/sort/filter, click to jump
  - Auto-save: every add/remove immediately PUTs parents; navigation waits for in-flight saves
- **`frontend/app.js`**: register `"tag-curation"` in PAGE_MAP
- **`frontend/shared/nav.js`**: add "Tag Curation" link to TOOLS_ITEMS

### Acceptance Criteria

- [x] Curation queue lists all Setlist tags sorted by name length (descending)
- [x] Search filter works (by tag name)
- [x] has_parents filter works (yes/no/any)
- [x] Sort by name/length/files/parents works
- [x] Each result includes parent tag details (id, name, category, icon)
- [x] Parent chips show category badges with correct colors
- [x] Typeahead search finds existing tags and can add them as parents
- [x] "Create & Add" flow creates a new tag and immediately adds as parent
- [x] Removing a parent chip removes the parent relationship
- [x] Auto-save: changes persist immediately via API
- [x] Navigation (prev/next/jump) works with auto-save
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

## Plan: fix-tag-case-duplicates

**Status**: done ✅
**Branch**: `fix/tag-case-duplicates`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `004_unique_tags_nocase.sql`

### Description

Fix two interlinked bugs discovered during investigation of "Groovy" tag showing 1060 tracks (2× the real 530):

1. **Playlist page cartesian product**: `playlists_handler` LEFT JOINs `v_tag_playlist` which returns multiple rows per playlist when case-different duplicate tags exist. This multiplies `COUNT(spt.track_id)` — e.g. 530 tracks × 2 matching tags ("Groovy" + "groovy") = 1060.

2. **No uniqueness on `tags.name`**: The `tags` table allows "Groovy" and "groovy" as separate tags. Since tag↔playlist matching is case-insensitive, duplicate tags are functionally identical — both resolve to the same playlists, tracks, and files.

### Root cause

- `v_tag_playlist` does `LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))` — case-insensitive
- `tags.name` has only a regular index (`idx_tags_name`), not UNIQUE
- `playlists_handler` does `LEFT JOIN v_tag_playlist vtp ON vtp.playlist_id = sp.id` without subquery/aggregation, so each playlist row fans out N×M when N duplicate tags match M playlists

### Other `v_tag_playlist` consumers (all safe)

| Query                           | Pattern                     | Safe? |
| ------------------------------- | --------------------------- | ----- |
| `tags_service_coverage_handler` | `COUNT(DISTINCT tag_id)`    | ✅    |
| `get_tag_service_connections`   | `DISTINCT vtp.service`      | ✅    |
| `get_playlists_without_tags`    | `NOT EXISTS (SELECT 1 ...)` | ✅    |
| `create_tags_from_playlists`    | `NOT EXISTS`                | ✅    |
| `get_tags_for_service_track`    | `SELECT DISTINCT t.id`      | ✅    |

Only `playlists_handler` is affected.

### Migration 004 (`migrations/004_unique_tags_nocase.sql`)

1. Create `tags_v2` with `name TEXT NOT NULL UNIQUE COLLATE NOCASE`
2. Copy distinct tags from `tags` (deduplicate by `LOWER(name)`, keep lowest `id`)
3. Build remapping table: old dup tag IDs → surviving tag ID
4. Re-point FKs in `tag_parents`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities`
5. Drop old `tags`, rename `tags_v2` → `tags`
6. Recreate indexes on `tags(id)`, `tags(category_id)`, `tags(name)`
7. Verify no orphan FKs

**Existing duplicates to merge**: Tag "groovy" (id 286) → merged into "Groovy" (id 88). Both are Vibe category, so no category conflict.

### Backend changes

- **`src/api.rs` — `playlists_handler`**: Replace `LEFT JOIN v_tag_playlist vtp ON vtp.playlist_id = sp.id` with a scalar subquery or `LEFT JOIN (SELECT DISTINCT playlist_id, tag_name FROM v_tag_playlist)`. This guards against any future cartesian product even if duplicate tags somehow reappear.

- No changes needed to `get_tag_by_name` (already uses `COLLATE NOCASE`) or `create_tag` (will naturally fail on duplicate with new UNIQUE constraint).

### Existing data

Current state:

```
Playlists:  "Groovy" (id 292, 530 tracks), "groovy" (id 133, 6 tracks)
Tags:       "Groovy" (id 88, Vibe), "groovy" (id 286, Vibe)
```

After migration:

```
Tags:       "Groovy" (id 88, Vibe) — only one
Playlists:  unchanged — both still match "Groovy" tag via case-insensitive join
Playlist page: "Groovy" shows 530 tracks ✅ (was 1060)
```

### Files to modify

- `migrations/004_unique_tags_nocase.sql` — new migration
- `src/api.rs` — fix `playlists_handler` JOIN

### Acceptance Criteria

- [ ] `tags.name` has UNIQUE COLLATE NOCASE constraint
- [ ] Cannot insert "groovy" when "Groovy" already exists
- [x] Existing duplicate tag "groovy" (id 286) merged into "Groovy" (id 88)
- [x] `tag_parents`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities` FKs remapped
- [x] Playlists page shows 530 tracks for "Groovy" playlist (not 1060)
- [ ] All other `v_tag_playlist` consumers produce identical results
- [ ] Tag "groovy" (id 286) deleted from tags table
- [x] Backend compiles (`cargo build`)
- [ ] Fresh DB: migrations 001→002→003 run cleanly
- [ ] Existing DB: migration 003 applies without errors

---

## Plan: fix-filter-button-feedback

**Status**: done ✅
**Branch**: `fix/filter-button-feedback`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Fix two bugs with the Files page filter toolbar buttons:

1. **No visual active state**: After clicking any filter button, its `.active` class was never toggled because `fetchAndRender` only re-renders `#files-content`, not the toolbar. Fixed by adding `btn.classList.toggle("active")` inline in all 5 button click handlers (Service, PMV Category, PMV Aggregate, Comment Status, File Type).

2. **Comment status pagination broken**: The `comment_statuses` filter was applied in Rust AFTER `LIMIT/OFFSET` in SQL, meaning a page expecting 100 results could return 5. Fixed by fetching ALL matching rows (no LIMIT/OFFSET) when comment status filter is active, computing `comment_needs_update` pre-filtering in Rust, then applying offset/limit in Rust. Cached `target_comment` results are reused in the downstream ApiFile conversion loop to avoid recomputation.

### Files modified

- `frontend/pages/files.js` — 5x `btn.classList.toggle("active")` added in filter button handlers
- `src/api.rs` — `get_files()` conditionally skips SQL LIMIT/OFFSET when comment_statuses is active, fetches all rows, filters in Rust, then slices for pagination

### Acceptance Criteria

- [x] All 5 filter button groups toggle `.active` class immediately on click
- [x] Multi-select buttons (Service, Comment, FileType) properly toggle on/off
- [x] Single-select (PMV Aggregate) properly clears sibling buttons
- [x] Comment status filter returns correct page sizes (LIMIT rows, not fewer)
- [x] Count query returns correct total for comment status filter
- [x] Cached target comments avoid recomputation in downstream conversion
- [x] Backend compiles (`cargo build`)
- [x] No regressions to other filters or pagination without comment status filter

---

## Plan: tracks-playlist-filter

**Status**: done ✅
**Branch**: `feat/playlist-sync-enhancements`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Add a playlist filter to the Tracks page — a typeahead search box with chips, matching the existing Tags filter pattern. Users type a playlist name, get suggestions from `/api/playlists`, click to add playlist chips, and the track list filters to only tracks belonging to any of the selected playlists. Multiple playlists are OR'd together (tracks in ANY selected playlist).

### Current State

- Backend `TracksQuery` has `playlist_id: Option<i64>` for single-playlist scoping (used by playlist context badge, not exposed as a user filter)
- Frontend toolbar LEFT column has: Tags (typeahead + chips), Date
- No way to filter tracks by playlist name(s) from the Tracks page

### Backend Changes (`src/api.rs`)

1. **Extend `TracksQuery`**: add `playlists: Option<String>` — comma-separated playlist names

2. **Modify `get_tracks()`**: when `playlists` is set, add JOIN + IN filter:

   ```sql
   SELECT DISTINCT st.* FROM service_tracks st
   JOIN service_playlist_tracks spt ON spt.track_id = st.id
   JOIN service_playlists sp ON sp.id = spt.playlist_id
   WHERE 1=1
     AND LOWER(sp.name) IN (?,?,...)
   ```

   Use `DISTINCT` to avoid duplicates when a track belongs to multiple selected playlists.

3. **Modify `get_tracks_count()`**: same JOIN + IN filter with `COUNT(DISTINCT st.id)`.

4. **Conflict handling**: when both `playlist_id` (single) and `playlists` (multi) are set, `playlists` takes precedence (multi-select replaces single-playlist scoping). The `playlist_id` param is used by the playlist context badge — when the user adds playlist chips, the badge should be cleared on the frontend side.

### Frontend Changes (`frontend/pages/tracks.js`)

#### 1. State additions

```javascript
selectedPlaylists: [],  // array of playlist name strings
playlistEnabled: true,
```

#### 2. Hash schema additions

```javascript
selectedPlaylists: { type: "array", default: [] },
```

#### 3. Toolbar HTML (LEFT column, between Tags and Date)

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="playlist">Playlists</span>
  <div class="typeahead-wrap" style="flex:1">
    <div class="tag-search-wrap">
      <i class="fas fa-list"></i>
      <input
        type="text"
        class="input-text input-search"
        id="tracks-playlist-search"
        placeholder="filter by PLAYLIST"
        autocomplete="off"
      />
      <div class="tag-dropdown" id="tracks-playlist-dropdown"></div>
    </div>
  </div>
  <div class="tag-chips" id="tracks-playlist-chips">${playlistChipsHtml}</div>
</div>
```

#### 4. `buildParams`

```javascript
if (state.selectedPlaylists && state.selectedPlaylists.length > 0) {
  params.set("playlists", state.selectedPlaylists.join(","));
}
```

#### 5. Wire typeahead (in `wireToolbarEvents`)

Same pattern as tags typeahead (already present in the same file for `#tracks-tag-search`):

- Debounced input → `fetchJSON("/api/playlists?search=...&page_size=20")`
- Dropdown with playlist names (+ service icon? optional: service badge for clarity)
- Keyboard nav (ArrowDown/Up, Enter, Escape)
- Click outside closes dropdown
- Click item → add to `state.selectedPlaylists`, clear input, close dropdown, re-fetch

#### 6. Wire chip removal

Delegate click on `.tag-chip-x` inside `#tracks-playlist-chips` → remove from `state.selectedPlaylists`, re-fetch.

#### 7. `updateFilterUI`

Include `.tag-chips` and `.typeahead-wrap` in the disable/enable toggle for `[data-filter="playlist"]`.

#### 8. Toggle handler

Add `playlistEnabled` to the generic toggle handler (click on disabled filter row re-enables it).

#### 9. `wireContentEvents` / `updateFilterUI`

Include playlist chip container + typeahead in filter UI state syncing.

### Files to modify

- `src/api.rs` — extend `TracksQuery`, update `get_tracks()`, `get_tracks_count()`
- `frontend/pages/tracks.js` — state, hash, toolbar HTML, typeahead wiring, chips, buildParams

### Acceptance Criteria

- [x] Playlist typeahead appears in LEFT column between Tags and Date
- [x] Typing searches playlists via `/api/playlists?search=...` with debounce
- [x] Dropdown shows matching playlist names; keyboard nav works
- [x] Clicking a dropdown item adds a playlist chip and filters tracks server-side
- [x] Multiple chips supported (OR logic — tracks in any selected playlist)
- [x] Removing a chip removes the filter and refreshes
- [x] Playlist filter is toggleable (collapsible, localStorage persistence)
- [x] Pagination works correctly with playlist filter active
- [x] Count query matches filtered result count
- [x] When playlist filter is active, the single-playlist context badge is cleared
- [x] No regressions: tags, PMV, type, date, service filters still work
- [x] No regressions: sort, pagination, column config, layout mode, bulk comments
- [x] Backend compiles (`cargo build`)
- [x] Test with `curl` first

---

## Plan: digging-multi-seed

**Status**: done ✅
**Branch**: `feat/digging-multi-seed`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Build the core multi-seed suggestion engine for the Digging/Curator workflow. Given a set of seed files (loaded by tag name or file IDs), find similar tracks from the local library using Camelot harmonic mixing + BPM proximity, scored and ranked. Deduplicate by ISRC. This is the backend engine — Phase 1 of 5.

### Design Decisions (from user)

1. **Embedded player**: browser-native `<audio>` — stem.m4a + FLAC both play natively in modern browsers, just need Range-request streaming
2. **ISRC dedup**: one suggestion per ISRC, prefer stem.m4a (plays in browser) — both versions stay in DB
3. **Outlier handling**: BPM range computed from seed cluster, tracks outside range excluded entirely

### Real Data (from production DB)

Tag "Collapse-capital" (id 434):

| File ID | ISRC         | Title             | Artist                    | BPM   | Key |
| ------- | ------------ | ----------------- | ------------------------- | ----- | --- |
| 4042    | US7NS2500009 | Games People Play | Paula van Klar            | 140.0 | 3m  |
| 4362    | US7NS2500009 | Games People Play | Paula van Klar            | 139.0 | 3m  |
| 4196    | QZ5FN2650988 | The Void          | Maite Dedecker            | 141.0 | 8m  |
| 4428    | QZ5FN2650988 | The Void          | Maite Dedecker            | 140.0 | 8m  |
| 5757    | DGA0H2483973 | This Summer       | Anna Reusch               | 140.0 | 6m  |
| 5769    | DGA0H2483973 | This Summer       | Anna Reusch               | 139.0 | 6m  |
| 3904    | ?            | Mean One          | Elon Bass Luciano Bradini | 160.0 | 1m  |
| 4538    | ?            | Mean One          | Elon Bass                 | 160.0 | 1m  |

BPM cluster of the 3 target tracks: 139–141. "Mean One" at 160 is an outlier, falls outside default ±8 range.
Eligible pool: 2184 files with BPM+Key, 1728 unique ISRCs.

### Backend Changes

#### 1. `src/digging.rs` — New types

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestRequest {
    /// Seed files: either provide file IDs directly...
    pub seed_file_ids: Option<Vec<i64>>,
    /// ...or a tag name whose files become the seeds
    pub seed_tag: Option<String>,
    /// BPM tolerance (± from seed BPM range boundaries)
    pub bpm_range: Option<f64>,  // default 8.0
    /// Active Camelot jumps
    pub camelot_jumps: Option<Vec<String>>,
    /// Max suggestions to return
    pub limit: Option<i64>,  // default 20, max 50
    /// Deduplicate suggestions by ISRC
    pub dedup_by_isrc: Option<bool>,  // default true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSeed {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub file_path: String,
    pub file_type: String,  // "flac", "stem.m4a", etc.
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Tags on this file (from v_file_resolved_tags)
    pub tags: Vec<DiggingTag>,
    /// Whether this seed was excluded as a BPM outlier
    pub excluded_as_outlier: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTag {
    pub id: i64,
    pub name: String,
    pub category_name: String,
    pub prefix: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestion {
    pub file_id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub file_path: String,
    pub file_type: String,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Which of the seeds this suggestion best matches
    pub matching_seed_id: i64,
    /// Camelot compatibility: "perfect", "good", "ok"
    pub camelot_compatibility: String,
    /// BPM difference from best-matching seed
    pub bpm_diff: Option<f64>,
    /// Tags shared with the best-matching seed
    pub shared_tags: Vec<String>,
    /// Scoring details (for transparency)
    pub score_breakdown: ScoreBreakdown,
    /// Combined score (lower = better)
    pub score: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    pub play_count_score: f64,
    pub recency_score: f64,
    pub bpm_score: f64,
    pub camelot_bonus: f64,
    pub tag_match_bonus: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestResponse {
    /// The seed tracks used (including excluded outliers)
    pub seeds: Vec<DiggingSeed>,
    /// The BPM range used for candidate search
    pub bpm_min: f64,
    pub bpm_max: f64,
    /// Scored + ranked suggestions
    pub suggestions: Vec<DiggingSuggestion>,
    /// Total candidates considered before ranking
    pub candidates_considered: usize,
}
```

#### 2. `src/digging.rs` — `get_multi_seed_suggestions()`

```rust
pub async fn get_multi_seed_suggestions(
    pool: &Pool<Sqlite>,
    req: &DiggingSuggestRequest,
) -> Result<DiggingSuggestResponse>
```

**Algorithm:**

1. **Resolve seeds**: if `seed_tag` is set, query `v_file_tags` for all files with that tag. Otherwise use `seed_file_ids`. Load full File rows + resolved tags.

2. **Outlier detection**: compute median BPM of seeds, exclude any seed whose BPM deviates >20 from median. Mark excluded seeds with `excluded_as_outlier: true`. Compute BPM range from non-excluded seeds: `[min(bpm) - range, max(bpm) + range]`.

3. **Candidate query**: fetch all files (not in seed set) within BPM range, that have both BPM and key:

   ```sql
   SELECT * FROM files
   WHERE id NOT IN (?,?,...)
     AND bpm IS NOT NULL
     AND musical_key IS NOT NULL
     AND bpm >= ? AND bpm <= ?
   ORDER BY play_count ASC, COALESCE(last_played, 0) ASC
   LIMIT ?  -- fetch 5x limit for scoring pool
   ```

4. **Camelot filtering**: for each candidate, parse its `musical_key` as Camelot. Check compatibility against each non-excluded seed using `are_keys_compatible()`. If compatible with at least one seed, keep. Track which seed was the best match.

5. **Scoring** (per candidate, best seed match):
   - `play_count_score = min(play_count, 100) * 2.0` — fresher tracks preferred
   - `recency_score = (1000 - min(days_since_played, 1000)) * 0.5` — unplayed = 0, recent = high
   - If never played: `recency_bonus = -50.0`
   - `bpm_score = |candidate_bpm - seed_bpm| * 1.5`
   - `camelot_bonus`: perfect = -30, good = -15, ok = 0
   - `tag_match_bonus`: count shared resolved tags with the matching seed, -5 per shared tag
   - `total_score = play_count_score + recency_score + bpm_score + camelot_bonus + tag_match_bonus`

6. **ISRC dedup**: if `dedup_by_isrc` is true, group candidates by ISRC. For each ISRC group, keep the one with the lowest score. If ISRC is NULL, treat each as unique. Prefer `stem.m4a` over `flac` when scores tie.

7. **Sort + limit**: sort by score ascending, truncate to `limit`.

8. **Load tags**: for each suggestion, load resolved tags via `v_file_resolved_tags` for the `shared_tags` field (intersection with matching seed's tags).

#### 3. `src/api.rs` — Handler + Route

**Route**: `.route("/api/digging/suggest", post(digging_suggest_handler))`

**Handler**:

```rust
async fn digging_suggest_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DiggingSuggestRequest>,
) -> impl IntoResponse {
    match crate::digging::get_multi_seed_suggestions(&state.db, &request).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}
```

Validation:

- Either `seed_file_ids` or `seed_tag` must be provided (400 if neither)
- At least 1 seed file must be found (404 if tag resolves to no files)
- `limit` clamped to 1..50, default 20
- `bpm_range` clamped to 1..30, default 8.0
- `camelot_jumps` defaults to all jumps if not provided

#### 4. `src/api.rs` — Audio Streaming Endpoint

**Route**: `.route("/api/files/{id}/stream", get(file_stream_handler))`

**Handler**:

```rust
async fn file_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    request: axum::http::Request<Body>,
) -> impl IntoResponse
```

- Look up file by ID, get `file_path`
- Open file, get size
- Support `Range` header for seeking (HTTP 206 Partial Content)
- Content-Type based on extension: `.flac` → `audio/flac`, `.m4a` → `audio/mp4`, `.mp3` → `audio/mpeg`, `.wav` → `audio/wav`, `.aif`/`.aiff` → `audio/aiff`
- Accept-Ranges: bytes
- Without Range header: stream entire file (HTTP 200)
- Security: only serve files that are in the `files` table (no arbitrary path traversal)

#### 5. `src/digging.rs` — Audio format preference for dedup

```rust
/// When deduplicating by ISRC, prefer formats that play in browsers.
/// stem.m4a > mp3 > flac > wav > aiff > other
fn audio_format_preference(file_type: &str) -> u8 {
    match file_type.to_lowercase().as_str() {
        "stem.m4a" | "m4a" => 0,
        "mp3" | "mpeg" => 1,
        "flac" => 2,
        "wav" | "wave" => 3,
        "aif" | "aiff" => 4,
        _ => 5,
    }
}
```

### Existing code to reference

- `src/digging.rs`: `CamelotKey`, `parse_camelot_key()`, `are_keys_compatible()`, `ScoredTrack`, `get_suggestions()` (single-seed — can borrow scoring logic)
- `src/api.rs`: `get_tags_for_file()` in db.rs returns resolved tags via `v_file_resolved_tags`
- `migrations/001_initial_schema.sql`: `v_file_resolved_tags` view (in migration 002)
- `frontend/shared/utils.js`: `fetchJSON()` for API calls

### Files to modify

- `src/digging.rs` — new types + `get_multi_seed_suggestions()` + ISRC dedup helper
- `src/api.rs` — `digging_suggest_handler` + `file_stream_handler` + routes

### Acceptance Criteria

- [x] `POST /api/digging/suggest` with tag name resolves seed files from `v_file_tags`
- [x] `POST /api/digging/suggest` with seed file IDs works directly
- [x] BPM outlier detection excludes "Mean One" (160 BPM) when seeds are the 3 collapse-capital tracks at 139-141
- [x] BPM range computed as [min(bpm)-range, max(bpm)+range] from non-outlier seeds only
- [x] Candidates filtered to BPM range, must have BPM + key
- [x] Camelot compatibility checked against all non-excluded seeds (OR logic)
- [x] Scoring: play_count, recency, bpm_diff, camelot_bonus, tag_match_bonus all contribute correctly
- [x] ISRC dedup: same ISRC appears only once, stem.m4a preferred over flac
- [x] NULL ISRC files treated as unique (not deduplicated)
- [x] Response includes `seeds` array with outlier flags, `bpm_min`/`bpm_max`, `suggestions` with score_breakdown
- [x] `GET /api/files/{id}/stream` returns audio with correct Content-Type
- [x] `GET /api/files/{id}/stream` supports Range header (HTTP 206) for seeking
- [x] `GET /api/files/{id}/stream` returns 404 for non-existent file or file not in DB
- [x] 400 if neither seed_file_ids nor seed_tag provided
- [x] 404 if seed_tag resolves to no files
- [x] Backend compiles (`cargo build`)
- [x] Test with curl against real data: `curl -X POST localhost:3000/api/digging/suggest -H 'Content-Type: application/json' -d '{"seedTag":"Collapse-capital","limit":10}'`

---

## Plan: digging-frontend

**Status**: done ✅
**Branch**: `feat/digging-frontend`
**Ready for review**: yes
**Depends on**: `feat/digging-multi-seed` (Phase 1)
**Migration needed**: no

### Description

Build the `#digging` SPA page — a split-view Digging/Curator workflow. Left panel: tag-based seed selection with track cards showing BPM/Key/tags, config controls (BPM range, Camelot jumps). Right panel: scored & ranked suggestions with embedded `<audio>` players, tag overview, and action buttons (add to tag).

### Design

```
┌─────────────────────────────────────────────────────┐
│ DIGGING                                    [Config]│
├───────────────────────┬─────────────────────────────┤
│ SEEDS                 │ SUGGESTIONS                 │
│                       │                             │
│ [Collapse-capital  ✕] │ +-+ +-+ +-+ +-+ +-+ +-+  │
│ [Find Similar]        │ |#1| | Games People Play |  │
│                       │ |  | | Paula van Klar    |  │
│ Config:               │ |  | | 140BPM 3m perfect |  │
│ BPM: [====8====] ±8   │ |  | | [+▶] [+ Add]     |  │
│ Jumps: [+1][-1][+2]   │ +-+ +-+ +-+ +-+ +-+ +-+  │
│        [-2][+7][A↔B]  │                             │
│                       │ +-+ +-+ +-+ +-+ +-+ +-+  │
│ +-+ +-+ +-+ +-+     │ |#2| | The Void          |  │
│ | Games People Play  | │ |  | | Maite Dedecker    |  │
│ | Paula van Klar    | │ |  | | 141BPM 8m perfect |  │
│ | 140 BPM · 3m      | │ |  | | [+▶] [+ Add]     |  │
│ | ⚠ OUTLIER         | │ +-+ +-+ +-+ +-+ +-+ +-+  │
│ +-+ +-+ +-+ +-+     │                             │
│                       │ [Load More]                 │
└───────────────────────┴─────────────────────────────┘
```

### Frontend: `frontend/pages/digging.js`

#### State

```javascript
const state = {
  selectedTag: null, // { id, name }
  seeds: [], // DiggingSeed[]
  bpmRange: 8,
  camelotJumps: {
    "+1": true,
    "-1": true,
    "+2": true,
    "-2": true,
    "+7": true,
    "-7": true,
    a_to_b: true,
    same: true,
  },
  limit: 10,
  suggestions: [],
  bpmMin: null,
  bpmMax: null,
  candidatesConsidered: 0,
  loading: false,
  configOpen: false,
  activeAudio: null,
};
```

#### Functions

| Function                        | Purpose                                              |
| ------------------------------- | ---------------------------------------------------- |
| `init(container)`               | Entry point: renders layout + wires events           |
| `renderLayout(container)`       | Renders split-panel HTML                             |
| `renderSeeds(container)`        | Renders seed cards into `#digging-seeds`             |
| `renderSuggestions(container)`  | Renders suggestion cards into `#digging-suggestions` |
| `wireEvents(container)`         | Wires all click/keyboard events                      |
| `buildRequest()`                | Builds `POST /api/digging/suggest` body from state   |
| `doSearch(container)`           | Calls API, updates state, re-renders suggestions     |
| `setupAudioPlayers(container)`  | Wires Play/Pause for `<audio>` elements              |
| `loadConfig()` / `saveConfig()` | localStorage persistence                             |

#### Audio Player

- One `<audio>` element per suggestion card pointing to `/api/files/{id}/stream`
- Clicking Play stops any currently playing audio, starts new one
- Button toggles ▶ / ⏸
- `onended` resets button

### Files to modify

- `frontend/pages/digging.js` — new file (~400 lines)
- `frontend/app.js` — register `"digging": "digging"` in PAGE_MAP
- `frontend/shared/nav.js` — add `{ href: "#digging", icon: "fa-magnifying-glass", label: "Digging" }` to TOOLS_ITEMS
- `frontend/style.css` — digging-specific styles (~100 lines)

### CSS (key classes to add)

```css
.digging-layout {
  display: flex;
  gap: 1.5rem;
  height: calc(100vh - 180px);
}
.digging-seeds {
  width: 40%;
  overflow-y: auto;
}
.digging-suggestions {
  width: 60%;
  overflow-y: auto;
}
.seed-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 0.75rem;
}
.seed-card.outlier {
  opacity: 0.5;
  border-style: dashed;
}
.suggestion-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 0.75rem;
  display: flex;
  gap: 1rem;
  align-items: flex-start;
}
.sugg-rank {
  font-size: 1.5rem;
  font-weight: 700;
  color: var(--muted);
  min-width: 2rem;
  text-align: center;
}
.sugg-body {
  flex: 1;
}
.badge.camelot.perfect {
  background: #2e7d32;
  color: #fff;
}
.badge.camelot.good {
  background: #1565c0;
  color: #fff;
}
.badge.camelot.ok {
  background: #666;
  color: #fff;
}
.btn-play {
  background: var(--primary);
  color: #fff;
  border: none;
  border-radius: 50%;
  width: 32px;
  height: 32px;
  cursor: pointer;
  font-size: 0.9rem;
  flex-shrink: 0;
}
.digging-config {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 1rem;
}
.jump-toggle {
  padding: 0.2rem 0.6rem;
  border: 1px solid var(--border);
  border-radius: 4px;
  cursor: pointer;
  font-size: 0.8rem;
  background: var(--bg);
}
.jump-toggle.active {
  background: var(--primary);
  color: #fff;
  border-color: var(--primary);
}
```

### Acceptance Criteria

- [x] `#digging` route loads the digging page
- [x] Nav link "Digging" in TOOLS section
- [x] Tag typeahead finds tags from `/api/tags?search=...`
- [x] Selecting a tag enables "Find Similar" button
- [x] "Find Similar" calls `POST /api/digging/suggest`, renders results
- [x] Seeds render as cards with BPM/Key/outlier warning
- [x] Suggestions render as ranked cards with score breakdown
- [x] Camelot compatibility badge (perfect=green, good=blue, ok=grey)
- [x] `<audio>` player: Play/Pause works, only one plays at a time
- [x] BPM range slider (2–20) triggers re-fetch
- [x] Camelot jump toggles trigger re-fetch
- [x] Config persists in localStorage
- [x] Loading spinner during API calls
- [x] Error states: toast for API errors, empty state when no tag selected
- [x] Responsive: stacks vertically on narrow screens
- [x] No regressions: other pages still load and function
- [x] Frontend compiles (ES modules load without errors)

---

## Plan: local-playlists

**Status**: done ✅
**Branch**: `feat/local-playlists`
**Ready for review**: yes
**Depends on**: `feat/digging-multi-seed` (Phase 1)
**Migration needed**: yes — `005_local_service.sql`

### Description

Add "local" as a first-class service source. A local playlist can contain any `service_track` (Spotify, YouTube, or newly created `local` tracks). The playlist→tag chain works automatically via existing `v_tag_playlist`. This enables the Digging workflow: save suggestions as a persistent local playlist, which creates a Setlist tag, which can be written into file comments.

### Why no new tables

- `service_playlists(service='local')` — already works, no FK constraint on service values
- `service_tracks(service='local')` — already works, `service_id` can be any string
- `service_playlist_tracks` — already works, any track can be in any playlist
- Only needed change: `v_file_track_link` view to match `service='local'` on `service_id = CAST(f.id AS TEXT)`

### Migration: `migrations/005_local_service.sql`

Recreate `v_file_track_link` with the local service match:

```sql
DROP VIEW IF EXISTS v_file_track_link;
CREATE VIEW v_file_track_link AS
SELECT f.id AS file_id, st.id AS track_id
FROM files f
JOIN service_tracks st ON (
    st.isrc = f.isrc
    OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
    OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
    OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
    OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
);
```

Also update `v_file_tags` and `v_file_resolved_tags` (in 001/002/004) — they reference `v_file_track_link` indirectly via service_playlist_tracks, so just re-running `DROP VIEW IF EXISTS ... CREATE VIEW ...` for those dependent views is needed. Or simpler: the migration just drops and recreates all affected views.

### Backend: `src/api.rs` — New endpoint

**Route**: `.route("/api/playlists/local", post(create_local_playlist_handler))`

**Request**:

```json
{
  "name": "collapse-capital-v2",
  "fileIds": [4042, 4196, 5757, 65, 831]
}
```

**Handler logic**:

```rust
async fn create_local_playlist_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateLocalPlaylistRequest>,
) -> impl IntoResponse {
    // 1. Für jedes File: service_track existiert? (via ISRC oder local service_id)
    //    Ja → dessen ID merken
    //    Nein → INSERT service_track(service='local', service_id=CAST(file.id AS TEXT),
    //                                title, artist, isrc=file.isrc)
    // 2. INSERT service_playlists(service='local', name=request.name)
    // 3. INSERT service_playlist_tracks(playlist_id, track_id) für alle resolved tracks
    // 4. Return { playlistId, trackCount, newTrackCount }
}
```

### Frontend Integration (in Phase 2)

The digging page's "Save as ..." button calls this endpoint. User types a playlist name, clicks save.

### What happens automatically after save

1. `v_tag_playlist` matches playlist name → creates tag (via `create_tags_from_playlists` or on next poll)
2. `v_file_tags` shows all saved files under the new tag
3. User goes to Files page, filters by the tag, clicks "Write Comments"
4. Files now have `[PMV] tags collapse-capital-v2` in their comment

### Future: mirror to Spotify

Because local playlist contains Spotify-track IDs, we can later:

1. `POST /api/services/spotify/create-playlist` → creates Spotify playlist
2. `POST /api/services/spotify/add-tracks` → adds tracks by Spotify ID
3. Update `service_playlists` with Spotify ID → subscription poller picks it up

### Acceptance Criteria

- [x] `v_file_track_link` matches `service='local'` on `service_id = CAST(f.id AS TEXT)`
- [x] `POST /api/playlists/local` creates playlist + ensures service_tracks + adds track entries
- [x] Creating a local playlist automatically creates a Setlist tag via name match
- [x] Files appear under the tag in `v_file_tags`
- [x] `v_file_resolved_tags` works for local playlists (tag parents supported)
- [x] Duplicate service_tracks not created (ISRC match reuses existing Spotify track)
- [x] No regressions: `v_file_track_link` still matches Spotify/SoundCloud/YouTube correctly
- [x] Backend compiles (`cargo build`)
- [x] Fresh DB: all migrations run cleanly (001→002→003→004→005→006)
- [x] Test with curl: create playlist, verify tag auto-created, verify file-tag-link

---

## Plan: digging-staging-area

**Status**: done ✅
**Branch**: `feat/digging-staging-area`
**Ready for review**: yes
**Depends on**: `feat/digging-frontend`, `feat/local-playlists`
**Migration needed**: no

### Description

Add a "staging area" to the left panel of the Digging page. Users click "Add" on suggestions, which moves tracks into staging. Tracks accumulate there until the user is happy, then they can persist the entire staging area as a new local playlist (using existing `POST /api/playlists/local`). Camelot key coverage indicator shows which keys are covered.

### State additions to `frontend/pages/digging.js`

```javascript
staging: [],          // DiggingSuggestion[] — accumulated tracks
showSaveDialog: false,
playlistName: "",
```

### Key functions

| Function                    | Purpose                                           |
| --------------------------- | ------------------------------------------------- |
| `addToStaging(suggestion)`  | Move from suggestions[] to staging[]              |
| `removeFromStaging(fileId)` | Move back from staging[] to suggestions[]         |
| `renderStaging()`           | Render staging cards + key coverage + save button |
| `clearStaging()`            | Empty staging (on new tag selection)              |
| `saveStagingAsPlaylist()`   | POST /api/playlists/local → toast                 |
| `getCoveredKeys()`          | Return sorted unique Camelot keys from staging    |

### Key coverage

Show which keys (1m–12m, 1d–12d) are present in staging. Gaps visible.

### Behavior

- Clicking "Add" on a suggestion moves it from suggestions list to staging area
- "Find Similar" is now a **Refine** button when staging is non-empty:
  - Uses `seedFileIds` = all original seed file IDs + all staging file IDs
  - Returns fresh suggestions based on the expanded seed pool
  - Staging tracks are NOT removed — they persist as seeds for the next round
- "Remove" returns track from staging to suggestions
- "Save as Playlist" opens name input → POST /api/playlists/local → clears staging
- Staging cleared on new tag selection
- Staging persists across "Find Similar" / "Load More" on same tag

### Why this is powerful

```
Round 1: Collapse-capital (6 seeds) → 10 suggestions → pick 3 → staging
Round 2: 6 + 3 = 9 seeds → 10 suggestions → pick 2 more → staging
Round 3: 6 + 5 = 11 seeds → 10 suggestions → pick 1 → staging
 → Saves as "collapse-capital-v2" playlist (6 seeds + 6 staging = 12 tracks)
```

Each round brings you closer to the musical space you're exploring.

### Files: `frontend/pages/digging.js` + `frontend/style.css`

### Acceptance Criteria

- [ ] "Add" moves suggestion to staging, removes from suggestion list
- [ ] "Remove" returns track to suggestions
- [ ] Key coverage indicator shows covered Camelot keys
- [ ] "Save as Playlist" → name input → POST → success toast
- [ ] Staging cleared on new tag / new search
- [ ] Staging survives multiple "Load More" calls
- [ ] No regressions: seeds, suggestions, audio, config all still work

---

## Plan: audio-player-waveform

**Status**: done ✅
**Branch**: `feat/audio-player-waveform`
**Ready for review**: yes
**Depends on**: `feat/digging-frontend`
**Migration needed**: no

### Description

Replace the basic play/pause button in suggestion and staging cards with a mini audio player featuring a seekable progress bar and waveform visualization. Waveform is rendered client-side using Web Audio API (no backend changes).

### Design

Each suggestion/staging card gets a mini player:

```
▶ ████████████░░░░░░░░░░  1:23 / 3:24
   ▂▃▄▅▆▇██▇▆▅▄▃▂▁▁▂▃▄▅▆▇██▇▆▅▄▃▂
```

### How it works

1. Click ▶ → `<audio>` plays (existing behavior)
2. On first play, fetch audio as ArrayBuffer, decode via `AudioContext.decodeAudioData()`
3. Downsample PCM data to ~200 peak bars, draw on `<canvas>`
4. Progress shown as colored fill on top of waveform
5. Click anywhere on waveform/progress to seek
6. Only one audio at a time (existing behavior)

### Key functions

| Function                                                                      | Purpose                                          |
| ----------------------------------------------------------------------------- | ------------------------------------------------ |
| `loadWaveform(fileId)`                                                        | Fetch + decode audio, compute peaks, draw canvas |
| `drawWaveform(fileId, peaks, progress)`                                       | Render waveform bars with progress fill          |
| `wireWaveformSeek()`                                                          | Click/drag handler for seeking                   |
| `setupProgressUpdates()`                                                      | setInterval to update progress + redraw waveform |
| Audio format: fetch full stream, decode PCM, store in Map (cached per fileId) |

### Player rendering (replaces current `.sugg-player`)

```html
<div class="audio-player" data-file-id="${s.fileId}">
  <button class="btn-play" data-file-id="${s.fileId}"><i class="fas fa-play"></i></button>
  <div class="waveform-wrap" data-file-id="${s.fileId}">
    <canvas
      class="waveform-canvas"
      data-file-id="${s.fileId}"
      width="200"
      height="40"
    ></canvas>
    <div class="waveform-progress" data-file-id="${s.fileId}"></div>
  </div>
  <span class="time-display" data-file-id="${s.fileId}"
    >0:00 / ${formatTime(duration)}</span
  >
  <audio class="audio-el" data-file-id="${s.fileId}" preload="none">
    <source src="/api/files/${s.fileId}/stream" />
  </audio>
</div>
```

### Files: `frontend/pages/digging.js` + `frontend/style.css` (no backend)

### Acceptance Criteria

- [ ] ▶ fetches audio, decodes, renders waveform as peak bars
- [ ] Progress shown as colored fill over waveform
- [ ] Click on waveform/progress bar seeks to position
- [ ] Only one audio at a time
- [ ] Waveform cached per fileId (no re-fetch on re-play)
- [ ] Waveform works in both suggestion cards and staging cards
- [ ] Time display shows current/total
- [ ] No regressions: Add, Remove, Refine, Save, key coverage still work
- [ ] Backend unchanged
