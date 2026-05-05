# Momo's Music Manager — Agent Guidance & Implementation Plan

> **Last Updated**: 2026-05-01 — Comprehensive plan based on full code review

---

## Project Context

Music library management for DJs. Rust backend (Axum/SQLx/SQLite) + modular SPA frontend (vanilla JS, ES modules).
Single developer, no production data, no backward compatibility needed.

---

## Key Principles

1. **Schema**: 12 tables/views — `tag_categories`, `tags`, `service_tracks`, `service_playlists`, `service_playlist_tracks`, `files`, `service_config`, `folders`, `subscriptions`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities` (plus views: `unified_tracks`, `v_file_track_link`, `v_tag_playlist`, `v_file_tags`, `v_subscriptions`, `v_tag_categories`, `v_tags_with_categories`)
2. **Single Migration**: Only `migrations/001_initial_schema.sql` — replace it and delete all DB files if schema changes
3. **Separate Types**: `File` (local files with BPM/Key) vs `ServiceTrack` (service entries, no BPM/Key) — linked via `v_file_track_link` view
4. **Tags = Playlists**: Via name matching (case-insensitive). Setlist is default category.
5. **Comment Format**: `[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}` — e.g. `[PMV] build jazzy warehouse sp:xxx`
6. **Service IDs**: Direct columns on `files` (`spotify_id`, `soundcloud_id`, `youtube_id`)
7. **Key Matching**: Rust-only (Camelot wheel, no DB table)
8. **Task Manager**: In-memory task tracking — 4 operation types (ServiceSync, WriteComment, RecomputeEmbeddings, ScanFolder)
9. **Sync State**: In-memory `TaskManager` — tasks auto-pruned 5 min after completion
10. **Config Priority** (highest wins): Env vars > `~/.config/momos-music-manager/config.toml` > built-in defaults

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
- `SPOTIFY_API_CACHE` — `record`/`replay` for dev
- `SCAN_CACHE` — `record`/`replay` for dev

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

# Delete old DBs + restart
rm -f app.db && cargo run -- serve --host 127.0.0.1 --port 3000

# Record Spotify API responses for later replay
SPOTIFY_API_CACHE=record cargo run -- serve

# Replay cached responses (no API calls, seconds instead of minutes)
SPOTIFY_API_CACHE=replay cargo run -- serve

# Clear cached API responses
rm -rf dev-data/spotify-api

# Record folder scan metadata for later replay
SCAN_CACHE=record cargo run -- serve

# Replay cached folder scan (no lofty/exiftool calls, seconds instead of minutes)
SCAN_CACHE=replay cargo run -- serve

# Clear cached scan metadata (forces re-extraction next scan)
rm -rf dev-data/scan-cache

# Dump DB to JSON (save state before deleting app.db)
cargo run -- dump

# Restore DB from JSON dump
cargo run -- restore

# Import Traktor collection.nml
cargo run -- serve  # then use the Traktor import page in the frontend
```

---

## Important Gotchas

- **Before testing**: Always delete old DB files (`app.db`, `compile_check.db`, `test.db`)
- **If you see "migration 27" errors**: DELETE ALL DB files and start fresh
- **No SoundCloud/YouTube OAuth yet** — framework is ready, actual flow not implemented
- **Frontend is an SPA** — modular vanilla JS with ES modules in `frontend/`. Hash-based router (`app.js`), shared modules in `shared/`, pages in `pages/`. Serve embedded via `rust-embed`, no separate dev server needed.
- **Docker** was removed — will be recreated later. Use `cargo run` for now.
- **digging.html** is a standalone HTML page (not part of the SPA) for the digging/curation workflow
- **Playlist subscriptions** poll every 30s in the background — managed in `poller.rs`

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
├── scan_cache.rs        # File scan caching (record/replay)
├── spotify/
│   ├── mod.rs
│   ├── client.rs        # Spotify OAuth client
│   ├── models.rs        # PlaylistInfo, TrackInfo
│   ├── replay.rs        # API response cache (record/replay)
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

# 🎯 FRONTEND ROLLOUT PLAN

> `frontend/prototype.html` is the validated reference implementation.
> All list/table pages follow the same canonical CRUD pattern:
> stable toolbar + re-rendered body (stats, table, pagination) + sort + page size + hash sync + column config.

---

## Canonical Pattern (from prototype)

Each CRUD page module follows this contract:

```js
// Exported entry point (called by app.js)
export async function init(container, signal, hashParams) { ... }
```

**Internal structure:**

| Function                                   | Renders | Responsibility                             |
| ------------------------------------------ | ------- | ------------------------------------------ |
| `renderToolbar(state)`                     | Once    | Search + filters + view-specific controls  |
| `renderBody(data, state)`                  | On data | Stats + table + pagination + page-size     |
| `buildParams(state)`                       | —       | Serialises state → API query params        |
| `fetchAndRender(container, signal, state)` | —       | Fetches data, calls renderBody             |
| `updateHash(state)`                        | —       | Silently syncs to `window.location.hash`   |
| `init(container, signal, hashParams)`      | —       | Parses hash, renders toolbar, wires events |

**State shape (common fields):**

```js
const state = {
  page: 0, pageSize: from localStorage,
  search: "", sort: "", order: "asc",
  // view-specific filters...
};
```

---

## Phase 0: Baseline — Ensure Clean Foundation

Before rolling out any page, make sure the system works end-to-end:

- [ ] **Backend:** Sort + pagination on all list endpoints (`sort`, `order`, `limit`, `offset`, `pageSize`)
- [ ] **Backend:** `apply_sort` helper with whitelist validation in `api.rs`
- [ ] **Frontend:** All shared modules present and working (`crud.js`, `column-config.js`, `search-filter.js`, `components.js`, `api.js`)
- [ ] **Frontend:** Confirm CSS for sortable columns, page-size selector, column config is in `style.css`
- [ ] **Smoke test:** `#files` page works with sort, page size, hash sync

---

## Phase 1: Files Page (Reference Blueprint)

**File:** `frontend/pages/files.js`

This is the richest page — get it right and all others follow the same pattern.

- [ ] **Stable toolbar** — Search input + filter panel (BPM, key grid, tags) rendered once, preserved across re-renders. Comment-writer sidebar stays stable.
- [ ] **Body** — Stats row (count + refresh + page-size selector), sortable table with column config, pagination
- [ ] **Hash sync** — `updateHash()` on every filter change, `parseHash()` on init
- [ ] **Column config** — `loadColumnConfig("files", FILES_COLUMNS)`, column visibility/reorder/resize via `column-config.js`
- [ ] **Cell renderers** — Each column has a renderer in `FILES_CELL_RENDERERS`. Comment column shows diff (old strikethrough, new green).
- [ ] **PMV filter** — Category + aggregate split (P/M/V multi-select | Full/Partial/None single), either/or logic, reads comment bracket
- [ ] **Service filter** — Multi-select icons, class-based selector for both views
- [ ] **Actions panel** — Refresh Metadata + Write Comments buttons to the right of filter panel (CSS grid `4fr 1fr`)

---

## Phase 2: Tracks Page

**File:** `frontend/pages/tracks.js`

Already has stable toolbar + playlist context badge. Needs retrofit:

- [ ] **Sortable headers** — Use `sortableTh`, `wireSortableHeaders` from `crud.js`
- [ ] **Page size selector** — Use `renderPageSizeSelector`, `wirePageSizeSelector`
- [ ] **Hash sync** — Use `updateHash` instead of reading-only
- [ ] **Column config** — `loadColumnConfig("tracks", TRACKS_COLUMNS)`
- [ ] **Playlists column** — Show tag chips with category colors using `playlistTags` from API
- [ ] **Comment column** — Same diff rendering as files, using linked file comments
- [ ] **Playlist count column** — Numeric badge showing how many playlists a track belongs to
- [ ] **PMV + Service filters** — Same pattern as files page

---

## Phase 3: Playlists Page

**File:** `frontend/pages/playlists.js`

Full retrofit following files/tracks pattern:

- [ ] **Stable toolbar** — Search input + service filter dropdown + Create Tags button
- [ ] **Column model** — Name, Service, Tracks (count + local mismatch), Tags, Deemix, Sync, Imported, Updated, Subscribe, View, Actions
- [ ] **Sort** — All sortable columns (name, service, track_count, imported_at, updated_at)
- [ ] **Hash sync + page size** — Standard pattern
- [ ] **Service filter** — Dropdown or icon buttons (like files/tracks)
- [ ] **Deemix column** — Status badges (queued/downloading/completed/failed) + add/retry buttons
- [ ] **Subscription column** — Bell icon toggle (green = subscribed)
- [ ] **View Tracks link** — `#tracks?playlistId=...` with playlist context
- [ ] **Create/Edit tag** — Buttons open inline or modal

---

## Phase 4: Tags Page

**File:** `frontend/pages/tags.js`

Currently client-side filtered. Needs server-side pagination:

- [ ] **Backend:** Add `sort`, `order`, `search`, `category`, `limit`, `offset` to `GET /api/tags` + `GET /api/tags/count`
- [ ] **Column model** — Tag Name (sortable), Category (badge with icon, sortable), Files (count from `v_tag_file_counts`), Created (sortable), Actions
- [ ] **Stable toolbar** — Search + category filter
- [ ] **Standard pattern** — Hash sync, page size, column config

---

## Phase 5: Tag Categories Page

**File:** `frontend/pages/tag-categories.js`

Special UI (drag-and-drop reorder, energy levels). Less table-like but still needs:

- [ ] Consistent tooling — reuse `fetchJSON`, `escapeHtml`, `showToast` from shared modules
- [ ] Energy level editor — inline or modal for Phase tags

---

## Phase 6: Validate & Polish

- [ ] All pages consistent: same toolbar/body pattern, same hash sync, same page-size storage
- [ ] No duplicated toast/escapeHtml/fetch code across pages
- [ ] All column config storage keys per-page (`columnConfig_files`, `columnConfig_tracks`, etc.)
- [ ] Responsive: filter panel collapses, tables horizontal-scroll
- [ ] Keyboard: Search input preserves focus, Escape clears filters
- [ ] Empty states: Zero data / no results / loading states all handled

---

## Phase 7 (Optional): Remaining Pages

Apply same pattern to folders, tasks, deemix-queue, services pages following the blueprint.

---

## Implementation Order

| Phase | Page           | Effort | Dependencies      |
| ----- | -------------- | ------ | ----------------- |
| 0     | Baseline       | Medium | None              |
| 1     | Files          | Large  | Phase 0 ✓         |
| 2     | Tracks         | Medium | Phase 1 (pattern) |
| 3     | Playlists      | Medium | Phase 1 (pattern) |
| 4     | Tags           | Medium | Phase 1 + backend |
| 5     | Tag Categories | Small  | None              |
| 6     | Validate       | Small  | All above         |

Phases 2–4 can run in parallel after Phase 1 completes (disjoint write scopes).

### 0.1 — Tracks search ✅ (resolved)
