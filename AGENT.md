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

### Acceptance Criteria

**Phase 1 — Scaffold + First Page:**

- [ ] `cd frontend && npm install` succeeds
- [ ] `npx tsc --noEmit` passes
- [ ] `npx vite build` produces a bundle
- [ ] At least one page (Digging) renders identically to the vanilla version
- [ ] Playwright tests pass for the ported page(s)
- [ ] Old and new stacks coexist (some pages vanilla, some React)

**Phase 2 — Navigation + Core Pages:**

- [ ] Sidebar nav renders with 7 items (Dig, Daily, Pack, Tracks, Lists, Tags, Setup)
- [ ] React Router handles hash-based navigation
- [ ] Dashboard ported to React
- [ ] Tracks page (unified or separate) ported to React
- [ ] Lists page ported to React

**Phase 3 — Tags Hub:**

- [ ] Tags page renders with 7 collapsible sections
- [ ] Energy Curve section: 6 default phase tags with energy levels
- [ ] Mood/Vibe/Genre sections: chip grids with typeahead
- [ ] Merkmal section: freeform typeahead
- [ ] Setlist section: table with parent tags, backpack toggle, curate button
- [ ] Bundles section: static + dynamic bundle management

**Phase 4 — Setup Hub + Cleanup:**

- [ ] Setup page renders with cards for all 8 setup concerns
- [ ] All vanilla JS files deleted
- [ ] `app.js` deleted
- [ ] Playwright tests ported to new selectors

**Cross-cutting:**

- [ ] Migration 021 runs cleanly (001→021)
- [ ] Comment format `[PMVG]` generated and parsed correctly
- [ ] All existing `cargo test` passes
- [ ] All Playwright tests pass
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
