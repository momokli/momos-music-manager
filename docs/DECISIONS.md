# Momo's Music Manager — Architectural Decision Records

## ADR-014: Modular SPA with ES Modules (frontend_next)

**Date**: 2025-04-27  
**Status**: Accepted (implemented)

**Context**: The original `frontend/` folder contained monolithic HTML files with duplicated `API_BASE`, raw fetch logic, and inline CSS/Tailwind classes. The `frontend_rethought/mock.html` consolidated the design into a single 2300-line mock but lacked modularity. We needed a production-ready SPA that:

- Eliminates code duplication across pages
- Uses a consistent design system with CSS variables
- Supports lazy loading (each page loads on demand)
- Has proper error/loading/empty states on every page
- Can be served without a build step

**Decision**: Build `frontend_next/` as a vanilla JS SPA with:

1. **Hash-based routing** in `app.js` — listens for `hashchange`, dynamically imports page modules
2. **ES modules** — no bundler, native `import`/`export` via `<script type="module">`
3. **CSS variables + utility classes** — consistent theme tokens (e.g. `--accent`, `--surface`, `--border`) with helper classes (`.btn`, `.badge`, `.card`, `.data-table`, `.pagination`)
4. **Shared modules** in `shared/` — `api.js` (fetch with error handling), `components.js` (Loading/Empty/Error/Pagination), `format.js` (date/BPM/duration), `nav.js` (sidebar renderer)
5. **11 page modules** in `pages/` — each exports `init(container, signal)`, follows the same pattern: render loading → fetch data (or use mock) → render table/content → attach events
6. **Event delegation** — one listener per container, not per element
7. **Mock data first** — each page ships with inline mock data that can be swapped for real `fetchJSON("/api/...")` calls

**Consequences**:

- Zero build tooling — works with any static file server
- Pages load on demand (dynamic `import()`)
- Consistent error/loading/empty states across all pages

---

## ADR-015: Digging Curator — Chain-based Session Builder

**Date**: 2025-06-05  
**Status**: Accepted (implemented)

**Context**: DJ workflow requires preparing "digging sessions" — playlists of rarely-heard or untagged tracks to rediscover. Existing `files` page shows all tracks but doesn't help with harmonic selection or session scoping. Need a tool that:

- Lets DJs browse unplayed/low-playcount tracks as "seeds"
- Builds a harmonic chain (Camelot wheel compatible) of 10–30 tracks
- Works for two use cases: **Digging** (untagged/unplayed tracks) and **Setlist Curation** (PMV-tagged tracks arranged dramatically)
- Saves chains as tags that get written into file comments for Traktor filtering
- Supports PMV tag energy levels (Phase → energy level mapping)

**Decision**: Build as a new module with three layers:

### 1. Backend: `src/digging.rs`

- **Camelot Wheel Engine**: Parses keys like `8A`, `12B`, checks compatibility with configurable jumps (`+1`, `-1`, `+2`, `-2`, `+7`, `-7`, `A↔B`, `±0`)
- **Suggestion Engine**: Takes a seed track + jump config + BPM range → scores candidates by play count ASC, last played ASC, BPM distance, key match → returns top 20
- **Tag Energy Levels**: New `tag_energy_levels` table (tag_id → energy_level 0–5) for Phase tags (Start=0, Build=1, Peak=2, Release=3, Sustain=4, End=5)

### 2. API: New Routes

| Endpoint                          | Method     | Purpose                                           |
| --------------------------------- | ---------- | ------------------------------------------------- |
| `/api/digging/seeds`              | GET        | 20 tracks for seed browsing (sortable/filterable) |
| `/api/digging/suggestions`        | POST       | 20 compatible tracks given a seed                 |
| `/api/digging/save-chain`         | POST       | Save chain as tag + write comments                |
| `/api/tag-energy-levels`          | GET        | List phase tags with energy levels                |
| `/api/tag-energy-levels/{tag_id}` | PUT/DELETE | Set/remove energy level                           |

### 3. Frontend: `pages/digging.js`

