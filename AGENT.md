# Momo's Music Manager — Agent Guidance

> **Last Updated**: 2026-06-11 — v0.8.1

---

# Section 1: Agent Reference

This section is **static** — it's the system prompt for any agent working on this project.

---

## Project Context

Music library management for DJs. Rust backend (Axum/SQLx/SQLite) + React/TypeScript frontend (Vite).
Single developer, no production data, no backward compatibility needed.

---

## Key Principles

### Workflow

1. **`main` is always clean** — never commit directly to `main`. Every change goes through a feature branch or the review staging branch.
2. **Feature branches** — `feat/short-description` or `fix/short-description`, branched from `main`.
3. **Staging branch** — `review/all-features` collects feature branches for a release. Feature branches are merged into it (not rebased). Small features or cleanup can be committed directly on it.
4. **Plan first** — every task starts with a Plan entry in Section 2 of this file. User reviews the plan, then agents are spawned.
5. **Additive migrations** — never modify `001_initial_schema.sql`. New schema changes get a new migration file.

### Release Process

When bundling features for a release:

1. **Collect branches** — merge all `feat/*` and `fix/*` branches into `review/all-features`.
2. **Consolidate migrations** — merge same-release migration files into the earliest new one.
3. **Write CHANGELOG** — from `git diff main..review/all-features`. Group by Added / Changed / Fixed.
4. **Update ADRs** — add an ADR per feature in `docs/DECISIONS.md`.
5. **Update README + AGENT.md** — bump "Last Updated", mark plans done.
6. **Verify** — `cargo build` must pass. Delete `app.db*` and test migrations from scratch.
7. **Rebase onto main** — `git rebase main` on `review/all-features`, then `git checkout main && git merge --ff-only review/all-features`.
8. **Tag** — `git tag v0.X.0` on `main`.

### Architecture

- **Schema**: 16 tables + 10 views. Run `sqlite3 app.db ".schema"` for canonical truth.
- **Separate Types**: `File` (local files with BPM/Key) vs `ServiceTrack` (service entries) — linked via `v_file_track_link` view. After v0.8, Tracks API also exposes aggregated BPM/Key/Plays from linked files.
- **Tags = Playlists**: Via name matching (case-insensitive). Setlist is default category.
- **Six system tag categories**: Setlist (S), Phase (P), Mood (M), Vibe (V), Genre (G), Merkmal (E). Each has dedicated subsystems. Phase tags carry 0–5 energy levels. Mood/Vibe/Genre are prefilled with defaults.
- **Comment Format**: `[{P}{M}{V}{G}] {tags} {source_id}` — e.g. `[PMVG] bdth build dark techno house sp:xxx`
- **Config Priority** (highest wins): Env vars > `~/.config/momos-music-manager/config.toml` > built-in defaults
- **Server-Side Filtering**: All filters must be server-side on paginated pages.
- **Testing**: 680+ tests. ≥75% line coverage target. See `tests/README.md`.

---

## System Tag Categories

These are NOT just rows in a table — each has dedicated subsystems, algorithms, and UI.

| Category | Prefix | Sort | Energy? | Subsystems                                                                                         |
| -------- | ------ | ---- | ------- | -------------------------------------------------------------------------------------------------- |
| Setlist  | S      | 0    | —       | Playlist→tag auto-resolution, `tag_parents` (comment substitution), Backpack, `file_resolved_tags` |
| Phase    | P      | 1    | 0–5 int | `tag_energy_levels` table, `compute_track_energy()`, energy filter in Digging, track similarity    |
| Mood     | M      | 2    | —       | Comment bracket `[PMVG]`, PMV filter on Files/Tracks, Digging similarity. Prefilled defaults.      |
| Vibe     | V      | 3    | —       | Same as Mood                                                                                       |
| Genre    | G      | 4    | —       | Same as Mood/Vibe. Prefilled defaults: techno, house, drum-and-bass, trance, etc.                  |
| Merkmal  | E      | 5    | —       | Freeform hashtag-like characteristics. Appears in tags section of comment, not bracket.            |

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

**Override with env vars** — `.env` file or direct export. Dev-only: `DATABASE_URL` (default `sqlite:app.db`).

---

## Agent Workflow: Before You Code

**Always get ground truth from the codebase — don't rely on this document alone.**

### Quick Orientation

```bash
# 1. Build baseline
cargo build 2>&1 | tail -5

# 2. Current DB schema (canonical truth)
rm -f /tmp/agent_app.db
DATABASE_URL=sqlite:/tmp/agent_app.db cargo run -- serve --host 127.0.0.1 --port 3001 &
sleep 2
sqlite3 /tmp/agent_app.db ".schema" | head -200
kill %1 2>/dev/null; rm -f /tmp/agent_app.db

# 3. Source modules
ls src/*.rs src/*/mod.rs src/api/*.rs src/db/*.rs 2>/dev/null | sort

# 4. Frontend pages (PAGE_MAP in app.js is authoritative)
ls frontend/pages/*.js 2>/dev/null | sort
ls frontend/src/pages/*.tsx 2>/dev/null | sort

# 5. Git state
git branch --show-current && git status --short | head -20
```

### Schema Rules

- **Never reconstruct schema from migration files.** Query the live DB.
- `sqlite3 app.db ".schema"` output IS the canonical schema.
- Migrations are additive — never edit earlier files.

### Frontend Rules

- Current: `frontend/app.js` PAGE_MAP + `frontend/shared/nav.js` NAV_SECTIONS/TOOLS_ITEMS.
- **Planned rewrite** (see Section 2): React + TypeScript + Vite replaces vanilla JS.
- During migration, both stacks coexist. New pages in `frontend/src/pages/*.tsx`.
- Playwright tests live in `frontend/tests/`.

---

## Testing

### Backend (`cargo test`)

- **Single source of truth for backend behavior.**
- Every plan adding/modifying an API endpoint MUST include integration tests.
- Coverage threshold: ≥75% line (`cargo llvm-cov --fail-under-lines 75`).
- Unit tests: `#[cfg(test)] mod tests` in source files. Integration tests: `tests/api_*.rs`.
- Integration tests use fresh in-memory SQLite, run all migrations, seed data, hit API, assert exact results.
- Test files mirror API structure: `tests/api_files.rs` ↔ `/api/files*`.

### Frontend (`cd frontend && npx playwright test`)

- **Single source of truth for frontend behavior.**
- Every new page/feature MUST include Playwright tests.
- Tests auto-start Rust server with isolated test DB, seed data via `POST /api/testing/seed`.
- Available seed scenarios: `basic`, `files_filter`, `digging`, `wav_variants`, `dynamic_bundles`.
- Every smoke test MUST assert `pageerror` events length === 0.
- New seed scenarios go in `src/db/testing.rs`, registered in `src/api/infrastructure.rs`.
- After React rewrite: add `npx tsc --noEmit` gate before Playwright.

### Agent Validation Checklist

