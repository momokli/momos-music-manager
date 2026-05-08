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

**Status**: proposed
**Branch**: `feat/tags-filter-box`
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

**Status**: proposed
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

**Status**: proposed
**Branch**: `feat/column-resize-pixel`
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

**Status**: proposed
**Branch**: `feat/import-export-ui`
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

**Status**: proposed
**Branch**: `feat/server-side-filtering`
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

## Completed Archives

- **Phase 1** (Files Page) — Reference blueprint complete ✅
- **Phase 2** (Tracks Page) — Complete, awaiting server-side filter fix ✅
- **Phase 3** (Playlists Page) — Complete with tag lookup optimization ✅
- **Phase 4** (Tags Page) — Complete ✅
- **Phase 8.1** (Playlists filter box) — Complete ✅