- **Seed Selection Panel** (Browse/Search tabs): Sort by play_count, last_played, BPM, random; filter by BPM range, play count max
- **Chain Panel**: Tracks in order with connection lines showing Camelot jump type; remove per track; save with tag name
- **Suggestions Panel**: Togglable Camelot jump chips, BPM range slider, scored track cards with compatibility badges (perfect/good/ok)

**Key Design Decisions**:

- **Seed-first workflow**: Always pick a single seed track, never a "batch" — keeps focus on harmonic mixing
- **Camelot chips as filters**: Each jump type is a toggle, not a fixed rule — DJ decides the harmonic flexibility
- **Energy levels separate from tags**: New table, not a field on tags — cleaner separation, future-proof
- **Save as tag, not playlist**: Tags get written into file comments → directly filterable in Traktor
- **Same UI for Digging & Setlist**: Mode determined by seed source (Browse shows unplayed tracks vs. selecting a specific PMV-tagged track)

**Consequences**:

- New DB table requires migration reset (delete `app.db` and re-run)
- Adds ~500 lines backend, ~1000 lines frontend
- Suggestion engine is deterministic (algorithmic scoring) — AI-powered suggestions can be added later
- Chain building is manual (add one track at a time) — auto-chain generation could be v2
- Easy to swap mock data for real API calls later
- New pages can be added by creating a file in `pages/` and adding one line to `app.js`
- CSS is ~1079 lines but no Tailwind dependency
- ~2300 lines total across 17 files (comparable to the single mock.html)

## ADR-001: Rust Backend with SQLite

**Date**: Initial  
**Status**: Accepted (implemented)

**Context**: We needed a backend for a desktop music management application that handles file system operations, metadata processing, and HTTP API serving. The application should be performant, reliable, and easy to deploy.

**Decision**: Use Rust with the Axum web framework and SQLite as the embedded database. Axum provides a modern async HTTP layer, SQLx gives compile-time SQL verification.

**Consequences**:

- Single binary deployment with embedded database
- Memory safety and thread safety guarantees
- Excellent performance for audio metadata processing
- SQLx compile-time query verification catches SQL errors early
- SQLite concurrency is sufficient for single-user desktop use

---

## ADR-003: Simplified Camelot Wheel Key Compatibility

**Date**: Initial  
**Status**: Accepted (implemented)

**Context**: We needed a harmonic matching algorithm for DJs that balances musical accuracy with computational efficiency. The Camelot wheel is industry standard.

**Decision**: Implement simplified key compatibility — only the number matters (±1, wrapping 12↔1). The A/B (major/minor) distinction is ignored.

**Consequences**:

- Fast computation suitable for real-time filtering
- Intuitive for users (simpler rules)
- Good enough for most mixing scenarios
- Loses nuance of relative major/minor compatibility

---

## ADR-004: Priority-Based Similarity Algorithm

**Date**: Initial  
**Status**: Accepted (implemented)

**Context**: We needed to combine multiple similarity dimensions (key, BPM, tags) into a single score for ranking track recommendations.

**Decision**: Use weighted scoring with priority order: Key (50%) > BPM (30%) > Tags (20%). Incompatible keys receive a zero score regardless of other matches.

**Consequences**:

- Musical compatibility prioritized appropriately
- Tunable weights allow user customization
- Clear failure modes (key incompatibility)
- Hard cutoff for key incompatibility may be too strict for some use cases

---

## ADR-006: SQLite without External Cache

**Date**: Initial  
**Status**: Accepted (implemented)

**Context**: As a single-user desktop application, we considered whether to add a cache layer (Redis, in-memory) for performance.

**Decision**: Optimize SQLite queries with strategic indexes instead of adding an external cache layer.

**Consequences**:

- Zero additional dependencies
- Simpler deployment and maintenance
- SQLite's built-in cache is sufficient for single-user workloads
- May need optimization for very large libraries (>100k tracks)

---

## ADR-011: Tag Categories with Icons

**Date**: Phase 1  
**Status**: Accepted (implemented)