```bash
cargo build                          # 1. Backend compiles
cargo test                           # 2. All backend tests pass
cd frontend && npx playwright test   # 3. All frontend tests pass
# After React rewrite, also:
# cd frontend && npx tsc --noEmit    # TypeScript compiles
```

---

## Current Migration Map (001–020)

Use as quick index. For actual SQL, query `sqlite3 app.db ".schema"`.

| File                              | What it does                                                                                                                            |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `001_initial_schema.sql`          | 13 tables + 9 views + seed data (5 tag categories, phase tags, energy levels)                                                           |
| `002_playlist_fetch_tracking.sql` | `last_fetched_at` + `remote_track_count` on `service_playlists`; `tag_parents` table + `v_resolved_tags` + `v_file_resolved_tags` views |
| `003_remote_unique_count.sql`     | `remote_unique_count` on `service_playlists`                                                                                            |
| `004_unique_tags_nocase.sql`      | Rebuilds `tags` with `UNIQUE COLLATE NOCASE`, deduplicates, remaps FKs                                                                  |
| `005_v_playlist_tag_category.sql` | `v_playlist_tag_category` view                                                                                                          |
| `006_local_service.sql`           | `'local'` in `service_tracks` CHECK, updates `v_file_track_link`                                                                        |
| `007_playlist_snapshot.sql`       | `snapshot_id` on `service_playlists`                                                                                                    |
| `008_playlist_track_archive.sql`  | Soft-delete `deleted_at` on `service_playlist_tracks` + `archive_deleted` toggle                                                        |
| `009_file_lifecycle.sql`          | `file_locations`, `followed`→`backpack` on tags, `source_of` on files, folder backup config                                             |
| `010_auto_backup.sql`             | `auto_backup` on `folders`                                                                                                              |
| `011_file_resolved_tags.sql`      | Materialized `file_resolved_tags` table + indexes                                                                                       |
| `012_wav_stem_type.sql`           | `stem_type` on `files`                                                                                                                  |
| `013_backup_discovery.sql`        | `last_verified_local` on `files`                                                                                                        |
| `014_v_track_tags.sql`            | `v_track_tags` view                                                                                                                     |
| `015_track_resolved_tags.sql`     | Materialized `track_resolved_tags` table                                                                                                |
| `016_backpack_rename.sql`         | Renamed `tags.followed` → `tags.backpack`                                                                                               |
| `017_tag_bundles.sql`             | `tag_bundles` table                                                                                                                     |
| `018_canonical_playlist_id.sql`   | `canonical_playlist_id` on `service_playlists`                                                                                          |
| `019_dynamic_bundles.sql`         | `dynamic_bundles` table                                                                                                                 |
| `020_dynamic_bundle_filters.sql`  | Additional dynamic bundle filter columns                                                                                                |

---

## Important Gotchas

- **Migrations are additive** — never edit earlier files.
- **Schema truth is in the DB**, not migration files.
- **Frontend**: Currently vanilla JS, hash-based SPA router. Pending React+TS rewrite.
- **Playlist subscriptions** poll every 30s (`poller.rs`).
- **Global playlist poller** checks all Spotify playlists every 15 min via snapshot detection (`global_poller.rs`).
- **Maintainer** runs background housekeeping every 1h (`maintainer.rs`).
- **Backpack sync** manages offline file presence, pulls from NAS, cleans redundant formats.

---

## Source Modules

> Run `ls src/*.rs src/*/mod.rs src/api/*.rs src/db/*.rs | sort` for authoritative list.

```
src/
├── main.rs                 # CLI, router, server start
├── audio_extensions.rs     # AudioExtension enum
├── comment.rs              # Comment parsing/generation ([PMVG] format)
├── config.rs               # Config.toml + env var loading
├── digging.rs              # Multi-seed curator, energy computation, track similarity
├── dump.rs                 # DB dump/restore (JSON)
├── embeddings.rs           # Semantic tag embeddings (candle/ML)
├── global_poller.rs        # Global playlist poller (snapshot-based)
├── launch_agent.rs         # macOS launch agent integration
├── maintainer.rs           # Background housekeeping scheduler
├── poller.rs               # Playlist subscription poller
├── scan_cache.rs           # File scan result caching
├── traktor.rs              # Traktor collection.nml parser
├── watch.rs                # Folder watcher
├── api/
│   ├── mod.rs              # Router composition
│   ├── daily.rs            # Daily tagging queue
│   ├── deemix_api.rs       # Deemix queue endpoints
│   ├── digging.rs          # Digging suggest/search/tracks
│   ├── explorer.rs         # Backup explorer (SSH)
│   ├── files.rs            # Files CRUD + filters
│   ├── folders.rs          # Folder management
│   ├── infrastructure.rs   # Health, dump, restore, testing seed, tag-energy
│   ├── playlists.rs        # Playlist CRUD + push-to-spotify
│   ├── services.rs         # Service config/auth
│   ├── spotify_sync.rs     # Spotify sync routes
│   ├── storage.rs          # Backup/prune/settings
│   ├── tags.rs             # Tags + tag-energy-levels + tag-categories
│   ├── tracks.rs           # Tracks CRUD + filters
│   └── websocket.rs        # WebSocket (Spotify OAuth)
├── backup/
│   └── mod.rs              # BackupEngine (SSH/rsync)
├── db/
│   ├── mod.rs              # Re-exports
│   ├── files.rs            # File queries, scan, WAV linking
│   ├── folders.rs          # Folder queries
│   ├── playlists.rs        # Playlist queries, tag resolution
│   ├── schema.rs           # Schema introspection
│   ├── storage.rs          # Prune candidates, storage status
│   ├── tags.rs             # Tag queries, bundles
│   ├── testing.rs          # Test seed scenarios
│   └── tracks.rs           # Track queries
├── deemix/
│   ├── mod.rs, cli.rs, client.rs, models.rs
├── soundcloud/
│   ├── mod.rs, client.rs, models.rs, sync_worker.rs
├── spotify/
│   ├── mod.rs, client.rs, models.rs, sync_worker.rs
└── tasks/
    └── mod.rs              # TaskManager + all task workers
```

---

## Frontend Pages (current vanilla JS SPA)

> **Authoritative**: `PAGE_MAP` in `frontend/app.js` + `NAV_SECTIONS`/`TOOLS_ITEMS` in `frontend/shared/nav.js`.

