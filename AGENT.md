# Momo's Music Manager — Agent Guidance

> **Last Updated**: 2026-06-08 — v0.8.0

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

6. **Schema**: 14 tables — `tag_categories`, `tags`, `service_tracks`, `service_playlists`, `service_playlist_tracks`, `files`, `service_config`, `folders`, `subscriptions`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities`, `tag_parents`, `file_resolved_tags` (plus views: `unified_tracks`, `v_file_track_link`, `v_tag_playlist`, `v_file_tags`, `v_subscriptions`, `v_tag_categories`, `v_tags_with_categories`, `v_resolved_tags`, `v_file_resolved_tags`)
7. **Separate Types**: `File` (local files with BPM/Key) vs `ServiceTrack` (service entries, no BPM/Key) — linked via `v_file_track_link` view
8. **Tags = Playlists**: Via name matching (case-insensitive). Setlist is default category.
9. **Comment Format**: `[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}` — e.g. `[PMV] build jazzy warehouse sp:xxx`
10. **Service IDs**: Direct columns on `files` (`spotify_id`, `soundcloud_id`, `youtube_id`)
11. **Key Matching**: Rust-only (Camelot wheel, no DB table)
12. **Task Manager**: In-memory task tracking — 4 operation types (ServiceSync, WriteComment, RecomputeEmbeddings, ScanFolder)
13. **Sync State**: In-memory `TaskManager` — tasks auto-pruned 5 min after completion
14. **Config Priority** (highest wins): Env vars > `~/.config/momos-music-manager/config.toml` > built-in defaults
15. **Server-Side Filtering**: All filters must be server-side on paginated pages. Client-side filtering after pagination breaks page counts.
16. **Testing**: 645 tests (375 unit + 18 binary + 252 integration). Every endpoint tested, every query param covered. 59.28% line coverage target (goal: ≥75%). See `tests/README.md`.

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

## Agent Workflow: Before You Code

**Always get ground truth from the codebase — don't rely on this document alone.**
Documents rot; the filesystem and compiler don't.

### Quick Orientation (run these first)

```bash
# 1. Establish build baseline (catches compilation errors immediately)
cargo build 2>&1 | tail -5

# 2. Get the CURRENT database schema (one command, no migration archaeology)
rm -f /tmp/agent_app.db
DATABASE_URL=sqlite:/tmp/agent_app.db cargo run -- serve --host 127.0.0.1 --port 3001 &
sleep 2
sqlite3 /tmp/agent_app.db ".schema" | head -200
kill %1 2>/dev/null; rm -f /tmp/agent_app.db

# 3. List actual source modules (not what this doc says)
ls src/*.rs src/*/mod.rs | sort