**Context**: We needed to organize tags meaningfully for DJ workflows — Setlist, Phase, Mood, Vibe, Merkmal.

**Decision**: Implement a category system with configurable icons, prefixes, and sort order. Setlist is the default category. Categories are stored in the `tag_categories` table.

**Consequences**:

- Better tag organization for users
- PMV comment format derived from Phase/Mood/Vibe categories
- Flexible category system can be extended
- Database stores category metadata (icon, prefix, sort_order)

---

## ADR-013: Local-First Architecture

**Date**: Initial  
**Status**: Accepted (implemented)

**Context**: The application manages personal music libraries and sensitive OAuth credentials.

**Decision**: Design as a local-first application with no required cloud dependencies.

**Consequences**:

- User data privacy by default
- Works offline
- No reliance on external services
- Sync across devices requires manual setup
- Backup responsibility falls to the user

---

## ADR-014: Single-User Focus

**Date**: Initial  
**Status**: Accepted (implemented)

**Context**: The application could support multiple users or remain focused on individual DJ workflows.

**Decision**: Design for single-user use with potential for multi-user extensions later.

**Consequences**:

- Simpler security and data model
- No authentication/login system needed
- Focused on core DJ workflow
- Multi-user would require significant rework later

---

## ADR-015: Structured Comment Format with PMV Indicators

**Date**: 2026-04-17  
**Status**: Accepted (implemented)

**Context**: We needed a consistent way to store metadata in file comments that combines DJ workflow concepts (Phase, Mood, Vibe) with service identifiers and tags.

**Decision**: Implement a standardized comment format: `[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}`

Where:

- Phase/Mood/Vibe chars are 'P', 'M', 'V' or '\_' if missing
- Tags are space-separated keywords (sorted by category priority)
- Source IDs use service prefixes: `sp:xxx`, `sc:xxx`, `yt:xxx`

**Consequences**:

- Single field stores multiple dimensions of metadata
- Easy to parse programmatically
- PMV indicators provide visual categorization
- Supports multiple service IDs in target comments
- Fixed format limits flexibility for future metadata types
- Implemented in `src/comment.rs` with full parser and generator

---

## ADR-016: Separate Service Tracks API with Playlist-Based Tag Matching

**Date**: 2026-04-19  
**Status**: Accepted (implemented)

**Context**: Service tracks (Spotify, SoundCloud, YouTube) have no BPM/Key metadata and are managed via playlist associations rather than file system paths. They needed to be handled separately from local files.

**Decision**: Implement separate API endpoints for `File` vs `ServiceTrack` entities:

- **Files** (`/api/files`): Local files with BPM/Key, direct service IDs
- **Tracks** (`/api/tracks`): Service entries without BPM/Key
- Tag association via playlist name matching (case-insensitive)
- No junction tables — associations are computed at query time

**Consequences**:

- Clean separation between local files (with BPM/Key) and service tracks (without)
- Playlist-based tag matching aligns with DJ workflow (playlists = tags)
- No junction tables needed — simpler schema
- Tags are the single source of truth for categorization
- Tag association chain: File → ServiceTrack → Playlist → Tag (via name match)

---

## ADR-017: POC Phase — Fresh Database Strategy

**Date**: 2026-04-19  
**Status**: Accepted (project policy)

**Context**: This project is in Proof of Concept (POC) phase with no users, no production data, and no backward compatibility requirements.

**Decision**: Adopt a "fresh start always" approach:

- **Single migration file**: Only `migrations/001_initial_schema.sql` — replace it entirely when the schema changes
- **Delete all DB files** on schema changes (`app.db`, `test.db`, etc.)
- **No migration history** — treat the one file as the source of truth
- **No backward compatibility** — throw away old data without hesitation

**Consequences**:

- Eliminates migration complexity during rapid POC development
- Forces clean schema design without legacy baggage
- Simplifies testing — always start with a fresh database
- **Will need proper migration system before production deployment**
- Each agent must delete old DB files before testing

---

## ADR-018: Folder CRUD API with Manual Scan