| Route              | Nav Section | Description                                  |
| ------------------ | ----------- | -------------------------------------------- |
| `#dashboard`       | Overview    | Stats cards + recent activity                |
| `#files`           | Library     | Local files table                            |
| `#tracks`          | Library     | Service tracks table                         |
| `#playlists`       | Library     | All playlists                                |
| `#tags`            | Library     | Tags table                                   |
| `#tag-categories`  | Library     | Tag categories                               |
| `#services`        | Services    | Service status/config                        |
| `#tasks`           | Services    | Task manager                                 |
| `#folders`         | Services    | Folder management                            |
| `#deemix-queue`    | Services    | Deemix download queue                        |
| `#traktor-import`  | Services    | Traktor collection import                    |
| `#tag-curation`    | Tools       | Tag parent curation                          |
| `#auto-categorize` | Tools       | AI tag categorization                        |
| `#digging`         | Tools       | Digging Curator (multi-seed, audio, staging) |
| `#data`            | Tools       | Import/export database                       |
| `#key-comparison`  | Tools       | Traktor vs Spotify BPM/Key                   |
| `#storage`         | Tools       | Backup/prune management                      |
| `#backpack`        | Tools       | Offline file sync                            |
| `#daily`           | Tools       | Daily tagging queue                          |
| `#tag-bundles`     | Tools       | Static tag bundles                           |
| `#dynamic-bundles` | Tools       | Filter-based dynamic bundles                 |
| `#track-detail`    | (linked)    | Single track metadata detail                 |
| `#file-detail`     | (linked)    | Single file metadata detail                  |
| `#folder-detail`   | (linked)    | Folder detail                                |

---

## Docs

- `docs/ARCHITECTURE.md` — System design
- `docs/DECISIONS.md` — ADRs
- `docs/COMMENT_SYSTEM.md` — Comment format spec (`[PMVG]`)
- `docs/TASK_MANAGER.md` — Task manager details
- `CHANGELOG.md` — Release changelog
- `tests/README.md` — Test structure + coverage

---

## Handover

1. Document progress and decisions in `docs/DECISIONS.md`
2. Run `cargo build` — must pass
3. Run `cargo test` — all tests must pass
4. Run `cd frontend && npx playwright test` — all frontend tests must pass
5. After React migration: also run `cd frontend && npx tsc --noEmit`

---

---

# Section 2: Active Plans

**Lifecycle**: `proposed` → `approved` → `in-progress` → `done`

---

## Plan: frontend-rewrite

**Status**: proposed
**Branch**: `feat/frontend-rewrite-plan`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `021_genre_category.sql` (add Genre as 6th system category)

### Description