# 4. List actual frontend pages (the PAGE_MAP in app.js is authoritative)
ls frontend/pages/*.js | sort

# 5. Check current git branch + dirty state
git branch --show-current && git status --short | head -20
```

### Schema Rules

- **Never reconstruct the schema from migration files.** Migrations 001–011 have
  overlapping view definitions, ALTER TABLEs, and DROP+RECREATE cycles. Reading them
  sequentially is error-prone. Always query the live DB or do the dry-run above.
- The `sqlite3 app.db ".schema"` output IS the canonical schema. Trust it over any
  plan's embedded SQL snippets.
- The schema includes views (`v_file_track_link`, `v_file_tags`, `v_file_resolved_tags`,
  `v_tag_playlist`, `v_tag_file_counts`, `v_resolved_tags`, `v_tag_categories`,
  `v_tags_with_categories`, `unified_tracks`, `v_subscriptions`, `v_playlist_tag_category`).
  Query `.schema` or `.tables` to see them all.

### Frontend Rules

- The `PAGE_MAP` object in `frontend/app.js` is the **authoritative** list of pages.
  If a page isn't there, it won't load. The table below is a convenience reference only.
- The `NAV_SECTIONS` and `TOOLS_ITEMS` arrays in `frontend/shared/nav.js` control
  what shows in the nav bar. New pages must be added to both app.js and nav.js.

---

### Testing

Testing is **not optional**. Every feature must be validated at both the backend
(API + logic) and frontend (DOM + user interaction) levels before delivery.

#### Backend Testing (`cargo test`)

- **`cargo test` is the single source of truth for backend behavior.** Every API
  endpoint, every filter parameter, every query variation must have a
  corresponding integration test.
- **Every plan that adds or modifies an API endpoint or filter parameter MUST
  include "add/update integration test" as an acceptance criterion.**
- **Coverage threshold**: ≥75% line coverage (via `cargo llvm-cov`). Run
  `cargo llvm-cov --fail-under-lines 75` before merging.
- **659 tests**: 379 lib + 18 bin + 262 integration. See `tests/README.md` for
  the full breakdown.
- **Unit tests** go in `#[cfg(test)] mod tests` within the source file for pure
  functions. Integration tests go in `tests/api_*.rs` files.
- **Integration tests use a self-contained SQLite DB.** Each test creates a
  fresh in-memory DB, runs all migrations, seeds hand-crafted data, hits the API,
  and asserts exact results (row counts, field values, response shapes).
- **Test files mirror API structure.** `tests/api_files.rs` → `/api/files*`,
  `tests/api_tracks.rs` → `/api/tracks*`, etc.
- **Migration integrity is tested.** A dedicated test creates a fresh DB and
  runs all migrations end-to-end.

#### Frontend Testing (`npx playwright test`)

- **`npx playwright test` is the single source of truth for frontend behavior.**
  Every new page, feature, filter, or UI interaction MUST include Playwright
  acceptance tests.
- **Every plan that modifies a frontend page or adds a new one MUST include
  "add/update Playwright tests" as an acceptance criterion.**
- Test files live in `frontend/tests/` — one file per page:
  `smoke.spec.js`, `files.spec.js`, `tracks.spec.js`, etc.
- **Tests are self-contained**: Playwright auto-starts the Rust server with an
  isolated test DB (`test-playwright.db`), runs tests, then kills the server.
  One command: `cd frontend && npx playwright test`.
- **Every test seeds its own data** via `POST /api/testing/seed` in `beforeEach`.
  Available scenarios: `basic`, `files_filter`, `digging`, `wav_variants`.
  The seed endpoint guarantees deterministic state — no flaky tests.
- **New seed scenarios** needed by tests go in `src/db/testing.rs` and are
  registered in the `testing_seed_handler` match block in
  `src/api/infrastructure.rs`.
- **Selectors**: use `#id` or `[data-*]` attributes from the page's HTML.
  Never rely on CSS class order or nth-child selectors — those change.
- **Page errors**: every smoke test MUST listen for `pageerror` events and
  assert `errors.length === 0`. Catches `ReferenceError`, `TypeError`, etc.

#### Agent Validation Checklist (run before declaring "done")

```bash
# 1. Backend compiles
cargo build

# 2. All backend tests pass
cargo test

# 3. All frontend tests pass (auto-starts server, seeds DB, runs tests, kills server)
cd frontend && npx playwright test
```

#### When to Extend Tests

| Change                     | Action                                                                   |
| -------------------------- | ------------------------------------------------------------------------ |
| New API endpoint           | Add integration test in `tests/api_*.rs`                                 |
| New API filter param       | Add test case in existing test file                                      |
| New frontend page          | Create `frontend/tests/{page}.spec.js` + register in `app.js` + `nav.js` |
| New frontend filter/button | Add Playwright test that clicks it and asserts result                    |
| Changed frontend behavior  | Update existing Playwright test to match new behavior                    |
| New seed data shape needed | Add scenario to `src/db/testing.rs` + register in seed handler           |
| Bug fix                    | Write a failing test FIRST, then fix. The test proves the bug is dead.   |

## Dev Commands

```bash
# Start backend
cargo run -- serve --host 127.0.0.1 --port 3000

# Start frontend (separate terminal)
cd frontend && python3 -m http.server 8000

# Kill everything
./kill-all.sh

# Run integration tests (self-contained, no server needed)
cargo test

# Run integration tests with output (see println! / dbg!)
cargo test -- --nocapture

# Run a specific test file
cargo test --test api_files

# Run a single test by name
cargo test files_filter_is_local_true

# Run all frontend Playwright tests (auto-starts server, seeds DB, runs tests)
cd frontend && npx playwright test

# Run tests for a specific page
cd frontend && npx playwright test tests/files.spec.js

# Run with browser visible (debugging)
cd frontend && npx playwright test --headed

# Debug a failed test (interactive trace viewer)
cd frontend && npx playwright show-trace test-results/.../trace.zip

# Smoke test against a running server (legacy, light use only)
./test.sh

# Scan single file for metadata debugging
cargo run -- scan-file /path/to/file.stem.m4a

# Delete old DBs + restart (only when messing with migrations during dev)
rm -f app.db && cargo run -- serve --host 127.0.0.1 --port 3000

# Dump DB to JSON
cargo run -- dump

# Restore DB from JSON dump
cargo run -- restore

# Dump current schema from a live DB (canonical truth, beats reading migrations)
sqlite3 app.db ".schema"

# Quick schema overview (tables + indexes, no view SQL)
sqlite3 app.db ".schema" | grep -E "CREATE TABLE|CREATE INDEX"

# List all views in the DB
sqlite3 app.db ".tables" | tr ' ' '\n' | grep '^v_'

# Import Traktor collection.nml
cargo run -- serve  # then use the Traktor import page in the frontend
```

---

## Current Migration Map (001–018)

Use this as a quick index. For actual SQL, query the live DB with `sqlite3 app.db ".schema"`.

| File                              | What it does                                                                                                                                         |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `001_initial_schema.sql`          | 13 tables + 9 views + seed data (tag categories, phase tags, energy levels)                                                                          |
| `002_playlist_fetch_tracking.sql` | Adds `last_fetched_at` + `remote_track_count` to `service_playlists`; creates `tag_parents` table + `v_resolved_tags` + `v_file_resolved_tags` views |
| `003_remote_unique_count.sql`     | Adds `remote_unique_count` to `service_playlists`                                                                                                    |
| `004_unique_tags_nocase.sql`      | Rebuilds `tags` with `UNIQUE COLLATE NOCASE`, deduplicates case-variant tags, remaps FKs, recreates all dependent views                              |
| `005_v_playlist_tag_category.sql` | Creates `v_playlist_tag_category` view for category-ID-based playlist filtering                                                                      |
| `006_local_service.sql`           | Adds `'local'` to `service_tracks` CHECK constraint, updates `v_file_track_link` for local service matching, recreates dependent views               |
| `007_playlist_snapshot.sql`       | Adds `snapshot_id` to `service_playlists` for global poller change detection                                                                         |
| `008_playlist_track_archive.sql`  | Soft-delete `deleted_at` on `service_playlist_tracks` + `archive_deleted` toggle on `service_playlists`                                              |
| `009_file_lifecycle.sql`          | `file_locations` table, `followed` on tags, `source_of` on files, folder backup config                                                               |
| `010_auto_backup.sql`             | `auto_backup` column on `folders`                                                                                                                    |
| `011_file_resolved_tags.sql`      | Materialized `file_resolved_tags` table, missing indexes for query performance                                                                       |
| `012_wav_stem_type.sql`           | `stem_type` column on `files` for WAV source component tracking                                                                                      |
| `013_backup_discovery.sql`        | `last_verified_local` on `files` for local presence tracking                                                                                         |
| `014_v_track_tags.sql`            | `v_track_tags` view — track→tag resolution via playlist name matching                                                                                |
| `015_track_resolved_tags.sql`     | Materialized `track_resolved_tags` table for track-level tag query performance                                                                       |
| `016_backpack_rename.sql`         | Renamed `tags.followed` to `tags.backpack`                                                                                                           |
| `017_tag_bundles.sql`             | New `tag_bundles` table for bundle/curation tags — aggregate multiple member tags into one                                                           |
| `018_canonical_playlist_id.sql`   | `canonical_playlist_id` on `service_playlists` for multi-provider playlist linking (daily-tagging-queue push-to-spotify)                             |

---

## Important Gotchas

- **Migrations are additive** — never edit `001_initial_schema.sql`. Create a new migration file instead.
- **To reset a dirty migration state**: delete `app.db` and re-run — all 16 migrations run sequentially from scratch.
- **Schema truth is in the DB, not the migration files.** Migrations recreate views and tables, so earlier files contain stale SQL. Always query `sqlite3 app.db ".schema"` for the current schema.
- **Frontend is an SPA** — modular vanilla JS with ES modules in `frontend/`. Hash-based router (`app.js`), shared modules in `shared/`, pages in `pages/`. Serve embedded via `rust-embed`, no separate dev server needed.
- **`digging.html`** is a standalone HTML page (not part of the SPA) for the digging/curation workflow.
- **The `#digging` SPA page** (`frontend/pages/digging.js`) is a different interface — the full-featured Digging Curator with multi-seed suggestions and audio players.
- **Playlist subscriptions** poll every 30s in the background — managed in `poller.rs`.
- **Global playlist poller** checks ALL Spotify playlists every 15 min via snapshot-based detection — managed in `global_poller.rs`.
- **No SoundCloud/YouTube OAuth yet** — framework is ready, actual flow not implemented.
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

> Run `ls src/*.rs src/*/mod.rs | sort` for the authoritative list.

```
src/
├── main.rs              # CLI, router, server start
├── api.rs               # All API endpoints
├── audio_extensions.rs  # AudioExtension enum
├── comment.rs           # Comment parsing/generation
├── config.rs            # Config.toml + env var loading
├── db.rs                # Database queries, scanning, comment computation
├── deemix/              # Deemix download integration
│   ├── mod.rs
│   ├── cli.rs           # CLI subcommands for deemix
│   ├── client.rs        # HTTP client for deemix-pyweb API
│   └── models.rs        # Deemix queue/status types
├── digging.rs           # Curator/session-builder for track discovery
├── dump.rs              # DB dump/restore (JSON)
├── embeddings.rs        # Semantic tag embeddings (candle/ML)
├── global_poller.rs     # Global playlist poller (all Spotify playlists, snapshot-based)
├── launch_agent.rs      # macOS launch agent integration
├── poller.rs            # Playlist subscription background poller
├── scan_cache.rs        # File scan result caching
├── spotify/
│   ├── mod.rs
│   ├── client.rs        # Spotify OAuth client
│   ├── models.rs        # PlaylistInfo, TrackInfo, AudioFeatures
│   └── sync_worker.rs   # Background sync worker
├── tasks/
│   └── mod.rs           # TaskManager (generic) + task workers
├── traktor.rs           # Traktor collection.nml parser
└── watch.rs             # Folder watcher (auto-started on boot, polls active folders)
```

---

## Frontend Pages (SPA)

> **Authoritative source**: `PAGE_MAP` in `frontend/app.js`. Run `ls frontend/pages/*.js | sort` to verify.
> Nav visibility is controlled by `NAV_SECTIONS` + `TOOLS_ITEMS` in `frontend/shared/nav.js`.

| Route              | Module                              | Nav Section | Description                                                      |
| ------------------ | ----------------------------------- | ----------- | ---------------------------------------------------------------- |
| `#dashboard`       | `frontend/pages/dashboard.js`       | Overview    | Stats cards + recent activity                                    |
| `#files`           | `frontend/pages/files.js`           | Library     | Local files table + comment status                               |
| `#tracks`          | `frontend/pages/tracks.js`          | Library     | Service tracks table                                             |
| `#playlists`       | `frontend/pages/playlists.js`       | Library     | All playlists                                                    |
| `#tags`            | `frontend/pages/tags.js`            | Library     | Tags table                                                       |
| `#tag-categories`  | `frontend/pages/tag-categories.js`  | Library     | Tag categories                                                   |
| `#services`        | `frontend/pages/services.js`        | Services    | Service status/config                                            |
| `#tasks`           | `frontend/pages/tasks.js`           | Services    | Task manager UI                                                  |
| `#folders`         | `frontend/pages/folders.js`         | Services    | Folder management                                                |
| `#deemix-queue`    | `frontend/pages/deemix-queue.js`    | Services    | Deemix download queue                                            |
| `#traktor`         | `frontend/pages/traktor-import.js`  | Services    | Traktor collection import                                        |
| `#tag-curation`    | `frontend/pages/tag-curation.js`    | Tools       | Tag parent curation workflow                                     |
| `#auto-categorize` | `frontend/pages/auto-categorize.js` | Tools       | AI tag categorization wizard                                     |
| `#digging`         | `frontend/pages/digging.js`         | Tools       | Digging Curator (multi-seed suggestions, audio players, staging) |
| `#data`            | `frontend/pages/data.js`            | Tools       | Import/export database                                           |
| `#key-comparison`  | `frontend/pages/key-comparison.js`  | Tools       | Traktor vs Spotify BPM/Key comparison                            |
| `#track-detail`    | `frontend/pages/track-detail.js`    | (linked)    | Single track metadata detail                                     |
| `#file-detail`     | `frontend/pages/file-detail.js`     | (linked)    | Single file metadata detail                                      |
| `digging.html`     | (standalone HTML)                   | —           | Legacy curator/session-builder page                              |

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
3. Run `cargo build` — must pass
4. Run `cargo test` — all 659+ tests must pass
5. Run `cd frontend && npx playwright test` — all frontend tests must pass
6. If you added a new endpoint, verify with `curl` first

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

## Plan: spotify-audio-features

**Status**: done ✅
**Branch**: `feat/spotify-audio-features-comparison`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `008_spotify_audio_features.sql`

### Description

Fetch Spotify Audio Features (tempo/BPM, key, mode, danceability, energy, valence, acousticness, instrumentalness, liveness, speechiness, loudness, time_signature) during track sync. Convert Spotify's pitch-class+mode notation to Camelot wheel notation for direct comparison with Traktor's key annotations. Add a comparison API endpoint showing Traktor vs Spotify BPM/Key side-by-side with match/mismatch summary.

### Files modified

- `migrations/008_spotify_audio_features.sql` — new migration: 12 audio features columns on `service_tracks`
- `src/spotify/models.rs` — `AudioFeatures` struct, `spotify_key_to_camelot()` conversion, extended `TrackInfo`
- `src/spotify/client.rs` — `get_audio_features_batch()` method (batches of 100)
- `src/spotify/sync_worker.rs` — `update_audio_features_batch()` method; injected audio features fetch after track sync in `sync_tracks_for_playlist`
- `src/db.rs` — `ServiceTrack` extended with 12 audio features columns; `update_track_audio_features()`; `KeyComparisonRow`/`KeyComparisonSummary` types; `get_key_comparison()`
- `src/api.rs` — `GET /api/files/key-comparison?tag=X&limit=N` endpoint
- `src/global_poller.rs`, `src/poller.rs` — added `audio_features: None` to episode `TrackInfo` constructors
- `frontend/pages/key-comparison.js` — new comparison page with tag typeahead, summary cards, sortable table
- `frontend/pages/track-detail.js` — new detail page showing all metadata for a single track
- `frontend/app.js` — registered `"key-comparison"` and `"track-detail"` routes
- `frontend/shared/nav.js` — added "Key Comparison" to TOOLS_ITEMS
- `frontend/style.css` — `.kc-*` and `.detail-*` styles

### Acceptance Criteria

- [x] Spotify sync fetches audio features in batches of 100 after track storage
- [x] `spotify_key_to_camelot()` maps all 24 keys (12 minor + 12 major)
- [x] All 12 audio features stored on `service_tracks` (tempo, key_raw, mode, key_camelot, danceability, energy, valence, acousticness, instrumentalness, liveness, speechiness, loudness, time_signature)
- [x] `GET /api/files/key-comparison?tag=X` returns side-by-side Traktor vs Spotify BPM/Key
- [x] Summary shows match/mismatch counts for BPM (±1 tolerance) and Key (exact Camelot match)
- [x] Works for files with no Spotify link (skipped gracefully)
- [x] Skip audio features in replay mode (cache mode)
- [x] Audio features fetch is non-fatal — tracks are stored even if features fail
- [x] Web UI at `#key-comparison` with tag typeahead, summary cards, sortable table, ✓/✗ indicators
- [x] Backend compiles (`cargo build`)
- [ ] Fresh DB: migrations 001→008 run cleanly
- [ ] Test with live data: sync a playlist, open `#key-comparison`, pick a tag

---

## Plan: multi-provider-playlists

**Status**: proposed
**Branch**: `feat/multi-provider-playlists`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `017_canonical_playlist_id.sql`

### Description

Make playlists multi-provider: a logical playlist can exist on multiple services
simultaneously (local, Spotify, future SoundCloud). Add `canonical_playlist_id`
to tie provider rows into a single logical entity. Add `POST /api/playlists/{id}/push-to-spotify`
to mirror a local playlist to Spotify. Add write OAuth scopes to the Spotify client.

### Why `canonical_playlist_id` instead of `spotify_playlist_id`

A playlist isn't "owned" by one service. It's a named collection of tracks that can
be present on multiple providers. Tying two rows together via a shared UUID models
this as a peer relationship rather than a one-way pointer:

```
canonical_playlist_id: "a1b2c3d4-..."
├── service='local'    playlist_id='local-a1b2c3d4'
└── service='spotify'  playlist_id='37i9dQZEVXcJ...'
```

This unlocks: (a) a playlist can be local AND on Spotify, (b) pushing to new
providers adds rows without schema changes, (c) future two-way sync becomes
straightforward — compare tracks across canonical groups.

### Migration 017

```sql
ALTER TABLE service_playlists ADD COLUMN canonical_playlist_id TEXT;
CREATE INDEX IF NOT EXISTS idx_sp_canonical ON service_playlists(canonical_playlist_id);
```

### Backend Changes

#### 1. OAuth scopes — add write permissions

**Files**: `src/spotify/client.rs` (~line 77), `src/api/services.rs` (3 locations),
`src/api/websocket.rs` (1 location)

Add `playlist-modify-public` and `playlist-modify-private` to all `scopes!()` invocations.
Existing tokens without these scopes will get a 403 from Spotify on write operations
(rspotify's token refresh won't silently add scopes). The handler catches this and
returns: "Spotify token needs write permissions. Re-authenticate on the Services page."

#### 2. Spotify client — new write methods (`src/spotify/client.rs`)

```rust
/// Create a new Spotify playlist. Returns (playlist_id, spotify_url).
pub async fn create_playlist(
    &self, user_id: &str, name: &str, public: bool, description: Option<&str>,
) -> Result<(String, String)>

/// Add tracks in batches of 100 (Spotify API limit).
pub async fn add_tracks_to_playlist(
    &self, playlist_id: &str, track_ids: &[String],
) -> Result<()>
```

Both use rspotify's `OAuthClient` trait (`user_playlist_create`, `playlist_add_items`).

#### 3. API endpoint (`src/api/playlists.rs`)

**Route**: `.route("/api/playlists/{id}/push-to-spotify", post(push_to_spotify_handler))`

**Request**:

```json
{ "name": "optional-override", "public": false }
```

**Response**:

```json
{
  "data": {
    "spotifyPlaylistId": "37i9dQZ...",
    "spotifyUrl": "https://open.spotify.com/playlist/37i9dQZ...",
    "tracksPushed": 12,
    "tracksSkipped": 3,
    "skippedReasons": { "no-spotify-link": 3 }
  }
}
```

**Handler logic**:

1. Fetch the playlist + verify it exists
2. If `canonical_playlist_id` is already set and a Spotify row exists → 409 (already pushed)
3. Resolve Spotify track IDs: `SELECT st.service_id FROM service_playlist_tracks spt JOIN service_tracks st ON st.id = spt.track_id AND st.service = 'spotify' WHERE spt.playlist_id = ?`
4. Skip tracks without Spotify links, count them
5. Create `SpotifyClient::from_stored_tokens()`
   - If 403 with "insufficient scopes" → return clear error: "Re-authenticate on Services page"
6. `GET /v1/me` → get current user ID
7. `POST /v1/users/{user_id}/playlists` → create Spotify playlist
8. `POST /v1/playlists/{id}/tracks` in batches of 100
9. Generate a canonical ID: use the local row's existing `canonical_playlist_id` if set, otherwise generate a new UUID
10. If the local row had `canonical_playlist_id = NULL`, UPDATE it to the new UUID
11. INSERT new `service_playlists` row with `service='spotify', playlist_id=<spotify_id>, canonical_playlist_id=<uuid>`
12. Return result

#### 4. DB helpers (`src/db/playlists.rs`)

```rust
/// Get Spotify track IDs for a playlist. Returns Vec<(service_track_id, spotify_id)>.
pub async fn get_playlist_spotify_track_ids(
    pool: &Pool<Sqlite>, playlist_id: i64,
) -> Result<Vec<(i64, String)>>
```

#### 5. `Playlist` struct — add `canonical_playlist_id` + `services`

Add `canonical_playlist_id: Option<String>` and `services: Option<String>` to the API response.
Both fields use `#[sqlx(default)]` to avoid runtime errors when other queries use `query_as::<Playlist>`
without these columns. The playlist list handler continues to return one row per
`service_playlists` row (no dedup in v1 — that's a separate frontend concern).
The `services` field is computed with a subquery:

```sql
SELECT sp.*, ...
  COALESCE(
    (SELECT GROUP_CONCAT(DISTINCT sp2.service) FROM service_playlists sp2
     WHERE sp2.canonical_playlist_id = sp.canonical_playlist_id),
    sp.service
  ) as services
FROM service_playlists sp
```

`COALESCE` ensures: canonical group → `"spotify,local"`, no canonical → row's own service.

### Frontend Changes

#### 1. Playlists page (`frontend/pages/playlists.js`)

- **Service badges**: add a `services` column to `PLAYLISTS_COLUMNS` — renders colored badges from the `services` field: `[local] [spotify]`
- **Push button**: extend the existing `sync` cell renderer. Shown when `service='local'` and Spotify is not in `services`
  - Click → small dialog: optional name override + public/private toggle + track count preview
  - On success → toast with clickable Spotify URL, row refreshes with new badges
  - On 403 → toast: "Spotify token needs write permissions. Re-authenticate on the Services page."
- **Open in Spotify button**: also in `sync` cell, shown when `services` includes `spotify` —
  links to `https://open.spotify.com/playlist/{id}`

#### 2. Digging page (`frontend/pages/digging.js`)

- After "Save as Playlist", add a checkbox: "Also push to Spotify"
- When checked, chains the create-local → push-to-spotify calls

#### 3. CSS (`frontend/style.css`)

- `.service-badges` — inline flex row of small colored service badges
- `.push-spotify-dialog` — tiny modal for name/public toggle

### What happens after push

1. New `service_playlists` row with `service='spotify'` exists in DB
2. **Global poller** picks up the new Spotify playlist on its next cycle, syncs tracks
3. **Tag matching** works automatically — same playlist name → same tag
4. **Subscription poller** can subscribe to the Spotify row if user wants live sync
5. Files linked via ISRC stay linked — the new Spotify tracks match existing files

### Why not dedup playlist list yet

Deduping `service_playlists` into one entry per canonical group requires frontend
changes to show per-service actions (push/delete/open) within a unified card.
That's a separate UI plan. V1 keeps the list flat — each service row is a separate
list entry, grouped visually by the shared `canonical_playlist_id`. The `services`
field tells the frontend which badges to show.

### Files to modify

| File                                       | Change                                                                             |
| ------------------------------------------ | ---------------------------------------------------------------------------------- |
| `migrations/017_canonical_playlist_id.sql` | New migration                                                                      |
| `src/spotify/client.rs`                    | Write scopes + `create_playlist()` + `add_tracks_to_playlist()`                    |
| `src/api/services.rs`                      | Write scopes in 3 OAuth locations                                                  |
| `src/api/websocket.rs`                     | Write scopes in OAuth                                                              |
| `src/api/playlists.rs`                     | `push_to_spotify_handler` + route + `Playlist` struct fields + `services` subquery |
| `src/db/playlists.rs`                      | `get_playlist_spotify_track_ids()` helper                                          |
| `frontend/pages/playlists.js`              | Service badges, Push button, Open in Spotify button                                |
| `frontend/pages/digging.js`                | "Also push to Spotify" checkbox                                                    |
| `frontend/style.css`                       | `.service-badges`, `.push-spotify-dialog`                                          |

### Acceptance Criteria

- [ ] Migration 017 runs cleanly on fresh DB (001→017)
- [ ] Migration 017 runs cleanly on existing DB with data
- [ ] `canonical_playlist_id` column added + indexed
- [ ] Write scopes present in all 5 OAuth scope locations
- [ ] `create_playlist()` creates a Spotify playlist and returns ID + URL
- [ ] `add_tracks_to_playlist()` adds tracks in batches of 100
- [ ] `POST /api/playlists/{id}/push-to-spotify` creates Spotify playlist for a local playlist
- [ ] All tracks with Spotify links are added; tracks without links counted as skipped
- [ ] New `service_playlists` row inserted with `service='spotify'` + shared `canonical_playlist_id`
- [ ] Local row gets `canonical_playlist_id` assigned if it was NULL (first push)
- [ ] 400 when playlist has zero Spotify-linked tracks
- [ ] 403 when token lacks write scopes — error message mentions re-auth
- [ ] 409 when a Spotify row already exists for this canonical group
- [ ] `GET /api/playlists` includes `canonicalPlaylistId` + `services` in response
- [ ] `services` subquery returns correct comma-separated list for canonical groups
- [ ] Frontend: Push button on local playlists, dialog with name + public/private
- [ ] Frontend: Success toast with clickable Spotify URL
- [ ] Frontend: Service badges rendered from `services` field
- [ ] Frontend: Digging "Also push to Spotify" checkbox works
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing tests + new ones for push handler)

### Out of scope (v2)

- **Deduped playlist list**: One card per canonical group with per-service action buttons
- **Two-way sync**: Comparing tracks across canonical group members, adding missing ones
- **Remove from Spotify**: Deleting a Spotify playlist when the local playlist is deleted
- **Spotify → local pull**: Creating a local mirror from an existing Spotify playlist
- **Pushing to SoundCloud / YouTube**: Same pattern, different API clients
- **Playlist image**: Setting custom cover art on Spotify
- **Track ordering**: Preserving local playlist order on Spotify

---

## Plan: tag-bundles

**Status**: done ✅
**Branch**: `feat/tag-bundles`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `017_tag_bundles.sql`

### Description

New concept: a "bundle tag" aggregates multiple member tags. Files with any member tag also get the bundle tag. This is ADDITIVE (members stay visible, bundle appears additionally). Used so the user can filter by a single tag in Traktor + add a BPM range on top — solving Traktor's smartlist limitation (can't do OR-of-multiple-tags AND BPM).

Unlike `tag_parents` (which does SUBSTITUTION — Setlist tag replaced by its P/M/V/E parents for comment writing), tag bundles are purely aggregative and work for any tag category.

### New table

```sql
CREATE TABLE tag_bundles (
    id INTEGER PRIMARY KEY,
    bundle_tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    member_tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE (bundle_tag_id, member_tag_id)
);
```

### Resolution

Extended `refresh_file_resolved_tags()` and `refresh_track_resolved_tags()` with transitive bundle resolution loop (fixed-point iteration). After the existing view-based INSERT, repeatedly finds bundle tags whose members are present in resolved tags, inserts the bundle tag too. Repeats until no new rows (handles multi-level: A→B→C). 20-iteration safety limit.

### Backend endpoints

- `GET /api/tags/bundles?search=X` — list bundle tags with member counts
- `GET /api/tags/{id}/bundle-members` — member tags of a bundle
- `PUT /api/tags/{id}/bundle-members` — set members with validation (existence, self-ref, cycle detection via DFS)
- `GET /api/tags/{id}/bundle-of` — which bundles is this tag a member of?

### Frontend

New `#tag-bundles` SPA page with two-panel layout:

- **Left**: searchable list of bundle tags with member counts
- **Right**: selected bundle → member chips with category badges, typeahead search to add, × to remove, auto-save on every change
- "New Tag" button → creates Setlist tag, opens for member assignment
- File preview section showing first 10 files with this bundle tag

### Comment output

Bundle tags are Setlist category, so they appear in the tags section of the comment automatically (via `file_resolved_tags` which now includes bundle resolution):

```
[PMV] spät afterhour schnell afterhour-jonas sp:xxx
```

In Traktor: filter comment → contains `afterhour-jonas` AND BPM 120-130 ✅

### Files created/modified

| File                             | Change                                                                                                                            |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `migrations/017_tag_bundles.sql` | New migration                                                                                                                     |
| `src/db/tags.rs`                 | 5 new functions: `get_bundle_members`, `get_bundle_of`, `check_bundle_cycle`, `set_bundle_members`, `get_bundle_tags_with_counts` |
| `src/db/playlists.rs`            | Extended `refresh_file_resolved_tags` + `refresh_track_resolved_tags` with bundle transitive closure                              |
| `src/api/tags.rs`                | 4 new handlers + 3 new routes                                                                                                     |
| `frontend/pages/tag-bundles.js`  | New SPA page (~680 lines)                                                                                                         |
| `frontend/app.js`                | Register route                                                                                                                    |
| `frontend/shared/nav.js`         | Add nav link                                                                                                                      |
| `frontend/style.css`             | Bundle page styles                                                                                                                |
| `tests/api_tags.rs`              | 6 integration tests                                                                                                               |

### Acceptance Criteria

- [x] Migration 017 runs cleanly
- [x] Bundle member CRUD works (set, get, reverse lookup)
- [x] Cycle detection rejects circular bundles
- [x] Self-reference rejected with 400
- [x] `refresh_file_resolved_tags` includes bundle resolution transitively
- [x] Multi-level bundles resolve correctly (A→B→C)
- [x] `#tag-bundles` page renders with two-panel layout
- [x] Typeahead search + chipping works for adding/removing members
- [x] Auto-save on member changes
- [x] `cargo build` passes
- [x] All 379 lib + 41 API tags integration tests pass

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

---

## Plan: playlist-track-archive

**Status**: done ✅
**Branch**: `feat/playlist-track-archive`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `008_playlist_track_archive.sql`

### Description

Instead of hard-deleting tracks from `service_playlist_tracks` when they're removed from a Spotify playlist, soft-delete them with a `deleted_at` timestamp. Add a per-playlist `archive_deleted` toggle that controls whether deleted tracks are still treated as active for tag resolution (comment writing, digging, filtering). Followed/subscribed playlists default to `archive_deleted = true` (collect all ever-added entries), personal playlists default to `archive_deleted = false` (respect deletions).

### Why

- Followed playlists like "Beatport Top 100 - Tech House" rotate tracks frequently — users want to keep all historical entries for tagging
- Personal playlists should reflect real state — when you remove a track, it should stop being tagged
- Spotify Discover Weekly / Release Radar are "followed" type — keep all ever as active
- Users can toggle per-playlist if the default doesn't match their intent

### Schema Changes

#### Migration 008 (`migrations/008_playlist_track_archive.sql`)

1. Add `deleted_at INTEGER` to `service_playlist_tracks` (NULL = active, timestamp = deleted)
2. Add `archive_deleted BOOLEAN NOT NULL DEFAULT 0` to `service_playlists`
3. Set `archive_deleted = 1` for all playlists that have a subscription (followed playlists)
4. Drop + recreate all views that depend on `service_playlist_tracks`:
   - `v_file_tags` — add filter: `AND (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)`
   - `v_file_resolved_tags` — same filter
   - `v_tag_file_counts` — already depends on `v_file_tags`, automatically updated

```sql
-- Step 1: Add deleted_at to service_playlist_tracks
ALTER TABLE service_playlist_tracks ADD COLUMN deleted_at INTEGER;

-- Step 2: Add archive_deleted to service_playlists
ALTER TABLE service_playlists ADD COLUMN archive_deleted BOOLEAN NOT NULL DEFAULT 0;

-- Step 3: Set archive_deleted = 1 for subscribed playlists
UPDATE service_playlists SET archive_deleted = 1
WHERE EXISTS (
    SELECT 1 FROM playlist_subscriptions ps
    WHERE ps.service = service_playlists.service
      AND ps.playlist_id = service_playlists.playlist_id
);

-- Step 4: Drop dependent views
DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_file_tags;

-- Step 5: Recreate v_file_tags with archive_deleted filter
CREATE VIEW v_file_tags AS
SELECT DISTINCT f.id AS file_id,
       t.id AS tag_id, t.name AS tag_name,
       t.sort_order, t.created_at,
       tc.id AS category_id, tc.name AS category_name,
       tc.is_default, tc.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 6: Recreate v_file_resolved_tags with archive_deleted filter
CREATE VIEW v_file_resolved_tags AS
SELECT DISTINCT
    f.id AS file_id,
    rt.tag_id,
    rt.tag_name,
    rt.sort_order,
    rt.created_at,
    rt.category_id,
    rt.category_name,
    rt.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN v_resolved_tags rt ON rt.source_tag_id = t.id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 7: Recreate v_tag_file_counts
CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

SELECT 'Migration 008 applied: soft-delete playlist tracks + archive_deleted toggle' as status;
```

### Backend Changes

#### 1. `src/db.rs` — `ServicePlaylistTrack` struct

Add `deleted_at: Option<i64>` field.

#### 2. `src/db.rs` — `add_track_to_playlist_with_added_at()`

Change from `INSERT OR IGNORE` to `INSERT ... ON CONFLICT(playlist_id, track_id) DO UPDATE`:

```rust
sqlx::query(
    r#"
    INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at, deleted_at)
    VALUES (?, ?, ?, ?, NULL)
    ON CONFLICT(playlist_id, track_id) DO UPDATE SET
        position = excluded.position,
        added_at = excluded.added_at,
        deleted_at = NULL
    "#,
)
```

This handles re-adds: a track that was previously soft-deleted gets `deleted_at = NULL` (re-activated).

#### 3. `src/db.rs` — New functions

```rust
/// Mark all tracks in a playlist as deleted (used before re-syncing from Spotify)
pub async fn mark_playlist_tracks_deleted(
    conn: &mut SqliteConnection,
    playlist_id: i64,
) -> Result<u64> {
    let now = chrono::Utc::now().timestamp();
    let rows = sqlx::query(
        "UPDATE service_playlist_tracks SET deleted_at = ? WHERE playlist_id = ? AND deleted_at IS NULL"
    )
    .bind(now)
    .bind(playlist_id)
    .execute(conn)
    .await?;
    Ok(rows.rows_affected())
}

/// Toggle archive_deleted for a playlist
pub async fn set_playlist_archive_deleted(
    pool: &Pool<Sqlite>,
    playlist_id: i64,
    archive: bool,
) -> Result<()> {
    sqlx::query("UPDATE service_playlists SET archive_deleted = ? WHERE id = ?")
        .bind(archive)
        .bind(playlist_id)
        .execute(pool)
        .await?;
    Ok(())
}
```

#### 4. `src/db.rs` — `ServicePlaylist` struct

Add `archive_deleted: bool` field.

#### 5. `src/db.rs` — `update_playlist_fetch_tracking()`

The unique count query currently counts all `service_playlist_tracks` rows. When `archive_deleted = true`, we want the count to reflect ALL tracks (including soft-deleted). When `archive_deleted = false`, only active tracks. For consistency, keep counting all rows (the unique count is about what's stored, not what's active). The views handle the filtering.

Actually, we should count active-only for `remote_unique_count` comparison purposes. Let `remote_unique_count` reflect only active (non-deleted) tracks to match the frontend display. Update the count query:

```sql
SELECT COUNT(*) FROM service_playlist_tracks spt
JOIN service_playlists sp ON sp.id = spt.playlist_id
WHERE sp.service = ? AND sp.playlist_id = ? AND spt.deleted_at IS NULL
```

#### 6. `src/spotify/sync_worker.rs` — `sync_tracks_for_playlist()`

Replace the `DELETE FROM service_playlist_tracks WHERE playlist_id = ?` with:

```rust
// Soft-delete: mark all existing tracks as deleted, then re-insert from stream.
// Re-added tracks will get deleted_at = NULL via ON CONFLICT DO UPDATE.
if let Ok(Some((pl_id,))) = sqlx::query_as::<_, (i64,)>(
    "SELECT id FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?",
)
.bind(playlist_id)
.fetch_optional(&self.db)
.await
{
    let deleted_count = crate::db::mark_playlist_tracks_deleted(&mut *self.db.acquire().await?, pl_id).await.unwrap_or(0);
    if deleted_count > 0 {
        debug!("Soft-deleted {} track(s) from playlist '{}'", deleted_count, playlist_name);
    }
}
```

When `archive_deleted = false`, the views will exclude these soft-deleted tracks. When `archive_deleted = true`, they remain visible.

Optionally, if the playlist has `archive_deleted = false`, we could still do a hard delete for efficiency. But soft-delete is simpler and consistent.

#### 7. `src/api.rs` — `Playlist` response struct

Add `archive_deleted: bool` field.

#### 8. `src/api.rs` — `playlists_handler()`

Add `sp.archive_deleted` to the SELECT:

```sql
SELECT sp.*, COUNT(spt.track_id) as track_count, vtp.tag_name, sp.archive_deleted
FROM service_playlists sp ...
```

When `archive_deleted = false`, the `track_count` should only count active tracks. Currently it's `COUNT(spt.track_id)`. Update to:

```sql
COUNT(CASE WHEN spt.deleted_at IS NULL THEN 1 END) as track_count
```

For playlists with `archive_deleted = true`, we might want to show both active and total. That's a UI consideration.

#### 9. `src/api.rs` — New endpoint: toggle archive

**Route**: `.route("/api/playlists/{id}/archive", put(toggle_playlist_archive_handler))`

**Request**: `{ archiveDeleted: true }`

**Handler**:

```rust
async fn toggle_playlist_archive_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let archive = body.get("archiveDeleted").and_then(|v| v.as_bool()).unwrap_or(false);
    match crate::db::set_playlist_archive_deleted(&state.db, id, archive).await {
        Ok(()) => Json(json!({"data": {"id": id, "archiveDeleted": archive}})).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}
```

#### 10. `src/api.rs` — `PlaylistsQuery`

Add `archive: Option<String>` filter (`"archived"` / `"active"` / `"all"`).

### Frontend Changes (`frontend/pages/playlists.js`)

#### 1. Add `archiveDeleted` to the adapted playlist object

```javascript
archiveDeleted: p.archiveDeleted ?? p.archive_deleted ?? false,
```

#### 2. Add Archive toggle button in each row

Add to `PLAYLISTS_COLUMNS`:

```javascript
{ id: "archive", label: "Archive", sortable: false, defaultWidth: 60 },
```

Add renderer in `PLAYLISTS_CELL_RENDERERS`:

```javascript
archive(r) {
  const icon = r.archiveDeleted ? "fa-archive" : "fa-box-open";
  const title = r.archiveDeleted
    ? "Archiving: deleted tracks remain active for tagging"
    : "Active: deleted tracks are removed from tagging";
  return `<button class="btn btn-sm btn-icon archive-toggle-btn"
    data-id="${r.id}" data-archive="${r.archiveDeleted ? "1" : "0"}"
    title="${title}">
    <i class="fas ${icon}"></i>
  </button>`;
}
```

#### 3. Wire archive toggle click

In `wireContentEvents`, delegate click on `.archive-toggle-btn`:

- Toggle the boolean
- `PUT /api/playlists/{id}/archive` with `{ archiveDeleted: !current }`
- Update button icon + tooltip inline (no full re-render needed)
- Toast: "Archive mode {enabled/disabled} for '{playlistName}'"

#### 4. Add Archive filter to toolbar

In the RIGHT column (Classification section), add a filter row:

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="archive">Archive</span>
  <div class="filter-group">
    <button class="filter-btn" data-value="archived">
      <i class="fas fa-archive"></i> Archiving
    </button>
    <button class="filter-btn" data-value="active">
      <i class="fas fa-box-open"></i> Active
    </button>
    <button class="filter-btn active" data-value="all">All</button>
  </div>
</div>
```

Add `archive: "all"` to state and hash schema.

#### 5. Track count display

When `archiveDeleted = true`, show both active + total in the Tracks column:

```
142 / 287
```

(active / total including soft-deleted). When `archiveDeleted = false`, just show the active count.

### Files to modify

- `migrations/008_playlist_track_archive.sql` — new migration
- `src/db.rs` — `ServicePlaylistTrack` + `ServicePlaylist` structs, `add_track_to_playlist_with_added_at`, new `mark_playlist_tracks_deleted` + `set_playlist_archive_deleted` functions
- `src/spotify/sync_worker.rs` — replace DELETE with soft-delete in `sync_tracks_for_playlist()`
- `src/api.rs` — `Playlist` struct + `playlists_handler` query + `toggle_playlist_archive_handler` endpoint + `PlaylistsQuery` archive filter
- `frontend/pages/playlists.js` — archive column + toggle button + wire click + toolbar filter + track count display
- `frontend/style.css` — `.archive-toggle-btn` styles

### Acceptance Criteria

- [ ] Migration 008 runs cleanly on fresh DB (001→008)
- [ ] Migration 008 runs cleanly on existing DB with data
- [ ] Subscribed playlists default to `archive_deleted = true`
- [ ] Non-subscribed playlists default to `archive_deleted = false`
- [ ] Full sync marks removed tracks with `deleted_at` instead of deleting
- [ ] Re-added tracks get `deleted_at = NULL` (re-activated)
- [ ] When `archive_deleted = true`: `v_file_tags` + `v_file_resolved_tags` include all tracks regardless of `deleted_at`
- [ ] When `archive_deleted = false`: only active (non-deleted) tracks appear in tag resolution
- [ ] Toggle button in playlist row switches between archive/active modes
- [ ] PUT `/api/playlists/{id}/archive` toggles the flag
- [ ] Archive filter in toolbar works (archived/active/all)
- [ ] Track count column shows active/total for archiving playlists
- [ ] `compute_target_comment()` correctly resolves tags based on archive status (via updated views)
- [ ] Digging suggestions respect archive status (via updated views)
- [ ] No regressions: subscription poller + global poller still work (they only add, don't delete)
- [ ] Backend compiles (`cargo build`)
- [ ] Test with curl: toggle archive, verify `v_file_tags` returns correct counts

---

## Plan: soundcloud-integration

**Status**: proposed
**Branch**: `feat/soundcloud-integration`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no (SoundCloud already in schema since 001)

### Description

Implement SoundCloud as a first-class service — full playlist + track sync, matching the Spotify integration pattern. The `soundcloud-rs` crate (v0.14.0) is already a dependency. SoundCloud uses a simpler authentication model (no OAuth — auto-discovers `client_id` from their site, or uses a provided `api_key`), so the implementation is simpler than Spotify: no token refresh, no subscription poller needed for v1.

### Current State (already wired)

- **Schema**: `service_tracks` CHECK includes `'soundcloud'`, `files.soundcloud_id` column, `v_file_track_link` matches `service='soundcloud' AND service_id = f.soundcloud_id`, index on `idx_files_soundcloud_id`
- **Config**: `SoundcloudToml` + `ServiceCredentials.soundcloud_api_key` + `is_soundcloud_configured()`
- **Frontend**: `services.js` has SoundCloud service meta (name, icon, color `#ff5500`)
- **API stubs**: `service_sync_handler` returns "not yet implemented", `service_auth_handler` returns "not yet implemented"
- **Dependency**: `soundcloud-rs = "0.14.0"` in Cargo.toml (already compiles)

### What `soundcloud-rs` provides

| Method                                   | Returns                       | Notes                                          |
| ---------------------------------------- | ----------------------------- | ---------------------------------------------- |
| `Client::new()`                          | `Client`                      | Auto-discovers SC `client_id` from site        |
| `get_user(Identifier)`                   | `User`                        | Full user profile                              |
| `get_user_playlists(id, Option<Paging>)` | `Playlists(PagingCollection)` | Paginated playlist list                        |
| `get_playlist(Identifier)`               | `Playlist`                    | Single playlist including `tracks: Vec<Track>` |
| `health_check()`                         | `bool`                        | `/me` endpoint check                           |

Key models:

- **Playlist**: `id (i32)`, `title`, `track_count`, `tracks: Vec<Track>`, `user (UserSummary)`, `urn`, `permalink_url`, `description`
- **Track**: `id (i64)`, `title`, `isrc`, `bpm (f64)`, `genre`, `duration (i64 ms)`, `user (UserSummary)`, `urn`, `permalink_url`, `artwork_url`
- **UserSummary**: `id`, `username`, `permalink_url`, `avatar_url`
- **Paging**: `limit`, `offset`, `linked_partitioning`
- **PagingCollection<T>**: `collection: Vec<T>` (note: the crate bundles ALL pages into one collection for `get_user_playlists` — no manual pagination needed)

### Auth model

SoundCloud has **no OAuth**. The `soundcloud-rs` crate auto-discovers a public `client_id` by:

1. Fetching `soundcloud.com` HTML
2. Extracting JS script URLs
3. Searching each script for a 32-char `client_id` pattern

If the auto-discovery fails (SC changes their site), the user can provide their own `client_id` via `config.toml`:

```toml
[soundcloud]
api_key = "your_client_id_here"  # Falls back to auto-discovery if not set
user_id = "12345"                 # SoundCloud user ID (numeric or permalink)
```

The `user_id` is required — this is whose playlists/likes we sync.

### Backend Changes

#### 1. New module: `src/soundcloud/`

```
src/soundcloud/
├── mod.rs          # Module declarations + re-exports
├── client.rs       # ScClient wrapper (thin wrapper over soundcloud_rs::Client)
├── models.rs       # ScPlaylistInfo, ScTrackInfo (our own types for DB)
└── sync_worker.rs  # ScSyncWorker (background sync with task tracking)
```

#### 1a. `src/soundcloud/models.rs` — Our internal types

```rust
/// Our internal playlist info (separate from soundcloud_rs::Playlist)
#[derive(Debug, Clone)]
pub struct ScPlaylistInfo {
    pub id: i32,
    pub title: String,
    pub description: Option<String>,
    pub track_count: i32,
    pub urn: Option<String>,
    pub permalink_url: Option<String>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

/// Our internal track info (separate from soundcloud_rs::Track)
#[derive(Debug, Clone)]
pub struct ScTrackInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,       // from user.username on the track
    pub isrc: Option<String>,
    pub bpm: Option<f64>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub urn: Option<String>,
    pub permalink_url: Option<String>,
}
```

Conversions from `soundcloud_rs` types:

- `From<&Playlist>` → `ScPlaylistInfo`
- `From<&Track>` → `ScTrackInfo`

#### 1b. `src/soundcloud/client.rs` — SC client wrapper

```rust
pub struct ScClient {
    client: soundcloud_rs::Client,
    user_id: String,  // from config
}

impl ScClient {
    /// Create client. If api_key is provided, use it directly.
    /// Otherwise, auto-discover via soundcloud_rs.
    pub async fn new(config: &ServiceCredentials) -> Result<Self>;

    /// Health check
    pub async fn health_check(&self) -> bool;

    /// Get the user's playlists (paginated collection)
    pub async fn get_user_playlists(&self) -> Result<Vec<ScPlaylistInfo>>;

    /// Get a single playlist with its tracks
    pub async fn get_playlist(&self, playlist_id: i32) -> Result<(ScPlaylistInfo, Vec<ScTrackInfo>)>;

    /// Sync playlists only (metadata, no tracks)
    pub async fn sync_playlists_only(&self) -> Result<Vec<ScPlaylistInfo>>;

    /// Get user ID as Identifier
    fn user_identifier(&self) -> Identifier;
}
```

The `api_key` config field should allow injection: when provided, we can construct the `Client` with that key directly (instead of auto-discovery). This requires checking how `soundcloud_rs::Client` stores the `client_id` — it's in `RwLock<String>`, so we can set it after construction.

#### 1c. `src/soundcloud/sync_worker.rs` — Background sync

Follow the Spotify `SyncWorker` pattern:

```rust
pub struct ScSyncWorker {
    db: Pool<Sqlite>,
    sc_client: ScClient,
    task_id: String,
    sync_type: SyncType,
    cancel_token: CancellationToken,
    progress: Arc<RwLock<SyncProgress>>,
}

impl ScSyncWorker {
    pub fn new(db, sc_client, task_id, sync_type, cancel_token) -> Self;

    /// Run the sync operation
    pub async fn run(&self) -> Result<SyncResult>;

    /// Sync all playlists (metadata only — no tracks)
    async fn sync_playlists(&self) -> Result<usize>;

    /// Sync tracks for a single playlist
    async fn sync_playlist_tracks(&self, playlist_id: i32, playlist_name: &str) -> Result<usize>;

    /// Full sync: playlists + all tracks
    async fn sync_full(&self) -> Result<(usize, usize)>;
}
```

**Sync flow (full sync)**:

1. Create task in TaskManager with `SyncType::Full`
2. Fetch user playlists via `get_user_playlists()`
3. For each playlist: upsert into `service_playlists` (service='soundcloud')
4. For each playlist: fetch full playlist with tracks via `get_playlist(id)`
5. For each track: upsert into `service_tracks` (service='soundcloud', service_id=track.id)
6. Link tracks to playlists in `service_playlist_tracks`
7. Update task progress after each playlist

**Sync modes** (same as Spotify):

- `SyncType::PlaylistsOnly` — just playlist metadata
- `SyncType::Full` — playlists + all tracks
- `SyncType::SinglePlaylist` — one specific playlist

**Rate limiting**: SoundCloud doesn't have documented rate limits, but we should add a 200ms delay between playlist detail fetches to be safe.

#### 2. `src/db.rs` — DB functions

Reuse existing functions (no new ones needed):

- `upsert_service_playlist()` — already handles any service
- `upsert_service_track()` — already handles `service='soundcloud'` via CHECK constraint
- `get_service_config()` / `update_service_config()` — already works for 'soundcloud'
- `add_track_to_playlist_with_added_at()` — already generic

The `service_tracks` table stores BPM in `metadata_json` (since only `files` has `bpm`/`musical_key` columns). For SC tracks, store BPM via `metadata_json`:

```json
{ "bpm": 128.0, "genre": "Techno" }
```

#### 3. `src/api.rs` — Endpoints

**New routes** (following Spotify pattern):

```rust
.route("/api/services/soundcloud/sync/playlists", post(sc_sync_playlists_handler))
.route("/api/services/soundcloud/sync/full", post(sc_sync_full_handler))
.route("/api/services/soundcloud/sync/playlists/{playlist_id}/tracks", post(sc_sync_playlist_tracks_handler))
.route("/api/services/soundcloud/sync/{task_id}", get(sc_sync_task_handler).delete(sc_sync_cancel_handler))
```

**Modify existing handlers**:

- `service_sync_handler` — route `"soundcloud"` to SC handlers instead of returning "not yet implemented"
- `service_auth_handler` — for SC, just validate the config (no OAuth flow needed), set `is_connected = true` in `service_config`
- `service_callback_handler` — return 200 for SC (no callback needed)

**SC-specific handlers**:

```rust
async fn sc_sync_playlists_handler(State, Json) -> impl IntoResponse;
async fn sc_sync_full_handler(State, Json) -> impl IntoResponse;
async fn sc_sync_playlist_tracks_handler(State, Path, Json) -> impl IntoResponse;
async fn sc_sync_task_handler(State, Path) -> impl IntoResponse;
async fn sc_sync_cancel_handler(State, Path) -> impl IntoResponse;
```

Each handler:

1. Validates SC is configured
2. Creates `ScClient`
3. Spawns `ScSyncWorker` via TaskManager
4. Returns `{ taskId, status }`

#### 4. `src/main.rs` — Module declaration

```rust
mod soundcloud;
```

### Files to create

- `src/soundcloud/mod.rs` — module declarations + re-exports
- `src/soundcloud/models.rs` — `ScPlaylistInfo`, `ScTrackInfo` + `From` impls
- `src/soundcloud/client.rs` — `ScClient` wrapper
- `src/soundcloud/sync_worker.rs` — `ScSyncWorker`

### Files to modify

- `src/main.rs` — add `mod soundcloud;`
- `src/api.rs` — add SC sync routes + handlers; update `service_sync_handler`/`service_auth_handler`/`service_callback_handler` for SC
- `frontend/pages/services.js` — enable Sync button for SoundCloud (remove "not implemented" handling)

### Acceptance Criteria

- [ ] `ScClient::new()` creates a working client (auto-discover or api_key)
- [ ] `ScClient::health_check()` returns true when SC is reachable
- [ ] `GET /api/services/soundcloud/sync/playlists` fetches user's playlists into DB
- [ ] `GET /api/services/soundcloud/sync/full` fetches playlists + all tracks into DB
- [ ] `GET /api/services/soundcloud/sync/playlists/{id}/tracks` fetches tracks for one playlist
- [ ] SoundCloud tracks appear on Tracks page (filterable by `service=soundcloud`)
- [ ] SoundCloud playlists appear on Playlists page
- [ ] Tag matching: playlist names auto-create tags via `v_tag_playlist`
- [ ] Files linked via `soundcloud_id` column match SC tracks in `v_file_track_link`
- [ ] BPM stored in `metadata_json` for SC tracks (no DB column needed)
- [ ] Sync progress visible in Tasks page
- [ ] Cancel works via task cancellation token
- [ ] Config override: `api_key` in config.toml takes priority over auto-discovery
- [ ] Services page shows SoundCloud as "Configured" / "Connected" after auth
- [ ] Sync button on Services page triggers SC sync (not "not yet implemented")
- [ ] No regressions: Spotify sync, local playlists, digging, comments all unchanged
- [x] Backend compiles (`cargo build`)
- [ ] Test with `curl` against real SoundCloud API
- [ ] Fresh DB: all migrations run cleanly (no new migration needed)

### Out of scope (v2)

- Subscription poller for SC (Spotify-only for now — SC doesn't have a subscription concept)
- Global poller for SC (SC playlists don't have snapshot-based change detection)
- Automatic SC playlist creation from local playlists
- SC audio streaming in the digging page (different API for stream URLs)
- SC track search in digging suggestions
- SC user likes/reposts syncing

---

## Plan: file-lifecycle-management

**Status**: done ✅
**Branch**: `feat/file-lifecycle-management`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `009_file_lifecycle.sql`

### Description

File lifecycle management system. Three pillars:

1. **WAV source indexing** — Track nuo-stems WAV source files in the DB, back them up to NAS, delete locally
2. **Tag-based file presence** — "Follow" a tag → its files stay local. "Backpack" tag for quick-add from Tracks page
3. **Backup + prune** — Copy files to NAS via SSH/SCP, verify, then safely prune local copies that are backed up and not "followed"

### Data Model

#### Migration 009: `migrations/009_file_lifecycle.sql`

```sql
-- file_locations: tracks where a file physically exists
CREATE TABLE IF NOT EXISTS file_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    location_type TEXT NOT NULL CHECK (location_type IN ('local', 'backup')),
    path TEXT NOT NULL,
    file_size INTEGER,
    last_verified INTEGER,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(file_id, location_type)
);

-- tags: add followed flag
ALTER TABLE tags ADD COLUMN followed BOOLEAN NOT NULL DEFAULT 0;
-- Default follow for "backpack" tag (created on first use if not present)

-- files: add source_of for WAV→stem parent linking
ALTER TABLE files ADD COLUMN source_of INTEGER REFERENCES files(id);

-- folders: add scan_sources + backup_path
ALTER TABLE folders ADD COLUMN scan_sources BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN backup_path TEXT;

CREATE INDEX IF NOT EXISTS idx_file_locations_file_id ON file_locations(file_id);
CREATE INDEX IF NOT EXISTS idx_file_locations_type ON file_locations(location_type);
CREATE INDEX IF NOT EXISTS idx_files_source_of ON files(source_of);
CREATE INDEX IF NOT EXISTS idx_tags_followed ON tags(followed);

SELECT 'Migration 009 applied: file lifecycle management' as status;
```

### Rust Types

New types in `src/db.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileLocation {
    pub id: i64,
    pub file_id: i64,
    pub location_type: String,  // 'local' | 'backup'
    pub path: String,
    pub file_size: Option<i64>,
    pub last_verified: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneCandidate {
    pub file_id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub reason: String,  // "flac_with_stem" | "wav_backed_up" | "not_followed"
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatus {
    pub local_file_count: i64,
    pub local_size_bytes: i64,
    pub local_stems: i64,
    pub local_flacs: i64,
    pub backup_count: i64,
    pub wav_source_dirs: i64,
    pub prune_candidate_count: i64,
    pub prune_candidate_bytes: i64,
}
```

### DB Functions (in `src/db.rs`)

```rust
// ─── Tag following ─────────────────────────────────────────────
pub async fn set_tag_followed(pool: &Pool<Sqlite>, tag_id: i64, followed: bool) -> Result<()>
pub async fn get_followed_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>>
pub async fn get_backpack_tag(pool: &Pool<Sqlite>) -> Result<Option<Tag>>
pub async fn ensure_backpack_tag(pool: &Pool<Sqlite>) -> Result<Tag>
pub async fn is_file_followed(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool>

// ─── File locations ────────────────────────────────────────────
pub async fn set_file_location(pool: &Pool<Sqlite>, file_id: i64, location_type: &str, path: &str, file_size: i64) -> Result<()>
pub async fn remove_file_location(pool: &Pool<Sqlite>, file_id: i64, location_type: &str) -> Result<()>
pub async fn get_file_locations(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<FileLocation>>
pub async fn get_unbacked_up_files(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<File>>
pub async fn record_backup_result(pool: &Pool<Sqlite>, file_id: i64, success: bool, file_size: i64, backup_path: &str) -> Result<()>
pub async fn clear_backup_status(pool: &Pool<Sqlite>, folder_id: i64) -> Result<()>

// ─── Source-of (WAV→stem linking) ─────────────────────────────
pub async fn get_wav_source_subdirs(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<String>>
pub async fn set_file_source_of(pool: &Pool<Sqlite>, file_id: i64, source_file_id: i64) -> Result<()>
pub async fn get_files_by_source(pool: &Pool<Sqlite>, source_file_id: i64) -> Result<Vec<File>>

// ─── Pruning ──────────────────────────────────────────────────
pub async fn get_prune_candidates(pool: &Pool<Sqlite>) -> Result<Vec<PruneCandidate>>
pub async fn delete_local_file_by_id(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool>

// ─── Storage status ───────────────────────────────────────────
pub async fn get_storage_status(pool: &Pool<Sqlite>) -> Result<StorageStatus>

// ─── Folder backup config ─────────────────────────────────────
pub async fn update_folder_backup_config(pool: &Pool<Sqlite>, folder_id: i64, backup_path: Option<&str>, scan_sources: Option<bool>) -> Result<()>
```

### Backup Engine (`src/backup/mod.rs`)

New module with SSH-based backup:

```rust
pub struct BackupEngine {
    ssh_host: String,
    ssh_key_path: Option<String>,
}

impl BackupEngine {
    pub fn new(ssh_host: String) -> Self;

    /// Copy a local file to the backup destination. Returns (success, remote_size).
    pub async fn copy_file(
        &self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<(bool, i64)>;

    /// Verify a file exists on backup with matching size.
    pub async fn verify_file(
        &self,
        remote_path: &str,
        expected_size: i64,
    ) -> Result<bool>;

    /// Get size of a remote file (None if doesn't exist).
    pub async fn remote_file_size(&self, remote_path: &str) -> Result<Option<i64>>;

    /// List files in a remote directory.
    pub async fn list_remote_files(&self, remote_dir: &str) -> Result<Vec<String>>;

    /// Run rsync in dry-run mode to show what would be transferred.
    pub async fn dry_run_sync(&self, local_dir: &str, remote_dir: &str) -> Result<Vec<String>>;
}
```

The engine shells out to `scp` and `ssh` commands using `tokio::process::Command`, reading `~/.ssh/config` for host resolution. The `backup` host is passed as `ssh_host` (your `~/.ssh/config` maps `backup` → your NAS).

### API Endpoints (in `src/api.rs`)

```
GET    /api/storage/status               → StorageStatus
POST   /api/storage/backup/{folder_id}   → BackupResult { copied: usize, verified: usize, errors: usize }
POST   /api/storage/prune-preview        → [PruneCandidate]
POST   /api/storage/prune                → PruneResult { deleted: usize, freedBytes: i64 }
PUT    /api/tags/{id}/follow             → { followed: bool }
POST   /api/tracks/{id}/backpack          → { inBackpack: bool }
GET    /api/files/{id}/backup-status      → { backedUp: bool, locations: [FileLocation] }
PUT    /api/folders/{id}/backup           → { backupPath: string, scanSources: bool }
POST   /api/folders/{id}/scan-sources     → { wavIndexed: usize, linkedToStems: usize }
POST   /api/storage/backup-wavs/{folder_id} → { wavDirsBackedUp: usize, localWavsDeleted: usize }
```

### Frontend

#### Tags page (`frontend/pages/tags.js`)

- New column: "Follow" with toggle button per row
- Calls `PUT /api/tags/{id}/follow` to toggle
- Shows "Backpack" icon for tracks with backpack tag
- Followed tags show a filled 👁 or pinned icon
- Followed state included in API tag response

#### Tracks page (`frontend/pages/tracks.js`)

- New column: "Backpack" with toggle button per row (like subscribe bell on playlists)
- Click toggles the "backpack" Setlist tag on the track via `POST /api/tracks/{id}/backpack`
- Shows filled/empty backpack icon based on whether the track has the "backpack" tag

#### Storage page (`frontend/pages/storage.js`) — NEW

- Storage status cards (local vs backup)
- Backup buttons per folder (stems, flacs)
- WAV backup + cleanup button
- Prune preview table with checkboxes + execute button
- Dry-run before any destructive action

#### Folders page (`frontend/pages/folders.js`)

- Edit folder modal: add "Backup Path" text input + "Scan Sources" checkbox
- Show backup status per folder in table

### Files to modify

- `migrations/009_file_lifecycle.sql` — new migration
- `src/db.rs` — FileLocation, PruneCandidate, StorageStatus types + 15 new functions
- `src/backup/mod.rs` — BackupEngine (new module)
- `src/api.rs` — 10 new handler functions + routes
- `src/main.rs` — add `mod backup;`
- `frontend/pages/tags.js` — Follow toggle column
- `frontend/pages/tracks.js` — Backpack toggle column
- `frontend/pages/storage.js` — new page module
- `frontend/pages/folders.js` — Backup Path + Scan Sources fields
- `frontend/app.js` — register storage page
- `frontend/shared/nav.js` — add Storage nav link
- `frontend/style.css` — storage page styles

### Acceptance Criteria

- [ ] Migration 009 runs cleanly on fresh DB (001→009)
- [ ] Migration 009 runs cleanly on existing DB with data
- [ ] Tags page shows Follow toggle; clicking it persists and filters correctly
- [ ] Tracks page shows Backpack toggle; clicking adds/removes "backpack" tag
- [ ] Following a tag prevents its files from being pruned
- [ ] Storage page shows local vs backup stats
- [ ] Backup folder operation copies unbacked-up files via SCP, verifies
- [ ] WAV backup operation indexes subdirectories, backs up WAVs, records locations
- [ ] Prune preview shows candidates with reasons (flac_with_stem, wav_backed_up, not_followed)
- [ ] Prune execute deletes local files only if they have confirmed backup
- [ ] Folders page lets you set backup_path and scan_sources
- [ ] `cargo build` passes
- [ ] Frontend loads without errors

---

## Plan: storage-holistic-cleanup

**Status**: proposed
**Branch**: `fix/storage-holistic-cleanup`
**Ready for review**: no
**Depends on**: `feat/file-lifecycle-management` (already merged)
**Migration needed**: no

### Audit: Current state

| Data point           | Value                                           |
| -------------------- | ----------------------------------------------- |
| Total files          | 5,006 (1,770 stems + 2,104 FLACs + 1,132 WAVs)  |
| Total size           | 196.8 GB                                        |
| Backed up            | 3,167 files (1,762 stems + 1,405 FLACs)         |
| ISRCs with stem+FLAC | 682 (redundant FLACs ~60 GB)                    |
| WAVs from subdirs    | 1,132 indexed, 0 backed up, 0 with source_of    |
| Prune candidates     | 2,962 (too high — includes 682 redundant FLACs) |

### Problem #1: No format preference for pruning

When a track (same ISRC) has a `.stem.m4a` version, other formats (FLAC, MP3, WAV) are redundant locally. The nuo-stems workflow is: convert FLAC to stem, keep stem, archive FLAC to NAS. Currently 682 FLACs have a corresponding stem but both count as "kept".

**Fix**: Global "Prefer stem files" toggle in Storage page. When on, the prune query excludes FLACs/MP3s/WAVs whose same-ISRC stem exists. This converts 682 redundant FLACs into valid prune candidates.

**Storage**: Toggle persisted as `stem_preferred` in a config store (service_config table or new column on Settings).

**Prune query change** — add AND NOT clause:

```
AND NOT (
    f.file_type != 'stem.m4a'
    AND EXISTS (
        SELECT 1 FROM files f2
        WHERE f2.isrc = f.isrc AND f2.isrc IS NOT NULL
        AND f2.file_type = 'stem.m4a'
    )
)
```

### Problem #2: WAV source tracking incomplete

1,132 WAVs are indexed (from subdirs, since scan_recursive=true reached them), but:

- `source_of` is never populated (no linking to parent stem)
- `wav_source_dirs` in StorageStatus counts 0 because it queries `source_of IS NOT NULL`
- WAVs aren't tracked as source files vs independent files

**Fix**: After scanner indexes WAVs from subdirs, post-process to set `source_of`. Match: directory name (without extension) → stem filename in parent dir.

### Problem #3: Storage page layout is messy

Current layout mixes file types oddly (FLACs as subtitle of Stems card), WAV Sources card is confusing, and there's no size breakdown per file type.

**Fix**: Clean card layout:

- Row 1: Local Files | Backed Up | Prune Candidates (summary)
- Row 2: Per-type breakdown with sizes (stems, FLACs, WAVs, MP3s)
- Stem preference toggle section
- Folders section (keep as-is, already nice)

Add size fields to StorageStatus: `local_stems_size`, `local_flacs_size`, `local_wavs_size`, `local_mp3s_size`.

### Files to modify

- `src/db.rs` — add `stem_preferred` config, per-type size fields, update prune query, fix wav_source_dirs
- `src/api.rs` — add `GET/PUT /api/storage/settings`, update StorageStatus construction
- `frontend/pages/storage.js` — overhaul layout, stem preference toggle, per-type sizes
- `frontend/style.css` — storage layout styles

### Acceptance Criteria

- [ ] Stem preference toggle shows in Storage page, persists correctly
- [ ] With stem_preferred=true, 682 FLACs with same-ISRC stem become prune candidates
- [ ] With stem_preferred=false, current behavior preserved
- [ ] WAV source_of populated by scanner for subdir WAVs
- [ ] StorageStatus includes per-type size breakdown
- [ ] Clean card layout — no format treated as subtitle
- [ ] `cargo build` passes
- [ ] No regression to backup/reconcile/prune

### Problem #4: Tag file counts don't include parent-resolved files

`v_tag_file_counts` uses `v_file_tags` (direct tag→playlist matching). But `v_file_resolved_tags` already exists and correctly resolves parent tags. The fix: either update `v_tag_file_counts` to use `v_file_resolved_tags`, or create a new `v_resolved_tag_file_counts` view and use it in the Tags page.

Similarly, `get_tags_count` and `get_all_tags` use `v_tag_file_counts`. Change to `v_file_resolved_tags`.

**Example**: "Droid House" has parent "house". Currently "house" shows 0 files. After fix: "house" shows 571+ files (sum of all child tags).

**Fix**:

- Create or update `v_tag_file_counts` to join through `v_file_resolved_tags`
- Update `get_all_tags` SQL to use the new count source

### Problem #5: Tag edit modal doesn't show parent tags

The modal in the Tags page (tag edit flow) only shows name + category selector. It should also show:

- Current parent tags as chips with category badges
- Button to navigate to Tag Curation page for full parent management

**Fix**: Add parent tag chips section to `showEditTagModal`, populated from `GET /api/tags/{id}/parents`.

---

## Plan: fix-filter-visual-feedback

**Status**: done ✅
**Branch**: `fix/filter-visual-feedback`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Fix filter button visual feedback bugs across all CRUD pages. The root cause is the "render toolbar once, patch DOM imperatively" pattern: when an event handler mutates state but forgets to toggle `.active` on the button, the button appears frozen. Additionally, some filters never reach the backend (placebo buttons), and some UI elements (playlist badge, Create Tags spinner) have lifecycle bugs.

### Architecture context

All four CRUD pages use the same pattern:

- `renderToolbar(state)` generates HTML with `${condition ? " active" : ""}` inline — runs **once** in `init()`
- `fetchAndRender()` only replaces `#page-content` div, NOT the toolbar
- Event handlers must imperatively update DOM (`.classList.toggle`, `.innerHTML`, `.style.display`)
- If a handler mutates state but skips DOM update → visual freeze

### Issues found

#### A. files.js — 3 button groups with no visual toggle

| Button group                  | Lines     | Symptom                                                                                      |
| ----------------------------- | --------- | -------------------------------------------------------------------------------------------- |
| Key buttons (24 Camelot keys) | 823–866   | `state.keys` mutates, `.active` never toggled. ALL/NONE actions also skip `.active` updates. |
| Linked / Unlinked toggle      | 1091–1125 | `state.linkedOnly`/`state.unlinked` mutate, buttons never toggle.                            |
| Non-Default Only toggle       | 1129–1141 | `state.nonDefaultOnly` toggles, button never updates.                                        |

**Fix for key buttons**: Add `btn.classList.toggle("active")` in the regular key toggle handler. For ALL/NONE actions, re-sync all 24 button classes from `state.keys`.

**Fix for Linked/Unlinked**: Add `linkedBtn.classList.toggle("active", state.linkedOnly)` and `unlinkedBtn.classList.toggle("active", state.unlinked)` in each handler. Also update the sibling button (mutual exclusion).

**Fix for Non-Default Only**: Add `btn.classList.toggle("active", state.nonDefaultOnly)`.

#### B. tracks.js — Playlist context badge doesn't disappear

| Element                         | Lines     | Symptom                                                                                                                         |
| ------------------------------- | --------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Playlist context badge × button | 1838–1843 | `state.playlistId = null`, navigate to `#tracks`, `fetchAndRender` called — but toolbar was rendered once, badge HTML persists. |

**Fix**: Add DOM manipulation in the clear handler: `badge.style.display = "none"` or `badge.remove()`. Also, the `updatePlaylistBadge()` function at line 1132-1140 already exists and hides the badge when `selectedPlaylists` has items — extend it to also hide when `playlistId` is null.

#### C. playlists.js — Service filter is placebo (never sent to backend)

| Element                      | Lines            | Symptom                                                                                                                                                             |
| ---------------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Multi-select service buttons | 548–572, 418–434 | `state.selectedServices` toggles correctly, `syncServiceFilterUI()` works, but `buildParams()` never sends it and backend `PlaylistsQuery` has no `services` field. |

**Fix (option A, simpler)**: Convert to single-select using the existing `state.service` + `service` param. Change the multi-select button group to radio-style (only one active at a time).

**Fix (option B, more work)**: Add `services: Option<String>` to `PlaylistsQuery`, implement SQL `IN` filter in `playlists_handler`, add to `buildParams()`. Worth it if multi-service filtering is genuinely useful.

**Recommendation**: Option A (single-select). The existing `service` dropdown in the filter panel already provides single-service filtering. The multi-select buttons are redundant and broken.

#### D. playlists.js — "Create Tags" button stays spinning

| Element            | Lines     | Symptom                                                                                      |
| ------------------ | --------- | -------------------------------------------------------------------------------------------- |
| Create Tags button | 1147–1172 | On success, button HTML is set to spinner but never restored. Only re-enabled on error path. |

**Fix**: Add `finally` block that always restores the button: `createTagsBtn.disabled = false; createTagsBtn.innerHTML = '<i class="fas fa-tag"></i> Create Tags';`.

### Additional minor fixes

#### E. files.js — Filter panel collapse not persisted

Lines 774–787: The collapse toggle works but never calls `localStorage.setItem()`. Add it (pattern already exists in tracks.js and tags.js).

#### F. tags.js — Duplicate `wireActionsRefresh` call

Lines 895–902 and 1025–1032: Called twice. Second overwrites first. Delete the first instance (the second has the `refresh` button comment).

#### G. playlists.js + tags.js — Filter row toggle states not persisted

Both pages have `[data-filter]` toggle labels (Service, Category, etc.) whose enabled/disabled state resets on page re-entry. Add `localStorage` read on init + write on toggle. Pattern: `filterRowState_{page}_{filterName}`.

#### H. files.js + playlists.js — `untaggedOnly` has no UI button

`untaggedOnly` exists in `HASH_DEFAULTS`, `HASH_SCHEMA`, and `buildParams()` on playlists.js, but `renderToolbar()` has no button for it. Either add a button or remove the dead state.

### Files to modify

- `frontend/pages/files.js` — Key buttons `.active` toggle, Linked/Unlinked `.active`, Non-Default `.active`, filter collapse localStorage
- `frontend/pages/tracks.js` — Playlist badge clear DOM update
- `frontend/pages/playlists.js` — Service filter (convert to single-select), Create Tags `finally` block, filter row toggle localStorage, untaggedOnly UI
- `frontend/pages/tags.js` — Remove duplicate wireActionsRefresh, filter row toggle localStorage

### Acceptance Criteria

- [ ] Key buttons toggle `.active` visually on click
- [ ] ALL m / NONE m / ALL d / NONE d actions update all 24 key button states
- [ ] Linked/Unlinked buttons show active state, mutual exclusion works
- [ ] Non-Default Only button shows active state
- [ ] Playlist context badge disappears when × is clicked
- [ ] Service filter on playlists page actually filters results
- [ ] Create Tags button re-enables after success
- [ ] Filter panel collapse state persists across page navigations (files.js)
- [ ] No duplicate wireActionsRefresh in tags.js
- [ ] Filter row toggle states persist across page navigations (playlists.js, tags.js)
- [ ] No regressions: sort, pagination, search, column config, layout mode, bulk comments still work
- [ ] `cargo build` passes (no backend changes unless service filter chosen as option B)

---

## Plan: auto-backup

**Status**: done ✅
**Branch**: `feat/auto-backup`
**Ready for review**: no
**Depends on**: `feat/file-lifecycle-management` (already merged)
**Migration needed**: yes — `010_auto_backup.sql`

### Description

Auto-backup: when a folder has a `backup_path` configured, it automatically reconciles and rsyncs new files without manual intervention. Enabled by default, toggleable per folder.

### Current State

| What we have                          | What's missing                     |
| ------------------------------------- | ---------------------------------- |
| Reconcile + rsync via "Backup" button | No periodic auto-trigger           |
| Auto-reconcile on server startup      | Only runs once at boot             |
| Folder watcher (scans every 5 min)    | Watcher only scans, doesn't backup |

### Design

#### Migration 010: `migrations/010_auto_backup.sql`

```sql
ALTER TABLE folders ADD COLUMN auto_backup BOOLEAN NOT NULL DEFAULT 1;

SELECT 'Migration 010 applied: auto_backup column on folders' as status;
```

#### Folder struct update

Add `auto_backup: bool` to `Folder` in `src/db.rs`.

#### API: toggle auto_backup

Extend `PUT /api/folders/{id}/backup` (already exists for backup_path + scan_sources) to also accept `autoBackup: bool`.

Or add a new simpler endpoint:

```rust
.route("/api/folders/{id}/auto-backup", put(folder_auto_backup_handler))
```

```rust
async fn folder_auto_backup_handler(...) -> impl IntoResponse {
    // Toggle auto_backup
    sqlx::query("UPDATE folders SET auto_backup = ? WHERE id = ?")
        .bind(auto_backup).bind(id).execute(&state.db).await?;
    Json(ApiResponse { data: json!({ "autoBackup": auto_backup }) })
}
```

#### Auto-backup background task

In `src/main.rs` `serve()`, alongside the folder watcher, add an **auto-backup poller**:

```rust
// Auto-backup poller: periodically reconcile+backup folders with auto_backup enabled
let auto_db = state.db.clone();
let auto_tm = state.task_manager.clone();
tokio::spawn(async move {
    let interval = std::time::Duration::from_secs(600); // 10 minutes
    loop {
        tokio::time::sleep(interval).await;
        let folders: Vec<crate::db::Folder> = sqlx::query_as::<_, crate::db::Folder>(
            "SELECT * FROM folders WHERE auto_backup = 1 AND backup_path IS NOT NULL AND backup_path != ''"
        ).fetch_all(&auto_db).await.unwrap_or_default();

        for folder in folders {
            let unbacked = crate::db::get_unbacked_up_files(&auto_db, folder.id).await.unwrap_or_default();
            if !unbacked.is_empty() {
                tracing::info!("Auto-backup: folder '{}' has {} unbacked files", folder.folder_path, unbacked.len());
                crate::tasks::start_backup_folder_task(&auto_tm, &auto_db, folder.id).await;
            }
        }
    }
});
```

This runs every 10 minutes. For each folder with `auto_backup=true`:

- Checks if unbacked files exist
- If yes, triggers backup task (reconcile first, then rsync only new files)
- If no, skips (minimal overhead: one lightweight SQL query)

#### Frontend

**Folder edit modal** — add a checkbox:

```html
<label class="checkbox-label">
  <input type="checkbox" id="edit-folder-auto-backup" ${f.autoBackup ? "checked" : ""}>
  Auto-backup new files
</label>
<span class="help-text">Automatically reconcile and sync new files to the backup destination</span>
```

**Storage page folder cards** — show auto-backup status with a green dot when enabled.

### Files to modify

- `migrations/010_auto_backup.sql` — new migration
- `src/db.rs` — add `auto_backup` to `Folder` struct
- `src/api.rs` — add `folder_auto_backup_handler` + route
- `src/main.rs` — add auto-backup poller
- `frontend/pages/folders.js` — add checkbox to edit modal
- `frontend/pages/storage.js` — show auto-backup status on folder cards

### Acceptance Criteria

- [ ] Migration 010 applies cleanly (001→010)
- [ ] New folders default to `auto_backup = true`
- [ ] Folder edit modal has auto-backup checkbox
- [ ] Storage page shows auto-backup status per folder
- [ ] Auto-backup poller runs every 10 minutes
- [ ] Poller only triggers backup when unbacked files exist
- [ ] No manual intervention needed: files appear → auto-reconciled → auto-rsynced
- [ ] `cargo build` passes

---

## Plan: digging-enrichment

**Status**: done ✅
**Branch**: `feat/digging-enrichment`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Enrich the Digging track browser with play count, rating, last played from linked files. Add server-side sorting. Add absolute BPM filter. Auto-load tracks on open.

### Backend: `src/digging.rs`

#### 1. Add play_count, rating, last_played to DiggingTrackResult

```rust
pub play_count: i32,
pub rating: i32,
pub last_played: Option<i64>,
```

#### 2. Add sort params to DiggingTracksQuery

```rust
pub sort_by: Option<String>,   // "relevance","playCount","rating","bpm","energy","lastPlayed","tagCount"
pub sort_order: Option<String>, // "asc" or "desc"
```

Default sort when no filters: `rating desc → playCount desc`. With filters: `fileMatchCount desc → then rating desc`.

#### 3. Update TrackDiggingRow + SQL

Add subqueries for play_count, rating, last_played from linked files (MAX aggregate). Add tag_category_count computation in Rust.

### Frontend: `frontend/pages/digging.js`

- Add ▶7 plays, ★4 rating, "3d ago" badges to track cards
- Add sort dropdown (Relevance, Plays, Rating, BPM, Energy, Tags) + ↑/↓ toggle
- Add BPM from/to number inputs (absolute filter, independent of ladder)
- Auto-load tracks on page open

### Files to modify

- `src/digging.rs`
- `frontend/pages/digging.js`
- `frontend/style.css`

### Acceptance Criteria

- [ ] `playCount`, `rating`, `lastPlayed` in API response
- [ ] Sort by playCount/rating/bpm/energy/tagCount all work
- [ ] Default sort (empty page): rating desc, playCount desc
- [ ] Card badges: plays, rating stars, last played
- [ ] Sort dropdown + direction toggle in filter bar
- [ ] BPM from/to inputs work independently
- [ ] Auto-load on page open
- [ ] Backend compiles

---

## Plan: digging-flat-ladder

**Status**: done ✅
**Branch**: `feat/digging-flat-ladder`
**Ready for review**: no
**Depends on**: `feat/digging-enrichment`
**Migration needed**: no

### Description

Redesign the Digging page: swap panes (browser left, ladder right), remove energy curve/steps concept, make the ladder a flat ordered list of identical track cards. Filters derive from ALL ladder tracks (not selected steps). Add drag-to-reorder, session persistence. Unified card design used identically in both panes.

### Layout

```
┌──────────────────────────────┬───────────────────────────────┐
│ BROWSER (left, 55%)          │ LADDER (right, 45%)           │
│                              │                               │
│ [search]  sort: [▾] ↑↓     │ #1 ██ Full track card         │
│ BPM from/to inputs           │    with waveform, play, ×    │
│                              │                               │
│ Filters (all toggleable):    │ #2 ██ Full track card         │
│ ☑ ⚡Energy 1-4              │    ...                        │
│ ☐ 🔑Keys (±1▾)             │                               │
│ ☐ 🎵BPM (±5▾)              │ #3 ██ Full track card         │
│ ☑ 🏷️Tags + chips           │    ...                        │
│                              │                               │
│ Track cards (paginated)      │ Computed from ladder:         │
│ ┌──────────────────────────┐│ BPM: 119-133 · Keys: 4m,5m   │
│ │ ⠿ Title · Artist         ││ Tags: deep, dark, house      │
│ │ 122BPM·4m·⚡3.2·▶7·★3  ││                              │
│ │ tags: deep dark house    ││ [Save Session] [Load]        │
│ │ [▶────waveform────]     ││ [Save as Playlist]           │
│ │ FLAC ✓ STEM ✓            ││                              │
│ └──────────────────────────┘│                               │
│                              │                               │
│ [Prev] Page N [Next]        │                               │
└──────────────────────────────┴───────────────────────────────┘
```

### Key changes from current

| Aspect              | Current                                        | New                                     |
| ------------------- | ---------------------------------------------- | --------------------------------------- |
| Panes               | Ladder left, browser right                     | Browser left, ladder right              |
| Ladder structure    | Energy curve steps (⚡1,⚡2...) with selection | Flat ordered list #1,#2,#3...           |
| Filter source       | Selected steps' energy/keys                    | ALL ladder tracks combined              |
| Ladder items        | Minimal text (title, BPM, energy, ×)           | Full track cards (identical to browser) |
| Reorder             | None                                           | Drag handle to reorder within ladder    |
| Session persistence | None                                           | Save/Load to localStorage               |
| Curve selector      | Sawtooth, Peak Hour, etc.                      | Removed                                 |

### Track card (unified, used in both panes)

```
┌──────────────────────────────────────────────────┐
│ ⠿  Title                                   [▶]  │
│    Artist                                        │
│                                                  │
│    122 BPM · 4m · ⚡3.2 · ▶7 · ★★★★            │
│    house  deep  dark  warehouse  +3 more         │
│                                                  │
│    ▂▃▄▅▆▇██▇▆▅▄▃▂▁▁▂▃▄▅▆▇██▇▆▅▄▃▂  0:45/5:32  │
│                                                  │
│    FLAC ✓(💾)  STEM ✓(💻)  |  Spotify · 3 lists  │
└──────────────────────────────────────────────────┘
```

In browser: ⠿ = drag handle (drag to ladder). In ladder: ⠿ = reorder handle.

### Filter logic

When ladder has tracks, filters derive from ALL tracks:

- ⚡Energy: all unique energy levels (±0.5 each), OR'd → `energyLevels=1,3,4`
- 🔑Key: all keys, expanded by user's range (±1/±2/A↔B) → `keyList=4m,5m,3d&keyRange=+1,-1,same`
- 🎵BPM: median BPM of all ladder tracks ± user slider → `bpmMin=...&bpmMax=...`
- 🏷️Tags: all non-Phase tags from ladder (OR) + user chips → `tags=deep,dark,house`

Each filter toggleable independently. Default: Energy ON, Tags ON, Keys OFF, BPM OFF.

### Session persistence

Save/Load to localStorage under key `diggingSession`:

```javascript
{
  ladder: [{ id, title, artist, bpm, musicalKey, energyLevel, ... }],
  filters: { energyEnabled, keyEnabled, bpmEnabled, tagsEnabled, keyRange },
  bpmRange, sortBy, sortOrder,
  savedAt: epochMs
}
```

Two buttons: "Save Session" (writes), "Load Session" (reads + restores). Auto-save on every change (debounced). Load on page open if session exists.

### Backend

No changes needed. `GET /api/digging/tracks` already supports all filter params.

### Files to modify

- `frontend/pages/digging.js` — major rewrite (~400 lines changed)
- `frontend/style.css` — layout adjustments

### Acceptance Criteria

- [ ] Browser on left, ladder on right
- [ ] Ladder is flat numbered list (no energy curve/steps)
- [ ] Identical track cards in both panes
- [ ] Drag from browser ⠿ to ladder adds at drop position
- [ ] Drag ⠿ within ladder reorders
- [ ] × on ladder card removes from ladder
- [ ] Filters derive from ALL ladder tracks (not selected steps)
- [ ] Energy, Key, BPM, Tags filters all toggleable
- [ ] Key range dropdown (±1, ±2, A↔B, etc.)
- [ ] BPM range slider adjusts ±N from ladder median
- [ ] Tag chips work (add/remove, OR with ladder tags)
- [ ] Search, sort, BPM from/to inputs all work
- [ ] Save Session / Load Session via localStorage
- [ ] Auto-save on every change (debounced 2s)
- [ ] Auto-restore session on page open
- [ ] Save as Playlist still works (collects all ladder track IDs)
- [ ] `cargo build` passes (no backend changes)

---

## Plan: digging-filter-row

**Status**: done ✅
**Branch**: `feat/digging-filter-row`
**Ready for review**: no
**Depends on**: `feat/digging-flat-ladder`
**Migration needed**: no

### Description

Add three persistent filter rows to the browser pane (PMV, KEY, Phase) that filter server-side alongside the ladder-derived toggles. These are independent AND filters — a track must match all active groups.

### Layout

```
┌─────────────────────────────────────────────────┐
│ PMV: [P] [M] [V]  |  Full  Partial  None       │
│ KEY: 1m 2m ... 12m  |  ALL m  NONE m            │
│      1d 2d ... 12d  |  ALL d  NONE d            │
│ Phase: End Start Release Build Sustain Peak     │
├─────────────────────────────────────────────────┤
│ ☑ ⚡Energy 1-4  ☑ 🔑Ladder keys  ☐ BPM  ☑ Tags │
└─────────────────────────────────────────────────┘
```

### Filter details

| Row   | Behavior                                                                                                                     | Backend param                        |
| ----- | ---------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| PMV   | Multi-select P/M/V + single-select Full/Partial/None. Picking category clears aggregate, picking aggregate clears categories | NEW: `pmvCategories`, `pmvAggregate` |
| KEY   | 24 toggle buttons. ALL m = select all minor. ALL/NONE per mode                                                               | Existing: `keyList`                  |
| Phase | 6 multi-select buttons. Adds phase tag names to OR tag filter                                                                | Existing: `tags`                     |

### Updated ladder-derived energy

Energy now uses range ±1 from ALL ladder tracks' energy levels:

```
Ladder: Start(⚡1) + Build(⚡4) + Release(⚡2)
→ 1±1 = 1,2,3; 4±1 = 3,4,5; 2±1 = 1,2,3
→ union: 1,2,3,4,5
→ energyLevels=1,2,3,4,5
```

### Backend: `src/digging.rs`

Add to `DiggingTracksQuery`:

```rust
pub pmv_categories: Option<String>,  // comma P,M,V
pub pmv_aggregate: Option<String>,   // "full", "partial", "none"
```

Add to `search_digging_tracks`:

- Parse `pmv_categories` into `Vec<String>`
- PMV category filter (OR): EXISTS subquery joining v_file_tags → tag_categories.prefix IN (...)
- PMV aggregate full (AND): 3 EXISTS subqueries for p, m, v prefixes
- PMV aggregate partial (OR): same as categories with all three
- PMV aggregate none (NOT): NOT EXISTS subquery for any PMV prefix

### Frontend: `frontend/pages/digging.js`

Add three filter rows above the existing toggle bar in `renderFilterBar()`. Update `loadTracks()` to send new params and compute energy range ±1.

### Acceptance Criteria

- [ ] P, M, V buttons multi-select; clicking toggles active
- [ ] Full/Partial/None mutually exclusive, clear categories on select
- [ ] KEY: all 24 buttons toggleable, ALL/NONE per mode work
- [ ] Phase: 6 buttons append Phase tag names to tags param
- [ ] Energy: ladder-derived now uses ±1 range from each track's energy (union)
- [ ] Filters compose: PMV AND key AND phase AND ladder-energy AND ladder-tags
- [ ] Backend compiles

---

## Plan: digging-audit-fixes

**Status**: done ✅
**Branch**: `fix/digging-audit`
**Ready for review**: no
**Depends on**: `feat/digging-filter-row`
**Migration needed**: no

### Description

Fix issues discovered during digging page audit: playback, card tag display, rating data, filter wiring verification.

### Issue 1: Playback

`pickAudioFile()` only accepted `location === "local"`. All production files are `location: "backup"`. Fixed to accept any file (prefers FLAC > stem.m4a). Verify `/api/files/{id}/stream` works for backup files.

### Issue 2: Card tags

Tags split into PHASE (with ⚡), MOOD, VIBE, TAGS rows by category prefix. Removed playlist badges (duplicated tag names). Removed averaged ⚡ badge.

### Issue 3: Rating

All ratings are 0. Traktor RANKING may not be in collection.nml. Show stars only when > 0.

### Issue 4: Filter wiring audit

Verify all filters (PMV, KEY, Phase, Energy, BPM, Tags, Search, Sort) work end-to-end with curl tests.

### Acceptance Criteria

- [ ] Playback works for tracks with any file (any location)
- [ ] Card tags organized by category with PHASE/MOOD/VIBE/TAGS rows
- [ ] No duplicate tag display
- [ ] Rating stars when > 0
- [ ] All filters verified end-to-end
- [ ] `cargo build` passes

---

## Plan: query-performance-optimization

**Status**: done ✅
**Branch**: `feat/query-performance-optimization`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `011_file_resolved_tags.sql`

### Description

Overhaul query performance for files, playlists, and digging pages. Replace the `v_file_resolved_tags` view (5-join chain with unindexable LOWER/TRIM) with a materialized `file_resolved_tags` table. Add batch comment computation. Fix the deemix playlist join to use exact match. Extract FileFilterBuilder to eliminate duplicated filter SQL.

### Files modified

- `migrations/011_file_resolved_tags.sql` — new migration: `file_resolved_tags` table + 4 indexes + 3 missing indexes (`file_locations`, `deemix_downloads`, `spt.deleted_at`)
- `src/db.rs` — new functions: `compute_target_comments_batch()`, `get_file_resolved_tags_batch()`, `refresh_file_resolved_tags()`
- `src/api.rs` — replaced all `v_file_resolved_tags`/`v_file_tags` view references with `file_resolved_tags` table; batch comment computation in `get_files()` and `get_files_count()`; fixed deemix `LIKE '%/'` → exact match
- `src/digging.rs` — batch tag loading in `search_digging_tracks()` instead of per-row N+1 queries

### Acceptance Criteria

- [x] Migration 011 runs cleanly on fresh DB (001→011)
- [x] Migration 011 runs cleanly on existing DB with data
- [x] `file_resolved_tags` table populated from `v_file_resolved_tags` view
- [x] All `v_file_resolved_tags` and `v_file_tags` view references replaced with `file_resolved_tags` table
- [x] Batch comment computation: `get_files()` with `commentStatuses=needs_update` uses 2 queries instead of N+1
- [x] Batch tag loading: `search_digging_tracks()` uses 1 query instead of N per-row queries
- [x] Deemix join uses exact match (`=`) instead of `LIKE '%/'`, indexable
- [x] New indexes: `idx_frt_tag_name`, `idx_file_locations_file_type`, `idx_deemix_downloads_url`, `idx_spt_deleted`
- [x] `cargo build` passes
- [x] No regressions: files, playlists, digging, tracks pages all work

## Plan: fix-comment-diff-display

**Status**: proposed
**Branch**: `fix/comment-diff-display`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Fix two bugs with the Files page comment diff column:

**Bug 1**: `✓null` shown for unchanged comments. When `comment` is null and target is also empty, `escapeHtml(null)` produces `"null"` in the HTML. Should show empty or `(empty)`.

**Bug 2**: When filtering for "Needs Update", some rows show unchanged comments (✓). The `renderCommentDiff` function uses `f.commentUnchanged` (client-computed) instead of `f.needsUpdate` (server-computed from `comment_needs_update`). These can disagree.

### Root cause

In `computeDiff`: `oldStr = oldComment || ""` handles null. But `renderCommentDiff` passes `f.comment` to `escapeHtml` which on null → `"null"`.

In `renderCommentDiff`: decision to show diff vs unchanged uses `f.commentUnchanged` from `computeDiff`. The server's `comment_needs_update` is also available as `f.needsUpdate` but is not used for rendering. When they disagree, the visual and the filter are mismatched.

### Fix

**`frontend/pages/files.js`** — `renderCommentDiff`:

- Use `f.needsUpdate` (server value) to decide whether to show diff or unchanged
- When unchanged and no comment, show `(empty)` instead of `null`
- When diff view, show `(empty)` for empty old/new values

```javascript
function renderCommentDiff(f) {
  if (f.needsUpdate) {
    return `<div class="diff-line">
      <div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld || "(empty)")}</div>
      <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>
    </div>`;
  }
  return `<div class="diff-line-unchanged"><span class="diff-sign check">✓</span>${f.comment ? escapeHtml(f.comment) : '<span class="text-muted">(empty)</span>'}</div>`;
}
```

### Acceptance Criteria

- [ ] "Needs Update" filter shows ONLY files with actual comment changes
- [ ] No `✓null` display — empty comments show `(empty)` instead
- [ ] Diff view shows `(empty)` for empty old/new lines
- [ ] No regressions: "Up to Date" filter still works
- [ ] `cargo build` passes (frontend only change)

---

## Plan: wav-source-linking

**Status**: proposed
**Branch**: `feat/wav-source-linking`
**Ready for review**: no
**Depends on**: `feat/file-lifecycle-management` (already merged)
**Migration needed**: yes — `012_wav_stem_type.sql`

### Description

Three-phase plan to properly handle nuo-stems WAV source files: link them to parent stems via `source_of`, track which stem part each WAV is (`stem_type`), make backed-up+linked WAVs prunable, and enrich track metadata with file variant information.

### Investigation Results (2026-05-28)

| File Type | Count |   Size | Backed Up | source_of set | stem_type tracked |
| --------- | ----: | -----: | :-------: | :-----------: | :---------------: |
| wav       | 6,647 | 277 GB | 6,647 ✅  |     0 ❌      |        ❌         |
| stem.m4a  | 1,770 |  90 GB |   1,770   |      N/A      |        N/A        |
| flac      | 2,205 |  64 GB |   1,602   |      N/A      |        N/A        |

**File system layout:**

```
/Users/momo/Music/stems/
├── WILL FERRO - Dreams.stem.m4a          ← stem file (top-level)
├── WILL_FERRO_Dreams/                    ← WAV source subdir (1,330 of these)
│   ├── WILL FERRO - Dreams_vocals.wav
│   ├── WILL FERRO - Dreams_bass.wav
│   ├── WILL FERRO - Dreams_drums.wav
│   ├── WILL FERRO - Dreams_instrumental.wav
│   └── WILL FERRO - Dreams_other.wav
```

**Naming convention discovered:** WAV files follow `{stem_name}_{stem_type}.wav` where `stem_type ∈ {vocals, bass, drums, instrumental, other}`. The stem file is `{stem_name}.stem.m4a` in the parent directory. This is reliably parseable — the stem*type is always the text after the LAST `*`before`.wav`, if it matches the known set.

**What's broken:**

1. `ScanWavSources` worker counts WAVs but never calls `set_file_source_of()` — `linked_to_stems` is declared `0usize` and never incremented (see `src/tasks/mod.rs` lines ~2520-2560)
2. `BackupWavs` worker passes `file_id=0` to `record_backup_result()` (line 2329), so it can't link backup records to the right files. (Current backup records came from the regular `BackupFolder` task, which passes correct `file.id`.)
3. `get_prune_candidates()` explicitly excludes WAVs with `f.file_type != 'wav'` — even backed-up + linked WAVs can't be pruned
4. No `stem_type` column exists — we know a WAV is a source file but not which part it is
5. No track enrichment — no way to see "this track has FLAC + stem + 5 WAV source files"

### Phase 1: Add stem_type + Populate source_of Linking

#### Migration 012 (`migrations/012_wav_stem_type.sql`)

```sql
-- Add stem_type column for tracking which nuo-stems part a WAV source file represents
ALTER TABLE files ADD COLUMN stem_type TEXT CHECK (
    stem_type IS NULL OR stem_type IN ('vocals', 'bass', 'drums', 'instrumental', 'other')
);

CREATE INDEX IF NOT EXISTS idx_files_stem_type ON files(stem_type);

SELECT 'Migration 012 applied: stem_type column on files' as status;
```

Rationale for dedicated column over `metadata_json`:

- `files` already uses dedicated columns for audio metadata (title, artist, genre, bpm, musical_key, etc.) — this fits the pattern
- CHECK constraint ensures data integrity at DB level
- Directly queryable: `SELECT * FROM files WHERE stem_type = 'vocals'`
- No JSON parsing overhead
- Self-documenting schema

#### Rust: `src/db.rs` — `File` struct

Add `stem_type: Option<String>` field to `File` struct (after `source_of`):

```rust
// Source WAV linking (WAV source subdirectory → stem file)
pub source_of: Option<i64>,

// Stem type for WAV source files (vocals, bass, drums, instrumental, other)
pub stem_type: Option<String>,
```

Update both `extract_minimal_file_metadata` and `extract_audio_metadata_from_file` to set `stem_type: None`.

#### Rust: `src/db.rs` — Preserve `source_of` and `stem_type` during re-scan

**Critical:** `scan_and_store_file()` (line 762) does INSERT + ON CONFLICT UPDATE without including `source_of` in either clause. If a WAV file is re-scanned (by folder watcher or manual scan), the linkage established by `ScanWavSources` would be silently lost. Same applies to the new `stem_type`.

Fix: add both columns to INSERT and use COALESCE in ON CONFLICT UPDATE to preserve existing values:

```rust
// In the INSERT column list, add:
source_of, stem_type,

// In VALUES, add two more bindings:
.bind(&file.source_of)
.bind(&file.stem_type)

// In ON CONFLICT DO UPDATE SET, add:
source_of = COALESCE(excluded.source_of, files.source_of),
stem_type = COALESCE(excluded.stem_type, files.stem_type),
```

Using COALESCE ensures: on first insert, values come from the file struct (NULL for both); on re-scan, the previously-set `source_of` and `stem_type` are preserved because the incoming values from `extract_audio_metadata_from_file` are NULL.

#### Rust: `src/db.rs` — `get_file_by_path()`

Note: `get_file_by_path()` already exists at `src/db.rs` line 1016 — no need to create it. Reuse the existing function.

#### Rust: `src/db.rs` — WAV→stem matching

New function `link_wav_to_stem()`:

```rust
/// Parse a WAV filename and link it to its parent stem file.
///
/// Pattern: `{stem_name}_{stem_type}.wav` where stem_type ∈ {vocals,bass,drums,instrumental,other}
/// The stem file is `{stem_name}.stem.m4a` in the parent of the parent directory.
///
/// Returns Some(file_id of stem) on success, None if no matching stem found.
pub async fn link_wav_to_stem(
    pool: &Pool<Sqlite>,
    wav_file_id: i64,
    wav_file_path: &str,
) -> Result<Option<(i64, String)>> {
    let path = std::path::Path::new(wav_file_path);
    let filename = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Known stem types in nuo-stems
    const STEM_TYPES: &[&str] = &["vocals", "bass", "drums", "instrumental", "other"];

    // Extract stem_type: text after last '_' before '.wav'
    let stem_name_no_ext = filename.strip_suffix(".wav").unwrap_or(filename);
    let (stem_name, stem_type) = if let Some(last_underscore) = stem_name_no_ext.rfind('_') {
        let candidate = &stem_name_no_ext[last_underscore + 1..];
        if STEM_TYPES.contains(&candidate) {
            (&stem_name_no_ext[..last_underscore], candidate.to_string())
        } else {
            // Unknown suffix — not a stem part WAV
            return Ok(None);
        }
    } else {
        // No underscore — not a stem part WAV
        return Ok(None);
    };

    // The stem file is in the parent of the parent directory:
    // /stems/ARTIST_Title/Artist - Title_vocals.wav
    //   → parent = /stems/ARTIST_Title
    //   → parent's parent = /stems
    //   → stem = /stems/Artist - Title.stem.m4a
    let parent = path.parent();  // /stems/ARTIST_Title
    let stems_root = parent.and_then(|p| p.parent());  // /stems

    let expected_stem_path = if let Some(root) = stems_root {
        format!("{}/{}.stem.m4a", root.display(), stem_name)
    } else {
        return Ok(None);
    };

    // Look up the stem file
    let stem = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE file_path = ? AND file_type = 'stem.m4a'"
    )
    .bind(&expected_stem_path)
    .fetch_optional(pool)
    .await?;

    match stem {
        Some(s) => {
            // Link: set source_of and stem_type
            sqlx::query("UPDATE files SET source_of = ?, stem_type = ? WHERE id = ?")
                .bind(s.id)
                .bind(&stem_type)
                .bind(wav_file_id)
                .execute(pool)
                .await?;
            Ok(Some((s.id, stem_type)))
        }
        None => Ok(None),
    }
}
```

Key design decisions in this algorithm:

- Uses the LAST `_` before `.wav` to find stem*type — works for titles containing `*`(e.g.,`Artist\_-_Title_vocals.wav`)
- Checks against known stem_type values — silently skips unknown suffixes
- Uses `file_path = ?` exact match lookup — indexed, fast
- Stem is in parent-of-parent directory — derived from WAV path structure, no need to guess directory naming

#### Rust: `src/tasks/mod.rs` — Fix `ScanWavSources` worker

Replace the stub counting loop (~lines 2550-2600) with actual linking:

```rust
let mut wav_indexed = 0usize;
let mut linked_to_stems = 0usize;  // was: const 0usize

for (i, subdir_name) in subdirs.iter().enumerate() {
    // ... cancel check ...

    let local_subdir = format!("{}/{}", local_dir.trim_end_matches('/'), subdir_name);
    let dir_path = std::path::Path::new(&local_subdir);

    if !dir_path.is_dir() { continue; }

    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            wav_indexed += 1;

            // Look up the WAV file in DB by path
            let wav_path_str = entry_path.to_string_lossy().to_string();
            if let Ok(Some(wav_file)) = crate::db::get_file_by_path(&db_clone, &wav_path_str).await {
                match crate::db::link_wav_to_stem(&db_clone, wav_file.id, &wav_path_str).await {
                    Ok(Some((stem_id, stem_type))) => {
                        linked_to_stems += 1;
                        tracing::debug!(
                            "Linked WAV {} (type={}) → stem #{}",
                            wav_path_str, stem_type, stem_id
                        );
                    }
                    Ok(None) => {
                        tracing::debug!("No matching stem for WAV: {}", wav_path_str);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to link WAV {}: {}", wav_path_str, e);
                    }
                }
            }
        }
    }
    // ... progress update ...
}

let msg = format!(
    "WAV source scan complete: {} WAV files indexed, {} linked to stems in {} subdirectories",
    wav_indexed, linked_to_stems, subdirs.len()
);
```

#### Rust: `src/tasks/mod.rs` — Fix `BackupWavs` worker

Replace `crate::db::record_backup_result(&db_clone, 0, true, file_size, &remote_wav_path)` at line 2329 with a proper file lookup:

```rust
// Look up the WAV file in DB by local path to get correct file_id
let local_wav_path = entry_path.to_string_lossy().to_string();
let file_id = if let Ok(Some(f)) = crate::db::get_file_by_path(&db_clone, &local_wav_path).await {
    f.id
} else {
    continue;  // skip files not in DB
};
let _ = crate::db::record_backup_result(
    &db_clone,
    file_id,
    true,
    file_size,
    &remote_wav_path,
)
.await;
```

### Phase 2: Allow Pruning of Backed-up + Linked WAVs

#### Rust: `src/db.rs` — Modify `get_prune_candidates()`

**Remove** the `f.file_type != 'wav'` exclusion. Replace with conditional logic:

- For **non-WAV** files: same logic as before (backed up + not followed → candidate)
- For **WAV** files: backed up + `source_of IS NOT NULL` + not followed → candidate with `reason = "wav_backed_up"`

Change the initial fetch from:

```sql
WHERE fl.location_type = 'backup' AND f.file_type != 'wav'
```

to:

```sql
WHERE fl.location_type = 'backup'
  AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL))
```

This ensures:

- WAVs without `source_of` (not yet linked) are NOT prune candidates — we need the metadata first
- WAVs with `source_of` that are backed up → eligible for pruning
- Non-WAV files: behavior unchanged

In the reason assignment, add:

```rust
let reason = if row.file_type == "wav" {
    "wav_backed_up".to_string()
} else {
    "not_followed".to_string()
};
```

Also add `reason` to the SQL SELECT, importing the value from the file_type:

```sql
SELECT f.id, f.file_path, f.file_type, f.file_size, ...
```

Then in Rust, assign reason based on file_type.

### Phase 3: Track Enrichment API

#### API: `GET /api/files/{id}/variants`

Returns all file variants for a track, grouped by common identity. Groups files by:

- Same ISRC (most reliable)
- Same `source_of` parent (WAVs belonging to same stem)

Response:

```json
{
  "fileId": 4362,
  "title": "Games People Play",
  "artist": "Paula van Klar",
  "isrc": "US7NS2500009",
  "variants": [
    {
      "id": 4362,
      "fileType": "stem.m4a",
      "filePath": "...",
      "fileSize": 12345,
      "backedUp": true
    },
    {
      "id": 4042,
      "fileType": "stem.m4a",
      "filePath": "...",
      "fileSize": 12345,
      "backedUp": true
    },
    {
      "id": 9801,
      "fileType": "flac",
      "filePath": "...",
      "fileSize": 45678,
      "backedUp": true
    },
    {
      "id": 9802,
      "fileType": "wav",
      "stemType": "vocals",
      "filePath": "...",
      "fileSize": 89012,
      "backedUp": true
    },
    {
      "id": 9803,
      "fileType": "wav",
      "stemType": "bass",
      "filePath": "...",
      "fileSize": 89012,
      "backedUp": true
    }
  ]
}
```

Implementation in `src/db.rs`:

```rust
pub async fn get_file_variants(pool: &Pool<Sqlite>, file_id: i64) -> Result<FileVariants> {
    // First, get the file to find its ISRC
    let file = get_file_by_id(pool, file_id).await?.ok_or_else(|| anyhow!("File not found"))?;

    // Find all files with same ISRC (if ISRC is not null)
    let mut variant_ids = std::collections::HashSet::new();
    variant_ids.insert(file.id);

    if let Some(ref isrc) = file.isrc {
        let same_isrc: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM files WHERE isrc = ? AND id != ?"
        )
        .bind(isrc)
        .bind(file.id)
        .fetch_all(pool)
        .await?;
        variant_ids.extend(same_isrc);
    }

    // Also include WAV source files (source_of points to this stem)
    let wav_sources: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM files WHERE source_of = ? AND file_type = 'wav'"
    )
    .bind(file.id)
    .fetch_all(pool)
    .await?;
    variant_ids.extend(wav_sources);

    // If this file is a WAV, include its stem parent and siblings
    if let Some(source_of) = file.source_of {
        variant_ids.insert(source_of);
        let sibling_wavs: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM files WHERE source_of = ? AND id != ?"
        )
        .bind(source_of)
        .bind(file.id)
        .fetch_all(pool)
        .await?;
        variant_ids.extend(sibling_wavs);
    }

    // Fetch full details for all variants
    let placeholders: Vec<String> = variant_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size, f.stem_type,
                CASE WHEN fl.id IS NOT NULL THEN 1 ELSE 0 END as backed_up
         FROM files f
         LEFT JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
         WHERE f.id IN ({})
         ORDER BY f.file_type, f.stem_type",
        placeholders.join(",")
    );
    // ... bind and fetch ...
}
```

#### Route

```rust
.route("/api/files/{id}/variants", get(file_variants_handler))
```

#### Frontend: File Detail page (`frontend/pages/file-detail.js`)

Add a "Variants" section below the metadata, showing:

- List of all file variants with type badges (stem, flac, wav-vocals, wav-bass, etc.)
- Backup status per variant (✓ backed up / ✗ local only)
- File size per variant

### Files to modify

- `migrations/012_wav_stem_type.sql` — new migration
- `src/db.rs` — `File` struct + `stem_type`, `link_wav_to_stem()`, `get_file_by_path()`, `get_file_variants()`, update `get_prune_candidates()`
- `src/tasks/mod.rs` — fix `ScanWavSources` worker (actual linking), fix `BackupWavs` worker (correct file_id)
- `src/api.rs` — add `GET /api/files/{id}/variants` route + handler
- `frontend/pages/file-detail.js` — variants section
- `frontend/style.css` — variant badge styles

### Acceptance Criteria

**Phase 1:**

- [ ] Migration 012 runs cleanly on fresh DB (001→012)
- [ ] Migration 012 runs cleanly on existing DB with data
- [ ] `stem_type` column added with CHECK constraint
- [ ] `link_wav_to_stem()` correctly parses: `WILL FERRO - Dreams_vocals.wav` → stem_type=`vocals`, links to `WILL FERRO - Dreams.stem.m4a`
- [ ] `link_wav_to_stem()` handles edge cases: unknown suffix → skips, no underscore → skips, no matching stem → skips
- [ ] `link_wav_to_stem()` handles titles with `_` (e.g., `Artist_-_Title_vocals.wav`)
- [ ] `link_wav_to_stem()` handles names with parentheses (e.g., `Jon.K - Madness (Malandra Jr. Remix)_bass.wav`)
- [ ] `ScanWavSources` task populates `source_of` and `stem_type` for WAVs with matching stem files (~81% of 6,647 = ~5,405 linked; remaining ~1,242 skipped gracefully)
- [ ] `ScanWavSources` task logs counts: WAVs indexed, linked to stems, skipped
- [ ] `BackupWavs` task uses correct file_id in `record_backup_result()`
- [ ] `scan_and_store_file()` preserves existing `source_of` and `stem_type` on re-scan (COALESCE)
- [ ] `cargo build` passes

**Phase 2:**

- [ ] Backed-up WAVs with `source_of IS NOT NULL` appear as prune candidates with `reason = "wav_backed_up"`
- [ ] Backed-up WAVs without `source_of` are NOT prune candidates (not yet linked)
- [ ] Non-WAV prune behavior unchanged
- [ ] `cargo build` passes

**Phase 3:**

- [ ] `GET /api/files/{id}/variants` returns all variants grouped by ISRC + source_of
- [ ] Response includes `fileType`, `stemType` (for WAVs), `fileSize`, `backedUp`
- [ ] File detail page shows variants section
- [ ] `cargo build` passes

### One-time operation after deploy

After Phase 1 is deployed, run the `ScanWavSources` task on folder #1 (stems) to populate `source_of` and `stem_type` for all 6,647 existing WAVs. This is a one-time batch — future scans via the folder watcher will pick up new WAVs incrementally.

---

## Plan: backup-as-truth

**Status**: proposed
**Branch**: `feat/backup-as-truth`
**Ready for review**: no
**Depends on**: `feat/wav-source-linking` (Phase 1-2 done, Phase 3 partial)
**Migration needed**: yes — `013_backup_discovery.sql`

### Description

Rethink the file lifecycle model: treat the NAS backup as the **source of truth** and local disk as a **working cache**. Files flow: NAS → load to local → Traktor scan for BPM/key → write comment → confirm backup → delete local. This requires: tracking local file presence explicitly, discovering backup-only files, enriching track-detail with WAV source variants, and changing the prune criteria to require metadata completeness.

### Investigation Results (2026-05-28)

**Current data model gap:**

| Concept               |       Currently Tracked        | Issue                                                                                                  |
| --------------------- | :----------------------------: | ------------------------------------------------------------------------------------------------------ |
| File metadata         |         `files` table          | ✅ OK                                                                                                  |
| Backup location       | `file_locations` (type=backup) | ✅ OK                                                                                                  |
| Local presence        |        **Not tracked**         | ❌ Implicit: "being in `files` means local" — but files can be deleted locally and still be in `files` |
| Backup-only files     |       **Not discovered**       | ❌ Files on NAS with no local DB record are invisible to the app                                       |
| Metadata completeness |           Partially            | ❌ 10,622 files total, only 3,726 have BPM+Key, 3,849 have comments                                    |

**Key findings:**

- 0 `file_locations` entries with `location_type='local'` — local presence is implicit
- 10,019 files backed up, 603 not backed up (all FLACs)
- **track-detail page has NO variant/source integration** — `get_track_detail()` doesn't traverse `source_of`
- Track #1487 (Boris Brejcha - Black Unicorn) has 2 linked files (FLAC + stem) but 5 WAV sources linked to the stem are invisible
- WAV linking works in isolation: `GET /api/files/{id}/variants` shows WAV sources when querying the stem file

**The conceptual gap:**

1. `track-detail` → linked files → but stops there, doesn't traverse `source_of` to find WAV sources
2. `file-detail` → variants → works for the specific file, but the track-detail user never sees this connection
3. Backup reconciliation only matches REMOTE files against LOCAL files in DB — if a file was deleted locally, it stays in DB but there's no way to know it's no longer on disk
4. Files on NAS that were NEVER scanned locally are completely invisible

### Part A: Enrich Track Detail with WAV Source Variants

Extend `get_track_detail()` and the track-detail page to show all file variants, not just directly-linked files.

#### Backend: `src/db.rs` — extend `get_track_detail()`

After fetching linked files (step 2), add a step to fetch their WAV source children:

```rust
// Step 2b: For each linked stem file, fetch WAV source files
let stem_ids: Vec<i64> = files.iter().map(|f| f.id).collect();
if !stem_ids.is_empty() {
    // Find WAV files whose source_of points to any linked file
    let placeholders: Vec<String> = stem_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size, f.stem_type, f.isrc,
                f.title, f.artist, f.bpm, f.musical_key, f.duration_ms,
                COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
                fl_backup.path as backup_path
         FROM files f
         LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
         WHERE f.source_of IN ({}) AND f.file_type = 'wav'
         ORDER BY f.stem_type",
        placeholders.join(",")
    );
    let mut query = sqlx::query_as::<_, TrackDetailFile>(&sql);
    for id in &stem_ids {
        query = query.bind(id);
    }
    let wav_files = query.fetch_all(pool).await.unwrap_or_default();
    files.extend(wav_files);
}
```

The WAV files already have `stem_type` set — the frontend can use it to render "WAV (vocals)", "WAV (bass)", etc.

#### Backend: `src/db.rs` — `TrackDetailFile` struct

Add `stem_type: Option<String>` field to `TrackDetailFile`:

```rust
#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailFile {
    pub id: i64,
    pub file_path: String,
    pub file_type: String,
    pub stem_type: Option<String>,  // NEW: for WAV sources
    // ... existing fields ...
}
```

Make sure the SQL queries in `get_track_detail()` include `f.stem_type` in the SELECT.

#### Frontend: `frontend/pages/track-detail.js`

Add variant cards rendering for each linked file. When a file has `stem_type` set, show it as a WAV source badge.

In the renderFileInfo section (or as a new Variants section), iterate `data.files` and show:

- File type badge (FLAC, stem.m4a, or WAV with stem_type label)
- Backup status indicator
- File size

Group by `source_of` / file type to make the relationship visible:

```
stem.m4a (124 BPM, 9m) ✓ backed up
  ├── WAV vocals    ✓
  ├── WAV bass      ✓
  ├── WAV drums     ✓
  ├── WAV instrumental ✓
  └── WAV other     ✓
FLAC                   ✓
```

### Part B: Track Local File Presence

Add explicit `file_locations` entries with `location_type = 'local'` so we can distinguish "file is on disk" from "file is in DB but deleted."

#### Migration 013 (`migrations/013_backup_discovery.sql`)

```sql
-- Migration 013: Track local file presence + backup discovery support

-- Add last_verified_local to files for tracking when the file was last confirmed on disk
-- NULL = never been local (backup-only), timestamp = last confirmed on disk
ALTER TABLE files ADD COLUMN last_verified_local INTEGER;

-- Note: DO NOT backfill file_locations.local entries here!
-- The scanner populates them on next scan (see scan_and_store_file changes).
-- A blind backfill would mark deleted files as "local" — false positives.

SELECT 'Migration 013 applied: local file tracking + backup discovery support' as status;
```

#### Rust: `src/db.rs` — `File` struct

Add `last_verified_local: Option<i64>` field to `File` struct.

#### Rust: `src/db.rs` — Scanner changes

**`scan_and_store_file()`** already preserves `source_of`/`stem_type` via COALESCE (done in earlier plan). Now also needs to create/update a `file_locations` entry with `location_type = 'local'` after the successful INSERT/UPDATE:

```rust
// In scan_and_store_file, after successful INSERT/UPDATE (before Ok(row)):
let _ = sqlx::query(
    "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
     VALUES (?, 'local', ?, ?, unixepoch(), unixepoch())
     ON CONFLICT(file_id, location_type) DO UPDATE SET
         file_size = excluded.file_size,
         last_verified = excluded.last_verified"
)
.bind(row.id)
.bind(&row.file_path)
.bind(row.file_size)
.execute(pool)
.await;

// Also update last_verified_local on the files row
let _ = sqlx::query("UPDATE files SET last_verified_local = unixepoch() WHERE id = ?")
    .bind(row.id)
    .execute(pool)
    .await;
```

#### Cleanup of stale local entries

The folder watcher polls active folders every 5 min. When a file that was previously scanned disappears from disk, the scanner correctly skips it (no re-scan). But the `file_locations.local` entry from the previous scan persists forever.

**Fix**: Add this cleanup to `scan_folder()` in `src/db.rs` (called by both manual scans via API and automatic scans via the folder watcher). This ensures stale local entries are purged regardless of scan trigger. After the walk loop, before `Ok(count)`, add:

This mirrors how `scan_and_store_file` works: it UPDATEs `last_scanned` for every file it encounters. Files that weren't encountered keep their old `last_scanned`:

```rust
// After folder scan completes, remove stale local entries
let folder_path = /* ... */;
sqlx::query(
    "DELETE FROM file_locations WHERE location_type = 'local'
     AND file_id IN (
         SELECT f.id FROM files f
         JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
         WHERE fol.id = ? AND f.last_scanned < ?
     )"
)
.bind(folder_id)
.bind(scan_start_time)
.execute(pool)
.await?;
```

This ensures `file_locations.local` always reflects reality.

#### Frontend: File detail + Storage page

Show local presence status clearly:

- ✓ Local (verified 2 days ago)
- ✓ Backed up (verified 2 days ago)
- ✗ Local (not on disk)

### Part C: Backup-Only File Discovery

Add the ability to scan the NAS and discover files that exist ONLY on backup, not in the local DB. These are files that were backed up through some other process, or backed up and then the DB record was lost.

#### New API endpoint: `POST /api/storage/discover-backup/{folder_id}`

Triggers a background task that:

1. Lists ALL files on the NAS backup destination for a folder with **full remote paths** (not just filenames)
2. For each remote file, checks if there's a matching entry in the `files` table (by filename match — all 10,622 filenames are currently unique)
3. For files that EXIST in `files` but NOT on backup: logs warning (removed from NAS?)
4. For files that exist on backup but NOT in `files`: creates a "backup-only" file record:
   - Create a `files` row with `file_path` reconstructed from `folder.folder_path` + remote relative path
   - Create `file_locations` entry with `location_type='backup'`
   - Mark as `last_verified_local = NULL` (never been local)
   - **`file_hash`**: `files.file_hash` is `NOT NULL` — use a sentinel like `"backup-only-{file_size}"` for files we can't compute a real hash on.

**Potential fragility**: The current `list_remote_files_with_depth()` in `src/backup/mod.rs` strips paths to basenames only (line 149). For backup discovery + pull-from-backup, we need full remote paths. Create a new method `list_remote_files_full()` that returns paths relative to backup base — keeps the existing function unchanged for reconciliation.

**Filename matching**: Match by reconstructed full path (`folder.folder_path` + remote relative path), not by basename alone. Though all local filenames are currently unique, the NAS can have identically-named files in different subdirectories (e.g., `Artist1/Title_vocals.wav` and `Artist2/Title_vocals.wav`).

```rust
pub struct BackupDiscoveryResult {
    pub files_on_backup: usize,       // total files found on NAS
    pub already_tracked: usize,        // files already in DB with backup record
    pub newly_discovered: usize,       // files on backup but not in DB → created
    pub missing_from_backup: Vec<(i64, String)>,  // in DB but not on backup (path, reason)
}
```

This gives the app a complete inventory of what's on backup.

### Part D: Metadata Completeness as Prune Gate

Change the prune criteria: being backed up is NOT enough. A file is safe to delete locally only when:

1. ✅ Backed up to NAS (has `file_locations` with `type='backup'`)
2. ✅ Metadata extracted (has `bpm IS NOT NULL` or `comment IS NOT NULL` — at least one)
3. ✅ Not followed (not in any followed tag)
4. ✅ Local presence confirmed (has `file_locations` with `type='local'`)

**Rationale:**

- WAV source files have NO metadata (they're raw audio) — but they're linked to stems. The condition for WAVs should be: `source_of IS NOT NULL` (linked to stem) instead of metadata check.
- Stems and FLACs should have BPM/key or comment extracted before safe deletion.
- 603 FLACs lack backup — they should be backed up first.

#### Modified prune query:

```sql
-- Step 1: backed-up file IDs (now includes WAVs)
SELECT DISTINCT fl.file_id FROM file_locations fl
JOIN files f ON f.id = fl.file_id
WHERE fl.location_type = 'backup'
  AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL));