**Date**: 2026-04-19  
**Status**: Accepted (implemented, modified)

**Context**: We needed a way to manage monitored folders for file scanning.

**Decision**: Implement a complete CRUD API for folders with a polling-based watcher and a manual scan trigger endpoint.

**Consequences**:

- Full CRUD lifecycle for monitored folders via REST API
- Manual scan trigger (`POST /api/folders/{id}/scan`) spawns a background async job
- Polling watcher exists but is not auto-started — scans are manual
- Path validation with shell expansion
- File count tracking per folder using SQL queries

---

## ADR-019: Folder Scanning Configuration

**Date**: 2026-04-20  
**Status**: Accepted (implemented)

**Context**: We needed configurable folder scanning — for example, scanning only top-level `.stem.m4a` files in the stems folder.

**Decision**: Add configuration columns to the `folders` table:

- `scan_recursive` (default false) — top-level only
- `fixed_extensions` (default false) — wildcard = all audio
- `file_extensions` — comma-separated extension enum values
- `max_depth` — recursion depth

**Consequences**:

- Fine-grained control per folder
- `AudioExtension` enum with case-insensitive matching
- Compound extension support (`.stem.m4a` matched as `StemM4a`, not `M4a`)
- Validation on folder creation — invalid extensions are rejected

---

## ADR-022: Target Comment Computation

**Date**: 2026-04-23  
**Status**: Accepted (implemented)

**Context**: Files have comment metadata that may become stale when service tracks are added to new playlists. Users need visibility into when comments are outdated.

**Decision**: Compute a "target comment" for each file by traversing the tag association chain (File → Track → Playlist → Tag). Extend the `ApiFile` response with three new fields:

- `comment_current` — current file comment
- `comment_target` — computed target comment
- `comment_needs_update` — boolean difference indicator

**Consequences**:

- Immediate visibility into stale file comments
- No schema changes — pure query-time computation
- Batch query optimization for list endpoints
- Frontend shows visual diff (green checkmark or strikethrough → target)

---

## ADR-023: Generic TaskManager replaces SyncManager

**Date**: 2026-04-24  
**Status**: Accepted (implemented)

**Context**: The old SyncManager was Spotify-specific and couldn't handle other background operations like writing comments to files.

**Decision**: Create a generic `TaskManager` in `src/tasks/` that supports multiple task types:

- `SpotifySync` (migrated from old sync module)
- `WriteComment` (new)

**Consequences**:

- All tasks share the same lifecycle (Pending → Running → Completed/Failed/Cancelled)
- Tasks are stored in memory only
- Task IDs are UUIDs
- Cancellation via `CancellationToken`
- API: `GET /api/tasks`, `GET /api/tasks/{id}`, `DELETE /api/tasks/{id}`

---

## ADR-024: WriteComment as Background Task

**Date**: 2026-04-24  
**Status**: Accepted (implemented)

**Context**: Writing comments to files via exiftool can take seconds per file. A synchronous HTTP request would block the UI.

**Consequences**:

- `POST /api/files/{id}/write-comment` — returns task_id
- `POST /api/files/write-comments` — batch write all files needing update
- Frontend shows spinner while task is running
- File-level granularity — errors in one file don't stop the batch
- Edge cases handled: already-up-to-date skip, missing file error, DB-update-after-write warning

---

## ADR-025: Semantic Tag Categorization with Embeddings

**Date**: 2026-06-xx  
**Status**: Accepted (implemented)

**Context**: Over 400 music tags exist in the database, all assigned to the default "Setlist" category. Manually categorizing each tag into Phase/Mood/Vibe/Merkmal is tedious. The system needed AI-assisted suggestions based on semantic similarity.

**Decision**: Implement local Sentence Embeddings using `candle-core` + `all-MiniLM-L6-v2` (384-dim) for on-device, privacy-preserving tag categorization:

- **Model**: `sentence-transformers/all-MiniLM-L6-v2` via HuggingFace safetensors
- **Inference**: Pure Rust with `candle-core`, `candle-transformers`, `tokenizers` crate
- **Lazy loading**: Model is downloaded from HuggingFace on first API call, cached in `~/.cache/huggingface/hub/`
- **Embedding storage**: New `tag_embeddings` table (tag_id → f32 BLOB, 1536 bytes per embedding)
- **Category embedding**: Mean of all tag embeddings in that category, computed on-the-fly
- **Suggestion**: Cosine similarity between tag embedding and each category's mean embedding
- **Setlist excluded**: AI never suggests the default "Setlist" category — user must choose it manually

**Wizard workflow**:

- New page `auto-categorize.html` processes tags one-by-one
- Queue management: unreviewed tags sorted by name, skip rotates within local JS queue
- `reviewed_at` column on `tags` table tracks which tags have been processed (NULL = unreviewed)
- AI button at fixed position with confidence score + 5 manual category buttons
- Explicit "Setlist" choice vs. default distinction via `reviewed_at` timestamp

**New API endpoints**:

- `GET /api/tags/unreviewed` — returns sorted queue of unreviewed tags
- `PUT /api/tags/{id}/categorize` — sets `category_id` + `reviewed_at`
- `GET /api/tags/{id}/suggest` — AI recommendation via cosine similarity
- `GET /api/embeddings/status` — model load state + embedding count
- `POST /api/embeddings/recompute` — rebuild all embeddings from scratch
- `POST /api/embeddings/reset-review` — reset all `reviewed_at` to NULL

**Consequences**:

- All ML inference runs locally on CPU — no data leaves the machine
- Embeddings are computed once and cached in SQLite (BLOB)
- Category means update incrementally as tags are categorized
- AI suggestions improve over time as more tags are categorized (more data points per category)
- First API call triggers ~90MB model download from HuggingFace (~32s for 400 tags)
- Requires `candle-core`, `candle-transformers`, `candle-nn`, `hf-hub`, `tokenizers` dependencies
- Schema change: `tags.reviewed_at` column + `tag_embeddings` table added
- Binary size increases by ~15MB (candle + safetensors)
- POC phase — model version pinned to `all-MiniLM-L6-v2`, upgradeable via recompute endpoint

---

## ADR-026: Unified Task System with Progress Tracking

**Date**: 2026-06-xx
**Status**: Accepted (implemented)

**Context**: The TaskManager had a dual progress system (`progress_text` for generic tasks vs `SyncProgress` for Spotify sync), duplicate task-type specific "already running" checks scattered across workers, and some operations (folder scanning) bypassed the task system entirely with raw `tokio::spawn`.

**Decision**: Refactor the task system with unified abstractions:

- **`TaskType` enum** now has four variants:
  - `ServiceSync { service, operation }` — replaces old `SpotifySync(SyncConfig)`, works for any service
  - `WriteComment { file_ids }` — unchanged
  - `RecomputeEmbeddings` — unchanged
  - `ScanFolder { folder_id }` — new, for background folder scanning

- **`SyncOperation`** replaces `SyncType` as the canonical operation enum. `pub type SyncType = SyncOperation;` provides backward compatibility.

- **Unified `Progress` struct** with `status`, `percent: Option<f32>`, `message`, and `sub_items: Vec<ProgressItem>` — every task type populates this. The old `SyncProgress` is kept for backward compat with `SpotifySyncWorker` and converted on serialization.

- **Conflict key system** via `task_type_conflict_key()` — `TaskManager::start_task_unique()` checks if a task of the same type (same service, same folder, etc.) is already running and rejects duplicates.

- **Auto-pruning** — `prune_old_tasks(duration)` removes terminal tasks older than the given age, called every 60 seconds in `serve()` with a 5-minute retention.

- **Old `sync/mod.rs` deleted** — `SyncProgress`, `SyncResult`, `SyncType` moved into `tasks/mod.rs`. `SpotifySyncWorker` imports from `crate::tasks` directly.

- **`ServiceConnection` cleaned** — removed duplicate sync progress fields (`sync_current_playlist`, `sync_current_track`, `sync_total_playlists`, `sync_total_tracks`, `sync_log`) that were always `None`.