Rewrite the frontend from vanilla JS SPA to React + TypeScript + Vite. The goal is **agent confidence** — three gates (TypeScript compiler → Vite build → Playwright E2E) that, when all green, guarantee the code is correct. The current vanilla JS architecture has a systemic bug: manual DOM manipulation drifts from state (filter buttons freeze, badges don't disappear, spinners never stop). React's declarative model eliminates this class of bugs entirely.

Simultaneously, restructure the navigation from a 21-page horizontal top bar to a 7-page vertical sidebar organized by user workflow frequency — candy first (Dig, Daily, Pack), then library (Tracks, Lists, Tags), then setup.

### Why React + TypeScript + Vite

| Concern               | Vanilla JS (current)           | React + TypeScript (proposed)      |
| --------------------- | ------------------------------ | ---------------------------------- |
| State↔DOM sync        | Manual, often forgotten → bugs | Automatic reconciliation           |
| Typos in API params   | Silent runtime failure         | Compiler rejects at build time     |
| Props/contracts       | Convention, easily broken      | TypeScript interfaces              |
| Loading/error states  | Manual boilerplate per page    | React Query handles it             |
| Pre-flight confidence | 1 gate (Playwright)            | 3 gates (tsc → build → Playwright) |

### Target Architecture

```
frontend/
├── index.html                 # Vite entry point
├── src/
│   ├── main.tsx               # App entry: router + query client
│   ├── App.tsx                # Layout: sidebar + content area
│   ├── components/            # Shared UI components
│   │   ├── Sidebar.tsx
│   │   ├── FilterPanel.tsx
│   │   ├── CrudTable.tsx
│   │   ├── Typeahead.tsx
│   │   ├── ChipInput.tsx
│   │   ├── BPMRangeSlider.tsx
│   │   ├── KeyGrid.tsx
│   │   ├── PaginationBar.tsx
│   │   ├── AudioPlayer.tsx
│   │   ├── Waveform.tsx
│   │   └── Toast.tsx
│   ├── pages/                 # Page components
│   │   ├── Dashboard.tsx
│   │   ├── Digging.tsx
│   │   ├── Daily.tsx
│   │   ├── Backpack.tsx
│   │   ├── Tracks.tsx         # Unified Files+Tracks
│   │   ├── Lists.tsx          # Playlists
│   │   ├── Tags.tsx           # Hub: 7 sections
│   │   └── Setup.tsx          # Hub: cards for services, storage, etc.
│   ├── api/                   # React Query hooks
│   │   ├── client.ts
│   │   ├── tracks.ts
│   │   ├── playlists.ts
│   │   ├── tags.ts
│   │   ├── digging.ts
│   │   └── storage.ts
│   ├── types/                 # Shared TypeScript interfaces
│   │   ├── track.ts
│   │   ├── tag.ts
│   │   ├── playlist.ts
│   │   └── api.ts
│   └── utils/                 # Pure functions (unit-testable)
│       ├── camelot.ts
│       ├── format.ts
│       └── time.ts
├── tests/                     # Playwright E2E tests
└── vite.config.ts, tsconfig.json, playwright.config.ts
```

### New Nav: Sidebar Layout

```
┌──────────────┬──────────────────────────────────────────────────────┐
│  🎵 momo's   │                                                      │
│              │                                                      │
│ WORKFLOWS    │                                                      │
│ 🔍 Dig       │                                                      │
│ 📅 Daily     │                  PAGE CONTENT                        │
│ 🎒 Pack      │                                                      │
│              │                                                      │
│ LIBRARY      │                                                      │
│ 🎵 Tracks    │                                                      │
│ 📋 Lists     │                                                      │
│ 🏷️ Tags      │                                                      │
│              │                                                      │
│ ⚙ Setup      │                                                      │
└──────────────┴──────────────────────────────────────────────────────┘
```

### Page Merge Plan (21 → 7)

| Before (21 pages)                                                                                     | After (7 pages) | Notes                                                                                |
| ----------------------------------------------------------------------------------------------------- | --------------- | ------------------------------------------------------------------------------------ |
| Dashboard                                                                                             | Dashboard       | Kept as landing                                                                      |
| Files + Tracks                                                                                        | **Tracks**      | Unified track browser (Phase 2 — keep separate initially)                            |
| Playlists                                                                                             | **Lists**       | Renamed                                                                              |
| Tags + Tag Categories + Tag Curation + Auto-Categorize + Tag Bundles + Dynamic Bundles                | **Tags**        | Single page, 7 sections (Energy Curve, Mood, Vibe, Genre, Merkmal, Setlist, Bundles) |
| Services + Folders + Storage + Tasks + Deemix Queue + Traktor Import + Import/Export + Key Comparison | **Setup**       | Card-layout hub page                                                                 |
| Digging                                                                                               | **Dig**         | Promoted to top-level                                                                |
| Daily                                                                                                 | **Daily**       | Promoted                                                                             |
| Backpack                                                                                              | **Pack**        | Promoted                                                                             |

### Tags Hub Design

Each system category gets its own tailored section — not a flat table:

```
TAGS PAGE
┌─ ENERGY CURVE ──────────────────────────────────────────────────┐
│  [visual energy curve: Peak(5) Build(4) Sus(3) Start(2) Rel(1) End(0)] │
│  Energy levels drive track similarity in Digging.               │
└──────────────────────────────────────────────────────────────────┘
┌─ MOOD ──────────────────────────────────────────────────────────┐
│  [dark] [melodic] [hypnotic] [raw] [groovy] [euphoric] ...     │
│  Prefilled defaults. User adds custom moods.                    │
└──────────────────────────────────────────────────────────────────┘
┌─ VIBE ──────────────────────────────────────────────────────────┐
│  [techno] [house] [warehouse] [afterhour] [industrial] ...     │
│  Same pattern as Mood.                                          │
└──────────────────────────────────────────────────────────────────┘
┌─ GENRE ─────────────────────────────────────────────────────────┐
│  [techno] [house] [drum-and-bass] [trance] [hard-techno] ...   │
│  New system category. Prefilled defaults.                       │
└──────────────────────────────────────────────────────────────────┘
┌─ MERKMAL ───────────────────────────────────────────────────────┐
│  [bassline] [warehouse] [rolling] [acid] [breakdown] ...       │
│  Freeform hashtags. No defaults. User creates all.              │
└──────────────────────────────────────────────────────────────────┘
┌─ SETLIST ─────────────────────────────────────────────── [CURATE]┐
│  [table: tag name | parent tags | files | backpack]             │
│  Auto-created from playlist names. Curate button opens          │
│  parent-tag assignment workflow.                                │
└──────────────────────────────────────────────────────────────────┘
┌─ BUNDLES ───────────────────────────────────────────────────────┐
│  Static: [Hard Techno 140-160] [Deep House 120-128]            │
│  Dynamic: [Current Rotation] [Peak Time]                        │
│  [+ New Bundle]                                                 │
└──────────────────────────────────────────────────────────────────┘
```

### Migration 021: Genre System Category

```sql
-- Add Genre as the 6th system tag category
INSERT INTO tag_categories (name, icon, prefix, sort_order, is_default) VALUES
    ('Genre', 'fa-solid fa-guitar', 'G', 4, FALSE);

-- Update Merkmal sort_order (was 4, now 5)
UPDATE tag_categories SET sort_order = 5 WHERE prefix = 'E';

-- Seed default genre tags
INSERT INTO tags (name, category_id, sort_order) VALUES
    ('techno', (SELECT id FROM tag_categories WHERE prefix = 'G'), 0),
    ('house', (SELECT id FROM tag_categories WHERE prefix = 'G'), 1),
    ('drum-and-bass', (SELECT id FROM tag_categories WHERE prefix = 'G'), 2),
    ('trance', (SELECT id FROM tag_categories WHERE prefix = 'G'), 3),
    ('hard-techno', (SELECT id FROM tag_categories WHERE prefix = 'G'), 4),
    ('minimal', (SELECT id FROM tag_categories WHERE prefix = 'G'), 5),
    ('deep-house', (SELECT id FROM tag_categories WHERE prefix = 'G'), 6),
    ('tech-house', (SELECT id FROM tag_categories WHERE prefix = 'G'), 7),
    ('progressive', (SELECT id FROM tag_categories WHERE prefix = 'G'), 8),
    ('dub-techno', (SELECT id FROM tag_categories WHERE prefix = 'G'), 9);

-- Comment format extends from [PMV] to [PMVG]
-- Update compute_target_comment() in src/db/files.rs to handle 4-char bracket
SELECT 'Migration 021 applied: Genre system category + seed tags' as status;
```

### Agent Validation Gates (after rewrite)

```bash
cd frontend

# Gate 1: TypeScript compiles? (catches ~80% of bugs before a single line runs)
npx tsc --noEmit

# Gate 2: Build succeeds?
npx vite build

# Gate 3: All E2E tests pass?
npx playwright test

# All three green → ship with confidence.
```

### Migration Strategy: Strangler Fig

1. Add Vite + React + TypeScript alongside existing vanilla JS
2. Build new pages in React first (Digging, Daily, Tags Hub — the candy)
3. Port existing pages one by one — each port deletes the old `.js` file
4. Delete `app.js` router when last vanilla page is ported
5. Old and new stacks coexist during migration — no big-bang cutover

### Files to create

- `frontend/package.json`, `tsconfig.json`, `vite.config.ts` — Vite + React + TS scaffold
- `frontend/src/` — entire React app directory tree
- `migrations/021_genre_category.sql` — Genre system category + seed tags

### Files to modify

- `src/db/files.rs` — extend `compute_target_comment()` for `[PMVG]` bracket
- `src/comment.rs` — update comment parser for 4-character brackets
- `AGENT.md` — this update

### Files to delete (gradually, as pages are ported)

- `frontend/pages/*.js` — all 24 vanilla JS pages
- `frontend/shared/` — utilities and components
- `frontend/app.js` — old hash router

### TDD Strategy: Tests First, Always

Every line of React/TypeScript code is justified by a Playwright test that:

1. **Fails first** — the test proves the feature doesn't exist yet
2. **Passes after implementation** — the test proves the feature works
3. **Survives refactors** — the test catches regressions

This means tests are written BEFORE the React component. The agent workflow:

```bash
# Agent's TDD cycle for each page/feature:
# 1. Write the test → run → it FAILS (feature doesn't exist)
cd frontend && npx playwright test -- tests/new-page.spec.ts

# 2. Implement the React component
# 3. Run the test → it PASSES
cd frontend && npx playwright test -- tests/new-page.spec.ts

# 4. Add to tsconfig, vite build, app router
# 5. Run all gates → all green → ship
npx tsc --noEmit && npx vite build && npx playwright test
```

### Test Infrastructure Changes

#### Dual-stack Playwright config

During migration, Playwright tests two separate apps from one config:

```typescript
// playwright.config.ts — two projects
projects: [
  {
    name: "react",
    testDir: "./tests/",
    testMatch: /.*\.spec\.ts/,
    use: { baseURL: "http://localhost:5173" }, // Vite dev server
  },
  {
    name: "vanilla",
    testDir: "./tests/",
    testMatch: /.*\.spec\.js/,
    use: { baseURL: "http://localhost:3000" }, // Rust embedded
  },
];
```

- **React tests** (`.spec.ts`): run against Vite dev server (`npm run dev`), use TypeScript
- **Vanilla tests** (`.spec.js`): run against Rust embedded server, unchanged during migration
- **Shared seed endpoint**: both projects hit `POST /api/testing/seed` on the Rust server (port 3000)
- **Global setup**: starts both Vite dev server + Rust server before tests

#### New seed scenarios needed

| Scenario        | Needed by          | What it seeds                                                                                                                         |
| --------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------------- |
| `tags_hub`      | Tags page tests    | All 6 system categories, phase tags with energy levels, mood/vibe/genre defaults, setlist tags with parents, static + dynamic bundles |
| `setup_hub`     | Setup page tests   | Services in various states, folders with backup config, storage stats with files                                                      |
| `sidebar_nav`   | Navigation tests   | Basic data + all page routes registered                                                                                               |
| `digging_react` | Digging page tests | Seed files with BPM/Key/tags, same as `digging` scenario but with React-compatible data                                               |

---

### Phase 0: Scaffold (test infrastructure first)

**Test file**: `frontend/tests/scaffold.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test("Vite dev server serves index.html", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (err) => errors.push(err.message));

  await page.goto("/");
  await expect(page.locator("#root")).toBeVisible({ timeout: 10000 });
  expect(errors).toEqual([]);
});

test("React mounts without errors", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (err) => errors.push(err.message));

  await page.goto("/");
  // React renders SOMETHING into #root (even if just a placeholder)
  const root = page.locator("#root");
  await expect(root).not.toBeEmpty({ timeout: 10000 });
  expect(errors).toEqual([]);
});

test("TypeScript compiles without errors", async () => {
  // This is verified by the tsc --noEmit gate, but document it
  // The test passes when the CI script runs tsc first
});
```

**Acceptance**:

- [ ] `npx playwright test -- tests/scaffold.spec.ts` FAILS (no Vite/React yet)
- [ ] After `npm create vite@latest`, `npm install`, test PASSES
- [ ] `npx tsc --noEmit` passes
- [ ] `npx vite build` produces a bundle

---

### Phase 1: Sidebar Navigation (TDD)

**Write these tests FIRST — they will all FAIL until the sidebar is built.**

**Test file**: `frontend/tests/sidebar.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test.describe("Sidebar Navigation", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders 7 nav items", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/");
    const nav = page.locator("[data-sidebar]");
    await expect(nav).toBeVisible();

    // 7 nav links: Dig, Daily, Pack, Tracks, Lists, Tags, Setup
    const links = nav.locator("[data-nav-item]");
    await expect(links).toHaveCount(7);
    expect(errors).toEqual([]);
  });

  test("has three section headers", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-nav-section="workflows"]')).toBeVisible();
    await expect(page.locator('[data-nav-section="library"]')).toBeVisible();
    await expect(page.locator('[data-nav-section="setup"]')).toBeVisible();
  });

  test("workflow items are Dig, Daily, Pack", async ({ page }) => {
    await page.goto("/");
    const workflowSection = page.locator('[data-nav-section="workflows"]');
    const items = workflowSection.locator("[data-nav-item]");
    await expect(items).toHaveCount(3);
    await expect(items.nth(0)).toContainText("Dig");
    await expect(items.nth(1)).toContainText("Daily");
    await expect(items.nth(2)).toContainText("Pack");
  });

  test("library items are Tracks, Lists, Tags", async ({ page }) => {
    await page.goto("/");
    const libSection = page.locator('[data-nav-section="library"]');
    const items = libSection.locator("[data-nav-item]");
    await expect(items).toHaveCount(3);
    await expect(items.nth(0)).toContainText("Tracks");
    await expect(items.nth(1)).toContainText("Lists");
    await expect(items.nth(2)).toContainText("Tags");
  });

  test("setup item exists", async ({ page }) => {
    await page.goto("/");
    const setupSection = page.locator('[data-nav-section="setup"]');
    await expect(setupSection.locator("[data-nav-item]")).toContainText("Setup");
  });

  test("active nav item is highlighted", async ({ page }) => {
    await page.goto("/");
    await page.click('[data-nav-item="dig"]');
    // The Dig item should have an active class or attribute
    await expect(page.locator('[data-nav-item="dig"]')).toHaveAttribute(
      "data-active",
      "true",
    );
  });

  test("clicking nav item navigates to correct page", async ({ page }) => {
    await page.goto("/");
    await page.click('[data-nav-item="tags"]');
    // URL should reflect the Tags page (hash or path-based)
    await expect(page).toHaveURL(/.*tags.*/);
    // Tags page content should be visible
    await expect(page.locator('[data-page="tags"]')).toBeVisible();
  });

  test("hash-based navigation still works for backward compat", async ({ page }) => {
    await page.goto("/#digging");
    // Should navigate to the Dig page
    await expect(page.locator('[data-page="digging"]')).toBeVisible();
  });
});
```

**Acceptance**:

- [ ] All 8 tests FAIL before Sidebar.tsx exists
- [ ] All 8 tests PASS after Sidebar.tsx + App.tsx + React Router implemented
- [ ] Sidebar renders at 220px width, sticky, dark background
- [ ] Section headers (WORKFLOWS, LIBRARY, SETTINGS) visible with muted styling
- [ ] Active item has accent color + left border indicator
- [ ] Version number displayed at bottom of sidebar

---

### Phase 2: Dashboard (TDD)

**Test file**: `frontend/tests/dashboard.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test.describe("Dashboard Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/");
    await expect(page.locator('[data-page="dashboard"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("shows stats cards", async ({ page }) => {
    await page.goto("/");
    // At minimum, shows files/tracks/playlists/tags counts
    await expect(page.locator('[data-stat="files"]')).toBeVisible();
    await expect(page.locator('[data-stat="tracks"]')).toBeVisible();
    await expect(page.locator('[data-stat="playlists"]')).toBeVisible();
    await expect(page.locator('[data-stat="tags"]')).toBeVisible();
  });

  test("shows service status", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-service-status]")).toBeVisible();
  });

  test("shows recent activity", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-recent-activity]")).toBeVisible();
  });

  test("quick action buttons work", async ({ page }) => {
    await page.goto("/");
    // Sync all button
    const syncBtn = page.locator('[data-action="sync-all"]');
    await expect(syncBtn).toBeVisible();
    // Navigate buttons
    await expect(page.locator('[data-action="go-to-files"]')).toBeVisible();
  });
});
```

**Acceptance**:

- [ ] 5 tests FAIL before Dashboard.tsx exists
- [ ] 5 tests PASS after Dashboard implemented
- [ ] Stats cards show real counts from API, not hardcoded zeros
- [ ] Service status shows connected/not-configured per service
- [ ] Page is the default landing route (`/`)

---

### Phase 3: Digging Page (TDD)

**Test file**: `frontend/tests/digging.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test.describe("Digging Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "digging" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/#/digging");
    await expect(page.locator('[data-page="digging"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("tag typeahead finds and selects tags", async ({ page }) => {
    await page.goto("/#/digging");
    const input = page.locator("[data-digging-tag-search]");
    await input.fill("collapse");
    // Dropdown should appear with matching tags
    await expect(page.locator("[data-digging-tag-dropdown]")).toBeVisible();
    await page.locator("[data-digging-tag-dropdown] >> text=Collapse-capital").click();
    // Tag chip should appear
    await expect(page.locator("[data-digging-tag-chip]")).toContainText(
      "Collapse-capital",
    );
  });

  test("find similar returns suggestions", async ({ page }) => {
    await page.goto("/#/digging");
    // Select a tag
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page.locator("[data-digging-tag-dropdown] >> text=Collapse-capital").click();
    // Click Find Similar
    await page.locator('[data-action="find-similar"]').click();
    // Suggestions should appear
    await expect(page.locator("[data-digging-suggestion]").first()).toBeVisible({
      timeout: 10000,
    });
  });

  test("suggestions show BPM, key, camelot compatibility", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page.locator("[data-digging-tag-dropdown] >> text=Collapse-capital").click();
    await page.locator('[data-action="find-similar"]').click();

    const first = page.locator("[data-digging-suggestion]").first();
    await expect(first.locator('[data-field="bpm"]')).toBeVisible({ timeout: 10000 });
    await expect(first.locator('[data-field="key"]')).toBeVisible();
    await expect(first.locator("[data-camelot-compat]")).toBeVisible();
  });

  test("BPM range slider filters suggestions", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page.locator("[data-digging-tag-dropdown] >> text=Collapse-capital").click();
    await page.locator('[data-action="find-similar"]').click();
    await expect(page.locator("[data-digging-suggestion]").first()).toBeVisible({
      timeout: 10000,
    });

    // Adjust BPM range
    const slider = page.locator("[data-bpm-range]");
    await expect(slider).toBeVisible();
  });

  test("audio player plays and pauses", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page.locator("[data-digging-tag-dropdown] >> text=Collapse-capital").click();
    await page.locator('[data-action="find-similar"]').click();

    // Click play on first suggestion
    const playBtn = page
      .locator("[data-digging-suggestion]")
      .first()
      .locator('[data-action="play"]');
    await expect(playBtn).toBeVisible({ timeout: 10000 });
    await playBtn.click();
    // Button should change to pause icon
    await expect(playBtn.locator(".fa-pause")).toBeVisible();
  });

  test("add to staging and save as playlist", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page.locator("[data-digging-tag-dropdown] >> text=Collapse-capital").click();
    await page.locator('[data-action="find-similar"]').click();
    await expect(page.locator("[data-digging-suggestion]").first()).toBeVisible({
      timeout: 10000,
    });

    // Add first suggestion to staging
    await page
      .locator("[data-digging-suggestion]")
      .first()
      .locator('[data-action="add-to-staging"]')
      .click();
    // Staging area should show 1 track
    await expect(page.locator("[data-staging-count]")).toContainText("1");
  });
});
```

**Acceptance**:

- [ ] 7 tests FAIL before Digging.tsx exists
- [ ] 7 tests PASS after Digging page implemented
- [ ] Tag typeahead works with debounced search
- [ ] Suggestions render sorted by match quality
- [ ] Camelot compatibility badges (perfect=green, good=blue, ok=grey)
- [ ] Audio player loads from `/api/files/{id}/stream`, supports Range requests
- [ ] Staging area accumulates tracks, shows key coverage
- [ ] Behavior matches the vanilla `digging.js` page (parity test)

---

### Phase 4: Tags Hub (TDD)

**Test file**: `frontend/tests/tags.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test.describe("Tags Hub Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "tags_hub" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/#/tags");
    await expect(page.locator('[data-page="tags"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  // ── ENERGY CURVE ──────────────────────────────────────────

  test("energy curve section shows 6 default phase tags", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="energy-curve"]');
    await expect(section).toBeVisible();

    // 6 default phase tags: End(0), Release(1), Start(2), Sustain(3), Build(4), Peak(5)
    const tags = section.locator("[data-energy-tag]");
    await expect(tags).toHaveCount(6);
    await expect(tags.nth(0)).toContainText("Peak");
    await expect(tags.nth(5)).toContainText("End");
  });

  test("energy curve shows energy levels 0-5", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="energy-curve"]');
    // Each phase tag has a data-energy attribute
    await expect(section.locator('[data-energy="5"]')).toBeVisible();
    await expect(section.locator('[data-energy="0"]')).toBeVisible();
  });

  test("energy curve can add a new phase tag", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="energy-curve"]');
    await section.locator('[data-action="add-phase-tag"]').click();
    // Typeahead or input appears
    const input = section.locator("[data-new-tag-input]");
    await expect(input).toBeVisible();
    await input.fill("Intro");
    // Energy level selector appears
    await section.locator("[data-energy-select]").selectOption("2");
    await section.locator('[data-action="save-tag"]').click();
    // New tag appears in the curve
    await expect(section.locator("[data-energy-tag]")).toHaveCount(7);
  });

  // ── MOOD / VIBE / GENRE ───────────────────────────────────

  test("mood section shows chip grid with defaults", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="mood"]');
    await expect(section).toBeVisible();
    // Should have multiple mood chips (prefilled defaults)
    const chips = section.locator("[data-tag-chip]");
    await expect(chips.first()).toBeVisible();
    // At least 5 default moods
    expect(await chips.count()).toBeGreaterThanOrEqual(5);
  });

  test("vibe section shows chip grid with defaults", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="vibe"]');
    await expect(section).toBeVisible();
    const chips = section.locator("[data-tag-chip]");
    expect(await chips.count()).toBeGreaterThanOrEqual(5);
  });

  test("genre section shows chip grid with defaults", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="genre"]');
    await expect(section).toBeVisible();
    const chips = section.locator("[data-tag-chip]");
    // Should have the 10 seeded default genres
    expect(await chips.count()).toBeGreaterThanOrEqual(10);
    await expect(chips.first()).toContainText("techno");
  });

  test("mood/vibe/genre sections have add input with typeahead", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="mood"]');
    const addInput = section.locator("[data-add-tag-input]");
    await expect(addInput).toBeVisible();
    await addInput.fill("new");
    // Typeahead should appear for creating a new tag
    await expect(section.locator("[data-create-tag-option]")).toBeVisible();
  });

  // ── MERKMAL ───────────────────────────────────────────────

  test("merkmal section has freeform typeahead", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="merkmal"]');
    await expect(section).toBeVisible();
    const input = section.locator("[data-add-tag-input]");
    await expect(input).toBeVisible();
    // No default chips (merkmal is user-created only)
    const chips = section.locator("[data-tag-chip]");
    expect(await chips.count()).toBe(0);
  });

  // ── SETLIST ───────────────────────────────────────────────

  test("setlist section shows table with parent tags", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="setlist"]');
    await expect(section).toBeVisible();

    // Table with columns: tag name, files, parent tags, backpack
    const table = section.locator("table");
    await expect(table).toBeVisible();
    await expect(table.locator("th")).toHaveCount(4);
  });

  test("setlist section has curate button", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="setlist"]');
    await expect(section.locator('[data-action="curate"]')).toBeVisible();
  });

  test("setlist backpack toggle works", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="setlist"]');
    const backpackBtn = section.locator('[data-action="toggle-backpack"]').first();
    await expect(backpackBtn).toBeVisible();
    await backpackBtn.click();
    // Icon should change from outline to filled
    await expect(backpackBtn.locator(".fa-backpack")).toBeVisible();
  });

  // ── BUNDLES ───────────────────────────────────────────────

  test("bundles section shows static and dynamic bundles", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="bundles"]');
    await expect(section).toBeVisible();
    await expect(section.locator('[data-bundle-type="static"]')).toBeVisible();
    await expect(section.locator('[data-bundle-type="dynamic"]')).toBeVisible();
  });

  test("new bundle button opens creation form", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="bundles"]');
    await section.locator('[data-action="new-bundle"]').click();
    await expect(section.locator("[data-bundle-form]")).toBeVisible();
  });

  // ── COLLAPSIBLE SECTIONS ──────────────────────────────────

  test("sections are collapsible and state persists", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="mood"]');
    const toggle = section.locator("[data-section-toggle]");
    await toggle.click();
    // Section content should be hidden
    await expect(section.locator("[data-tag-chip]").first()).not.toBeVisible();

    // Reload page — collapse state should persist
    await page.reload();
    await expect(
      page.locator('[data-tags-section="mood"] [data-tag-chip]').first(),
    ).not.toBeVisible();
  });
});
```

**Acceptance**:

- [ ] 16 tests FAIL before Tags.tsx exists
- [ ] 16 tests PASS after Tags Hub implemented
- [ ] 7 sections rendered (Energy Curve, Mood, Vibe, Genre, Merkmal, Setlist, Bundles)
- [ ] Energy curve: visual bar chart, editable energy levels 0-5
- [ ] Mood/Vibe/Genre: chip grid + typeahead add input
- [ ] Merkmal: empty chip grid + typeahead add input (no defaults)
- [ ] Setlist: paginated table with search, sort, parent tags column, backpack toggle
- [ ] Bundles: static + dynamic bundle list with create/edit buttons
- [ ] Collapse state persisted in localStorage per section
- [ ] "+ New" button at top creates tag with category picker

---

### Phase 5: Setup Hub (TDD)

**Test file**: `frontend/tests/setup.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test.describe("Setup Hub Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "setup_hub" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/#/setup");
    await expect(page.locator('[data-page="setup"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("shows 8 concern cards", async ({ page }) => {
    await page.goto("/#/setup");
    const cards = page.locator("[data-setup-card]");
    await expect(cards).toHaveCount(8);
  });

  test("services card shows connection status", async ({ page }) => {
    await page.goto("/#/setup");
    const servicesCard = page.locator('[data-setup-card="services"]');
    await expect(servicesCard).toBeVisible();
    await expect(servicesCard.locator('[data-service="spotify"]')).toBeVisible();
    await expect(servicesCard.locator('[data-service="soundcloud"]')).toBeVisible();
    await expect(servicesCard.locator('[data-service="youtube"]')).toBeVisible();
    await expect(servicesCard.locator('[data-service="deemix"]')).toBeVisible();
  });

  test("folders card shows folder list with scan actions", async ({ page }) => {
    await page.goto("/#/setup");
    const foldersCard = page.locator('[data-setup-card="folders"]');
    await expect(
      foldersCard.locator('[data-action="scan-folder"]').first(),
    ).toBeVisible();
    await expect(foldersCard.locator('[data-action="full-scan"]').first()).toBeVisible();
  });

  test("storage card shows backup/prune stats", async ({ page }) => {
    await page.goto("/#/setup");
    const storageCard = page.locator('[data-setup-card="storage"]');
    await expect(storageCard).toBeVisible();
    await expect(storageCard.locator('[data-stat="local-files"]')).toBeVisible();
    await expect(storageCard.locator('[data-stat="backed-up"]')).toBeVisible();
  });

  test("tasks card shows recent tasks", async ({ page }) => {
    await page.goto("/#/setup");
    const tasksCard = page.locator('[data-setup-card="tasks"]');
    await expect(tasksCard).toBeVisible();
  });

  test("import/export card has download and upload", async ({ page }) => {
    await page.goto("/#/setup");
    const dataCard = page.locator('[data-setup-card="data"]');
    await expect(dataCard.locator('[data-action="export"]')).toBeVisible();
    await expect(dataCard.locator('[data-action="import"]')).toBeVisible();
  });

  test("key comparison card exists", async ({ page }) => {
    await page.goto("/#/setup");
    await expect(page.locator('[data-setup-card="key-comparison"]')).toBeVisible();
  });
});
```

**Acceptance**:

- [ ] 8 tests FAIL before Setup.tsx exists
- [ ] 8 tests PASS after Setup Hub implemented
- [ ] Card layout: 2-column or 3-column grid of concern cards
- [ ] Services card: connection status + auth/resync buttons per service
- [ ] Folders card: list with quick scan + full scan buttons
- [ ] Storage card: local/backup/prune stats with action buttons
- [ ] Tasks card: recent task list with status badges
- [ ] Deemix Queue card: download queue status
- [ ] Traktor Import card: file upload + status
- [ ] Import/Export card: download dump + upload restore
- [ ] Key Comparison card: links to comparison tool

---

### Phase 6: Port Remaining Pages (TDD)

For each ported page, the test file from `frontend/tests/*.spec.js` is converted
to TypeScript and updated for React selectors. The test is run FIRST (it fails),
then the React component is implemented.

**Port order** (simplest → most complex):

| Order | Page     | Test file                | Challenges                                     |
| ----- | -------- | ------------------------ | ---------------------------------------------- |
| 1     | Daily    | `tests/daily.spec.ts`    | Simple form + result card, no table            |
| 2     | Backpack | `tests/backpack.spec.ts` | Tag list + track status cards                  |
| 3     | Lists    | `tests/lists.spec.ts`    | Table with filters, push-to-spotify, archive   |
| 4     | Tracks   | `tests/tracks.spec.ts`   | Most complex: table, 20+ filters, bulk actions |

**TDD cycle per page**:

```bash
# Step 1: Convert existing vanilla test to TypeScript, update selectors
# Step 2: Run → FAILS (React page doesn't exist)
cd frontend && npx playwright test -- tests/lists.spec.ts

# Step 3: Implement React page
# Step 4: Run → PASSES
cd frontend && npx playwright test -- tests/lists.spec.ts

# Step 5: Delete the old vanilla .js file
rm frontend/pages/playlists.js

# Step 6: Remove from app.js PAGE_MAP (if last vanilla page)
```

---

### Phase 7: Cleanup

**Test file**: `frontend/tests/cleanup.spec.ts`

```typescript
import { test, expect } from "@playwright/test";

test.describe("Cleanup Verification", () => {
  test("all 7 pages render without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    const pages = [
      "/",
      "/#/digging",
      "/#/daily",
      "/#/backpack",
      "/#/tracks",
      "/#/lists",
      "/#/tags",
      "/#/setup",
    ];

    for (const path of pages) {
      await page.goto(path);
      await page.waitForTimeout(500);
    }
    expect(errors).toEqual([]);
  });

  test("vanilla JS files are deleted", async () => {
    // This is a build-time check — verified by the fact that
    // app.js no longer exists and all routes are React Router
  });

  test("all gates pass", async () => {
    // tsc --noEmit → 0 errors
    // vite build → success
    // playwright test → all pass
  });
});
```

---

### Complete TDD Test Map

| Phase     | Test file           | Test count    | What it proves                                    |
| --------- | ------------------- | ------------- | ------------------------------------------------- |
| 0         | `scaffold.spec.ts`  | 3             | Vite serves, React mounts, TS compiles            |
| 1         | `sidebar.spec.ts`   | 8             | 7 nav items, 3 sections, active state, navigation |
| 2         | `dashboard.spec.ts` | 5             | Stats cards, service status, quick actions        |
| 3         | `digging.spec.ts`   | 7             | Tag search, suggestions, audio, staging           |
| 4         | `tags.spec.ts`      | 16            | 7 sections, energy curve, chips, table, bundles   |
| 5         | `setup.spec.ts`     | 8             | 8 concern cards, service status, actions          |
| 6         | `daily.spec.ts`     | ~5            | Form, generate, result card                       |
| 6         | `backpack.spec.ts`  | ~5            | Tag list, track status, sync                      |
| 6         | `lists.spec.ts`     | ~8            | Table, filters, push, archive                     |
| 6         | `tracks.spec.ts`    | ~12           | Table, 20+ filters, sort, bulk actions            |
| 7         | `cleanup.spec.ts`   | 3             | All pages load, no JS errors                      |
| **Total** | **15 files**        | **~80 tests** | Full coverage of all 7 pages                      |

### Agent TDD Workflow Summary

```bash
# Every agent working on a page follows this exact cycle:

# 1. READ the test file to understand what the page must do
cat frontend/tests/tags.spec.ts

# 2. RUN the test → it FAILS (proves the feature is missing)
cd frontend && npx playwright test -- tests/tags.spec.ts

# 3. IMPLEMENT the React component
#    - Create frontend/src/pages/Tags.tsx
#    - Add to App.tsx router
#    - Wire up React Query hooks to API endpoints

# 4. RUN the test → it PASSES (proves the feature works)
cd frontend && npx playwright test -- tests/tags.spec.ts

# 5. RUN all gates → all green → commit
npx tsc --noEmit && npx vite build && npx playwright test

# 6. DELETE the old vanilla page (if porting)
rm frontend/pages/tags.js frontend/pages/tag-categories.js ...
```

### Acceptance Criteria

**Phase 0 — Scaffold (tests written first):**

- [ ] `scaffold.spec.ts` written and FAILING (no Vite/React yet)
- [ ] `npm create vite@latest` creates the project
- [ ] `npm install` succeeds with react, react-dom, react-router-dom, @tanstack/react-query, typescript, @types/react, @types/react-dom
- [ ] `scaffold.spec.ts` PASSES (3/3)
- [ ] `npx tsc --noEmit` passes
- [ ] `npx vite build` produces a bundle

**Phase 1 — Sidebar Navigation:**

- [ ] `sidebar.spec.ts` written and FAILING
- [ ] `Sidebar.tsx` + `App.tsx` implemented
- [ ] `sidebar.spec.ts` PASSES (8/8)
- [ ] React Router handles hash-based navigation (`/#/digging`)
- [ ] Active nav item has `data-active="true"`

**Phase 2 — Dashboard:**

- [ ] `dashboard.spec.ts` written and FAILING
- [ ] `Dashboard.tsx` implemented with React Query fetching `/api/` stats endpoints
- [ ] `dashboard.spec.ts` PASSES (5/5)

**Phase 3 — Digging:**

- [ ] `digging.spec.ts` written and FAILING
- [ ] `Digging.tsx` implemented with tag typeahead, suggestion cards, audio player
- [ ] `digging.spec.ts` PASSES (7/7)
- [ ] Behavior matches vanilla `digging.js` (manual parity check)

**Phase 4 — Tags Hub:**

- [ ] `tags.spec.ts` written and FAILING
- [ ] `Tags.tsx` implemented with 7 collapsible sections
- [ ] `tags.spec.ts` PASSES (16/16)
- [ ] Energy curve fetches from `/api/tag-energy-levels`
- [ ] Mood/Vibe/Genre sections fetch from `/api/tags?categoryId=X`
- [ ] Setlist table fetches from `/api/tags/curation-queue`
- [ ] Bundles fetch from `/api/tag-bundles` + `/api/dynamic-bundles`

**Phase 5 — Setup Hub:**

- [ ] `setup.spec.ts` written and FAILING
- [ ] `Setup.tsx` implemented with card grid linking to underlying endpoints
- [ ] `setup.spec.ts` PASSES (8/8)

**Phase 6 — Remaining Pages:**

- [ ] Each page: test written → FAILS → implemented → PASSES
- [ ] Old `.js` file deleted after React page passes tests
- [ ] `app.js` PAGE_MAP entries removed as pages are ported

**Phase 7 — Cleanup:**

- [ ] `app.js` deleted (all pages ported)
- [ ] `frontend/pages/` deleted (all vanilla JS gone)
- [ ] `frontend/shared/` deleted (utilities replaced by `src/utils/`)
- [ ] `cleanup.spec.ts` PASSES (3/3)

**Cross-cutting:**

- [ ] Migration 021 runs cleanly (001→021)
- [ ] Comment format `[PMVG]` generated and parsed correctly
- [ ] All existing `cargo test` passes
- [ ] All ~80 Playwright tests pass
- [ ] `npx tsc --noEmit` passes on every commit
- [ ] No regressions: all API endpoints, all filter combinations

---

## Proposed Plans (Backlog)

These plans are proposed but deferred. They live in git history — restore from there when ready.

| Plan                         | Branch                              | Dependencies                  |
| ---------------------------- | ----------------------------------- | ----------------------------- |
| tracks-filter-overhaul       | `feat/tracks-filter-overhaul`       | —                             |
| multi-provider-playlists     | `feat/multi-provider-playlists`     | —                             |
| soundcloud-integration       | `feat/soundcloud-integration`       | —                             |
| storage-holistic-cleanup     | `fix/storage-holistic-cleanup`      | file-lifecycle-management     |
| fix-comment-diff-display     | `fix/comment-diff-display`          | —                             |
| wav-source-linking           | `feat/wav-source-linking`           | file-lifecycle-management     |
| backup-as-truth              | `feat/backup-as-truth`              | wav-source-linking            |
| test-coverage-100            | `feat/test-coverage-100`            | integration-test-harness      |
| harness-completeness-audit   | `feat/harness-completeness`         | fix/scan-folder-task-tracking |
| fix-files-pmv-filter         | `fix/files-pmv-filter`              | fix/scan-folder-task-tracking |
| configurable-format-priority | `feat/configurable-format-priority` | fix-backpack-local-tracking   |
| relax-prune-safety-gates     | `feat/fix-backpack-local-tracking`  | auto-prune                    |
| staging-area-pull            | `feat/staging-area-pull`            | relax-prune-safety-gates      |