-- Step 2: filter for metadata completeness (for non-WAV files)
-- Non-WAV: must have bpm or comment
-- WAV: source_of IS NOT NULL is already checked above
AND (
    f.file_type = 'wav'
    OR f.bpm IS NOT NULL
    OR (f.comment IS NOT NULL AND f.comment != '')
);

-- Step 3: not followed (existing filter)
-- Step 4: has local presence (should be deletable)
```

### Part F: Replace Format Preference Toggle with Interactive Prune Preview Filters

**Problem with current "Prefer stem files" toggle:**

The existing checkbox in the Storage page suffers from four issues:

1. **Wrong numbers**: The hint says "Currently 2,205 FLACs would become prune candidates" — but that's the total FLAC count, not the actual data-dependent number
2. **Ambiguous language**: "Prefer stem files" — prefer them for what? Keeping locally? Making others deletable?
3. **No preview before toggling**: You can't see what will change without toggle + re-preview
4. **Global, static preference**: A single on/off is too coarse — what if you want to keep stems for one artist and FLACs for another?

**Replace with: Format relationship badges + prune preview filters**

Remove the global toggle and the `stem_preferred` config. Instead, move the logic into the prune preview table where the user can make informed per-file decisions.

#### Backend: `PruneCandidate` struct

Add a `has_stem_variant: bool` field:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneCandidate {
    pub file_id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub reason: String,      // "not_followed" | "wav_backed_up"
    pub backup_path: Option<String>,
    pub has_stem_variant: bool,  // NEW: same-ISRC stem.m4a exists in DB
}
```