**Consequences**:

- All background operations share the same lifecycle and API (`GET /api/tasks`, `DELETE /api/tasks/{id}`)
- Folder scans now have progress tracking, cancellation, and error visibility
- Duplicate task prevention is handled at the `TaskManager` level, not ad-hoc per worker
- No more unbounded memory growth — old tasks are pruned after 5 minutes
- `ServiceConnection` is leaner — sync progress comes from `GET /api/tasks`

---

## ADR-027: SPA Frontend Wired to Backend API

**Date**: 2026-06-xx
**Status**: Accepted (implemented)

**Context**: The `frontend_next/` SPA was initially built with inline mock data for all 11 pages, making it impossible to interact with the actual backend. Each page used `setTimeout` + `MOCK_DATA` to simulate async loading, with plans to swap to real API calls later.

**Decision**: Replace all mock data in the 11 page modules with real `fetchJSON()` calls to the backend API, following these patterns:

- **API layer**: `frontend_next/shared/api.js` provides `fetchJSON(url, options)` which prepends `API_BASE`, sets JSON headers, and throws descriptive errors on non-2xx responses
- **Imports**: Each page imports `fetchJSON` from `../shared/api.js`
- **Init functions**: All 11 page modules now export `async function init(container, signal)` — the router already supports this via `await import()`
- **Data adapters**: Each page has an adapter function (e.g. `adaptFolder`, `adaptTask`, `adaptTag`) that maps API response shapes (camelCase, with all fields present) to the shape expected by the existing render functions
- **Error handling**: All pages wrap API calls in `try/catch`, check `signal.aborted` between async steps, and render error states via `renderErrorBlock()`
- **Parallel fetching**: Dashboard uses `Promise.all` to fetch 6 endpoints simultaneously; playlist page fetches `/api/playlists` and `/api/tags` in parallel for tag matching

**Data flows per page**:

| Page            | Endpoints                                                                                                             | Transform                                                           |
| --------------- | --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| Dashboard       | `/api/files/count`, `/api/tracks/count`, `/api/playlists?limit=1`, `/api/tags`, `/api/services`, `/api/tasks?limit=5` | Aggregate into stats grid, service cards, task rows                 |
| Files           | `/api/files?limit=&offset=&search=&bpmMin=&bpmMax=&key=&tags=`, `/api/files/count`                                    | Compute comment diff from `comment` vs `commentTarget`              |
| Tracks          | `/api/tracks?limit=&offset=&service=&search=`, `/api/tracks/count`                                                    | Convert `durationMs` (ms→s), join `localFiles` array                |
| Playlists       | `/api/playlists?limit=&offset=&search=&service=`, `/api/tags`                                                         | Match playlist names to tag names (case-insensitive) for tag column |
| Tags            | `/api/tags`                                                                                                           | File count not available from API (shows 0)                         |
| Tag Categories  | `/api/tag-categories`                                                                                                 | Tag count per category not available (shows 0)                      |
| Services        | `/api/services`                                                                                                       | Map `configured`/`connected` to status enum                         |
| Folders         | `/api/folders`                                                                                                        | Map `fileCount` → `files`, `watchEnabled` → `watch`, etc.           |
| Tasks           | `/api/tasks?limit=&offset=&status=`                                                                                   | Extract progress % from string, map `taskType` to label             |
| Auto-Categorize | `/api/tags/unreviewed`, `/api/tags/{id}/suggest` (GET), `/api/tags/{id}/categorize` (PUT)                             | Wizard flow with next/previous tag progression                      |
| Bulk Import     | `/api/tags/bulk-import` (POST), `/api/tags/bulk-resolve` (POST), `/api/tag-categories`                                | Two-phase: check → resolve with conflict handling                   |

**Consequences**:

- All 11 pages now load real data from the backend instead of mocks
- Pages gracefully degrade to error states when the backend is unavailable
- AbortController signal is propagated to all fetch calls, preventing stale renders on rapid navigation
- Some API data gaps remain (e.g. file count per tag, sync timestamps per playlist) — will be addressed in future backend enhancements
- Category names/IDs in bulk-import and auto-categorize are fetched from `/api/tag-categories` at runtime (not hardcoded)
- Pagination and filtering (search, service filter) now trigger API re-fetches with appropriate query parameters

## ADR-028: Playlist Subscriptions with Background Polling

**Date**: 2026-04-28  
**Status**: Accepted (implemented)

**Context**: Users want to monitor specific playlists for new tracks without running a full sync of all playlists. A background poller should check subscribed playlists every 5 minutes, detect new tracks, and automatically associate them with the correct playlists.

**Decision**:

1. **New table `playlist_subscriptions`** (schema table #11): stores `service`, `playlist_id`, `service_playlist_id` (FK to `service_playlists`), `last_polled_at`, `poll_interval_secs` (default 300), and `is_active`.

2. **DB functions**: 9 new functions in `db.rs` covering CRUD, due-subscription queries, last-polled tracking, and track playlist association discovery.

3. **Background poller** (`src/poller.rs`): a tokio task that runs every 30 seconds, queries for due subscriptions, and polls each via the Spotify API. For new tracks found, it:
   - Upserts the track into `service_tracks`
   - Links it to the playlist via `service_playlist_tracks`
   - Queries `get_track_playlist_associations` to discover which _other_ playlists already contain the track (no extra API calls needed)
   - Logs the new track and its other playlist associations

4. **API endpoints**:
   - `GET /api/playlists/subscriptions` — list all subscriptions with playlist info
   - `POST /api/playlists/subscriptions` — subscribe to a playlist (body: `{ service, playlistId }`)
   - `DELETE /api/playlists/subscriptions/{id}` — unsubscribe

5. **Frontend**: The Playlists page now has:
   - A "Sub" column with a green bell icon for subscribed, gray for unsubscribed
   - Subscribe/Unsubscribe buttons in each row's action column
   - Subscriptions loaded alongside playlists and tags for status display

**Key insight**: When a new track is found in a subscribed playlist, we can immediately tell the user which other playlists (already in our DB) contain the same track by querying `service_playlist_tracks` locally. This requires zero additional API calls.

---

## ADR-029: Spotify API Response Cache for Development

**Date**: 2026-05-09  
**Status**: Accepted (implemented)

**Context**: Full Spotify syncs (420 playlists × ~100 tracks each) take 10–20 minutes of real API calls. During development this is a massive time sink, especially when iterating on sync logic, DB schema, or frontend features that depend on synced data.

**Decision**: Add an optional recording/replay layer that caches Spotify API responses to disk using our own serializable types (`PlaylistInfo`/`TrackInfo`), controlled by env vars:

1. **`SPOTIFY_API_CACHE` env var** with three modes:
   - `off` (default): live API calls, normal operation
   - `record`: makes real API calls + saves responses as JSON to `dev-data/spotify-api/`
   - `replay`: loads responses from cache files, zero network I/O

2. **Cache granularity**: per-operation (playlists, tracks per playlist), stored as `dev-data/spotify-api/playlists.json` and `dev-data/spotify-api/playlist_tracks/{id}.json`

3. **Uses our own types**: `PlaylistInfo` and `TrackInfo` (owned, `Serialize`/`Deserialize`) instead of rspotify's lifetime-parameterised types — avoids deserialization issues and keeps the cache format stable across rspotify version bumps.

4. **Stream buffering**: rspotify returns paginated streams for playlists/tracks. In record mode the stream is fully buffered into a `Vec` before saving. In replay mode the `Vec` is loaded from disk and converted back into a stream.

5. **Sync worker integration**: `SpotifySyncWorker` detects the cache mode at construction time and branches each method (`sync_playlists_only`, `sync_tracks_for_playlist`, `sync_all_tracks`) between live/record/replay paths. Record mode buffers data as a side-effect of the normal store path. Replay mode reads cache files and calls the same `store_playlist_core`/`store_track_core` methods.

6. **Clear cache**: `rm -rf dev-data/spotify-api` — cache dir is entirely disposable.

**Consequences**:

- Sync goes from 10–20 minutes to 2–5 seconds in replay mode
- Cache data is pure JSON — inspectable, diffable, commitable
- No changes to `SpotifyClient` or rspotify configuration needed
- New module: `src/spotify/replay.rs` (cache types + I/O)
- Cache is a development-only feature — no production impact
- Cache must be re-recorded when schema changes affect how data is stored (e.g. new fields in `PlaylistInfo`/`TrackInfo`)

---

## ADR-030: Scan Cache for Development

**Date**: 2026-05-09  
**Status**: Accepted (implemented)

**Context**: Folder scans walk the entire music library and call `lofty` (tag parsing) + `exiftool` (playback stats) on every single audio file. For a large collection this takes 30–45 minutes. During development this is even worse than the Spotify sync because it happens every time the backend restarts and triggers a folder scan.

**Decision**: Add an optional recording/replay layer that caches extracted file metadata to disk, controlled by the `SCAN_CACHE` env var:

1. **`SCAN_CACHE` env var** with three modes:
   - `off` (default): live extraction via lofty + exiftool
   - `record`: full extraction + saves results to `dev-data/scan-cache/entries/{HASHED_PATH}.json`
   - `replay`: loads cached metadata, zero lofty/exiftool calls. Falls back to live extraction for files not yet cached.

2. **Per-file granularity**: each file's metadata is cached independently, keyed by a hash of its absolute path. Cache entries include the file's `last_modified` and `file_hash` — if either changes, the entry is invalidated and the file is re-extracted automatically on the next scan.

3. **Injection point**: `extract_audio_metadata_from_file` in `db.rs` — the single function that all scan paths use. Before doing any extraction work, it checks the cache (replay mode). After successful extraction, it saves to the cache (record mode). No changes needed to callers like `scan_directory_with_config` or the folder scan task.

4. **New module**: `src/scan_cache.rs` — `ScanCacheMode` enum, `CacheResult` enum, `try_load`/`try_save`/`invalidate`/`clear_cache` functions.

5. **Clear cache**: `rm -rf dev-data/scan-cache` — forces re-extraction of all files on the next scan.

**Consequences**:

- Scan goes from 30–45 minutes to 10–30 seconds on replay (pure DB upsert speed)
- Cache auto-invalidates when files are modified (new mixes, BPM re-analyses, tag edits)
- No changes to existing scan logic or DB functions (cache is injected at the extraction layer)
- Cache is a development-only feature — no production impact
- Cache must be cleared when the `File` struct gains new fields that aren't populated from old cached data

---

## Revision History

| Date       | Decision                              | Description                                                                                        |
| ---------- | ------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Initial    | ADR-001, 003, 004, 006, 011, 013, 014 | Core architecture decisions                                                                        |
| 2026-04-17 | ADR-015                               | Structured comment format                                                                          |
| 2026-04-19 | ADR-016, 017, 018                     | Service tracks API, POC strategy, Folder CRUD                                                      |
| 2026-04-20 | ADR-019                               | Folder scanning configuration                                                                      |
| 2026-04-23 | ADR-022                               | Target comment computation                                                                         |
| 2026-04-24 | ADR-023, 024                          | TaskManager, WriteComment                                                                          |
| 2026-04-25 | —                                     | Cleanup: removed outdated ADRs (React, Docker, design.html, presets, bugfixes)                     |
| 2026-06-xx | ADR-025                               | Semantic tag categorization with local embeddings (candle + all-MiniLM-L6-v2)                      |
| 2026-06-xx | ADR-026                               | Unified task system with progress tracking, ScanFolder, conflict keys, pruning                     |
| 2026-06-xx | ADR-027                               | All 11 frontend pages wired to backend API with data adapters, error handling, and AbortController |
| 2026-04-28 | ADR-028                               | Playlist subscriptions with background polling (poll_interval_secs, auto-track-discovery)          |
