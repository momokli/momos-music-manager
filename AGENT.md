# Momo's Music Manager — Agent Guidance

> **Last Updated**: 2026-08-29 — v1.0.1 (release prep: merged feat/fix branches into review/all-features, ADRs 053–057)

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
6. **Update plans** — move completed plans to `plans/done/`, update `plans/README.md`, bump AGENT.md "Last Updated" date.
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
16. **Testing**: 744 tests (423 unit + 18 binary + 303 integration). Every endpoint tested, every query param covered. 59.28% line coverage target (goal: ≥75%). See `tests/README.md`.

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
- **744 tests**: 423 lib + 18 bin + 303 integration. See `tests/README.md` for
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

Plans live in [`plans/`](plans/) — one file per plan. See [`plans/README.md`](plans/README.md) for the full index with status and branches.

**Lifecycle**: `proposed` → `approved` → `in-progress` → `done` (archived to `plans/done/`)

**To create a plan**: Copy [`plans/_TEMPLATE.md`](plans/_TEMPLATE.md), fill it in, and add it to the index in `plans/README.md`.

**Quick stats**: 49 done · 0 in progress · 17 proposed