In `get_prune_candidates()`, compute `has_stem_variant` per candidate:

```rust
let has_stem: bool = if let Some(ref isrc) = isrc {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE isrc = ? AND file_type = 'stem.m4a' AND id != ?"
    )
    .bind(isrc)
    .bind(file_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    count > 0
} else {
    false
};
```

This is computed per-row (N+1), but prune previews are typically small (bounded to a few thousand). Alternatively, batch-fetch all ISRCs in one query for efficiency.

**Remove `stem_preferred`:**

- Remove the `stem_preferred` parameter from `get_prune_candidates()`
- Remove `GET/PUT /api/storage/settings` endpoint (or keep it for future settings)
- Remove the `StemPreference` toggle from Storage page
- The prune preview now returns ALL backed-up + not-followed files regardless of format

#### Frontend: Prune preview table

Each row shows:

```
☐  FLAC   Boris Brejcha - Black Unicorn   56 MB   [stem variant ✓]   not_followed
☐  WAV/vocals  ...                                 [stem variant ✓]   wav_backed_up
```

Filter toolbar above the table:

```
☐ Show only files with stem variant    ☐ Show only FLACs    ☐ Show only WAVs
[Select all with stem variant]  [Deselect all]
```

Bulk action buttons:

- "Delete Selected" (always visible)
- "Select all redundant" — selects all files where `has_stem_variant = true`

#### Why this is better

| Aspect              |      Current toggle      |            Proposed filter table            |
| ------------------- | :----------------------: | :-----------------------------------------: |
| See what changes    |         ❌ Blind         | ✅ Each file shows `has_stem_variant` badge |
| Control granularity |     ❌ Global on/off     |            ✅ Per-file checkbox             |
| Understand impact   | ❌ "Prefer" is ambiguous |      ✅ "Has stem variant" is factual       |
| Safety              |  ❌ Toggle + re-preview  |        ✅ Deselect individual files         |
| Adaptable           |       ❌ Hardcoded       |         ✅ Filter by any attribute          |

#### Files to modify

- `src/db.rs` — `PruneCandidate` struct + add `has_stem_variant`, remove `stem_preferred` from `get_prune_candidates()`, update `get_storage_status()`
- `src/api.rs` — `prune_preview_handler`/`prune_execute_handler` pass `false` (no more setting), optionally remove settings endpoint
- `frontend/pages/storage.js` — replace toggle with filter toolbar, add `has_stem_variant` column + bulk select, remove `renderStemPreference`
- `frontend/style.css` — filter toolbar styles

#### Acceptance Criteria

- [ ] `get_prune_candidates()` no longer takes `stem_preferred` parameter
- [ ] `PruneCandidate` includes `hasStemVariant` field
- [ ] FLACs with same-ISRC stem.m4a show `hasStemVariant: true`
- [ ] FLACs without same-ISRC stem.m4a show `hasStemVariant: false`
- [ ] WAVs with `source_of` show `hasStemVariant: true` (they ARE the stem variant)
- [ ] Prune preview table has filter: "Show only files with stem variant"
- [ ] "Select all with stem variant" bulk action works
- [ ] Old toggle removed, `stem_preferred` config key deprecated
- [ ] `cargo build` passes

### Part E: Pull-from-Backup Workflow

New API endpoint: `POST /api/files/{id}/pull-from-backup`

Copies a file that exists only on backup back to local disk — into the correct folder.

**Transparent path resolution**: The file goes back where it belongs:

- FLAC backup → local FLAC directory (`/Users/momo/Music/flacs/`)
- Stem backup → local stem directory (`/Users/momo/Music/stems/`)
- The local destination is determined by: (a) which folder the file originally belonged to, or (b) if it was never local, matching its remote path to the configured folder's `backup_path`.

**Configurable format preference**: When pulling a backup-only file, the user can specify which format they want via a query parameter `?prefer=stem` or `?prefer=flac`. If the preferred format isn't available, it falls back to whatever exists. This gives the user control — "download the FLAC version" vs "download the stem version".

```rust
async fn file_pull_from_backup_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(params): Query<PullFromBackupParams>,
) -> impl IntoResponse {
    // 1. Get the file record
    // 2. Check it has a backup location
    // 3. Determine local destination:
    //    - Find which folder this file path matches (flacs vs stems)
    //    - Construct local path: folder.folder_path + relative_path
    // 4. Rsync from backup (NAS) to local
    // 5. Update file_locations: add 'local' entry
    // 6. Update last_verified_local
    // 7. Return success + local path
}
```

This completes the cycle: backup → pull → scan → extract metadata → re-backup → delete.

### Files to modify

- `migrations/013_backup_discovery.sql` — new migration
- `src/db.rs` — `File` struct + `last_verified_local`, `TrackDetailFile` + `stem_type`, extend `get_track_detail()`, scanner lints `file_locations.local`, new `discover_backup_files()`, modified prune logic, `PruneCandidate` + `has_stem_variant`, remove `stem_preferred` param from `get_prune_candidates()`
- `src/api.rs` — extend `get_track_detail` response, `POST /api/storage/discover-backup/{folder_id}`, `POST /api/files/{id}/pull-from-backup`, route additions, remove/update `GET/PUT /api/storage/settings`
- `src/tasks/mod.rs` — new `BackupDiscovery` task type + worker
- `frontend/pages/track-detail.js` — WAV source variants section
- `frontend/pages/file-detail.js` — local presence indicator (variants section already exists)
- `frontend/pages/storage.js` — replace stem toggle with filter toolbar, backup discovery button, `has_stem_variant` column + bulk select
- `frontend/style.css` — filter toolbar styles, variant card styles (reuse from file-detail)

### Acceptance Criteria

**Part A:**

