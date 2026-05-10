# Momo's Music Manager — Agent Guidance

> **Last Updated**: 2026-06-10 — New workflow: feature branches + additive migrations

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

1. **`main` is always clean** — never commit directly to `main`. Every change goes through a feature branch.
2. **Feature branches** — `feat/short-description` or `fix/short-description`, branched from `main`.
3. **Rebase > Merge** — rebase feature branch onto `main`, then fast-forward merge. Linear history.
4. **Additive migrations** — never modify `001_initial_schema.sql`. New schema changes = `002_description.sql`, `003_description.sql`, etc. `sqlx::migrate!()` handles ordering.
5. **Plan first** — every task starts with a Plan entry in Section 2 of this file. User reviews the plan, then agents are spawned.

### Architecture

6. **Schema**: 12 tables/views — `tag_categories`, `tags`, `service_tracks`, `service_playlists`, `service_playlist_tracks`, `files`, `service_config`, `folders`, `subscriptions`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities` (plus views: `unified_tracks`, `v_file_track_link`, `v_tag_playlist`, `v_file_tags`, `v_subscriptions`, `v_tag_categories`, `v_tags_with_categories`)
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
| `digging.html`     | (standalone HTML)                   | Curator/session-builder page       |

---

## Docs

- `docs/ARCHITECTURE.md` — System design
- `docs/DECISIONS.md` — ADRs
- `docs/COMMENT_SYSTEM.md` — Comment format spec
- `docs/TASK_MANAGER.md` — Task manager details
- `docs/FRONTEND_BUILD_PLAN.md` — SPA migration history (mostly historical, some details outdated)
- `docs/FRONTEND_NEXT_PLAN.md` — Remaining frontend work (partially done)

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
- [ ] Backend compiles (`cargo build`)
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
- [ ] Backend compiles (`cargo build`)

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
- [ ] Backend compiles (`cargo build`)
- [ ] Test with `curl` first

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

**Status**: proposed
**Branch**: `feat/files-bulk-comments`
**Ready for review**: no
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
- [ ] Backend compiles (`cargo build`)
- [ ] Tested with curl

---

## Plan: spotify-rate-limit-retry

**Status**: proposed
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
- [ ] Backend compiles (`cargo build`)
- [ ] Batch sync runs without `429 Too Many Requests` failures (tested against Spotify)

---

## Plan: tag-parents

**Status**: done ✅
**Branch**: `feat/tag-parents`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `003_tag_parents.sql`

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