- [ ] `GET /api/tracks/{id}/detail` includes WAV source files (via `source_of`) in the `files` array
- [ ] WAV files have `stemType` field populated (null for non-WAV)
- [ ] Track-detail page shows WAV source variants grouped under their parent stem file
- [ ] Boris Brejcha - Black Unicorn (#1487) shows: 1 stem + 1 FLAC + 5 WAVs (grouped)
- [ ] `cargo build` passes

**Part B:**

- [ ] Migration 013 backfills `file_locations.local` for all existing files
- [ ] `scan_and_store_file()` creates/updates `file_locations.local` on scan
- [ ] `last_verified_local` updated on scan
- [ ] File detail shows local presence status
- [ ] `cargo build` passes

**Part C:**

- [ ] `POST /api/storage/discover-backup/{folder_id}` triggers background task
- [ ] Task lists NAS files, matches against DB, creates records for backup-only files
- [ ] Result shows: files_on_backup, already_tracked, newly_discovered, missing_from_backup
- [ ] Newly discovered files get `files` row + `file_locations.backup` entry
- [ ] `cargo build` passes

**Part D:**

- [ ] Prune candidates require: backed up + (metadata complete OR is linked WAV source) + not followed
- [ ] Files lacking BPM/key/comment are excluded from prune candidates (even if backed up)
- [ ] 603 unbacked-up FLACs not in prune (as before)
- [ ] `cargo build` passes

**Part E:**

- [ ] `POST /api/files/{id}/pull-from-backup` copies file from NAS to local
- [ ] Updates `file_locations.local` entry
- [ ] Updates `last_verified_local`
- [ ] Fails gracefully if no backup location exists
- [ ] `cargo build` passes

**Part F:**

- [ ] `get_prune_candidates()` no longer takes `stem_preferred` parameter
- [ ] `PruneCandidate` includes `hasStemVariant` field
- [ ] FLACs with same-ISRC stem.m4a show `hasStemVariant: true`
- [ ] FLACs without same-ISRC stem.m4a show `hasStemVariant: false`
- [ ] WAVs with `source_of` show `hasStemVariant: true` (they ARE the stem variant)
- [ ] Prune preview table has filter: "Show only files with stem variant"
- [ ] "Select all with stem variant" bulk action works
- [ ] Old toggle removed, `stem_preferred` config key deprecated
- [ ] All prune preview features work without a full page reload
- [ ] `cargo build` passes

---

## Plan: fix-local-file-tracking

**Status**: done ✅
**Branch**: `fix/local-file-tracking`
**Depends on**: `feat/backup-as-truth` (already implemented)
**Migration needed**: no

### Description

Fix the disconnect between the data model and the UI. The `file_locations` table correctly tracks local vs backup presence, but every page queries `SELECT * FROM files` which counts ALL tracked files (including backup-only ones deleted from disk). The Storage page shows 10,638 "Local Files" when only ~3,361 are actually on disk. The Files page can't distinguish "on disk" from "backup-only". Fix: add `is_local` to every File API response, fix Storage page counts, add local-presence filter to Files page.

### Current State (from investigation 2026-06-03)

| Data point                      | Value                | Source                                                                             |
| ------------------------------- | -------------------- | ---------------------------------------------------------------------------------- |
| DB records (`files` table)      | 10,638               | `SELECT COUNT(*) FROM files`                                                       |
| Actually on disk                | ~3,361               | `ls` + `find` on filesystem                                                        |
| `file_locations.local` entries  | 8 (should be ~3,361) | Scanner code exists but never populated (incremental scan skipped unchanged files) |
| `file_locations.backup` entries | 10,019               | Correct ✅                                                                         |
| Files without backup            | 619 (all FLACs)      | Need backup before pruning                                                         |
| Prune candidates                | 0                    | Query requires `file_locations.local` → only 8 files have it                       |

**Root cause**: The scanner code to create `file_locations.local` was added (in `backup-as-truth` Part B) but the scanner never ran a full scan since then. The last scan was incremental, skipping all unchanged files. Only 8 files got local entries (presumably new/modified files picked up by the watcher).

**What IS correct**: The data model (`file_locations` with `local`/`backup`) is clean. The UI just queries the wrong data source.

### Backend Changes

#### 1. `src/db.rs` — `get_storage_status()`: count from file_locations.local

Replace `SELECT COUNT(*) FROM files` with counts from `file_locations WHERE location_type = 'local'`:

```rust
// Before (wrong): counts ALL tracked files
let local_file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
    .fetch_one(pool).await.unwrap_or(0);

// After (correct): counts only files currently on disk
let local_file_count: i64 = sqlx::query_scalar(
    "SELECT COUNT(DISTINCT file_id) FROM file_locations WHERE location_type = 'local'"
)
    .fetch_one(pool).await.unwrap_or(0);
```

Same change for `local_size_bytes`, and all per-type counts (`local_stems`, `local_flacs`, `local_wavs`, `local_mp3s`). Join through `file_locations` with `location_type = 'local'`.

Add a new field: `tracked_file_count: i64` for the total archive size (`COUNT(*) FROM files`). Rename existing fields for clarity — keep `local_file_count` meaning "on disk" (data source changes from `files` to `file_locations.local`).

#### 2. `src/db.rs` — `get_files()`: add `is_local` field

The `get_files()` function builds dynamic SQL. Add a LEFT JOIN to `file_locations` for local presence:

```sql
SELECT f.*,
  COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
  COALESCE(fl_local.id IS NOT NULL, 0) as is_local
FROM files f
LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
LEFT JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
WHERE 1=1 ...
```

Add an `isLocal` filter to `FilesQuery`:

```rust
pub is_local: Option<bool>, // Some(true) = only local, Some(false) = only backup-only, None = all
```

When `is_local = Some(true)`: add `AND fl_local.id IS NOT NULL` — only files on disk.
When `is_local = Some(false)`: add `AND fl_local.id IS NULL` — only backup-only files.

#### 3. `src/api.rs` — `ApiFile` struct: add `is_local` field

```rust
pub struct ApiFile {
    // ... existing fields ...
    pub backed_up: bool,
    pub is_local: bool,       // NEW
    pub has_stem: bool,
    pub safe_to_delete: bool,
}
```

The `backedUp` filter already exists in `FilesQuery`. Keep it. Add the `isLocal` filter alongside it.

### Frontend Changes

#### 4. `frontend/pages/storage.js` — Overhaul Storage cards

Current cards: Local Files | Backed Up | Prune Candidates.

New cards:

```
On Disk         │ On Backup        │ Tracked
3,361 files     │ 10,019 files     │ 10,638 files
70 GB           │                  │ 431.9 GB (archive)

Not Backed Up   │ Prune Candidates
619 FLACs       │ X files · Y GB can be freed
64.3 GB         │
```

All counts come from `StorageStatus` response fields. The frontend already receives the data correctly after the backend fix (step 1).

#### 5. `frontend/pages/files.js` — Add "On Disk" filter button

In the toolbar filter panel (RIGHT column, near the Backup filter), add:

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="local">On Disk</span>
  <div class="filter-group">
    <button class="filter-btn" data-local-filter="all">All</button>
    <button class="filter-btn" data-local-filter="yes">
      <i class="fas fa-hdd"></i> Yes
    </button>
    <button class="filter-btn" data-local-filter="no">
      <i class="fas fa-cloud"></i> No
    </button>
  </div>
</div>
```

Add `isLocal: null` to state and hash. Wire `buildParams` to send `isLocal` and `buildFilterParams` to pass it. The comment writer should show a warning/badge when the file is not local ("Backup only — can't write comment").

### Files to modify

- `src/db.rs` — fix `get_storage_status()` counts, add `is_local` to `get_files()`, add `isLocal` to `FilesQuery`
- `src/api.rs` — add `is_local: bool` to `ApiFile`, add `isLocal` param support in `files_handler`
- `frontend/pages/storage.js` — overhaul card layout to 5 cards
- `frontend/pages/files.js` — add "On Disk" filter button, non-local warning in comment writer
- `frontend/style.css` — storage card layout adjustments if needed

### Not in this plan

- Running a full scan to populate `file_locations.local` — **CRITICAL**: without this, Storage page shows 0 "On Disk" files (worse than current). Scan trigger MUST be added to this plan.
- Backing up the 619 unbacked-up FLACs — user triggers manually via Storage page

### Acceptance Criteria

- [ ] `get_storage_status()` counts local files from `file_locations.local`, NOT from `files`
- [ ] Storage page shows "On Disk" count — with warning when local entries are empty
- [ ] Storage page shows "Tracked" as total archive size (`COUNT(*) FROM files`)
- [ ] Storage page shows "Not Backed Up" count (files without backup records)
- [ ] Full scan trigger on Storage page when `file_locations.local` is empty
- [ ] `ApiFile` includes `isLocal: bool` field
- [ ] Files page has "On Disk" filter (All / Yes / No)
- [ ] Files page shows non-local indicator when file is backup-only
- [ ] `isLocal` filter included in `get_files_count()` count query
- [ ] Comment writer deactivated or warns when file is not local
- [ ] `TracksQuery.has_local` fixed — currently queries `v_file_track_link` without joining `file_locations` for local presence
- [ ] `ApiFile.safe_to_delete` logic reviewed: currently `backed_up && has_stem` — should also check `is_local`
- [ ] `cargo build` passes
- [ ] No regressions: backup/reconcile/prune flows unchanged

### Additional concerns (investigated, not yet planned)

#### Track-centric file view

The user thinks in TRACKS, not files. A track can have multiple file versions: FLAC, stem.m4a, and WAV source parts. The track-detail page (`#track-detail?id=1487`) shows ALL file variants grouped by type:

- **Now working**: API returns 7 files (1 FLAC + 1 stem + 5 WAVs), frontend splits them into "Linked Files" + "WAV Sources" sections
- **Gap**: The WAV files show `backedUp: true` but are NOT on local disk. The path `/Users/momo/Music/stems/Boris_Brejcha_Black_Unicorn/...` is stale — the directory was deleted locally after backup. Without `is_local`, the UI can't tell the user "this file exists on backup only."
- Verified: 10+ tracks in the DB have all three types (WAV + FLAC + stem) simultaneously, but many WAVs are backup-only

#### All file versions on backup

619 FLACs are not yet backed up. Every track's file versions should eventually all be on backup. The "Not Backed Up" card on the Storage page shows this count. User should back these up via the Backup button per folder.

#### Conditional local keeping (format preference per-track)

User's rule:

- If a track has a **stem.m4a** locally → the FLAC can be pruned (if backed up)
- If a track has **only FLAC** (no stem) → keep FLAC locally for offline/Traktor use

The `hasStemVariant` badge on the prune preview already supports this per-file. But the user wants the prune preview organized **by track** — showing "for Track X: stem ✓, FLAC (redundant), WAVs (redundant)" grouped together.

#### Backpack feature for offline availability

The current "Follow" mechanism on Tags and "Backpack" toggle on Tracks are **the same thing** — both set `tags.followed = true` which causes `get_prune_candidates()` to exclude files with that tag. The mechanism is consistent but **passive** — it only prevents deletion, it doesn't actively pull files from backup or ensure the right format is local.

**What the user wants**:

- Rename "Follow" → "Backpack" everywhere (more intuitive: "I want these offline")
- A track is "in backpack" if EITHER individually tagged OR in a backpack tag
- Actively pull files from backup when they're not local
- Prefer format: stem > FLAC > MP3 (exactly one version per track)
- Clean up redundant formats when a better one exists

**Current flow:** Tag "Backpack" toggled → Files in that tag won't be pruned. That's it.

**Needed flow:** Tag "Backpack" toggled → Background task checks all tracks → pulls missing files from backup → prefers stem over FLAC → ensures exactly one version per track is local.

**UX flow sketch:**

```
Tags page: toggle "digging-2026-05-26" → Backpack
    ↓
Background "Backpack Sync" task starts:
    For each track in the tag:
      ├─ stem.m4a on disk? → keep, mark FLAC as safe-to-delete
      ├─ FLAC on disk (no stem)? → keep
      ├─ stem on backup only? → pull from backup
      ├─ Nothing anywhere? → skip (can't pull what doesn't exist)
    ↓
Notify: "Backpack sync complete: 5 files pulled, 3 formats cleaned"
```

**Possible new page**: A dedicated "Backpack" page showing all backpack tracks with their file status — which are local, which are backup-only, which need pulling. Could also show per-track format status (stem ✓, FLAC redundant, etc.).

**Dependencies**: Needs file_locations.local (the Maintainer) and is_local on track-detail before this can be built.

---

## Plan: backpack-system

**Status**: done ✅
**Branch**: feat/backpack-system
**Depends on**: fix/local-file-tracking + Maintainer
**Migration needed**: yes — rename tags.followed to tags.backpack (migration 014)

### Description

Overhaul "follow" into proper "Backpack" system. Rename everywhere (DB column + all code + UI), make tracks inherit backpack status from their tags, add active sync task that pulls missing files from backup and cleans up redundant formats. WAV source files excluded (Ableton only, metadata from linked track).

### Part 1: Rename followed → backpack everywhere

Migration 014: ALTER TABLE tags RENAME COLUMN followed TO backpack.

Rust: Tag.followed → Tag.backpack, set_tag_followed() → set_tag_backpack(), get_followed_tags() → get_backpack_tags(), is_file_followed() → is_in_backpack(). All SQL queries updated. get_prune_candidates() WHERE t.followed = 1 → WHERE t.backpack = 1.

Frontend: Tags page "Follow" → "Backpack", icon fa-eye → fa-backpack. Labels: "files are kept locally" → "files are kept offline".

### Part 2: Track inherits backpack from tags

A track is "in backpack" if ANY of its tags has backpack = true, OR if individually in the "backpack" playlist.

Add pub in_backpack: bool to track API responses (TrackDetail, Tracks list). Tracks page shows backpack icon for in_backpack tracks.

### Part 3: Backpack Sync task

New TaskType: BackpackSync { tag_ids }. When a tag is toggled to backpack, spawns background task:

- For each track in backpack tags: find best local file (stem > FLAC > MP3)
- If missing: pull from backup
- If redundant: mark safe-to-delete (stem exists → FLAC can be pruned)
- Skips WAV source files (Ableton only)
- Ensures exactly one version per track is local

### Part 4: Backpack Page (new #backpack route)

New SPA page showing: active backpack tags with track counts, per-track file status (stem ✓ / FLAC only / needs pull / nothing available), "Sync All" and "Pull Missing" bulk buttons.

### Files to modify

- migrations/014_backpack_rename.sql — new
- src/db.rs — rename all followed→backpack, add in_backpack computation, BackpackSync logic
- src/api.rs — in_backpack on track responses, backpack sync handler
- src/tasks/mod.rs — new BackpackSync task type + worker
- frontend/pages/tags.js — Follow→Backpack UI
- frontend/pages/tracks.js — backpack icon for in_backpack tracks
- frontend/pages/backpack.js — NEW page
- frontend/app.js + shared/nav.js — register route
- frontend/style.css — backpack page styles

### Acceptance Criteria

**Part 1:**

- [ ] Migration 014 runs cleanly (001→014)
- [ ] All code references: followed → backpack
- [ ] Tags page shows "Backpack" not "Follow"
- [ ] cargo build passes

**Part 2:**

- [ ] Track detail + tracks list return inBackpack: bool
- [ ] Track inherits backpack from tags
- [ ] Tracks page shows backpack icon
- [ ] cargo build passes

**Part 3:**

- [ ] BackpackSync task spawns on tag toggle
- [ ] Pulls missing files (stem > FLAC)
- [ ] Cleans redundant formats
- [ ] WAV source files excluded
- [ ] cargo build passes

**Part 4:**

- [ ] #backpack page renders tags + track status
- [ ] Sync/Pull buttons work
- [ ] Registered in app.js + nav.js
- [ ] cargo build passes

#### Background maintenance scheduler

The current system has multiple polling mechanisms with different intervals:

- Folder watcher: polls active folders every 5 min (incremental scan)
- Global playlist poller: every 15 min
- Subscription poller: every 30s
- Auto-backup poller: every 10 min
- **No** automatic full scan or stale-local cleanup

But there's no unified "housekeeping" that:

- Triggers a full scan when incremental scans have been running too long (files may have been deleted locally)
- Automatically cleans up stale `file_locations.local` entries for files no longer on disk
- Detects unbacked-up files and triggers backup for folders with `auto_backup` enabled
- Discovers backup-only files (via `discover_backup_files`) on a schedule
- Keeps all counts (`localFileCount`, `pruneCandidateCount`) current without manual button clicks

**Idea: A `Maintainer` background task** — single loop, configurable interval (default: 1h), does:

1. **Quick health check**: For each active folder, check if `last_scanned` is > 24h old → trigger a full scan
2. **Stale local cleanup**: For each scanned folder, remove `file_locations.local` for files that disappeared (already implemented in `scan_folder`, but only runs when a scan happens — if no scan triggers, stale entries persist)
3. **Unbacked-up check**: For folders with `auto_backup = true`, check if any files lack backup → log warning (backup sync is handled by the existing auto-backup poller)
4. **Backup discovery**: On a longer interval (e.g. weekly), trigger `discover_backup_files` to sync NAS inventory

This would replace the need for a manual "Run Full Scan" button — the maintainer just runs it when needed.

#### Backup-only file metadata extraction

When `discover_backup_files` finds files on the NAS that don't exist in the local DB, it currently creates a bare record with no metadata:

```json
{ "title": null, "artist": null, "isrc": null, "bpm": null, "fileSize": 0 }
```

**What the user wants**: Extract metadata (ISRC, title, artist, BPM, file size) directly from the backup location. This enables:

- Matching to service tracks (Spotify/SoundCloud) via ISRC → track-detail shows files even if never local
- Better prune/discovery workflows — knowing "this backup-only FLAC has ISRC X"

**How to extract remotely**: Via SSH we can run `exiftool` or `ffprobe` on the NAS:

```bash
ssh backup "ffprobe -v quiet -print_format json -show_format /volume1/media/flacs/Artist - Title.flac"
```

This returns: duration, format, tags (title, artist, album, ISRC, BPM if embedded). For WAVs there might be no metadata, but for FLACs and stems there usually is.

**What would change**:

- `discover_backup_files()` accepts an optional reference to a BackupEngine (for SSH)
- For new files, after creating the DB record, SSH to NAS to extract metadata
- Update the `files` row with: `title`, `artist`, `isrc`, `file_size`, `duration_ms`
- The `file_hash` stays as sentinel `"backup-only-{size}"` (can't hash remotely easily)
- **Result**: Track-detail can now show "this track has a FLAC on backup (+ ISRC matched)" even if you've never had the file locally

**Risk**: Running `ffprobe` on 6000+ FLACs over SSH would be slow — should be batched and only for newly discovered files, not on every maintainer cycle.

**TODO**: Decide if this is part of the Maintainer or a separate "enrich backup files" task.

---

## Plan: maintainer

**Status**: done ✅
**Branch**: feat/maintainer
**Depends on**: fix/local-file-tracking
**Migration needed**: no

### Description

Background task that keeps the DB in sync with reality. The system has 4 separate pollers but no coordinator. The Maintainer periodically checks for stale state and triggers corrective actions via existing task workers (ScanFolder, BackupFolder, BackupDiscovery).

### What it does

Single background loop, configurable interval (default 1h, env: MOMOS_MAINTAINER_INTERVAL_SECS):

| #   | Check             | Condition                                 | Action                                                           |
| --- | ----------------- | ----------------------------------------- | ---------------------------------------------------------------- |
| 1   | Full scan needed  | last_scanned > 24h for any active folder  | Spawn ScanFolder (populates file_locations.local, stale cleanup) |
| 2   | Unbacked-up files | auto_backup=true and files without backup | Spawn BackupFolder (reconcile + rsync)                           |
| 3   | Backup discovery  | Last discovery > 7 days, backup_path set  | Spawn BackupDiscovery (lists NAS, creates DB records)            |

Lightweight coordinator — doesn't do the work itself, only triggers tasks.

Replaces the manual "Run Full Scan" button — Maintainer does it proactively. Button stays as override.

### Config

```toml
[maintainer]
interval_secs = 3600          # cycle interval (default 1h)
full_scan_max_age_secs = 86400 # max age before full scan (default 24h)
backup_discovery_interval_secs = 604800 # backup discovery interval (default 7d)
```

### Files to modify

- src/maintainer.rs — NEW module
- src/main.rs — spawn in serve()
- src/config.rs — MaintainerConfig section

### Acceptance Criteria

- [ ] Maintainer starts on server boot
- [ ] Triggers full scan when last_scanned exceeds max age
- [ ] Triggers backup for auto-backup folders with unbacked-up files
- [ ] Triggers backup discovery on schedule
- [ ] Configurable via config.toml + env vars
- [ ] Cancel token honored for clean shutdown
- [ ] Zero cost when idle (timestamp checks only)
- [ ] cargo build passes

---

## Plan: integration-test-harness

**Status**: done ✅
**Branch**: `feat/integration-test-harness`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Build a self-contained Rust integration test harness. Replaces the curl-based
`test.sh` smoke tests with deterministic, fast tests that create a fresh SQLite
DB, run all migrations, seed hand-crafted data, hit every API endpoint with
every filter combination, and assert exact results. Run with `cargo test` — no
server needed, runs in seconds.

### Why

- Agents need fast, deterministic feedback. `cargo test` = single source of truth.
- curl-based tests against real data are fragile (data changes, manual server).
- 16 migrations, 14+ API endpoints, 50+ filter params — all untested.
- Every future plan MUST include tests (enforced by Section 1 Testing rules).

### Architecture: How `cargo test` creates a full app

1. **In-memory SQLite** with `datetime` → `UnixEpoch` conversion for
   `unixepoch()` compatibility (SQLite needs `DATETIME` format, Rust supplies
   Unix timestamps).
2. **Run all migrations** from `migrations/` directory, in order.
3. **Seed hand-crafted data** that exercises every edge case and view chain.
4. **Create a test `Router`** via a new `build_router()` function (extracted
   from `serve()` — see Phase 1).
5. **Hit endpoints with `reqwest`** (already a dependency), parse JSON
   responses, assert exact values.

#### Why in-memory SQLite instead of temp file?

In-memory with `datetime` type affinity works for all our use cases:
`unixepoch()` returns Unix timestamps, `datetime(unixepoch(), ...)` works.
Rust code stores `i64` Unix timestamps. The only caveat: `date('now')` returns
UTC date string, not Unix timestamp. Our queries use `unixepoch('now', ...)`
which works correctly in memory.

### Phase 1: Extract `build_router()` from `serve()`

**File**: `src/main.rs`

Move all `.route()` calls from `serve()` into a standalone function:

```rust
/// Build the Axum router from AppState. Extracted for testability.
/// Does NOT spawn background tasks (pollers, watchers, maintainer).
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/api/storage/status", get(api::storage_status_handler))
        .route("/api/storage/prune-preview", post(api::prune_preview_handler))
        // ... all existing routes ...
        .fallback(get(static_handler))
        .layer(CorsLayer::permissive())
}
```

In `serve()`, replace inline router construction with `build_router(state.clone())`.

**This is a pure refactor — zero behavior change.**

### Phase 2: Test helpers

**File**: `tests/common/mod.rs`

```rust
use sqlx::{Pool, Sqlite, SqlitePool};
use axum::Router;
use std::sync::Arc;

/// Create an in-memory SQLite DB, run all migrations, return pool.
pub async fn create_test_db() -> Pool<Sqlite> {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    // Enable WAL + normal sync for in-memory (fast, no durability concern)
    sqlx::query("PRAGMA journal_mode=WAL").execute(&pool).await.unwrap();
    // Run all migration files in order
    run_migrations(&pool).await;
    pool
}

/// Run all .sql files from migrations/ in numeric order.
async fn run_migrations(pool: &Pool<Sqlite>) {
    let mut files: Vec<_> = std::fs::read_dir("migrations")
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |e| e == "sql"))
        .collect();
    files.sort();
    for path in &files {
        let sql = std::fs::read_to_string(path).unwrap();
        sqlx::query(&sql).execute(pool).await.unwrap();
    }
}

/// Build a test AppState with the given DB pool.
pub async fn test_app_state(pool: Pool<Sqlite>) -> Arc<AppState> {
    Arc::new(AppState {
        db: pool,
        config: ServiceCredentials::defaults_for_test(),
        task_manager: TaskManager::new(),
        embeddings: Mutex::new(None),
        category_means: Mutex::new(None),
        public_url: None,
    })
}

/// Create a full test app (DB + migrations + router).
pub async fn test_app() -> (Router, Pool<Sqlite>) {
    let pool = create_test_db().await;
    let state = test_app_state(pool.clone()).await;
    let router = momos_music_manager::build_router(state);
    (router, pool)
}
```

**Note**: `AppState` and `TaskManager` need to be `pub` (or at least
`pub(crate)`). If they aren't already, make them so.

### Phase 3: Domain test files

Each test file follows the same pattern:

```rust
// tests/api_files.rs
mod common;
use axum_test::TestServer;  // or axum::test helpers

#[tokio::test]
async fn files_list_returns_paginated_results() {
    let (app, pool) = common::test_app().await;
    common::seed_basic_files(&pool).await;  // inserts 5 files

    let server = TestServer::new(app).unwrap();
    let resp = server.get("/api/files?limit=3").await;
    resp.assert_status_ok();

    let json: serde_json::Value = resp.json();
    let files = json["data"].as_array().unwrap();
    assert_eq!(files.len(), 3, "limit=3 should return 3 files");
}
```

#### Test files to create

| File                           | Covers                                                                       |
| ------------------------------ | ---------------------------------------------------------------------------- |
| `tests/common/mod.rs`          | DB creation, migration runner, seed helpers, test app factory                |
| `tests/migration_integrity.rs` | All 16 migrations run cleanly, schema has expected tables/views              |
| `tests/api_files.rs`           | All `FilesQuery` params: `isLocal`, `backedUp`, `safeToDelete`, `fileTypes`, |
|                                | `search`, `sort`, `order`, `tags`, `pmvCategories`, `pmvAggregate`,          |
|                                | `commentStatuses`, `linkedOnly`, `nonDefaultOnly`, `untaggedOnly`,           |
|                                | `keys`, plus count endpoint parity                                           |
| `tests/api_tracks.rs`          | All `TracksQuery` params: `services`, `fileTypes`, `fileTypeAgg`,            |
|                                | `hasLocal`, `hasBackup`, `playlists`, `tags`, `search`, `sort`,              |
|                                | plus `/api/tracks/{id}/detail` with `inBackpack` + WAV variants,             |
|                                | plus count endpoint parity                                                   |
| `tests/api_playlists.rs`       | `service` filter, `search`, `archive` filter, count, pagination              |
| `tests/api_tags.rs`            | `search`, `sort`, `categoryId`, `backpack` toggle (`PUT`), count             |
| `tests/api_storage.rs`         | `GET /api/storage/status` (all fields), `POST /api/storage/prune-preview`    |
|                                | (hasStemVariant, reasons), `POST /api/storage/prune` (if safe)               |
| `tests/api_folders.rs`         | Folder list, folder detail, backup config update                             |
| `tests/api_tasks.rs`           | Task list, task detail                                                       |
| `tests/api_digging.rs`         | `POST /api/digging/suggest` (seed tag + seed IDs), `/api/files/{id}/stream`  |
| `tests/api_file_variants.rs`   | `GET /api/files/{id}/variants` (stemType, WAV source grouping)               |

**Each test file is ~100–300 lines.** Total: ~2,000 lines of test code.

### Seed data design principles

- **Minimal, hand-crafted rows.** ~30–50 rows total across all tables.
- **One edge case per row.** A file with ISRC but no stem variant. A file with
  stem variant. A file backed up but not local. A WAV with `source_of`. A tag
  with `backpack=true`. A track in multiple playlists.
- **Deterministic IDs.** No `AUTOINCREMENT` guessing — seed inserts include
  explicit IDs where needed for cross-table references.
- **Reusable seed functions.** `common::seed_basic_files()`,
  `common::seed_track_with_variants()`, etc. Tests compose them.

### How tests stay up-to-date

**Mechanism 1: Hard rule in Section 1.** Every plan that touches an API endpoint
or filter MUST include "add/update integration test" as acceptance criterion.
Agents are instructed to enforce this.

**Mechanism 2: Obvious file placement.** The test file name mirrors the API path:
`tests/api_files.rs` ↔ `/api/files*`. When an agent modifies `src/api.rs`'s file
handlers, the corresponding test file is unambiguous.

**Mechanism 3: Meta-tests.** Each test file has a "count" test that asserts the
number of top-level filter params. If a param is added to the query struct but
no test exercises it, the count changes and the meta-test fails. Example:

```rust
#[test]
fn all_files_query_params_have_coverage() {
    // grep FilesQuery fields and compare to test function count
    // This is a canary — fails when a param is added without a test
}
```

This is a lightweight lint, not a full coverage tool. If it becomes annoying,
remove it — the hard rule (Mechanism 1) is the real enforcement.

**Mechanism 4: Migration integrity test.** `tests/migration_integrity.rs` creates
a fresh DB and runs all migrations. If a migration breaks (wrong order, syntax
error, missing dependency), this test catches it before any other test runs.

### Dependencies

No new dependencies. `reqwest` is already in `Cargo.toml`. `axum::test` is
built-in (behind the `axum/test` feature, enable if needed). Alternatively,
`axum_test` crate for ergonomic `TestServer` — or just use `reqwest` against a
bound port with `tokio::net::TcpListener`.

### Files to create

- `tests/common/mod.rs` — test helpers, migration runner, seed functions
- `tests/migration_integrity.rs` — migration chain test
- `tests/api_files.rs` — files endpoint tests
- `tests/api_tracks.rs` — tracks endpoint tests
- `tests/api_playlists.rs` — playlists endpoint tests
- `tests/api_tags.rs` — tags endpoint tests
- `tests/api_storage.rs` — storage endpoint tests
- `tests/api_folders.rs` — folders endpoint tests
- `tests/api_tasks.rs` — tasks endpoint tests
- `tests/api_digging.rs` — digging suggest + audio stream tests
- `tests/api_file_variants.rs` — file variants endpoint tests

### Files to modify

- `src/main.rs` — extract `build_router()` from `serve()`; make `AppState` fields `pub`
- `src/config.rs` — add `ServiceCredentials::defaults_for_test()` (or `#[cfg(test)]` constructor)
- `Cargo.toml` — enable `axum/test` feature if not already (check)

### Acceptance Criteria

- [ ] `build_router()` extracted; `serve()` delegates to it; `cargo build` passes
- [ ] `cargo test` creates fresh in-memory DB, runs all 16 migrations, no errors
- [ ] `tests/migration_integrity.rs`: asserts all expected tables + views exist
- [ ] `tests/api_files.rs`: ≥15 tests covering every `FilesQuery` param + count parity
- [ ] `tests/api_tracks.rs`: ≥12 tests covering every `TracksQuery` param + detail endpoint + count parity
- [ ] `tests/api_playlists.rs`: ≥5 tests (list, filter, search, archive, pagination)
- [ ] `tests/api_tags.rs`: ≥5 tests (list, search, sort, backpack toggle, count)
- [ ] `tests/api_storage.rs`: ≥4 tests (status fields, prune-preview hasStemVariant, prune-preview reasons)
- [ ] `tests/api_digging.rs`: ≥3 tests (seed by tag, seed by file IDs, audio stream range request)
- [ ] `tests/api_file_variants.rs`: ≥3 tests (stem variants, WAV source grouping, no-variants)
- [ ] All tests pass: `cargo test` exits 0
- [ ] `cargo test` completes in <10 seconds
- [ ] `test.sh` still works as legacy smoke test (no changes needed to test.sh)

---

## Plan: test-coverage-100

**Status**: proposed
**Branch**: `feat/test-coverage-100`
**Ready for review**: no
**Depends on**: `feat/integration-test-harness`
**Migration needed**: no

### Description

Achieve ~100% backend route coverage. Currently 17/59 routes tested (29%).
The goal is to test every route that doesn't require external services
(Spotify OAuth, SSH/NAS, WebSocket, ML models). Mutations (POST/PUT)
get basic smoke tests; read endpoints get filter-param coverage.

### Current coverage

| Domain         | Tested         | Untested                                                                                      |
| -------------- | -------------- | --------------------------------------------------------------------------------------------- |
| Files          | 7 of 12 routes | `latest`, `service-links`, `{id}/detail`, `{id}/write-comment`, `key-comparison`              |
| Tracks         | 4 of 7 routes  | `{id}`, `needs-comment-count`, `write-comments`                                               |
| Tags           | 3 of 8 routes  | `POST` (create), `{id}`, `curation-queue`, `unreviewed`, `categorize`                         |
| Playlists      | 1 of 4 routes  | `local` (POST), `{id}/archive`, `{id}`                                                        |
| Storage        | 2 of 5 routes  | `prune`, `backup/{id}`, `discover-backup/{id}`                                                |
| Folders        | 0 of 5 routes  | All 5                                                                                         |
| Tasks          | 0 of 2 routes  | Both                                                                                          |
| Digging        | 1 of 3 routes  | `search`, `tracks`                                                                            |
| Infrastructure | 1 of 8 routes  | `health`, `dump`, `restore`, `tag-energy-levels`×4, `tags/{id}/children`, `tags/{id}/suggest` |

**Untested filter params on already-tested endpoints:**

| Endpoint             | Missing params                                                                                                                 |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `GET /api/files`     | `commentStatuses`, `linkedOnly`, `nonDefaultOnly`, `keys`, `safeToDelete`, `pmvCategories`, `pmvAggregate`, `bpmMin`, `bpmMax` |
| `GET /api/tracks`    | `hasLocal`, `hasBackup`                                                                                                        |
| `GET /api/playlists` | `archive`, `categories`, `subscribed`, `stale`                                                                                 |

### Categorization of untested routes

**Tier A — Fully testable with seed data alone (37 routes):**
All CRUD reads, writes, and infrastructure endpoints that work against
SQLite with seeded data. No external services needed.

**Tier B — Partially testable (2 routes):**
`/api/services/{service}/sync` and `/api/services/{service}/reset` —
can test the "not configured" error response.

**Tier C — Not testable in CI (7 routes):**
OAuth (`/api/services/{service}/auth`, `/callback`), WebSocket (`/ws/spotify`),
SSH (`/api/backup/test`, `/api/backup/explore`), ML (`/api/embeddings/*`),
Traktor (`/api/traktor/import` — needs `.nml` file on disk, actually testable
if we write an inline NML string to a temp file before calling).

### Exclusions (routes we deliberately skip)

| Route                          | Reason                                                       |
| ------------------------------ | ------------------------------------------------------------ |
| `/api/services/{service}/auth` | OAuth redirect — can't test without real Spotify credentials |
| `/callback`                    | OAuth callback — same                                        |
| `/ws/spotify`                  | WebSocket — requires real-time auth token                    |
| `/api/backup/test`             | SSH connection test — needs NAS                              |
| `/api/backup/explore`          | SSH file listing — needs NAS                                 |
| `/api/embeddings/status`       | Requires ML model download (BERT, ~500MB)                    |
| `/api/embeddings/reset-review` | Same                                                         |

### Phase 1: Missing filter params (highest ROI — fills existing test files)

**File**: `tests/api_files.rs` — add ~8 tests

| Test                                         | What it proves                                                  |
| -------------------------------------------- | --------------------------------------------------------------- |
| `files_filter_comment_statuses_needs_update` | `?commentStatuses=needs_update` filters correctly               |
| `files_filter_comment_statuses_up_to_date`   | `?commentStatuses=up_to_date` filters correctly                 |
| `files_filter_linked_only`                   | `?linkedOnly=true` returns only files with service links        |
| `files_filter_unlinked`                      | `?unlinked=true` returns only files without service links       |
| `files_filter_non_default_only`              | `?nonDefaultOnly=true` returns only files with non-Setlist tags |
| `files_filter_key`                           | `?key=4m` returns files matching that Camelot key               |
| `files_filter_safe_to_delete`                | `?safeToDelete=true` filters correctly                          |
| `files_filter_pmv_categories`                | `?pmvCategories=p,m` returns files with Phase/Mood tags         |
| `files_filter_pmv_aggregate`                 | `?pmvAggregate=full` returns files with all 3 PMV tags          |

**File**: `tests/api_tracks.rs` — add ~3 tests

| Test                       | What it proves                                           |
| -------------------------- | -------------------------------------------------------- |
| `tracks_filter_has_local`  | `?hasLocal=true` filters to tracks with local files      |
| `tracks_filter_has_backup` | `?hasBackup=true` filters to tracks with backed-up files |
| `tracks_single_by_id`      | `GET /api/tracks/1` returns single track                 |

**File**: `tests/api_playlists.rs` — add ~4 tests

| Test                                | What it proves                                       |
| ----------------------------------- | ---------------------------------------------------- |
| `playlists_filter_archive_archived` | `?archive=archived` returns only archived playlists  |
| `playlists_filter_archive_active`   | `?archive=active` returns only active playlists      |
| `playlists_filter_subscribed`       | `?subscribed=true` returns only subscribed playlists |
| `playlists_filter_categories`       | `?categories=1,2` filters by tag category IDs        |

### Phase 2: Read-only endpoints (existing domains, new tests)

#### `tests/api_folders.rs` — NEW FILE

| Test                   | What it proves                                        |
| ---------------------- | ----------------------------------------------------- |
| `folders_list`         | `GET /api/folders` returns all seeded folders         |
| `folders_count`        | `GET /api/folders/count` matches list length          |
| `folders_single`       | `GET /api/folders/{id}/stats` returns folder metadata |
| `folders_toggle_watch` | `POST /api/folders/{id}/watch` toggles active flag    |
| `folders_not_found`    | `GET /api/folders/9999/stats` returns 404             |

#### `tests/api_tasks.rs` — NEW FILE

| Test                       | What it proves                                     |
| -------------------------- | -------------------------------------------------- |
| `tasks_list_empty`         | `GET /api/tasks` returns empty array on fresh DB   |
| `tasks_list_with_task`     | After triggering a scan, returns tasks with status |
| `tasks_single_not_found`   | `GET /api/tasks/xxx-xxx` returns 404               |
| `tasks_list_status_filter` | `?status=completed` filters correctly              |

#### Extend existing files

| File                     | Add                                                                                       |
| ------------------------ | ----------------------------------------------------------------------------------------- |
| `tests/api_files.rs`     | `files_latest` — `GET /api/files/latest` returns most recent                              |
| `tests/api_files.rs`     | `files_service_links` — `GET /api/files/service-links` returns Spotify/SC links           |
| `tests/api_files.rs`     | `files_detail` — `GET /api/files/1/detail` returns full metadata                          |
| `tests/api_files.rs`     | `files_key_comparison` — `GET /api/files/key-comparison?tag=Groovy` returns BPM/key table |
| `tests/api_tags.rs`      | `tags_single_by_id` — `GET /api/tags/7` returns tag with category info                    |
| `tests/api_tags.rs`      | `tags_curation_queue` — `GET /api/tags/curation-queue` returns Setlist tags               |
| `tests/api_tags.rs`      | `tags_unreviewed` — `GET /api/tags/unreviewed` returns tags without parents               |
| `tests/api_playlists.rs` | `playlists_single` — `GET /api/playlists/1` returns playlist detail                       |
| `tests/api_digging.rs`   | `digging_search` — `GET /api/digging/search?q=X` returns results                          |
| `tests/api_digging.rs`   | `digging_tracks` — `GET /api/digging/tracks?limit=5` returns paginated                    |

### Phase 3: Mutation endpoints

| File                     | Test                                                                                                                                                                                                                                              |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/api_files.rs`     | `files_write_comment` — `POST /api/files/1/write-comment` queues task, returns taskId                                                                                                                                                             |
| `tests/api_files.rs`     | `files_write_comments_bulk` — `POST /api/files/write-comments-by-ids` with body `{"fileIds": [1,2,3]}` queues bulk task (NOT `/api/files/write-comments` -- that endpoint takes filter params `{linkedOnly, tags, nonDefaultOnly}`, not file IDs) |
| `tests/api_tracks.rs`    | `tracks_needs_comment_count` — `POST /api/tracks/needs-comment-count` with `{"trackIds": [1]}`                                                                                                                                                    |
| `tests/api_tags.rs`      | `tags_create` — `POST /api/tags` with `{"name":"NewTag","categoryId":3}` returns created tag                                                                                                                                                      |
| `tests/api_tags.rs`      | `tags_categorize` — `PUT /api/tags/7/categorize` with `{"categoryId":4}` moves to Vibe                                                                                                                                                            |
| `tests/api_playlists.rs` | `playlists_create_local` — `POST /api/playlists/local` creates local playlist                                                                                                                                                                     |
| `tests/api_playlists.rs` | `playlists_toggle_archive` — `PUT /api/playlists/1/archive` toggles `archiveDeleted`                                                                                                                                                              |
| `tests/api_folders.rs`   | `folders_scan` — `POST /api/folders/1/scan` triggers scan task (note: folder path `/test/stems` doesn't exist on disk, so `GET /api/tasks/{id}` should show failed status)                                                                        |
| `tests/api_storage.rs`   | `storage_prune` -- First calls prune-preview to get candidates, then `POST /api/storage/prune` with body `{"fileIds": [candidate_ids...]}` (NOT `?confirm=true` -- that query param does not exist on this endpoint)                              |

### Phase 4: Error states & infrastructure

| File                     | Test                                                                                 |
| ------------------------ | ------------------------------------------------------------------------------------ |
| `tests/api_files.rs`     | `files_not_found` — `GET /api/files/9999` returns 404                                |
| `tests/api_tracks.rs`    | `tracks_not_found` — `GET /api/tracks/9999` returns 404                              |
| `tests/api_tags.rs`      | `tags_not_found` — `GET /api/tags/9999` returns 404                                  |
| `tests/api_playlists.rs` | `playlists_not_found` — `GET /api/playlists/9999` returns 404                        |
| `tests/api_digging.rs`   | `digging_suggest_no_seeds` — `POST /api/digging/suggest` with empty body returns 400 |
| `tests/api_playlists.rs` | `playlists_create_local_no_name` — `POST /api/playlists/local` with `{}` returns 400 |
| `tests/api_tags.rs`      | `tags_create_no_name` — `POST /api/tags` with `{}` returns 400                       |

#### Infrastructure

| File                        | Test                                                                           |
| --------------------------- | ------------------------------------------------------------------------------ |
| `tests/api_health.rs` — NEW | `health_check` — `GET /api/health` returns `{"status": "ok"}`                  |
| `tests/api_dump.rs` — NEW   | `dump_download` — `GET /api/dump` returns JSON with Content-Disposition header |
| `tests/api_dump.rs` — NEW   | `restore_no_confirm` — `POST /api/restore` without `?confirm=true` returns 400 |
| `tests/api_tags.rs`         | `tags_children` — `GET /api/tags/7/children` returns child tags                |
| `tests/api_tags.rs`         | `tags_suggest` — `GET /api/tags/7/suggest` returns category suggestion         |

#### Service endpoints (error paths only)

| File                          | Test                                                                                                    |
| ----------------------------- | ------------------------------------------------------------------------------------------------------- |
| `tests/api_services.rs` — NEW | `services_sync_not_configured` — `POST /api/services/soundcloud/sync` returns error (SC not configured) |
| `tests/api_services.rs` — NEW | `services_list` — `GET /api/services` returns service status array                                      |

### Seed data requirements

#### Fixes to existing seed (critical -- blocks Phase 1)

1. **Add `spotify_id` to file rows in `seed_basic_data()`** -- files need
   `spotify_id = 'spotify:track:aaa'` (matching service_track 1's `service_id`)
   for `v_file_track_link` to resolve. Without this, `hasLocal`, `hasBackup`,
   `linkedOnly`, and `unlinked` filters silently return empty results.

2. **Add a 4th unlinked file** -- file id=4 with ISRC `US999` (no matching
   service_track) so `?unlinked=true` can be proven to return files.

#### New seed functions (blocks Phase 1-2 tests)

| Function                     | Needed for                                                                                         | What it does                                                                                    |
| ---------------------------- | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `seed_files_with_comments()` | `commentStatuses=needs_update`, `up_to_date`                                                       | Files with `comment` set, `file_resolved_tags` populated so computed target differs from stored |
| `seed_tag_hierarchy()`       | `curation-queue`, `unreviewed`, `nonDefaultOnly`, `pmvCategories`, `pmvAggregate`, `tags_children` | Setlist-category tags with parent/child relationships + playlist matching + file links          |
| `seed_subscribed_playlist()` | `playlists_filter_subscribed`, `archive`                                                           | One row in `playlist_subscriptions` + playlist with `archive_deleted=true`                      |

#### Refresh pattern (call after seeding for tag-filter tests)

Every test that filters by tags, PMV, or non-default must call:

```rust
momos_music_manager::db::refresh_file_resolved_tags(&pool).await.unwrap();
```

### Immediate wins -- tests writable NOW (no seed changes)

21 tests can be written against existing `seed_basic_data()`:

| File               | Tests                                                                                                    |
| ------------------ | -------------------------------------------------------------------------------------------------------- |
| `api_files.rs`     | `files_filter_key`, `safe_to_delete`, `latest`, `service_links`, `detail`, `key_comparison`, `not_found` |
| `api_tracks.rs`    | `has_local`, `has_backup` (need spotify_id fix first), `single_by_id`, `not_found`                       |
| `api_playlists.rs` | `archive_active`, `single`, `not_found`                                                                  |
| `api_tags.rs`      | `single_by_id`, `not_found`, `create`, `categorize`, `create_no_name`                                    |
| `api_folders.rs`   | All 5 (list, count, single, toggle_watch, not_found)                                                     |
| `api_tasks.rs`     | All 4 (list_empty, list_with_task, not_found, status_filter)                                             |
| `api_storage.rs`   | `prune`                                                                                                  |
| `api_health.rs`    | `health_check`                                                                                           |
| `api_dump.rs`      | `dump_download`, `restore_no_confirm`                                                                    |
| `api_services.rs`  | `services_list`, `sync_not_configured`                                                                   |

### Filter combinations the frontend uses (test these together)

| Page         | Critical combo                                                 | Why                       |
| ------------ | -------------------------------------------------------------- | ------------------------- |
| files.js     | `isLocal=true` + `commentStatuses=needs_update`                | Tri-state filters combine |
| files.js     | `backedUp=true` + `isLocal=false`                              | Backup-only files         |
| files.js     | `fileTypes=flac,stem.m4a` + `safeToDelete=true`                | Cross-filter              |
| tracks.js    | `hasLocal=true` + `hasBackup=true`                             | AND'd boolean flags       |
| tracks.js    | `pmvCategories=p,m,v` + `hasLocal=true` + `fileTypes=stem.m4a` | Three independent dims    |
| digging.js   | `energyLevels` + `keyList/keyRange` + `tags` + `pmvCategories` | All 4 toggles on          |
| playlists.js | `archive=archived` + `subscribed=true`                         | Subscribed + archived     |

### Seed data ID ranges (documented to prevent collisions)

| Entity            | ID range | Source                                   |
| ----------------- | -------- | ---------------------------------------- |
| Tags              | 1-6      | Migration 001 (phase tags)               |
| Tags              | 7-9      | `seed_basic_data`                        |
| Tags              | 10-19    | `seed_tag_hierarchy` (new)               |
| Tags              | 20+      | `seed_pmv_tags` or inline                |
| Files             | 1-3      | `seed_basic_data`                        |
| Files             | 4        | Unlinked file (add to `seed_basic_data`) |
| Files             | 10-13    | `seed_digging_data`                      |
| Files             | 20-24    | `seed_wav_variant_data`                  |
| Files             | 30+      | `seed_files_with_comments` (new)         |
| Service playlists | 1-2      | `seed_basic_data`                        |
| Service playlists | 3+       | `seed_subscribed_playlist` (new)         |

### What "100%" actually means

52 of 59 routes (88%) for process-unique endpoints. 7 deliberately excluded:

- 2 OAuth (`/api/services/{service}/auth`, `/callback`)
- 1 WebSocket (`/ws/spotify`)
- 2 SSH (`/api/backup/test`, `/api/backup/explore`)
- 2 ML (`/api/embeddings/*`)

This considers only unique URL paths. Some paths have multiple handlers
(GET+POST+PUT). Tests cover all handler methods on covered paths.

### Files to create

- `tests/api_folders.rs` — 5 tests
- `tests/api_tasks.rs` — 4 tests
- `tests/api_health.rs` — 1 test
- `tests/api_dump.rs` — 2 tests
- `tests/api_services.rs` — 2 tests

### Files to modify

- `tests/api_files.rs` — add ~14 tests (filters + read endpoints + mutations + error states)
- `tests/api_tracks.rs` — add ~6 tests (filters + single + mutation + error)
- `tests/api_tags.rs` — add ~10 tests (read + create + categorize + error + children + suggest)
- `tests/api_playlists.rs` — add ~8 tests (archive/subscribed filters + single + create local + toggle archive + error)
- `tests/api_storage.rs` — add ~1 test (prune)
- `tests/api_digging.rs` — add ~3 tests (search, tracks, no-seeds error)
- `tests/common/mod.rs` — add `seed_files_with_comments()`, `seed_subscribed_playlist()`, `seed_tag_hierarchy()` helpers

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] Existing 129 tests still pass (no regressions)
- [ ] New test files created: `api_folders.rs`, `api_tasks.rs`, `api_health.rs`, `api_dump.rs`, `api_services.rs`
- [ ] All FilesQuery params have at least 1 test (22 params), count parity test covers all
- [ ] All TracksQuery params have at least 1 test (21 params)
- [ ] All PlaylistsQuery params have at least 1 test (11 params)
- [ ] All TagsQuery params tested (already done ✅)
- [ ] All CurationQueueQuery, FoldersQuery, TasksQuery params have at least 1 test
- [ ] All mutation endpoints return valid responses (200 or appropriate error)
- [ ] All 404 error paths tested (files, tracks, tags, playlists)
- [ ] `cargo test` completes in <15 seconds (current: ~5s with 129 tests)
- [ ] Total test count: ~200+ (129 existing + ~75 new)

## Plan: fix-scan-folder-task-tracking

**Status**: done ✅
**Branch**: `fix/scan-folder-task-tracking`
**Ready for review**: no
**Depends on**: `feat/test-coverage-100`
**Migration needed**: no

### Description

The `scan_folder_handler` in `src/api.rs` uses a raw `tokio::spawn` instead of
the TaskManager, making folder scans invisible to `/api/tasks` and the Tasks
page UI. Every other async operation (write comment, backup, prune, sync)
properly uses `start_*_task()` — this is the only outlier.

### Root cause

`src/api.rs` line 6910:

```rust
tokio::spawn(async move {
    match scan_folder(&db, id, scan_mode).await {
        Ok(file_count) => tracing::info!("Scanned {} files", file_count),
        Err(e) => tracing::error!("Failed to scan folder {}: {}", id, e),
    }
});
```

`start_scan_folder_task()` already exists in `src/tasks/mod.rs` (line 1479)
and supports `ScanMode`. The handler just isn't using it.

### Fix

**File**: `src/api.rs` — replace the `tokio::spawn` block in `scan_folder_handler`

```rust
// Use TaskManager so the task appears in /api/tasks and the Tasks UI
let task_id = match crate::tasks::start_scan_folder_task(
    &state.task_manager,
    &state.db,
    id,
    scan_mode,
).await {
    Ok(id) => id,
    Err(e) => return internal_error(e).into_response(),
};

Json(ApiResponse {
    data: serde_json::json!({
        "taskId": task_id,
        "folderId": id,
        "mode": if matches!(scan_mode, crate::db::ScanMode::Full) { "full" } else { "incremental" }
    }),
})
.into_response()
```

Also remove the unused `tokio::spawn` and the manual tracing calls (the task
worker handles those).

**File**: `tests/api_tasks.rs` — update `tasks_list_with_task`

Currently triggers a write-comment task to populate the task list. Now that
scan tasks appear, prefer scanning (it's a more natural fit for this test):

```rust
// Trigger a scan task on folder 1 (path doesn't exist, task will register)
let scan_resp = client
    .post(format!("{base}/api/folders/1/scan?mode=full"))
    .send().await.unwrap();
assert_eq!(scan_resp.status(), 200);
let scan_json: serde_json::Value = scan_resp.json().await.unwrap();
let task_id = scan_json["data"]["taskId"].as_str().unwrap();

// Verify the task appears in the list
let tasks_resp = client.get(format!("{base}/api/tasks")).send().await.unwrap();
let tasks_json: serde_json::Value = tasks_resp.json().await.unwrap();
let tasks = tasks_json["data"].as_array().unwrap();
assert!(!tasks.is_empty(), "scan task should appear in task list");
```

### Acceptance Criteria

- [ ] `scan_folder_handler` uses `start_scan_folder_task()` instead of `tokio::spawn`
- [ ] Response includes `taskId` for frontend progress polling
- [ ] `POST /api/folders/1/scan?mode=full` returns a taskId visible in `GET /api/tasks`
- [ ] All 190 existing tests still pass
- [ ] `cargo build` passes

---

## Plan: harness-completeness-audit

**Status**: proposed
**Branch**: `feat/harness-completeness`
**Ready for review**: no
**Depends on**: `fix/scan-folder-task-tracking`
**Migration needed**: no

### Description

Comprehensive audit of the test harness for coverage gaps. Three parallel
reviews — route coverage, seed data adequacy, and frontend→backend param
completeness — revealed 36 untested routes, 2 placebo tests, 1 silent frontend
bug, and ~50 untested parameter/endpoint combinations.

### Current state

| Metric                             | Value    |
| ---------------------------------- | -------- |
| Total routes (unique URL + method) | 112      |
| Tested                             | 56 (50%) |
| Untested (testable)                | 36 (32%) |
| Partial (error only or needs more) | 4 (4%)   |
| Excluded (OAuth, SSH, ML, WS)      | 16 (14%) |
| Frontend params tested             | ~55      |
| Frontend params untested           | ~50      |

### 🔴 Critical: Placebo tests (assert 200, prove nothing)

**PMV filter tests in `api_files.rs` are dead.** `files_filter_pmv_categories`
and `files_filter_pmv_aggregate_full` both seed data where all files have
`comment = NULL`. The PMV filter operates on the `[PMV]` bracket in the
`comment` column, so every filter variant returns 0 rows — and the tests
assert 0, passing regardless of whether the filter SQL is correct, inverted,
or completely broken.

**Fix**: Add a file with `comment = "[PMV] groovy"` to `seed_files_with_comments`,
then assert:

- `?pmvCategories=p` returns that file
- `?pmvCategories=m,v` returns that file
- `?pmvAggregate=full` returns that file
- `?pmvAggregate=partial` returns that file

### 🔴 Critical: Frontend bug — `untagged` parameter is a silent placebo

**File**: `frontend/pages/playlists.js` sends `params.set("untagged", "true")`
but `PlaylistsQuery` has **no `untagged` field**. The parameter is silently
ignored by serde. The "Untagged" filter button on the Playlists page does
nothing.

**Fix**: Either add `untagged_only: Option<bool>` to `PlaylistsQuery` and
implement the SQL filter, or remove the dead button from the frontend.

### 🟡 Phase 1: Unblock placebo PMV tests (blocks Phase 2 PMV coverage)

**File**: `tests/common/mod.rs` — `seed_files_with_comments()`

Add a file row with `comment = "[PMV] groovy"`, link it to a service track
and the "Groovy" playlist, add backup file_locations, then update the two
PMV filter tests to assert positive results instead of 0.

**Tests to update**: `files_filter_pmv_categories`, `files_filter_pmv_aggregate_full`
(and add `_partial` and `_none` variants).

### 🟡 Phase 2: Missing filter params on existing endpoints

**`tests/api_tracks.rs`** — add ~9 tests:

| Test                                | Param                                                           |
| ----------------------------------- | --------------------------------------------------------------- |
| `tracks_filter_pmv_categories`      | `?pmvCategories=m` (needs seed with PMV comment on linked file) |
| `tracks_filter_pmv_aggregate_full`  | `?pmvAggregate=full`                                            |
| `tracks_filter_file_types`          | `?fileTypes=flac`                                               |
| `tracks_filter_file_type_agg_any`   | `?fileTypeAgg=any`                                              |
| `tracks_filter_file_type_agg_none`  | `?fileTypeAgg=none`                                             |
| `tracks_filter_imported_after_days` | `?importedAfterDays=365`                                        |
| `tracks_filter_added_after_days`    | `?addedAfterDays=365`                                           |
| `tracks_single_playlist_id`         | `?playlistId=1` (single playlist param)                         |

### 🟡 Phase 3: Missing POST mutation endpoints

**`tests/api_tracks.rs`**:

- `tracks_write_comments` — `POST /api/tracks/write-comments` with `{"trackIds": [1]}`, verify returns taskId
- `tracks_backpack_toggle` — `POST /api/tracks/1/backpack` with `{"add": true}`, verify via detail endpoint

**`tests/api_files.rs`**:

- `files_needs_comment_count` — `POST /api/files/needs-comment-count` with `{"fileIds": [1,2]}`
- `files_write_comments_by_ids` — `POST /api/files/write-comments-by-ids` with `{"fileIds": [1]}`

**`tests/api_tags.rs`**:

- `tags_update` — `PUT /api/tags/7` with `{"name": "Groovy-Renamed"}`, verify via GET
- `tags_delete` — `DELETE /api/tags/{newly_created_id}`, verify 404 on re-fetch

**`tests/api_playlists.rs`**:

- `playlists_delete` — `DELETE /api/playlists/{newly_created_id}`, verify 404 on re-fetch

**`tests/api_folders.rs`**:

- `folders_create` — `POST /api/folders` with `{"folderPath": "/test/new", "active": true}`
- `folders_update` — `PUT /api/folders/1` with `{"active": false}`, verify via stats
- `folders_delete` — `DELETE /api/folders/{newly_created_id}`

### 🟡 Phase 4: Missing read endpoints

**`tests/api_tag_categories.rs`** — NEW FILE:

- `tag_categories_list` — `GET /api/tag-categories` returns 5 categories (Setlist, Phase, Mood, Vibe, Merkmal)
- `tag_categories_create` — `POST /api/tag-categories` creates a category

**`tests/api_tag_energy_levels.rs`** — NEW FILE:

- `tag_energy_levels_list` — `GET /api/tag-energy-levels` returns array

**Extend existing files**:

- `tests/api_tasks.rs` — `tasks_single_by_id` (use taskId from scan task)
- `tests/api_tasks.rs` — `tasks_cancel` — `DELETE /api/tasks/{taskId}`
- `tests/api_tags.rs` — `tags_parents_get` — `GET /api/tags/10/parents` returns parents (needs seed_tag_hierarchy)
- `tests/api_tags.rs` — `tags_parents_set` — `PUT /api/tags/10/parents` with body
- `tests/api_tags.rs` — `tags_from_playlists` — `GET /api/tags/from-playlists`
- `tests/api_tags.rs` — `tags_service_coverage` — `GET /api/tags/service-coverage`

### 🟡 Phase 5: Critical filter COMBINATIONS

The frontend sends multiple filters simultaneously. Verify count parity for:

| Combo                                           | Endpoint      |
| ----------------------------------------------- | ------------- |
| `isLocal=true` + `commentStatuses=needs_update` | `/api/files`  |
| `backedUp=true` + `isLocal=false`               | `/api/files`  |
| `hasLocal=true` + `hasBackup=true`              | `/api/tracks` |
| `pmvCategories=m,v` + `hasLocal=true`           | `/api/tracks` |

### 🟡 Phase 6: Digging tracks parameter coverage

**`tests/api_digging.rs`** — add ~5 tests for the `/api/digging/tracks` endpoint
params that the digging page ladder and filter rows send:
`energyLevels`, `keyList`, `keyRange`, `bpmMin`, `bpmMax`, `tags`, `sortBy`,
`sortOrder`, `pmvCategories`, `pmvAggregate`.

Each param gets one smoke test verifying the endpoint accepts it and returns
valid JSON. The digging engine already has complex logic tested via
`/api/digging/suggest`; these just verify the param plumbing.

### 🟢 Low priority (future rounds)

- Folder backup config tests (`PUT /api/folders/{id}/backup`, `PUT /api/folders/{id}/auto-backup`)
- Deemix queue CRUD (requires deemix-pyweb running)
- Traktor status/import (requires `.nml` file on disk — potentially testable)
- Storage backup/discover-backup (requires SSH/NAS)
- Tag similarities (depends on embeddings)
- `POST /api/restore` success path (requires valid dump JSON)
- Playlist subscriptions CRUD
- Dashboard-only endpoints (`/api/tags/from-playlists`, `/api/traktor/status`)

### Seed data changes needed

| Change                                                            | For                                |
| ----------------------------------------------------------------- | ---------------------------------- |
| Add `comment = "[PMV] groovy"` file to `seed_files_with_comments` | Phase 1 PMV tests                  |
| Add Phase-category tag + playlist + file link                     | Positive PMV category filter tests |

### Files to create

- `tests/api_tag_categories.rs` — 2 tests
- `tests/api_tag_energy_levels.rs` — 1 test

### Files to modify

- `tests/common/mod.rs` — add PMV-file to `seed_files_with_comments()`
- `tests/api_files.rs` — fix PMV placebo tests + add mutation tests
- `tests/api_tracks.rs` — add 13 tests (filters + mutations)
- `tests/api_tags.rs` — add 7 tests (mutations + read endpoints)
- `tests/api_playlists.rs` — add 1 test (delete)
- `tests/api_folders.rs` — add 3 tests (create, update, delete)
- `tests/api_digging.rs` — add ~5 tests (tracks params)
- `tests/api_tasks.rs` — add 2 tests (single, cancel)
- `frontend/pages/playlists.js` — fix `untagged` placebo (or add `untagged_only` to PlaylistsQuery)

### Acceptance Criteria

- [ ] PMV filter tests assert actual non-zero results (not placebo)
- [ ] `untagged` bug fixed (param handled or removed from frontend)
- [ ] All TracksQuery filter params tested (21/21)
- [ ] Files POST mutation endpoints tested (write-comments-by-ids, needs-comment-count)
- [ ] Tracks POST mutations tested (write-comments, backlog toggle)
- [ ] Tag update/delete endpoints tested
- [ ] Playlist delete endpoint tested
- [ ] Folder create/update/delete tested
- [ ] Digging tracks params have at least basic smoke coverage
- [ ] Tag-categories and tag-energy-levels endpoints tested
- [ ] `cargo build` passes
- [ ] All existing 190 tests still pass
- [ ] Total test count: ~230+

## Plan: fix-files-pmv-filter

**Status**: proposed
**Branch**: `fix/files-pmv-filter`
**Ready for review**: no
**Depends on**: `fix/scan-folder-task-tracking`
**Migration needed**: no

### Description

The Files PMV filter reads the `[PMV]` bracket string from the `comment`
column using `SUBSTR(files.comment, 2, 1)` — but that's a display/export
artifact, not the actual tag category data. The correct approach is to
query `file_resolved_tags.prefix`, which reflects the actual Phase/Mood/Vibe
tags assigned to a file through the tag→playlist→track→file resolution chain.

The Tracks PMV filter already does this correctly using
`track_resolved_tags.prefix`. Files uses the wrong data source in
**three separate places**: `get_files()`, `get_files_count()`, and
`build_files_filter_sql()`.

### Root cause

The `[PMV]` bracket in the comment string is a write-only export artifact.
When a file has Mood and Vibe tags, the comment writer writes `[ MV] tags...`
— but this string can go stale (comment not yet written, tags changed since
last write). The actual truth is in `file_resolved_tags.prefix`.

### Fix: Replace SUBSTR with file_resolved_tags EXISTS

**`get_files()`** (~line 7159) and **`get_files_count()`** (~line 7627):

```rust
// Before (wrong — parses comment string):
sql.push_str(" AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND     (SUBSTR(files.comment, 2, 1) = 'P' OR SUBSTR(files.comment, 3, 1) = 'P' OR SUBSTR(files.comment, 4, 1) = 'P'))");

// After (correct — queries actual tag category data):
sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt     WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ('p','m','v'))");
```

**Categories filter (OR logic)**:

```sql
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN (?,?,...))
```

**Full aggregate (AND logic)**:

```sql
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) = 'p')
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) = 'm')
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) = 'v')
```

**Partial aggregate (OR logic)**:

```sql
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ('p','m','v'))
```

**None aggregate**:

```sql
AND NOT EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ('p','m','v'))
```

**`build_files_filter_sql()`** (~line 1910) — same fix, same SQL pattern.

### Files to modify

- `src/api.rs` — replace PMV `SUBSTR` logic in `get_files()` (~lines 7159-7208)
- `src/api.rs` — replace PMV `SUBSTR` logic in `get_files_count()` (~lines 7627-7665)
- `src/api.rs` — replace PMV `SUBSTR` logic in `build_files_filter_sql()` (~lines 1910-1950)
- `tests/api_files.rs` — fix `files_filter_pmv_categories` and `files_filter_pmv_aggregate_full` tests (no longer need `[PMV]` comment strings, just need `file_resolved_tags` with P/M/V prefixes)
- `tests/api_tracks.rs` — add PMV filter tests for tracks (tracks already use correct `track_resolved_tags.prefix` mechanism)

### Seed data implications

After the fix, PMV filter tests don't need comment strings at all. They need:

- Tags in Phase, Mood, and Vibe categories
- Playlist→tag name matching
- Track→playlist linking
- File→track linking via ISRC/spotify_id
- `refresh_file_resolved_tags()` called after seeding

The existing `seed_tag_hierarchy()` already creates Mood (id=11, "shadow") and
Vibe (id=12, "techno") tags. With file 1 linked to those via parent resolution,
the PMV filter becomes testable. We just need to add a Phase-category tag to
complete the set.

### Acceptance Criteria

- [ ] `get_files()` PMV filter uses `file_resolved_tags.prefix`, not `SUBSTR(comment)`
- [ ] `get_files_count()` PMV filter uses same correct mechanism
- [ ] `build_files_filter_sql()` PMV filter uses same correct mechanism
- [ ] `?pmvCategories=m` returns files with Mood-category tags
- [ ] `?pmvCategories=v` returns files with Vibe-category tags
- [ ] `?pmvAggregate=full` returns files with P+M+V tags (all three)
- [ ] `?pmvAggregate=partial` returns files with at least one PMV tag
- [ ] `?pmvAggregate=none` returns files with no PMV tags
- [ ] Count endpoint matches list endpoint for all PMV filter variants
- [ ] All 190 existing tests still pass
- [ ] `cargo build` passes

## Plan: harden-test-harness

**Status**: done ✅
**Branch**: `fix/harden-test-harness`
**Ready for review**: no
**Depends on**: `fix/files-pmv-filter`
**Migration needed**: no

### Description

Harden the test harness based on three-audit findings: strengthen 12 weak
assertions, fix 6 handlers missing proper 404 responses, and fix 1 wrong
status code. This is the final polish pass — no new routes, just quality.

### Part A: Harden weak assertions (12 tests, ~50 lines)

| File             | Test                         | Current                                            | Fix                                                                                                                   |
| ---------------- | ---------------------------- | -------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `api_files.rs`   | `files_latest`               | Only checks `id` + `filePath` fields exist         | Assert files are ordered by `created_at` DESC (or at least 2 distinct files returned)                                 |
| `api_files.rs`   | `files_write_comment`        | Accepts `taskId` being null                        | Assert `taskId` is a non-empty string on success                                                                      |
| `api_files.rs`   | `files_key_comparison`       | Silent eprintln on 500, field-presence only on 200 | Assert `summary.matchCount` or `summary.totalCount` is present as a number                                            |
| `api_tracks.rs`  | `tracks_needs_comment_count` | Only checks field names exist                      | Assert `tracksNeedingUpdate` + `filesNeedingUpdate` are numbers                                                       |
| `api_storage.rs` | `storage_status_has_fields`  | 19 `contains_key` checks, no values                | After field-presence check, also verify `localFileCount` matches the value from `storage_status_counts` test seed (2) |
| `api_storage.rs` | `storage_prune_preview`      | Field-presence-only loop                           | Also assert `candidates.len() > 0` and first candidate has `fileSize > 0`                                             |
| `api_digging.rs` | `digging_tracks`             | Smoke test only                                    | Add one filter param (e.g., `?limit=3`) and verify returned count ≤ 3                                                 |
| `api_tasks.rs`   | `tasks_single_not_found`     | Accepts both 404 and 200                           | Assert strictly 404                                                                                                   |
| `api_tasks.rs`   | `tasks_list_status_filter`   | Lax comparison                                     | After scan task runs, verify `?status=running` or `?status=completed` returns non-empty                               |

### Part B: Fix missing 404 handlers (5 handlers, ~30 lines)

These handlers silently succeed or return 500 when the entity doesn't exist.
Each fix follows the same pattern: check existence first, then operate.

| Handler                        | File:Line      | Fix                                                           |
| ------------------------------ | -------------- | ------------------------------------------------------------- |
| `delete_tag_handler`           | `api.rs:3337`  | Query tag by ID first; return 404 if `None`, then `DELETE`    |
| `delete_folder_handler`        | `api.rs:6877`  | Query folder by ID first; return 404 if `None`, then `DELETE` |
| `update_folder_handler`        | `api.rs:6814`  | Query folder by ID first; return 404 if `None`, then `UPDATE` |
| `folder_backup_config_handler` | `api.rs:10401` | Query folder by ID first; return 404 if `None`, then update   |
| `folder_auto_backup_handler`   | `api.rs:10376` | Query folder by ID first; return 404 if `None`, then update   |

### Part C: Fix wrong status code (1 handler, ~2 lines)

| Handler                   | File:Line     | Fix                                                                                                   |
| ------------------------- | ------------- | ----------------------------------------------------------------------------------------------------- |
| `digging_suggest_handler` | `api.rs:2847` | Return 400 (StatusCode::BAD_REQUEST) instead of 500 when neither `seedTag` nor `seedFileIds` provided |

### Part D: Update tests for fixed handlers (4 tests, ~20 lines)

After fixing the 404 handlers, add/update tests:

| File             | Test                       | What                                                                   |
| ---------------- | -------------------------- | ---------------------------------------------------------------------- |
| `api_tags.rs`    | `tags_delete`              | `DELETE /api/tags/{new_id}` → 404 on valid ID, verify tag gone via GET |
| `api_folders.rs` | `folders_delete`           | `DELETE /api/folders/{new_id}` → 404 on valid ID                       |
| `api_folders.rs` | `folders_update_not_found` | `PUT /api/folders/9999` → 404                                          |
| `api_digging.rs` | `digging_suggest_no_seeds` | After fix, assert 400 instead of 500                                   |

### Files to modify

- `src/api.rs` — Part B (5 handlers) + Part C (1 handler)
- `tests/api_files.rs` — 3 weak tests
- `tests/api_tracks.rs` — 1 weak test
- `tests/api_storage.rs` — 2 weak tests
- `tests/api_digging.rs` — 2 tests (harden + fix status code)
- `tests/api_tasks.rs` — 2 weak tests
- `tests/api_tags.rs` — 1 new test (delete)
- `tests/api_folders.rs` — 2 new tests (delete + update 404)

### Acceptance Criteria

- [ ] All 12 weak assertions now verify specific values, not just field presence
- [ ] `delete_tag_handler` returns 404 for non-existent tag
- [ ] `delete_folder_handler` returns 404 for non-existent folder
- [ ] `update_folder_handler` returns 404 for non-existent folder
- [ ] `folder_backup_config_handler` returns 404 for non-existent folder
- [ ] `folder_auto_backup_handler` returns 404 for non-existent folder
- [ ] `digging_suggest_handler` returns 400 (not 500) for empty request
- [ ] All 192 existing tests still pass
- [ ] 4 new tests verify the fixed error paths
- [ ] `cargo build` passes
- [ ] Total test count: ~199 (195 + 4 new)

---

## Plan: 100-percent-coverage

**Status**: done ✅
**Branch**: `feat/100-percent-coverage`
**Ready for review**: no
**Depends on**: `feat/query-performance-optimization` (current branch), `fix/scan-folder-task-tracking` (done ✅)
**Migration needed**: no

### Description

Achieve effective 100% code coverage: every API endpoint tested (happy + sad paths),
every query param covered, all pure business logic unit-tested, external-service
modules tested with error paths. The goal is **behavioral coverage** — every code
path exercised by a test — not necessarily 100% line coverage (external services
can't run in CI).

### Current State

| Metric                               | Value                                       |
| ------------------------------------ | ------------------------------------------- |
| Source lines                         | 31,505                                      |
| Routes (unique `.route()` calls)     | 118                                         |
| Total tests                          | 195 (59 unit + 134 integration + 2 doctest) |
| Modules with 0 unit tests            | 17 of 22                                    |
| Route handler methods                | ~130 (GET+POST+PUT+DELETE across 118 paths) |
| Routes completely untested           | ~38                                         |
| Routes partially tested (error-only) | ~4                                          |

**Source modules by line count and test status**:

| Module                   | Lines  | Unit Tests | Status                               |
| ------------------------ | ------ | ---------- | ------------------------------------ |
| `api.rs`                 | 10,688 | 0          | Integration tests cover handlers     |
| `db.rs`                  | 5,265  | 1          | Vastly undertested                   |
| `tasks/mod.rs`           | 3,208  | 0          | Integration (via task endpoints)     |
| `digging.rs`             | 2,150  | 0          | Integration (via digging endpoints)  |
| `spotify/sync_worker.rs` | 1,395  | 0          | External service (error paths only)  |
| `dump.rs`                | 1,288  | 0          | Integration (via dump endpoints)     |
| `comment.rs`             | 819    | 37         | ✅ Well covered                      |
| `traktor.rs`             | 605    | 8          | ✅ Covered                           |
| `poller.rs`              | 575    | 0          | External service (error paths only)  |
| `config.rs`              | 568    | 0          | Untested (pure Rust, fully testable) |
| `global_poller.rs`       | 532    | 0          | External service (error paths only)  |
| `main.rs`                | 457    | 0          | CLI parsing testable                 |
| `embeddings.rs`          | 429    | 6          | ✅ Covered                           |
| `backup/mod.rs`          | 410    | 0          | SSH-dependent (parse logic testable) |
| `deemix/client.rs`       | 373    | 0          | External service (error paths only)  |
| `spotify/client.rs`      | 344    | 0          | External service (error paths only)  |
| `audio_extensions.rs`    | 343    | 6          | ✅ Well covered                      |
| `launch_agent.rs`        | 311    | 0          | macOS-specific (excluded)            |
| `scan_cache.rs`          | 277    | 0          | Pure Rust (fully testable)           |
| `spotify/replay.rs`      | 265    | 0          | Pure Rust (fully testable)           |
| `deemix/cli.rs`          | 227    | 0          | CLI parsing testable                 |
| `maintainer.rs`          | 221    | 0          | Scheduling logic testable            |
| `watch.rs`               | 207    | 0          | File system (smoke only)             |
| `deemix/models.rs`       | 166    | 0          | Pure Rust (fully testable)           |
| `spotify/models.rs`      | ~100   | 0          | Pure Rust (fully testable)           |

### What "100%" means (realistic target)

| Category                 | Lines  | Target         | Strategy                                                     |
| ------------------------ | ------ | -------------- | ------------------------------------------------------------ |
| `api.rs` handlers        | 10,688 | 90% behavioral | Integration (happy+sad paths, all params, all filter combos) |
| `db.rs` logic            | 5,265  | 80% line       | Unit (in-memory SQLite) + integration                        |
| `tasks/mod.rs`           | 3,208  | 40% line       | Integration (via task endpoints + scan/write triggers)       |
| `digging.rs`             | 2,150  | 80% line       | Integration (endpoints) + unit (scoring, keys, dedup)        |
| `spotify/sync_worker.rs` | 1,395  | 20% line       | Integration (error paths only — no real Spotify in CI)       |
| `dump.rs`                | 1,288  | 60% line       | Integration (endpoints) + unit (serialization)               |
| `comment.rs`             | 819    | 100% ✅        | Already covered (37 tests)                                   |
| `traktor.rs`             | 605    | 90% ✅         | Already covered (8 tests), add error paths                   |
| `poller.rs`              | 575    | 15% line       | Integration (error paths only)                               |
| `config.rs`              | 568    | 90% line       | Unit (TOML parsing, env override, priority)                  |
| `global_poller.rs`       | 532    | 15% line       | Integration (error paths only)                               |
| `main.rs`                | 457    | 50% line       | Unit (CLI parsing, build_router structure)                   |
| `embeddings.rs`          | 429    | 90% ✅         | Already covered (6 tests), add edge cases                    |
| `backup/mod.rs`          | 410    | 30% line       | Unit (path construction, output parsing)                     |
| `deemix/client.rs`       | 373    | 15% line       | Integration (error paths only)                               |
| `spotify/client.rs`      | 344    | 15% line       | Integration (error paths only)                               |
| `audio_extensions.rs`    | 343    | 100% ✅        | Already covered (6 tests)                                    |
| `launch_agent.rs`        | 311    | 0%             | Excluded (macOS launchd — can't test in CI)                  |
| `scan_cache.rs`          | 277    | 70% line       | Unit (cache hits, invalidation, expiry)                      |
| `spotify/replay.rs`      | 265    | 80% line       | Unit (replay mode, cache save/load)                          |
| `deemix/cli.rs`          | 227    | 50% line       | Unit (CLI arg parsing)                                       |
| `maintainer.rs`          | 221    | 30% line       | Unit (scheduling logic, age checks)                          |
| `watch.rs`               | 207    | 15% line       | Smoke (start/stop, no real FS in CI)                         |
| `deemix/models.rs`       | 166    | 85% line       | Unit (serialization, status variants)                        |
| `spotify/models.rs`      | ~100   | 85% line       | Unit (key conversion, From impls)                            |

**Overall behavioral target**: ≥90% of reachable code paths exercised.
**Overall line coverage target**: ≥75% (measured via `cargo-llvm-cov`).

Rationale for <100% line coverage:

- External service code (Spotify API, deemix server, SSH/NAS) can't run in CI without real credentials
- System-level code (launchd, file watchers) is inherently integration-level
- `tasks/mod.rs` and `api.rs` are mostly I/O orchestration — covered by integration tests
- The goal is **effective** coverage: every behavior path exercised, not every line

---

### Phase 1: Prerequisite fixes — ~115 lines changed

Merge the two existing proposed fix plans from this document. These must be
done first because they fix bugs in the code under test and strengthen weak
assertions that would mask regressions.

#### 1a: Fix PMV filter data source (`fix/files-pmv-filter` plan)

The Files PMV filter reads `SUBSTR(files.comment, 2, 1)` — a comment-string
artifact — instead of `file_resolved_tags.prefix` (the actual tag category
data). Fix in 3 places: `get_files()`, `get_files_count()`,
`build_files_filter_sql()`. Replace with `EXISTS (SELECT 1 FROM
file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN (...))`.

**Files**: `src/api.rs` (~30 lines, 3 locations)

#### 1b: Strengthen assertions + fix 404 handlers (`harden-test-harness` plan)

- **12 weak assertions**: Replace `contains_key()` / field-presence checks with
  specific value assertions (e.g., `localFileCount` must equal seed value 2,
  `taskId` must be non-empty string)
- **5 handlers missing proper 404**: `delete_tag_handler`, `delete_folder_handler`,
  `update_folder_handler`, `folder_backup_config_handler`,
  `folder_auto_backup_handler` — query entity first, return 404 if `None`
- **1 wrong status code**: `digging_suggest_handler` returns 500 instead of 400
  for empty request body — change to `StatusCode::BAD_REQUEST`

**Files**: `src/api.rs` (~35 lines), `tests/api_*.rs` (~50 lines)

---

### Phase 2: Missing route coverage — ~80 integration tests, ~2,000 lines

Every unique endpoint gets at least one test. Read endpoints get full filter-param
coverage. Mutations get smoke tests. Error paths (400, 404) tested.

#### 2a: Files endpoints (add ~8 tests to `tests/api_files.rs`)

| Test                               | Endpoint                                | Coverage                      |
| ---------------------------------- | --------------------------------------- | ----------------------------- |
| `files_sync_comment`               | `POST /api/files/{id}/sync-comment`     | Write comment for single file |
| `files_similar_tracks`             | `GET /api/files/{id}/similar-tracks`    | Similar tracks by tag         |
| `files_debug_comment`              | `GET /api/files/{id}/debug-comment`     | Debug comment computation     |
| `files_needs_comment_count_by_ids` | `POST /api/files/needs-comment-count`   | By file IDs                   |
| `files_write_comments_by_ids`      | `POST /api/files/write-comments-by-ids` | By file IDs                   |
| `files_backup_status`              | `GET /api/files/{id}/backup-status`     | Backup status for file        |
| `files_pull_from_backup_error`     | `POST /api/files/{id}/pull-from-backup` | Error: no SSH config          |
| `files_needs_update_count`         | `GET /api/files/needs-update-count`     | Filter-based count            |

Note: `/api/files/bulk-sync` and `/api/files/write-comments` share a handler.
`/api/files/needs-comment-count-all` and `/api/files/write-comments-all` are
higher-risk (operate on all files) — smoke-test via the existing filter-based
endpoints instead.

#### 2b: Tracks endpoints (add ~9 tests to `tests/api_tracks.rs`)

| Test                           | Endpoint                               | Coverage                              |
| ------------------------------ | -------------------------------------- | ------------------------------------- |
| `tracks_write_comments`        | `POST /api/tracks/write-comments`      | Bulk write by track IDs               |
| `tracks_needs_refresh_count`   | `POST /api/tracks/needs-refresh-count` | Refresh count                         |
| `tracks_refresh_comments`      | `POST /api/tracks/refresh-comments`    | Refresh comments                      |
| `tracks_backpack_toggle`       | `POST /api/tracks/{id}/backpack`       | Add/remove from backpack              |
| `tracks_filter_pmv_categories` | `?pmvCategories=m,v`                   | PMV filter (uses track_resolved_tags) |
| `tracks_filter_pmv_aggregate`  | `?pmvAggregate=full`                   | PMV aggregate                         |
| `tracks_filter_file_types`     | `?fileTypes=flac`                      | File type filter                      |
| `tracks_filter_file_type_agg`  | `?fileTypeAgg=any`                     | File type aggregate                   |
| `tracks_filter_date_imported`  | `?importedAfterDays=365`               | Import date filter                    |
| `tracks_filter_date_added`     | `?addedAfterDays=365`                  | Added date filter                     |
| `tracks_filter_playlist_id`    | `?playlistId=1`                        | Single playlist param                 |

#### 2c: Tags endpoints (add ~8 tests to `tests/api_tags.rs`)

| Test                         | Endpoint                               | Coverage                   |
| ---------------------------- | -------------------------------------- | -------------------------- |
| `tags_from_playlists`        | `GET /api/tags/from-playlists`         | Playlists without tags     |
| `tags_create_from_playlists` | `POST /api/tags/create-from-playlists` | Create tags from playlists |
| `tags_service_coverage`      | `GET /api/tags/service-coverage`       | Service coverage stats     |
| `tags_parents_get`           | `GET /api/tags/{id}/parents`           | Get parent tags            |
| `tags_parents_set`           | `PUT /api/tags/{id}/parents`           | Set parent tags            |
| `tags_bulk_categorize`       | `POST /api/tags/bulk-categorize`       | Bulk category assignment   |
| `tags_bulk_import`           | `POST /api/tags/bulk-import`           | Bulk import                |
| `tags_bulk_resolve`          | `POST /api/tags/bulk-resolve`          | Bulk resolve               |

#### 2d: Playlists endpoints (add ~8 tests to `tests/api_playlists.rs`)

| Test                           | Endpoint                                   | Coverage                                |
| ------------------------------ | ------------------------------------------ | --------------------------------------- |
| `playlists_delete`             | `DELETE /api/playlists/{id}`               | Delete playlist + verify 404 on refetch |
| `playlists_tracks`             | `GET /api/playlists/{id}/tracks`           | List tracks in playlist                 |
| `playlists_add_track`          | `POST /api/playlists/{id}/tracks`          | Add track to playlist                   |
| `playlists_subscriptions_list` | `GET /api/playlists/subscriptions`         | List subscriptions                      |
| `playlists_subscribe`          | `POST /api/playlists/subscriptions`        | Subscribe to playlist                   |
| `playlists_unsubscribe`        | `DELETE /api/playlists/subscriptions/{id}` | Unsubscribe                             |
| `playlists_comment_diff_stats` | `GET /api/playlists/comment-diff-stats`    | Comment diff stats                      |
| `playlists_filter_stale`       | `?stale=1`                                 | Stale playlists filter                  |

#### 2e: Folders endpoints (add ~6 tests to `tests/api_folders.rs`)

| Test                    | Endpoint                              | Coverage                                           |
| ----------------------- | ------------------------------------- | -------------------------------------------------- |
| `folders_create`        | `POST /api/folders`                   | Create folder                                      |
| `folders_update`        | `PUT /api/folders/{id}`               | Update folder                                      |
| `folders_scan`          | `POST /api/folders/{id}/scan`         | Trigger scan (path doesn't exist → task registers) |
| `folders_backup_config` | `PUT /api/folders/{id}/backup`        | Set backup config                                  |
| `folders_auto_backup`   | `PUT /api/folders/{id}/auto-backup`   | Toggle auto-backup                                 |
| `folders_scan_sources`  | `POST /api/folders/{id}/scan-sources` | Scan WAV sources                                   |

#### 2f: Storage endpoints (add ~5 tests to `tests/api_storage.rs`)

| Test                             | Endpoint                                 | Coverage                  |
| -------------------------------- | ---------------------------------------- | ------------------------- |
| `storage_settings_get`           | `GET /api/storage/settings`              | Get settings              |
| `storage_settings_put`           | `PUT /api/storage/settings`              | Update settings           |
| `storage_backup_no_ssh`          | `POST /api/storage/backup/{id}`          | Error: SSH not configured |
| `storage_backup_wavs_no_ssh`     | `POST /api/storage/backup-wavs/{id}`     | Error: SSH not configured |
| `storage_discover_backup_no_ssh` | `POST /api/storage/discover-backup/{id}` | Error: SSH not configured |

#### 2g: Service endpoints (add ~6 tests, extend `tests/api_services.rs`)

| Test                    | Endpoint                                   | Coverage                              |
| ----------------------- | ------------------------------------------ | ------------------------------------- |
| `services_config_get`   | `GET /api/services/{service}/config`       | Get service config                    |
| `services_config_put`   | `PUT /api/services/{service}/config`       | Update service config                 |
| `services_fetch_counts` | `GET /api/services/{service}/fetch-counts` | Fetch counts                          |
| `services_sync_status`  | `GET /api/services/{service}/sync-status`  | Sync status                           |
| `services_reset`        | `POST /api/services/{service}/reset`       | Reset service (error: not configured) |
| `services_deemix_auth`  | `POST /api/services/deemix/auth`           | Deemix auth (error: not configured)   |

#### 2h: Deemix endpoints (new file `tests/api_deemix.rs`, ~4 tests)

| Test                      | Endpoint                                     | Coverage                        |
| ------------------------- | -------------------------------------------- | ------------------------------- |
| `deemix_queue_list`       | `GET /api/services/deemix/queue`             | Queue list (empty — no server)  |
| `deemix_queue_add_error`  | `POST /api/services/deemix/queue`            | Add to queue (error: no server) |
| `deemix_queue_retry_404`  | `POST /api/services/deemix/queue/{id}/retry` | Retry non-existent → 404        |
| `deemix_queue_delete_404` | `DELETE /api/services/deemix/queue/{id}`     | Delete non-existent → 404       |

#### 2i: Tag energy levels (new file `tests/api_tag_energy_levels.rs`, ~3 tests)

| Test                      | Endpoint                              | Coverage         |
| ------------------------- | ------------------------------------- | ---------------- |
| `tag_energy_levels_list`  | `GET /api/tag-energy-levels`          | List all         |
| `tag_energy_levels_set`   | `PUT /api/tag-energy-levels/{tag_id}` | Set energy level |
| `tag_energy_levels_batch` | `PUT /api/tag-energy-levels/batch`    | Batch reorder    |

#### 2j: Tag categories (new file `tests/api_tag_categories.rs`, ~3 tests)

| Test                    | Endpoint                          | Coverage                            |
| ----------------------- | --------------------------------- | ----------------------------------- |
| `tag_categories_list`   | `GET /api/tag-categories`         | List all (5 defaults + any created) |
| `tag_categories_create` | `POST /api/tag-categories`        | Create category                     |
| `tag_categories_delete` | `DELETE /api/tag-categories/{id}` | Delete created category             |

#### 2k: Spotify sync endpoints (new file `tests/api_spotify_sync.rs`, ~5 tests)

All return errors when Spotify isn't configured — test the error paths:

| Test                                 | Endpoint                                           | Coverage              |
| ------------------------------------ | -------------------------------------------------- | --------------------- |
| `spotify_sync_playlists_error`       | `POST /api/services/spotify/sync/playlists`        | Error: not configured |
| `spotify_sync_new_playlists_error`   | `POST /api/services/spotify/sync/new-playlists`    | Error                 |
| `spotify_sync_playlists_batch_error` | `POST /api/services/spotify/sync/playlists/batch`  | Error                 |
| `spotify_sync_tracks_error`          | `POST /api/services/spotify/sync/tracks`           | Error                 |
| `spotify_refresh_playlist_error`     | `POST /api/services/spotify/refresh-playlist/{id}` | Error                 |

#### 2l: Infrastructure endpoints (add ~4 tests to existing files or new)

| Test                      | Endpoint                           | Coverage               |
| ------------------------- | ---------------------------------- | ---------------------- |
| `version_check`           | `GET /api/version`                 | Returns version string |
| `tag_similarities_status` | `GET /api/tag-similarities/status` | Similarities status    |
| `traktor_status`          | `GET /api/traktor/status`          | Traktor import status  |
| `traktor_import_no_file`  | `POST /api/traktor/import`         | Error: no file         |

#### 2m: Embeddings endpoints (add ~3 tests to existing or new)

| Test                         | Endpoint                               | Coverage                 |
| ---------------------------- | -------------------------------------- | ------------------------ |
| `embeddings_status`          | `GET /api/embeddings/status`           | Status (no model loaded) |
| `embeddings_recompute`       | `POST /api/embeddings/recompute`       | Triggers recompute task  |
| `tag_similarities_recompute` | `POST /api/tag-similarities/recompute` | Triggers task            |

#### 2n: Digging endpoints (add ~2 tests to `tests/api_digging.rs`)

| Test                         | Endpoint                                         | Coverage           |
| ---------------------------- | ------------------------------------------------ | ------------------ |
| `digging_ladder_suggest`     | `POST /api/digging/ladder/suggest`               | Ladder suggestions |
| `digging_tracks_with_params` | `GET /api/digging/tracks?energyLevels=1&limit=3` | Filter params      |

#### 2o: Filter combination tests (~4 tests across files)

The frontend sends multiple filters simultaneously — test that critical
combinations work and count parity holds:

| Combo                                           | Endpoint      | Test file       |
| ----------------------------------------------- | ------------- | --------------- |
| `isLocal=true` + `commentStatuses=needs_update` | `/api/files`  | `api_files.rs`  |
| `backedUp=true` + `isLocal=false`               | `/api/files`  | `api_files.rs`  |
| `hasLocal=true` + `hasBackup=true`              | `/api/tracks` | `api_tracks.rs` |
| `pmvCategories=m,v` + `hasLocal=true`           | `/api/tracks` | `api_tracks.rs` |

---

### Phase 3: Unit tests for untested modules — ~130 unit tests, ~2,000 lines

Every untested pure-Rust module gets a `#[cfg(test)]` module. External-service
modules get tests for their pure logic (parsing, conversion, error handling).

#### 3a: `src/config.rs` (~15 tests)

Test config file parsing, env var override, priority ordering, defaults:

- `config_loads_from_toml` — Parse a valid temp config.toml
- `config_env_override` — Env var `SPOTIFY_CLIENT_ID` overrides TOML
- `config_defaults` — Missing optional values get defaults
- `config_priority_order` — Env > TOML > hardcoded default
- `config_spotify_configured` — `is_spotify_configured()` returns bool
- `config_soundcloud_configured` — Same for SoundCloud
- `config_youtube_configured` — Same for YouTube
- `config_invalid_toml` — Graceful error on malformed TOML
- `config_missing_file` — Graceful when config.toml doesn't exist
- `config_empty_sections` — Empty `[spotify]` doesn't crash
- `config_polling_section` — Parse `[polling]` section
- `config_maintainer_section` — Parse `[maintainer]` section
- `config_database_url_env` — `DATABASE_URL` env var
- `config_public_url` — `PUBLIC_URL` / `MOMOS_PUBLIC_URL` env var
- `config_secrets_not_in_debug` — Secrets excluded from Debug output

#### 3b: `src/db.rs` (~40 tests)

Test pure functions and in-memory SQLite operations:

- 5 tests: Camelot key parsing (`parse_camelot_key`, display, edge cases)
- 5 tests: Comment computation (`compute_target_comment`, with/without parents, empty)
- 4 tests: Tag queries (`get_tag_by_name` nocase, `tag_exists`, by category, by backpack)
- 4 tests: File tag resolution (`get_file_resolved_tags`, `refresh_file_resolved_tags`)
- 4 tests: Prune candidates (`get_prune_candidates` with various filters)
- 3 tests: Storage status (`get_storage_status` field accuracy)
- 4 tests: File variants (`get_file_variants` ISRC grouping, WAV source grouping)
- 3 tests: WAV→stem linking (`link_wav_to_stem` parsing, matching, edge cases)
- 3 tests: File locations CRUD (local/backup tracking)
- 3 tests: Folder CRUD (create, update, delete)
- 2 tests: Playlist subscription CRUD

#### 3c: `src/digging.rs` (~20 tests)

- 5 tests: Camelot key compatibility (`are_keys_compatible` — perfect, good, ok, incompatible, edge cases)
- 3 tests: Scoring (`score_breakdown` math, edge cases, ranking order)
- 3 tests: BPM outlier detection (median-based, edge cases, single seed)
- 3 tests: ISRC dedup with format preference (stem.m4a > flac > wav)
- 2 tests: Audio format preference ranking
- 4 tests: Full `get_multi_seed_suggestions` flow (by tag, by file IDs, empty seeds, no candidates)

#### 3d: `src/dump.rs` (~10 tests)

- 3 tests: Dump to JSON (empty DB, populated DB, all tables present)
- 3 tests: Restore from JSON (valid, invalid, preserves IDs)
- 2 tests: Roundtrip (dump → restore → dump produces identical output)
- 2 tests: Edge cases (large dataset, special characters in strings)

#### 3e: `src/spotify/models.rs` (~5 tests)

- 3 tests: `spotify_key_to_camelot` — all 24 key mappings (12 minor + 12 major)
- 2 tests: Conversion from rspotify types (PlaylistInfo, TrackInfo)

#### 3f: `src/spotify/replay.rs` (~8 tests)

- 3 tests: Replay mode (enabled check, cache hit, cache miss)
- 3 tests: Cache operations (save, load, invalidation/clear)
- 2 tests: File I/O (save to temp file, load back, corrupt file error)

#### 3g: `src/scan_cache.rs` (~8 tests)

- 2 tests: Cache hit/miss (same path+size+mtime → hit, changed → miss)
- 3 tests: Cache lifecycle (expiry/TTL, clear all, LRU eviction at max entries)
- 2 tests: Serialization (save to file, load from file)
- 1 test: Empty cache behavior

#### 3h: `src/main.rs` (~5 tests)

- 3 tests: CLI parsing (`serve`, `scan-file`, `dump`, `restore` subcommands)
- 1 test: `build_router()` returns router with expected top-level routes
- 1 test: Help text includes all subcommands

#### 3i: `src/maintainer.rs` (~5 tests)

- 2 tests: Scheduling logic (interval calculation, next run time)
- 2 tests: Condition checks (full_scan_needed when last_scanned old, not needed when recent)
- 1 test: Auto-backup eligibility check

#### 3j: `src/backup/mod.rs` (~8 tests)

- 3 tests: Remote path construction (local→remote mapping)
- 2 tests: Output parsing (`ls -l` size extraction, `find` listing)
- 2 tests: Backup engine creation (with/without SSH config)
- 1 test: Dry-run output parsing

#### 3k: `src/deemix/models.rs` + `src/deemix/cli.rs` (~5 tests)

- 2 tests: Model deserialization (queue status JSON → struct)
- 2 tests: CLI argument parsing (subcommands)
- 1 test: Download status enum variants

---

### Phase 4: External service error paths — covered by Phase 2

For modules that require real external services (Spotify API, deemix server,
SSH/NAS), test only the "service not available" error paths. These are covered
by Phase 2 integration tests:

- **Spotify sync**: 5 error-path tests (Phase 2k) — `POST` returns error when not configured
- **Deemix queue**: 4 error-path tests (Phase 2h) — `POST`/`DELETE` returns error when no server
- **Backup/SSH**: 3 error-path tests (Phase 2f) — `POST` returns error when no SSH config
- **Service auth/config**: 6 tests (Phase 2g) — `GET`/`PUT`/`POST` on config endpoints

No additional work beyond Phase 2.

---

### Phase 5: Coverage measurement + iterative gap filling

#### 5a: Set up coverage tooling

```bash
# Install cargo-llvm-cov (requires nightly or Rust 1.74+)
cargo install cargo-llvm-cov

# Generate HTML coverage report
cargo llvm-cov --html --ignore-filename-regex 'tests/'

# Or with tarpaulin (works on stable)
cargo install cargo-tarpaulin
cargo tarpaulin --out Html --output-dir coverage --ignore-tests
```

#### 5b: Iterative gap filling process

1. Run coverage → generate HTML report
2. Sort modules by uncovered-line count descending
3. For each module with >20 uncovered lines, add targeted tests
4. Re-run coverage → verify improvement
5. Repeat until ≥75% line coverage

#### 5c: Coverage report as CI artifact

Add to release checklist (AGENT.md Section 1 Release Process):

- Step 7.5: Run `cargo llvm-cov --fail-under-lines 75` to verify coverage threshold

---

### Phase 6: Documentation

#### 6a: Add `tests/README.md`

Document:

- How to run tests: `cargo test`, `cargo test --test api_files`
- Test structure: unit (in `src/` via `#[cfg(test)]`), integration (in `tests/`)
- Seed data conventions: all in `tests/common/mod.rs`, explicit IDs, `refresh_file_resolved_tags()` after seeding
- How to add a new endpoint test: template + pattern
- Coverage measurement commands
- Coverage target: ≥75% line

#### 6b: Update AGENT.md Section 1 Testing rules

Replace the existing Testing section (or add to it):

```markdown
### Testing

- **`cargo test` is the single source of truth.** Every API endpoint, every filter
  parameter, every query variation must have a corresponding integration test.
  Agents must never merge code that doesn't pass `cargo test`.
- **Every plan that adds or modifies an API endpoint or filter parameter MUST
  include "add/update integration test" as an acceptance criterion.** Tests are
  not optional — they are part of the feature contract.
- **Coverage threshold**: ≥75% line coverage (via `cargo llvm-cov`). Run
  `cargo llvm-cov --fail-under-lines 75` before release.
- **Unit tests** go in `#[cfg(test)] mod tests` within the source file for
  pure functions. Integration tests go in `tests/api_*.rs` files.
- **Integration tests use a self-contained SQLite DB.** No external server, no
  real data. Each test creates a fresh in-memory DB, runs all migrations, seeds
  hand-crafted data that exercises edge cases, then hits the API and asserts
  exact results (row counts, field values, response shapes).
- **Test files mirror API structure.** `tests/api_files.rs` tests `/api/files*`,
  `tests/api_tracks.rs` tests `/api/tracks*`, etc.
- **Migration integrity is tested.** A dedicated test creates a fresh DB and
  runs all migrations end-to-end.
```

---

### Files to create

- `tests/api_deemix.rs` — ~4 tests (deemix queue endpoints)
- `tests/api_spotify_sync.rs` — ~5 tests (Spotify sync endpoints, error paths)
- `tests/api_tag_categories.rs` — ~3 tests
- `tests/api_tag_energy_levels.rs` — ~3 tests
- `tests/README.md` — documentation

### Files to modify

- `src/api.rs` — Phase 1 fixes (~65 lines)
- `src/config.rs` — add `#[cfg(test)]` module (~15 tests, ~200 lines)
- `src/db.rs` — add `#[cfg(test)]` module (~40 tests, ~600 lines)
- `src/digging.rs` — add `#[cfg(test)]` module (~20 tests, ~300 lines)
- `src/dump.rs` — add `#[cfg(test)]` module (~10 tests, ~150 lines)
- `src/spotify/models.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/spotify/replay.rs` — add `#[cfg(test)]` module (~8 tests, ~120 lines)
- `src/scan_cache.rs` — add `#[cfg(test)]` module (~8 tests, ~120 lines)
- `src/main.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/maintainer.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/backup/mod.rs` — add `#[cfg(test)]` module (~8 tests, ~120 lines)
- `src/deemix/models.rs` — add `#[cfg(test)]` module (~3 tests, ~50 lines)
- `src/deemix/cli.rs` — add `#[cfg(test)]` module (~2 tests, ~30 lines)
- `tests/common/mod.rs` — add seed helpers for new scenarios (~200 lines)
- `tests/api_files.rs` — add ~8 tests (~250 lines)
- `tests/api_tracks.rs` — add ~11 tests (~300 lines)
- `tests/api_tags.rs` — add ~8 tests (~200 lines)
- `tests/api_playlists.rs` — add ~8 tests (~200 lines)
- `tests/api_folders.rs` — add ~6 tests (~150 lines)
- `tests/api_storage.rs` — add ~5 tests (~120 lines)
- `tests/api_services.rs` — add ~6 tests (~150 lines)
- `tests/api_digging.rs` — add ~2 tests (~60 lines)
- `tests/api_tasks.rs` — add ~2 tests (~50 lines)
- `AGENT.md` — update Section 1 Testing rules, update "Last Updated"

### Acceptance Criteria

**Phase 1 (prerequisites):**

- [ ] Files PMV filter uses `file_resolved_tags.prefix`, not `SUBSTR(comment)`
- [ ] 5 handlers return proper 404 for non-existent entities (tag, folder, config)
- [ ] `digging_suggest_handler` returns 400 for empty request (not 500)
- [ ] 12 weak assertions strengthened to verify specific values
- [ ] All 195 existing tests still pass
- [ ] `cargo build` passes

**Phase 2 (route coverage):**

- [ ] Every unique API route has at least one integration test (happy or sad path)
- [ ] Every query param on FilesQuery, TracksQuery, PlaylistsQuery, TagsQuery has a test
- [ ] 400/404 error paths tested for all CRUD endpoints (create, read, update, delete)
- [ ] 4 critical filter combinations tested with count parity
- [ ] 5 new test files created: `api_deemix.rs`, `api_spotify_sync.rs`, `api_tag_categories.rs`, `api_tag_energy_levels.rs`, `tests/README.md`
- [ ] All 13 existing test files extended with missing endpoint/param coverage
- [ ] Total integration tests: ~215 (134 existing + ~80 new)

**Phase 3 (unit tests):**

- [ ] `config.rs`: 15 unit tests (TOML parsing, env override, priority, defaults)
- [ ] `db.rs`: 40 unit tests (camelot keys, comment computation, tag queries, file resolution, prune candidates, storage status, file variants, WAV linking, CRUD)
- [ ] `digging.rs`: 20 unit tests (camelot compatibility, scoring, BPM outliers, ISRC dedup, full flow)
- [ ] `dump.rs`: 10 unit tests (serialization, deserialization, roundtrip, edge cases)
- [ ] `spotify/models.rs`: 5 unit tests (key conversion, type conversions)
- [ ] `spotify/replay.rs`: 8 unit tests (replay mode, cache operations, file I/O)
- [ ] `scan_cache.rs`: 8 unit tests (hit/miss, TTL, LRU, serialization)
- [ ] `main.rs`: 5 unit tests (CLI parsing, build_router structure)
- [ ] `maintainer.rs`: 5 unit tests (scheduling, condition checks)
- [ ] `backup/mod.rs`: 8 unit tests (path construction, output parsing)
- [ ] `deemix/models.rs` + `deemix/cli.rs`: 5 unit tests (deserialization, CLI parsing)
- [ ] Total unit tests: ~190 (59 existing + ~130 new)

---

## Plan: coverage-post-modularization

**Status**: done ✅
**Branch**: `feat/coverage-round2`
**Ready for review**: no
**Depends on**: `main` (post-modularization at d59f477)
**Migration needed**: no

### Description

After the modularization of `api.rs` → `api/` (16 files) and `db.rs` → `db/` (8 files),
re-compute coverage and add targeted tests. The modularization made the coverage data
much more actionable — instead of two mega-files, we now see exactly which domain has gaps.

### Current State (post-modularization)

| Metric                             | Value                                                                                                               |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Tests                              | 415 (190 lib + 10 bin + 215 integration), all passing                                                               |
| Line coverage                      | 45.86% (22,983 lines, 10,541 covered)                                                                               |
| External (untestable) lines        | ~5,000 (spotify/\*, deemix/client, backup, global_poller, poller, watch, launch_agent, api/websocket, api/explorer) |
| Reachable (testable) line coverage | ~55.6% (9,991 / 17,983)                                                                                             |

### Coverage by module (key gaps)

**api/ modules:**

| Module                  | Lines | Cover% | Priority                                        |
| ----------------------- | ----- | ------ | ----------------------------------------------- |
| `api/files.rs`          | 2,660 | 60.58% | 🟡 +300 more lines reachable via edge cases     |
| `api/tracks.rs`         | 2,067 | 83.44% | ✅ Near max                                     |
| `api/tags.rs`           | 1,476 | 66.98% | 🟡 +150 more                                    |
| `api/playlists.rs`      | 882   | 73.76% | ✅ Good                                         |
| `api/folders.rs`        | 639   | 73.44% | ✅ Good                                         |
| `api/storage.rs`        | 371   | 44.70% | 🟡 +80 more                                     |
| `api/services.rs`       | 676   | 25.30% | 🔴 +200 more (config endpoints deeply untested) |
| `api/infrastructure.rs` | 364   | 59.30% | 🟡 +50 more                                     |
| `api/deemix_api.rs`     | 566   | 31.32% | 🔴 External dep (error paths max out ~40%)      |
| `api/explorer.rs`       | 218   | 10.94% | ⬛ External dep (SSH)                           |
| `api/websocket.rs`      | 181   | 2.92%  | ⬛ WebSocket (excluded)                         |
| `api/spotify_sync.rs`   | 405   | 22.22% | 🔴 External dep                                 |

**db/ modules:**

| Module            | Lines | Cover% | Priority                                                            |
| ----------------- | ----- | ------ | ------------------------------------------------------------------- |
| `db/files.rs`     | 1,750 | 31.00% | 🔴 **Biggest win** — mostly pure SQL builders, could add ~800 lines |
| `db/playlists.rs` | 567   | 25.21% | 🔴 +250 more via unit tests                                         |
| `db/tags.rs`      | 885   | 68.87% | ✅ Good                                                             |
| `db/folders.rs`   | 486   | 67.97% | ✅ Good                                                             |
| `db/tracks.rs`    | 245   | 56.52% | 🟡 +60 more                                                         |
| `db/storage.rs`   | 737   | 47.98% | 🔴 +200 more                                                        |
| `db/schema.rs`    | 192   | 53.00% | 🟡 +40 more                                                         |

**Other source modules:**

| Module          | Lines | Cover% | Priority                                              |
| --------------- | ----- | ------ | ----------------------------------------------------- |
| `tasks/mod.rs`  | 4,037 | 22.77% | 🔴 Big file, mostly I/O. Can add ~400 via integration |
| `digging.rs`    | 2,643 | 68.11% | 🟡 +200 via more unit tests                           |
| `dump.rs`       | 1,595 | 45.77% | 🔴 +300 via more roundtrip tests                      |
| `config.rs`     | 477   | 44.03% | 🔴 +150 via env-loading edge cases                    |
| `main.rs`       | 409   | 16.14% | 🔴 Hard to unit test (serve/startup). Can add ~80     |
| `maintainer.rs` | 163   | 34.97% | 🟡 +60 via more schedule tests                        |
| `scan_cache.rs` | 260   | 58.46% | 🟡 +40 via edge cases                                 |

**External (0-5% — can't improve without mocks):**
`spotify/*` (4 files), `global_poller.rs`, `poller.rs`, `watch.rs`,
`backup/mod.rs`, `deemix/client.rs`, `launch_agent.rs` — combined ~3,500 lines

### Target: 60% line coverage

Rationale: 75% is blocked by external services (~15% of codebase at near-0%).
60% is achievable by covering the reachable code (55.6% currently) up to ~72%.

72% of 17,983 reachable = 12,948 covered (currently 9,991 → need +2,957).
Plus external path error coverage: +200 lines → total 13,148 / 22,983 = 57.2%.

Wait — let's redo the math more carefully:

- Total lines: 22,983
- External (untestable): ~3,500 lines (0-2% coverage = ~100 covered)
- Reachable: ~19,483 lines (9,891 covered = 50.8%)

Target: 60% overall = 13,790 covered.
Currently: 10,541 covered. Need: +3,249.

To get +3,249 from reachable (19,483 lines): need 13,790 - 100 (external covered) = 13,690 from reachable.
That means reachable needs to go from 9,891 to 13,690 = +3,799 more covered reachable lines.
Reachable coverage target: 13,690 / 19,483 = 70.3%.

Hmm, that math double-counts. Let me restart:

- `reachable_covered` = total_covered - external_covered = 10,541 - ~100 = ~10,441
- `reachable_total` = total_lines - external_lines = 22,983 - 3,500 = 19,483
- `reachable_coverage` = 10,441 / 19,483 = 53.6%

To reach 60% overall: 60% \* 22,983 = 13,790 covered.
External can improve from ~100 to ~300 (more error path tests).
So reachable needs: 13,790 - 300 = 13,490.
Reachable needs +3,049 more covered lines.
Reachable target: 13,490 / 19,483 = 69.2%.

**Where the +3,049 comes from:**

| Source                                  | New tests | Est. lines gained |
| --------------------------------------- | --------- | ----------------- |
| `db/files.rs` unit tests                | ~30       | +800              |
| `db/playlists.rs` unit tests            | ~15       | +250              |
| `db/storage.rs` unit tests              | ~10       | +200              |
| `db/tracks.rs` unit tests               | ~5        | +60               |
| `db/schema.rs` unit tests               | ~3        | +40               |
| `api/services.rs` integration           | ~6        | +200              |
| `api/storage.rs` integration            | ~4        | +80               |
| `api/files.rs` integration (edge cases) | ~8        | +300              |
| `api/infrastructure.rs` integration     | ~3        | +50               |
| `api/tags.rs` integration               | ~4        | +150              |
| `digging.rs` unit tests                 | ~6        | +200              |
| `dump.rs` unit tests                    | ~8        | +300              |
| `config.rs` unit tests (env loading)    | ~6        | +150              |
| `tasks/mod.rs` integration (scan/write) | ~6        | +400              |
| `main.rs` unit tests                    | ~3        | +80               |
| `maintainer.rs` unit tests              | ~3        | +60               |
| `scan_cache.rs` unit tests              | ~3        | +40               |
| External error paths                    | ~8        | +200              |
| **Total**                               | **~125**  | **~3,560**        |

Phase 2 (external mocks) would push to 65-70%, but that's a separate plan.

### Phase 1: db/ unit tests — ~65 tests, +1,350 lines

#### 1a: `db/files.rs` — ~30 tests (~800 new covered lines)

This is the biggest gap (1,750 lines, 31%). Read the file first, then test pure functions:

- SQL builder functions that construct WHERE clauses for FilesQuery
- `build_files_filter_sql()` and variants
- File type detection helpers
- ISRC/comment/BPM processing functions
- `link_wav_to_stem()` logic
- File lifecycle queries

Each test creates an in-memory SQLite, creates the relevant tables, inserts seed rows,
calls the function, and asserts results.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Create minimal schema needed
        sqlx::query("CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY, ...)")
            .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_some_db_function() {
        let pool = test_db().await;
        // seed, call, assert
    }
}
```

#### 1b: `db/playlists.rs` — ~15 tests (~250 lines)

Test subscription CRUD, playlist query builders, stale detection logic.

#### 1c: `db/storage.rs` — ~10 tests (~200 lines)

Test prune candidate queries, storage status computation, file location CRUD.

#### 1d: Other db modules — ~10 tests (~100 lines)

`db/tracks.rs` (~5), `db/schema.rs` (~3), remaining gaps (~2).

### Phase 2: Integration test additions — ~35 tests, +830 lines

#### 2a: `api/services.rs` — ~6 tests (+200 lines)

The services config endpoints are deeply untested (25%). Add:

- `services_config_get_spotify` — get config when not configured
- `services_config_put_spotify` — update with valid JSON
- `services_config_put_invalid` — 422 on invalid body
- `services_fetch_counts_spotify` — fetch counts when not configured (500 expected)
- `services_sync_status_spotify` — sync status when not configured
- `services_reset_spotify` — reset (may error or succeed)

#### 2b: `api/files.rs` — ~8 tests (+300 lines)

Test edge cases not yet covered:

- `files_filter_bpm_exact` — exact BPM match
- `files_filter_multiple_keys` — OR list of keys
- `files_sort_play_count` — sort by play count
- `files_filter_energy` — energy level filter
- `files_filter_safe_to_delete_false` — negative case
- `files_write_comment_task_succeeds` — exercise full write flow
- `files_bulk_sync_by_filter` — filter-based bulk sync
- `files_filter_comment_missing` — files with null comment

#### 2c: Other integration gaps — ~21 tests (+330 lines)

- `api/storage.rs`: +4 tests (settings edge cases, prune execute dry run) (~80 lines)
- `api/infrastructure.rs`: +3 tests (embeddings/reset, similarities/recompute) (~50 lines)
- `api/tags.rs`: +4 tests (energy level edge cases, bulk import edge cases) (~150 lines)
- `api/services.rs`: external error path tests for spotify/deemix edge cases (+6 tests, ~200 lines)

### Phase 3: Other source module unit tests — ~30 tests, +1,130 lines

#### 3a: `digging.rs` — ~6 tests (+200 lines)

Add edge cases for the multi-seed suggestion engine:

- `suggest_with_no_compatible_tracks` — empty suggestions
- `suggest_bpm_range_clamped_to_min` — min range handling
- `suggest_bpm_range_clamped_to_max` — max range handling
- `suggest_camelot_jumps_all_off` — all jumps disabled returns empty
- `suggest_score_breakdown_exact_weights` — verify score weights
- `suggest_ranked_by_scoring_criteria` — full ranking pipeline

#### 3b: `dump.rs` — ~8 tests (+300 lines)

More roundtrip edge cases:

- `dump_with_all_table_types` — verify every table present
- `dump_large_dataset_roundtrip` — 100+ records
- `dump_unicode_strings` — special characters in paths/names
- `restore_from_corrupt_json` — halfway through valid JSON
- `restore_partial_tables` — some tables missing
- `dump_restore_preserves_foreign_keys` — FK integrity
- `dump_compares_identical` — two dumps produce identical output
- `restore_idempotent` — restoring twice produces same state

#### 3c: `config.rs` — ~6 tests (+150 lines)

Env var loading edge cases (currently 44% coverage — the env loading paths are complex):

- `config_env_or_toml_port_invalid_number` — port that's not numeric
- `config_env_or_toml_port_out_of_range` — port >65535
- `config_mixed_env_and_toml_priority` — some env, some TOML
- `config_secrets_masked_in_log` — debug doesn't leak tokens
- `config_bool_env_var_false` — "false" env var correctly parsed
- `config_bool_env_var_true` — "true" env var correctly parsed

#### 3d: Other source modules — ~10 tests (+480 lines)

- `tasks/mod.rs`: +6 integration tests for task lifecycle (create, poll, complete, cancel, timeout, error) (+400 lines)
- `main.rs`: +3 more CLI tests (+80 lines)
- `maintainer.rs`: +3 schedule edge cases (+60 lines)
- `scan_cache.rs`: +3 cache edge cases (+40 lines)

### Phase 4: External service error path coverage — ~8 tests, +200 lines

Add more error path tests for external service endpoint handlers:

- `api/deemix_api.rs`: +2 tests (deemix retry validation, delete validation) (~50 lines)
- `api/spotify_sync.rs`: +3 tests (refresh error, full sync error, task cancel error) (~80 lines)
- `api/services.rs`: +3 tests already covered in Phase 2c (~70 lines)

### Files to modify

- `src/db/files.rs` — add `#[cfg(test)]` module (~30 tests, ~600 lines)
- `src/db/playlists.rs` — add `#[cfg(test)]` module (~15 tests, ~250 lines)
- `src/db/storage.rs` — add `#[cfg(test)]` module (~10 tests, ~150 lines)
- `src/db/tracks.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/db/schema.rs` — add `#[cfg(test)]` module (~3 tests, ~40 lines)
- `src/digging.rs` — extend `#[cfg(test)]` module (~6 tests, ~150 lines)
- `src/dump.rs` — extend `#[cfg(test)]` module (~8 tests, ~200 lines)
- `src/config.rs` — extend `#[cfg(test)]` module (~6 tests, ~120 lines)
- `src/main.rs` — extend `#[cfg(test)]` module (~3 tests, ~50 lines)
- `src/maintainer.rs` — extend `#[cfg(test)]` module (~3 tests, ~40 lines)
- `src/scan_cache.rs` — extend `#[cfg(test)]` module (~3 tests, ~40 lines)
- `tests/api_services.rs` — add ~8 tests (~250 lines)
- `tests/api_files.rs` — add ~8 tests (~250 lines)
- `tests/api_storage.rs` — add ~4 tests (~80 lines)
- `tests/api_infrastructure.rs` — add ~3 tests (~50 lines)
- `tests/api_tags.rs` — add ~4 tests (~120 lines)
- `tests/api_tasks.rs` — add ~6 tests (~150 lines)
- `tests/api_deemix.rs` — add ~2 tests (~50 lines)
- `tests/api_spotify_sync.rs` — add ~3 tests (~80 lines)
- `tests/common/mod.rs` — add seed helpers (~100 lines)
- `tests/README.md` — update coverage numbers
- `AGENT.md` — update "Last Updated"

### Acceptance Criteria

- [ ] `db/files.rs` coverage: 31% → 60%+ (+30 tests)
- [ ] `db/playlists.rs` coverage: 25% → 55%+ (+15 tests)
- [ ] `db/storage.rs` coverage: 48% → 65%+ (+10 tests)
- [ ] `api/services.rs` coverage: 25% → 45%+ (+8 tests)
- [ ] `digging.rs` coverage: 68% → 75%+ (+6 tests)
- [ ] `dump.rs` coverage: 46% → 60%+ (+8 tests)
- [ ] `config.rs` coverage: 44% → 60%+ (+6 tests)
- [ ] All other targeted modules gain ≥5pp coverage
- [ ] Total tests: ~540 (415 existing + ~125 new)
- [ ] Overall line coverage: 45.86% → ≥60%
- [ ] Reachable line coverage: 53.6% → ≥69%
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all ~540 tests, <30s)
- [ ] No regressions to existing functionality

### Out of scope (requires external service mocking)

- `spotify/*` (4 files, ~2,300 lines) — needs trait-based test doubles
- `deemix/client.rs` (242 lines) — needs HTTP mock
- `backup/mod.rs` (388 lines) — needs SSH command mock
- `global_poller.rs` (314 lines) — needs Spotify client mock
- `poller.rs` (320 lines) — needs Spotify client mock
- `watch.rs` (107 lines) — needs filesystem fixture
- `launch_agent.rs` (203 lines) — macOS-specific, excluded

These 7 modules account for ~3,500 lines (15% of codebase). Mocking them would unlock
another 10-15pp coverage in a future plan.

---

### Agent Decomposition (all parallel, zero file conflicts)

The plan decomposes into 6 agents with **completely disjoint write scopes** — no
file is touched by more than one agent. All can run in parallel immediately.

| Agent | Files touched                                                                                                                | Work                                                                                         | Tests | Est. coverage gain |
| ----- | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | ----- | ------------------ |
| **A** | `src/db/files.rs`                                                                                                            | Unit tests for SQL builders, file helpers, WAV linking                                       | ~30   | +800 lines         |
| **B** | `src/db/playlists.rs`, `src/db/storage.rs`, `src/db/tracks.rs`, `src/db/schema.rs`                                           | Unit tests for subscription CRUD, prune queries, storage status, track queries               | ~35   | +550 lines         |
| **C** | `src/digging.rs`, `src/dump.rs`, `src/config.rs`                                                                             | Unit tests for scoring edge cases, dump roundtrip edge cases, env loading edge cases         | ~20   | +650 lines         |
| **D** | `src/main.rs`, `src/maintainer.rs`, `src/scan_cache.rs`                                                                      | Unit tests for CLI parsing, scheduling logic, cache edge cases                               | ~9    | +180 lines         |
| **E** | `tests/api_services.rs`, `tests/api_files.rs`, `tests/api_storage.rs`, `tests/common/mod.rs`                                 | Integration tests for service config, file edge cases, storage edge cases + ALL seed helpers | ~18   | +580 lines         |
| **F** | `tests/api_infrastructure.rs`, `tests/api_tags.rs`, `tests/api_tasks.rs`, `tests/api_deemix.rs`, `tests/api_spotify_sync.rs` | Integration tests for infra, tags, tasks, deemix, spotify sync                               | ~17   | +530 lines         |

**Write scope verification:**

- Agents A, B, C, D: all touch different `src/` files — zero overlap
- Agent E: touches `tests/api_services.rs`, `tests/api_files.rs`, `tests/api_storage.rs`, `tests/common/mod.rs` — none overlap with F
- Agent F: touches `tests/api_infrastructure.rs`, `tests/api_tags.rs`, `tests/api_tasks.rs`, `tests/api_deemix.rs`, `tests/api_spotify_sync.rs` — none overlap with E
- `tests/common/mod.rs` is only touched by Agent E, which adds ALL needed seed helpers

### Per-Agent Task Briefs

Each agent should:

1. Read the source files it's responsible for
2. Add tests following existing patterns in that file or sibling test files
3. Run `cargo test --lib` (for unit tests) or `cargo test --test FILENAME` (for integration) to verify
4. Run `cargo build` to check compilation
5. Report back with test counts, any failures, and coverage improvement estimates

Agent E additionally handles ALL seed helpers in `tests/common/mod.rs`.
Other integration agents (F) should use existing seed functions or inline seeding.

---

### Agent A: `db/files.rs` — ~30 unit tests, +800 lines

**File**: `src/db/files.rs` (1,750 lines, currently 31% coverage)

Read the file first. This module handles file queries, metadata extraction,
WAV source linking, and file lifecycle tracking. Focus on testable functions that don't require external state.

**Test targets (check what exists first, only add what's missing):**

1. SQL builder functions — construct WHERE clauses for FilesQuery, test various filter combinations
2. `build_files_filter_sql()` and variant filter builder functions
3. File type detection and classification helpers
4. BPM/comment/ISRC processing functions
5. `link_wav_to_stem()` — parse WAV filenames, extract stem_type, find parent stem
6. File lifecycle queries (create, update, delete, locations)

Use in-memory SQLite for DB-dependent tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Run migrations or create minimal schema
        pool
    }
}
```

**Verify**: `cargo test --lib -- db::files` must pass.

### Agent B: `db/playlists + storage + tracks + schema` — ~35 unit tests, +550 lines

**Files**:

- `src/db/playlists.rs` (567 lines, 25%)
- `src/db/storage.rs` (737 lines, 48%)
- `src/db/tracks.rs` (245 lines, 56%)
- `src/db/schema.rs` (192 lines, 53%)

Read all four files. Focus on testable functions:

**`db/playlists.rs`** (~15 tests):

- Subscription CRUD (subscribe, unsubscribe, list)
- Playlist query builders
- Stale detection logic (comparing local count vs remote_unique_count)
- Archive toggle queries

**`db/storage.rs`** (~10 tests):

- Prune candidate queries with filter combinations
- Storage status computation (local/backup/file counts)
- File location CRUD (insert local, insert backup, query, remove)
- Safe-to-delete logic

**`db/tracks.rs`** (~5 tests):

- Track query builders -`hasLocal`/`hasBackup` filter logic
- Playlist filtering logic

**`db/schema.rs`** (~3 tests):

- Schema introspection functions
- Migration verification helpers

**Verify**: `cargo test --lib -- db::playlists db::storage db::tracks db::schema` must pass.

### Agent C: `digging + dump + config` — ~20 unit tests, +650 lines

**Files**:

- `src/digging.rs` (2,643 lines, 68%)
- `src/dump.rs` (1,595 lines, 46%)
- `src/config.rs` (477 lines, 44%)

**`digging.rs`** (~6 tests):

- `suggest_with_no_compatible_tracks` — empty track set
- `suggest_bpm_range_clamped_to_min` — min range edge case
- `suggest_bpm_range_clamped_to_max` — max range edge case
- `suggest_camelot_jumps_all_off` — all jumps off → empty
- `suggest_score_breakdown_exact_weights` — verify score math
- `suggest_ranked_by_scoring_criteria` — verify ranking order

**`dump.rs`** (~8 tests):

- `dump_with_all_table_types` — every table present
- `dump_large_dataset_roundtrip` — 100+ records
- `dump_unicode_strings` — special chars in paths
- `restore_from_corrupt_json` — halfway-broken JSON
- `restore_partial_tables` — some tables missing
- `dump_restore_preserves_foreign_keys` — FK integrity
- `dump_compares_identical` — two dumps produce identical output
- `restore_idempotent` — restoring twice yields same state

**`config.rs`** (~6 tests):

- `config_env_or_toml_port_invalid_number` — non-numeric port
- `config_env_or_toml_port_out_of_range` — port >65535
- `config_mixed_env_and_toml_priority` — env + TOML mix
- `config_secrets_masked_in_log` — debug doesn't leak
- `config_bool_env_var_false` — "false" env var
- `config_bool_env_var_true` — "true" env var

**Verify**: `cargo test --lib -- digging dump config` must pass.

### Agent D: `main + maintainer + scan_cache` — ~9 unit tests, +180 lines

**Files**:

- `src/main.rs` (409 lines, 16%)
- `src/maintainer.rs` (163 lines, 35%)
- `src/scan_cache.rs` (260 lines, 58%)

**`main.rs`** (~3 tests):

- More CLI subcommand tests for edge cases
- `build_router()` structure test

**`maintainer.rs`** (~3 tests):

- Schedule edge cases (zero interval, very long interval)
- Condition check edge cases

**`scan_cache.rs`** (~3 tests):

- Cache edge cases (very large entries, concurrent access patterns)
- Serialization edge cases

**Verify**: `cargo test --lib -- main maintainer scan_cache` must pass.

### Agent E: Integration tests — services, files, storage + ALL seed helpers (~18 tests, +580 lines)

**Files**:

- `tests/api_services.rs`
- `tests/api_files.rs`
- `tests/api_storage.rs`
- `tests/common/mod.rs` (YOU handle ALL seed helpers needed by any integration test)

Read `tests/common/mod.rs` FIRST to understand seed patterns. Then:

**`tests/api_services.rs`** (~6 tests):

1. `services_config_get_spotify` — get config for unconfigured Spotify
2. `services_config_put_spotify` — update config with valid JSON body
3. `services_config_put_invalid` — malformed body → 422
4. `services_fetch_counts_spotify` — fetch counts for unconfigured
5. `services_sync_status_spotify` — sync status for unconfigured
6. `services_reset_spotify` — reset endpoint

**`tests/api_files.rs`** (~8 tests):

1. `files_filter_bpm_exact` — query with exact BPM value
2. `files_filter_multiple_keys` — OR list of Camelot keys
3. `files_sort_play_count` — sort by play_count field
4. `files_filter_energy` — energy level filter
5. `files_filter_safe_to_delete_false` — negative case
6. `files_write_comment_task_succeeds` — exercise full write comment flow
7. `files_bulk_sync_by_filter` — filter-based bulk sync (linked_only=true)
8. `files_filter_comment_missing` — filter files with null comment

**`tests/api_storage.rs`** (~4 tests):

1. `storage_settings_edge_cases` — test setting unusual values
2. `storage_prune_execute_dry_run` — test prune with empty file IDs
3. `storage_prune_execute_no_permission` — test error handling

**`tests/common/mod.rs`** — ADD ALL needed seed helpers:
Any new seed data needed by agents E or F. Check what `seed_basic_data()` provides, then add helpers for:

- Files with specific play counts, energy levels, null comments
- Service configs in various states

Existing test pattern:

```rust
let (client, base, pool) = common::spawn_test_app().await;
common::seed_basic_data(&pool).await;
// Add inline seeding as needed
```

Use inline seeding where possible to minimize seed helper surface area.

**Verify**: `cargo test --test api_services --test api_files --test api_storage` must pass.

### Agent F: Integration tests — infra, tags, tasks, deemix, spotify-sync (~17 tests, +530 lines)

**Files**:

- `tests/api_infrastructure.rs`
- `tests/api_tags.rs`
- `tests/api_tasks.rs`
- `tests/api_deemix.rs`
- `tests/api_spotify_sync.rs`

You do NOT modify `tests/common/mod.rs` — use existing seed functions or inline seeding.

**`tests/api_infrastructure.rs`** (~3 tests):

1. `embeddings_reset_review` — POST reset-review endpoint
2. `tag_similarities_recompute_again` — recompute with existing state
3. `version_endpoint_format` — verify version string format

**`tests/api_tags.rs`** (~4 tests):

1. `tag_energy_level_edge_cases` — set extreme energy levels (0, 10)
2. `tag_bulk_import_edge_cases` — empty import, duplicate names
3. `tag_bulk_categorize_multiple` — move multiple tags at once
4. `tag_curation_queue_pagination` — verify pagination on curation queue

**`tests/api_tasks.rs`** (~6 tests):

1. `tasks_cancel_running` — cancel a running task (scan)
2. `tasks_get_by_id` — fetch specific task by ID
3. `tasks_list_pagination` — paginated task list
4. `tasks_filter_by_type` — filter by task type (ScanFolder, WriteComment)
5. `tasks_single_not_found_strict_404` — verify 404 format
6. `tasks_multiple_concurrent` — trigger multiple tasks simultaneously

**`tests/api_deemix.rs`** (~2 tests):

1. `deemix_queue_retry_validation` — retry with invalid UUID format
2. `deemix_queue_delete_validation` — delete with no ID

**`tests/api_spotify_sync.rs`** (~3 tests):

1. `spotify_sync_task_cancel` — cancel a sync task
2. `spotify_refresh_playlist_not_found` — refresh non-existent playlist ID
3. `spotify_sync_full_error` — full sync without config

**Verify**:

```bash
cargo test --test api_infrastructure --test api_tags --test api_tasks --test api_deemix --test api_spotify_sync
```

---

### Agent Execution Order

All 6 agents can run **simultaneously** — no file overlaps. After all complete:

1. Run `cargo build` to verify compilation
2. Run `cargo test` to verify all tests pass
3. Run `cargo llvm-cov --html --ignore-filename-regex 'tests/'` to measure new coverage
4. Update `tests/README.md` with final numbers
5. Update AGENT.md plan status to done ✅

---

## Plan: configurable-format-priority

**Status**: proposed
**Branch**: `feat/configurable-format-priority`
**Depends on**: `feat/fix-backpack-local-tracking`
**Migration needed**: no

### Description

Replace hardcoded `format_preference()` ranking with a user-configurable priority
list stored in `service_config`, editable from the Storage page UI.

### API Contract (design-first)

```
GET  /api/storage/settings/format-priority
  → 200 { data: { priorities: ["stem.m4a", "flac", "mp3", "wav", "aiff"] } }

PUT  /api/storage/settings/format-priority
  ← { priorities: ["stem.m4a", "mp3", "flac"] }
  → 200 { data: { priorities: [...] } }
  → 400 { error: "priorities must be a non-empty array" }
  → 400 { error: "unknown format: xyz" }
```

### Agent Decomposition (TDD — tests written BEFORE implementation)

Two agents with **completely disjoint write scopes**:

---

#### Agent A: Backend TDD (`src/db/files.rs` + `src/api/storage.rs` + `tests/api_storage.rs`)

**Step 1 — Write failing tests** (commit these first, they WILL fail):

In `src/db/files.rs` `#[cfg(test)] mod tests`:

```rust
// Test: new format_preference respects configured order
#[test]
fn test_format_preference_with_config() {
    let prio = vec!["mp3".to_string(), "flac".to_string(), "stem.m4a".to_string()];
    assert!(format_preference_with("mp3", &prio) < format_preference_with("flac", &prio));
    assert!(format_preference_with("flac", &prio) < format_preference_with("stem.m4a", &prio));
    assert_eq!(format_preference_with("wav", &prio), u8::MAX); // not in list
}

// Test: default priorities match current hardcoded order
#[test]
fn test_default_priorities() {
    let defaults = default_format_priorities();
    assert_eq!(defaults[0], "stem.m4a");
    assert_eq!(defaults[1], "flac");
    assert_eq!(defaults[2], "mp3");
    assert_eq!(defaults[3], "wav");
}
```

In `tests/api_storage.rs`:

```rust
#[tokio::test]
async fn storage_format_priority_get_defaults() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    let resp = client.get(format!("{}/api/storage/settings/format-priority", base))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let prio = json["data"]["priorities"].as_array().unwrap();
    assert!(prio.len() >= 4, "should have at least 4 default formats");
}

#[tokio::test]
async fn storage_format_priority_put_and_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    // PUT custom order
    let put = client.put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": ["mp3", "stem.m4a", "flac"]}))
        .send().await.unwrap();
    assert_eq!(put.status(), 200);
    // GET should return the custom order
    let get = client.get(format!("{}/api/storage/settings/format-priority", base))
        .send().await.unwrap();
    let json: serde_json::Value = get.json().await.unwrap();
    let prio = json["data"]["priorities"].as_array().unwrap();
    assert_eq!(prio[0], "mp3");
    assert_eq!(prio[1], "stem.m4a");
}

#[tokio::test]
async fn storage_format_priority_put_invalid() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    // Empty array → 400
    let resp = client.put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": []}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 400);
    // Unknown format → 400
    let resp = client.put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": ["xyz"]}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 400);
}
```

**Step 2 — Implement to make tests pass**:

1. `src/db/files.rs`:
   - Rename `format_preference` → `format_preference_with(file_type, priorities)`
   - Add `default_format_priorities() -> Vec<String>` returning the current hardcoded list
   - Add `load_format_priorities(pool) -> Vec<String>` reading from `service_config`
   - Keep old `format_preference(file_type)` as a wrapper calling `format_preference_with(file_type, &default_format_priorities())` for backward compat
   - Update `get_backpack_pull_candidates()` to call `load_format_priorities()` and pass to `format_preference_with()`

2. `src/api/storage.rs`:
   - Add `format_priority_get_handler` → loads from `service_config`, returns JSON
   - Add `format_priority_put_handler` → validates, stores JSON array in `service_config`
   - Validate: non-empty array, all values are known audio extensions
   - Add routes: `.route("/api/storage/settings/format-priority", get(...).put(...))`

**Step 3 — Run tests, iterate until green**:

```bash
cargo test --lib -- format_preference
cargo test --test api_storage -- storage_format_priority
```

**Files touched**: `src/db/files.rs`, `src/api/storage.rs`, `tests/api_storage.rs`

---

#### Agent B: Frontend (`frontend/pages/storage.js` + `frontend/style.css`)

**Step 1 — Design verification**: Before writing code, manually verify the API:

```bash
curl -s http://localhost:3000/api/storage/settings/format-priority | jq
curl -s -X PUT ... -d '{"priorities":["mp3","flac"]}' | jq
```

**Step 2 — Implement**:

Add a "Format Priority" card to the Storage page below the existing stats cards.

HTML structure:

```html
<div class="card" id="format-priority-card">
  <h3><i class="fas fa-sort-amount-down"></i> Format Priority</h3>
  <p class="help-text">When pulling from backup, higher formats are preferred.</p>
  <ul class="format-priority-list" id="format-priority-list">
    <!-- JS-populated: draggable items with ▲▼ buttons -->
  </ul>
  <div class="format-priority-actions">
    <input type="text" id="format-priority-add" placeholder="flac" class="input-text" />
    <button id="format-priority-add-btn" class="btn">Add</button>
    <button id="format-priority-reset" class="btn btn-ghost">Reset defaults</button>
    <button id="format-priority-save" class="btn btn-primary">Save</button>
  </div>
</div>
```

JS behavior:

1. `loadFormatPriority()` — GET the endpoint, render list items with ▲▼ buttons
2. Click ▲/▼ → swap with neighbor, update data array
3. "Add" button → append new format to list (validate against known formats)
4. "Reset defaults" → fetch hardcoded defaults (or just use known list)
5. "Save" → PUT the endpoint, show toast
6. Drag-to-reorder (optional, nice-to-have — use HTML5 drag API or skip for v1)

**Step 3 — Manual test**:

1. Open `http://localhost:3000/#storage`
2. Change order → Save → refresh page → verify order persisted
3. Trigger backpack sync → verify preferred format pulled

**Files touched**: `frontend/pages/storage.js`, `frontend/style.css`

---

### Execution Order

Agents A and B can run **simultaneously** — zero file conflicts.

After both complete:

1. `cargo build` — verify compilation
2. `cargo test` — all tests pass (647 existing + new ones)
3. `./test-backpack.sh` — all 15 integration tests still pass

### Acceptance Criteria

- [ ] `GET /api/storage/settings/format-priority` returns default priority list
- [ ] `PUT /api/storage/settings/format-priority` persists custom order
- [ ] Empty array rejected with 400
- [ ] Unknown format rejected with 400
- [ ] `get_backpack_pull_candidates()` uses configured priority
- [ ] Default hardcoded order preserved when no config exists
- [ ] Unit tests: `format_preference_with` ordering + defaults
- [ ] Integration tests: GET defaults, PUT+GET roundtrip, PUT invalid
- [ ] Frontend: Format Priority card renders with ▲▼ reorder buttons
- [ ] Frontend: Save persists to backend, survives page refresh
- [ ] Frontend: Add format input validates against known extensions
- [ ] Frontend: Reset restores default order
- [ ] `cargo build` passes
- [ ] All existing 647 tests pass
- [ ] `./test-backpack.sh` passes (15/15)

---

## Plan: relax-prune-safety-gates

**Status**: proposed
**Branch**: `feat/fix-backpack-local-tracking` (current)
**Depends on**: auto-prune done ✅
**Migration needed**: no

### Description

Remove two overly-strict safety gates from `get_prune_candidates()` that block
legitimate prunes. The user trusts backup — if a file is on the NAS, it's safe
to delete locally.

### Problem

| Gate                                                 | Blocks               | Why                         |
| ---------------------------------------------------- | -------------------- | --------------------------- |
| `source_of IS NOT NULL` for WAVs                     | 1,242 backed-up WAVs | Never linked to parent stem |
| `bpm IS NOT NULL OR comment IS NOT NULL` for non-WAV | ~533 backed-up FLACs | Audiobooks, unscanned files |

### Fix

**`src/db/storage.rs`** — simplify `get_prune_candidates()` step 1 query.

Before:

```sql
WHERE fl.location_type = 'backup'
  AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL))
  AND (f.file_type = 'wav' OR f.bpm IS NOT NULL OR (f.comment IS NOT NULL AND f.comment != ''))
  AND EXISTS (file_locations WHERE location_type = 'local')
```

After:

```sql
WHERE fl.location_type = 'backup'
  AND EXISTS (file_locations WHERE location_type = 'local')
```

A file is safe to delete if: backed up + local + not in backpack. No other gates.

### TDD: Agent Decomposition

Single agent — one file, one change:

**Agent: Relax prune gates**

1. Read `src/db/storage.rs:520-540` to see the current query
2. Update the `get_prune_candidates` test in `src/db/storage.rs` `#[cfg(test)]` to expect more candidates (add a file without metadata but with backup+local)
3. Remove the two safety gates from the SQL
4. Verify: `cargo test --lib -- db::storage` + `cargo build`

### Acceptance Criteria

- [ ] 1,242 WAVs become prune candidates (pending `source_of`)
- [ ] ~533 metadata-less FLACs become prune candidates
- [ ] Backpack-protected files still excluded
- [ ] Non-backed-up files still excluded
- [ ] Files without `file_locations.local` still excluded
- [ ] `cargo build` + `cargo test` pass

---

## Plan: staging-area-pull

**Status**: proposed
**Branch**: `feat/staging-area-pull` (branch from feat/fix-backpack-local-tracking)
**Depends on**: `relax-prune-safety-gates`
**Migration needed**: no

### Description

A "Staging Area" for files that need metadata extraction. Pull backed-up but
metadata-less files from NAS to local, let the user scan them with Traktor,
then auto-prune deletes them again once metadata is in the DB.

### Flow

```
1. Storage page shows "X files need metadata — on NAS only"
2. User clicks "Pull for scanning" → rsyncs files from NAS to local
3. User opens Traktor, runs BPM/Key detection on the folder
4. Folder watcher detects changed files → rescans → extracts metadata
5. Next maintainer cycle: auto-prune deletes them (now backed up + metadata complete)
```

### Backend

**New endpoint**: `POST /api/storage/stage-for-scan`

```json
// Request
{ "fileTypes": ["flac"], "limit": 100 }

// Response
{ "pulled": 73, "failed": 2, "totalCandidates": 533 }
```

Logic:

1. Query files that are: backed up, NOT local, have no metadata (bpm=null AND comment=null)
2. For each: resolve backup host, rsync from NAS to local
3. Create `file_locations.local` + update `last_verified_local`
4. Return counts

**New DB function**: `src/db/files.rs` — `get_staging_pull_candidates()`

```rust
/// Files that are on backup but not local, and need metadata extraction.
pub async fn get_staging_pull_candidates(
    pool: &Pool<Sqlite>,
    file_types: Option<Vec<String>>,
    limit: Option<i64>,
) -> Result<Vec<PullCandidate>>
```

Query:

```sql
SELECT f.*, fl.path as backup_path
FROM files f
JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
WHERE f.bpm IS NULL AND (f.comment IS NULL OR f.comment = '')
  AND f.id NOT IN (SELECT file_id FROM file_locations WHERE location_type = 'local')
  AND (? IS NULL OR f.file_type IN (...))
ORDER BY f.file_type, f.file_path
LIMIT ?
```

### Frontend

Add a "Metadata Gap" card to the Storage page:

```
┌──────────────────────────────────────────────┐
│ METADATA GAP                                 │
│                                              │
│ 533 files on backup need metadata extraction │
│ (no BPM or comment)                          │
│                                              │
│ File types: [FLAC ▾]  Limit: [100 ▾]        │
│ [Pull for Scanning]                          │
│                                              │
│ Pulled 73/100 · 2 failed                     │
│                                              │
│ After scanning, the maintainer will auto-    │
│ prune them back to backup-only.              │
└──────────────────────────────────────────────┘
```

### Agent Decomposition (TDD)

Two agents, disjoint files:

**Agent A: Backend** (`src/db/files.rs` + `src/api/storage.rs` + `tests/api_storage.rs`)

Step 1 — Write failing tests:

- `test_staging_pull_candidates_no_metadata` (unit) — returns files with no bpm/comment, excludes files with metadata, excludes files already local
- `storage_stage_for_scan` (integration) — POST returns pulled/failed counts

Step 2 — Implement:

- `get_staging_pull_candidates()` in `src/db/files.rs`
- `stage_for_scan_handler` in `src/api/storage.rs`
- Route: `POST /api/storage/stage-for-scan`
- Reuse `resolve_backup_host()` and `BackupEngine::pull_file()`

**Agent B: Frontend** (`frontend/pages/storage.js` + `frontend/style.css`)

- Add "Metadata Gap" card to Storage page
- File type dropdown + limit input + "Pull for Scanning" button
- Shows results (pulled/failed counts)
- Explains the flow (scan → auto-prune)

### Acceptance Criteria

- [ ] `POST /api/storage/stage-for-scan` pulls files from NAS
- [ ] Only pulls files without metadata (no bpm, no comment)
- [ ] Skips files already local
- [ ] Respects file type filter and limit
- [ ] Storage page shows Metadata Gap card with counts
- [ ] Pull button works, shows results
- [ ] `cargo build` + `cargo test` pass

---

## Plan: daily-tagging-queue

**Status**: done ✅
**Branch**: `feat/daily-tagging-queue`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `018_canonical_playlist_id.sql`

### Description

"Daily Tagging Queue" — pick source tags, set a BPM range, generate a narrowed Spotify
playlist for on-the-go listening. Tagging happens BY adding tracks to tag-named playlists
in the Spotify mobile app. Two-way sync: tracks added on phone flow back via global poller.
The loop: curate → push → listen → tag (on phone) → sync back.

### What was built

#### Phase A: Push-to-Spotify

- **Migration 018**: `canonical_playlist_id` column + index on `service_playlists`
- **Write OAuth scopes**: Added `playlist-modify-public` + `playlist-modify-private` in 5 locations
  (`src/spotify/client.rs`, `src/api/services.rs` ×3, `src/api/websocket.rs`)
- **SpotifyClient methods**: `get_current_user_id()`, `create_playlist()`, `add_tracks_to_playlist()`
  in `src/spotify/client.rs`
- **Shared push function**: `push_playlist_to_spotify()` in `src/api/playlists.rs` — creates
  Spotify playlist, adds tracks in batches of 100, links via `canonical_playlist_id`
- **HTTP handler**: `POST /api/playlists/{id}/push-to-spotify` with `{ name?, public? }`

#### Phase B: Daily Generate Endpoint

- **New module**: `src/api/daily.rs`
- **`POST /api/daily/generate`**: Takes `{ tags, bpmMin, bpmMax, limit, excludeFullyTagged }`
  - Resolves tags → tracks via `track_resolved_tags`, filters by BPM, random sample + limit
  - Creates local playlist: `Daily-{tag}-{bpmMin}-{bpmMax}-{date}` (no spaces)
  - Best-effort push to Spotify via `push_playlist_to_spotify()`
  - Returns `{ playlistId, playlistName, trackCount, spotifyUrl }`

#### Phase C: Frontend Daily Page

- **New page**: `frontend/pages/daily.js` — tag typeahead, BPM presets, limit, exclude toggle, result card, localStorage history
- Registered in `frontend/app.js` PAGE_MAP + `frontend/shared/nav.js` TOOLS_ITEMS

### Acceptance Criteria

- [x] `cargo build` passes (zero new warnings)
- [x] `cargo test` passes (659 tests)
- [x] Migration 018 runs cleanly (001→018)
- [x] Write OAuth scopes in all 5 locations
- [x] `POST /api/daily/generate` creates playlist + pushes to Spotify
- [x] `POST /api/playlists/{id}/push-to-spotify` works independently
- [x] BPM filter, exclude-fully-tagged, random sample all work
- [x] `#daily` page renders with full form + history
- [ ] **User must re-authenticate Spotify** on Services page for write scopes

---

## Plan: push-to-spotify-ui

**Status**: done ✅
**Branch**: `feat/push-to-spotify-ui`
**Ready for review**: yes
**Depends on**: `feat/daily-tagging-queue`
**Migration needed**: no

### Description

Add "Push to Spotify" button and service badges to the Playlists page. Any
local playlist can be pushed to Spotify. Pushed playlists show an "Open in
Spotify" link. Uses the existing `POST /api/playlists/{id}/push-to-spotify`
endpoint and the new `services` field on the playlist list response.

### What was built

#### Backend: `services` field

- Added `services: Option<String>` to `Playlist` struct (with `#[sqlx(default)]`)
- SQL subquery: `COALESCE(GROUP_CONCAT(DISTINCT sp2.service), sp.service)` grouped by `canonical_playlist_id`
- Included in API response

#### Frontend: Playlists page

- **Service badges**: `services` field adapted into row data
- **Push button**: shown on local playlists not yet mirrored to Spotify
- **Open in Spotify**: green Spotify link when `services` includes `spotify`
- **Click handler**: calls `POST /api/playlists/{id}/push-to-spotify`, refreshes on success

#### Tests

- **Rust**: `playlists_list_includes_services_field` — creates local playlist, links Spotify row, asserts `services` contains both
- **Playwright**: `shows push-to-spotify button for local playlists` — seeds local playlist, asserts button visible

### Files modified

- `src/api/playlists.rs` — `Playlist.services` field, SQL subquery, JSON response
- `frontend/pages/playlists.js` — adapted `services`, push button in `actions()`, click handler
- `frontend/style.css` — `.btn-spotify` style
- `tests/api_playlists.rs` — `playlists_list_includes_services_field` test
- `frontend/tests/playlists.spec.js` — NEW FILE, 2 tests

### Acceptance Criteria

- [x] `GET /api/playlists` includes `services` field
- [x] Local-only playlist shows "Push to Spotify" button
- [x] Pushed playlist shows green Spotify button
- [x] Push button calls endpoint, refreshes on success
- [x] `cargo build` passes
- [x] `cargo test` passes (661 tests)
- [x] `cd frontend && npx playwright test` passes (15 tests)

---

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
