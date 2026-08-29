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

## ADR-031: Config.toml Migration (Config Priority)

**Date**: 2026-04-30  
**Status**: Accepted (implemented)

**Context**: Initially, all service credentials (Spotify client ID/secret, SoundCloud API key, YouTube API key) lived exclusively in a `.env` file loaded via `dotenvy`. This worked for a single developer but had several drawbacks:

1. The `.env` file sits in the project root — easy to accidentally commit (it's in `.gitignore` but still risky)
2. No standard config directory that CLI tools conventionally use (`~/.config/`)
3. Every env var must be re-exported when switching between projects or shell sessions
4. No obvious place for future configuration options (scan intervals, UI preferences, etc.)

**Decision**: Add a `config.toml` file at `~/.config/momos-music-manager/config.toml` as the primary configuration source, with environment variables as overrides.

### Priority order (highest wins)

1. Environment variables (`.env` file or shell exports like `SPOTIFY_CLIENT_ID=...`)
2. `~/.config/momos-music-manager/config.toml`
3. Built-in defaults (e.g. `redirect_uri = "http://localhost:3000/callback"`)

### Config.toml format

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

### Implementation details

1. **New dependency**: `toml` and `dirs` crates added to `Cargo.toml`
2. **`src/config.rs` rewritten**: Added `TomlConfig` structs (deserialize-only), `ServiceCredentials::load()` method that reads TOML then applies env overrides
3. **`ServiceCredentials::from_env()` preserved** for backward compatibility (tests, CI)
4. **`main.rs` updated**: `ServiceCredentials::load()` called instead of `from_env()`
5. **Dev-only env vars stay as env vars only**: `DATABASE_URL`, `SPOTIFY_API_CACHE`, `SCAN_CACHE` — these are session-specific and don't belong in a persistent config file
6. **Backward compatible**: An existing `.env` file in the project root still works — it overrides the TOML values via environment variables

**Consequences**:

- Users set up their credentials once in `~/.config/momos-music-manager/config.toml` and forget about them
- Quick dev switching via `.env` or inline env vars still works
- Future config options (scan intervals, UI themes, default folders) can be added to the same TOML file
- Using `dirs::config_dir()` follows XDG conventions on Linux (`~/.config/`), macOS (`~/Library/Application Support/`), and Windows
- No changes to any other modules — `ServiceCredentials` is passed around the same way as before

---

## ADR-032: Speculative Prefetch in Auto-Categorize Wizard

**Date**: 2025-07-18
**Status**: Accepted (implemented)

**Context**: The auto-categorize wizard (page `#auto-categorize`) loads unreviewed tags one at a time. For each tag it calls `GET /api/tags/{id}/suggest`, which involves loading the embedding model (lazy), deserializing the tag's embedding from the DB, computing mean category embeddings, and calculating cosine similarity — a sequence of several DB queries + ML inference taking hundreds of milliseconds. The user then picks a category and the wizard advances to the next tag, blocking on another suggest API call. This sequential, wait-then-act-then-wait loop felt sluggish.

**Decision**: Implement a "speculative prefetch" pattern with two parallel background forks:

1. **Fork 1 — Optimistic pre-categorize**: After rendering a tag and its AI recommendation, immediately fire a `PUT /api/tags/{id}/categorize` with the recommended category. This is fire-and-forget — if it fails, the user's explicit PUT on interaction is the authoritative one. This saves ~1 DB round-trip when the user accepts the default.

2. **Fork 2 — Next-tag suggestion prefetch**: After rendering a tag, immediately start `GET /api/tags/{nextId}/suggest` for the next tag in the queue. The response is stored in a `prefetchCache` map keyed by tag ID. When the user advances (via any category, Enter, Space, or Skip), `loadNextTag` checks the cache first and renders instantly if the prefetch data is available.

**Consequences**:

- **Positive**: The expensive suggest API call is hidden in the background. On average, the user's think time (~2-5s per tag) is longer than the request, so the next tag is almost always pre-cached. Transitions feel instant.
- **Positive**: The optimistic PUT is idempotent and cheap — the worst case is one extra DB write if the user overrides the recommendation.
- **Positive**: The prefetch cleanup is automatic — `AbortController.signal` aborts all in-flight requests on page navigation, and the `prefetchCache` is garbage-collected with the module state.
- **Neutral**: If the user navigates away, the prefetch and optimistic PUT are aborted/ignored. No stale state remains.

---

### Implementation Details

- The `state` object in `auto-categorize.js` gained: `prefetchCache`, `prefetchInFlight`, `optimisticTagId`, `optimisticCatId`, `aiRecommendation`
- `startSpeculativePrefetch()` is called at the end of `loadNextTag()` and handles both forks
- `selectCategory()` always sends the authoritative PUT (the optimistic one is purely a bonus for the Enter case)
- `loadNextTag()` checks `state.prefetchCache[currentTag.id]` before falling through to the normal `fetchJSON` call
- No backend changes — all optimization is frontend-only

---

## ADR-033: Single-Binary Shipping with Embedded Frontend

**Date**: 2026-04-30
**Status**: Accepted (implemented)

**Context**: The project originally required two separate processes during development:

1. The Rust backend on port 3000 (API + bare `index.html` via `include_str!`)
2. A Python HTTP server on port 8000 (serving JS modules, CSS, images)

Additionally, the frontend depended on Font Awesome from a CDN, making the app unusable offline. The embedding model (all-MiniLM-L6-v2) was downloaded at runtime via `hf-hub`, but that was acceptable for a single-user desktop tool.

For shipping a Release Candidate, the friction of the two-process setup was the biggest barrier to "it just works."

**Decision**: Bundle everything into a single binary using `rust-embed`:

1. **All frontend assets embedded** — `rust-embed` with `#[folder = "frontend/"]` compiles the entire `frontend/` directory into the binary at build time
2. **Catch-all static handler** — Axum serves exact file paths from the embedded assets, with a SPA fallback that returns `index.html` for any unrecognised route (client-side hash router handles the rest)
3. **Font Awesome self-hosted** — Downloaded Font Awesome Free 6.4.0 during build (`css/all.min.css` + `webfonts/*.woff2`), placed in `frontend/fontawesome/`, served locally
4. **Relative API_BASE** — Changed `frontend/shared/api.js` from hardcoded `http://localhost:3000` to `window.location.origin`, so the frontend talks to whichever origin served it

**Not bundled** (left as runtime dependencies):

- HuggingFace embedding model (all-MiniLM-L6-v2, ~90MB) — downloaded on first use via `hf-hub`; acceptable for single-user desktop use
- External API credentials (Spotify, SoundCloud, YouTube) — loaded from `~/.config/momos-music-manager/config.toml` or env vars

**Consequences**:

- **Positive**: Single binary, single command (`cargo run -- serve`), single URL (`http://localhost:3000`). No Python, no Node, no separate dev server.
- **Positive**: Fully offline SPA — icons, styles, and webfonts are served from the binary with no CDN dependency.
- **Positive**: Font Awesome webfonts get the same `Cache-Control` benefits as the rest of the embedded assets (ETags via Axum).
- **Neutral**: Binary size increased by ~2.5MB (Font Awesome CSS + webfonts). The Rust binary is still ~30MB stripped.
- **Neutral**: Frontend changes require a recompile (`cargo build`) to be reflected — but this is fine for a compiled SPA workflow, and `cargo watch` can be used for dev iteration.
- **Negative** (accepted): The `rust-embed` macro recompiles on any frontend file change, which adds ~5s to incremental builds. Mitigation: frontend changes are infrequent compared to backend changes.

---

---

## ADR-034: Database Path in Config.toml

**Date**: 2026-05-01
**Status**: Accepted (implemented)

**Context**: The database path was hardcoded to `sqlite:app.db` (relative to CWD). For deployment as a macOS service, the database needs a stable, standard location.

**Decision**: Add `[database]` section to config.toml with `url` field. Priority: `DATABASE_URL` env var > `[database].url` in config.toml > `sqlite:~/.local/share/momos-music-manager/library.db`. The default path uses `shellexpand` for `~` resolution, and the parent directory is auto-created on startup.

**Consequences**: Migrating from an existing `app.db` requires manually moving it to the new location or setting `DATABASE_URL`.

---

## ADR-035: macOS Launch Agent Deployment

**Date**: 2026-05-01
**Status**: Accepted (implemented)

**Context**: The server needed a way to auto-start on login for daily use.

**Decision**: Add CLI subcommands `install-launch-agent`, `uninstall-launch-agent`, and `service-status`. The agent uses launchd with `RunAtLoad` and `KeepAlive`. Conditionally compiled for macOS only via `#[cfg(target_os = "macos")]`.

**Consequences**: Users get seamless auto-start. Non-macOS users get a clear error message. Logs go to `~/Library/Logs/momos-music-manager/`.

---

## ADR-036: Dynamic Redirect URI for OAuth Callbacks

**Date**: 2026-05-01
**Status**: Accepted (implemented)

**Context**: The OAuth callback handlers hardcoded redirect URLs (`http://localhost:3000` and `http://localhost:8000`), which broke when deploying behind a reverse proxy.

**Decision**: Add `--public-url` CLI flag to `serve` command. Store it in `AppState`. Both callback handlers redirect to `state.public_url` or fall back to `http://{server_host}:{server_port}`. Priority: CLI flag > `PUBLIC_URL` env var / `[server].public_url` in config.toml > `None`.

**Consequences**: OAuth works both in dev (localhost) and production (behind reverse proxy). Users must update their Spotify app's redirect URI to match.

---

## ADR-037: Clean Startup Logging

**Date**: 2026-05-01
**Status**: Accepted (implemented)

**Context**: The startup log had 4 separate `info!` lines that could be consolidated for readability. The poller log didn't indicate whether subscriptions existed.

**Decision**: Consolidate startup logging into: database URL, migrations complete, subscription poller status (showing subscription count or "idle"), listening address, and a final startup banner. The poller now receives and logs the subscription count. The bound address is read from `listener.local_addr()` for accuracy.

**Consequences**: Cleaner, more informative startup output. Easy to spot when the server is ready.

---

## ADR-038: Deemix Download Service Integration

**Date**: 2026-05-01
**Status**: Accepted (implemented)

**Context**: Users need a way to download Spotify playlists as local audio files. Deemix-pyweb provides a web API that can download tracks from Deezer using Spotify playlist URLs. We needed to integrate it as a streaming service alongside Spotify, SoundCloud, and YouTube.

**Decision**: Add deemix as a fourth streaming service with web-UI-only configuration:

1. **No config.toml changes** — ARL (authentication cookie) + host URL are stored entirely in the `service_config` DB table, configured via the web UI
2. **Cookie-based auth** — The `connect.sid` session cookie lives only in the reqwest client's in-memory cookie jar (not persisted to DB). Auto-re-auth on HTTP 401 by loading the stored ARL and calling `/api/loginArl`
3. **New module `src/deemix/`** with:
   - `client.rs` — HTTP client for deemix-pyweb API (`POST /api/loginArl`, `GET /api/getQueue`, `POST /api/addToQueue`, `POST /api/retryDownload`)
   - `models.rs` — Response types for all deemix API endpoints + frontend-facing `DeemixCombinedQueueItem`
4. **New DB table `deemix_downloads`** — tracks download queue status per Spotify playlist URL
5. **API endpoints**: `POST /api/services/deemix/auth`, `GET/POST /api/services/deemix/queue`, `POST /api/services/deemix/queue/{id}/retry`, `DELETE /api/services/deemix/queue/{id}`
6. **Frontend integration**:
   - Services page: ARL + Host input fields, Test & Save button, status badges
   - Playlists page: Deemix column showing queue status (➕ add / ⏳ downloading / ✅ completed / 🔄 retry)
   - Tracks page: playlist context badge + playlist column + URL hash param support (`#tracks?playlistId=X&playlistName=...`)
7. **Backend playlist_id filter**: `TracksQuery.playlist_id` for scoped track listings + `GET /api/playlists/{id}` endpoint

**Consequences**:

- Users configure deemix once via the web UI and don't need to edit config files
- Default host port is 6595 (deemix-pyweb default)
- ARL cookie auto-refreshes on expiry without user intervention
- Playlist download status is visible directly in the playlists view
- Tracks page supports playlist-scoped viewing with URL-based state
- Schema was extended without breaking changes (migration delete + recreate)

| Date       | Decision                              | Description                                                                                         |
| ---------- | ------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Initial    | ADR-001, 003, 004, 006, 011, 013, 014 | Core architecture decisions                                                                         |
| 2026-04-17 | ADR-015                               | Structured comment format                                                                           |
| 2026-04-19 | ADR-016, 017, 018                     | Service tracks API, POC strategy, Folder CRUD                                                       |
| 2026-04-20 | ADR-019                               | Folder scanning configuration                                                                       |
| 2026-04-23 | ADR-022                               | Target comment computation                                                                          |
| 2026-04-24 | ADR-023, 024                          | TaskManager, WriteComment                                                                           |
| 2026-04-25 | —                                     | Cleanup: removed outdated ADRs (React, Docker, design.html, presets, bugfixes)                      |
| 2026-06-xx | ADR-025                               | Semantic tag categorization with local embeddings (candle + all-MiniLM-L6-v2)                       |
| 2026-06-xx | ADR-026                               | Unified task system with progress tracking, ScanFolder, conflict keys, pruning                      |
| 2026-06-xx | ADR-027                               | All 11 frontend pages wired to backend API with data adapters, error handling, and AbortController  |
| 2026-06-xx | ADR-028                               | Playlist subscriptions with background polling (poll_interval_secs, auto-track-discovery)           |
| 2026-04-29 | ADR-029                               | Spotify API response cache for development (record/replay, dev-data/spotify-api)                    |
| 2026-05-09 | ADR-030                               | Scan cache for development (record/replay, dev-data/scan-cache, auto-invalidation)                  |
| 2026-04-30 | ADR-031                               | Config.toml migration — config priority (env > config.toml > defaults)                              |
| 2026-04-30 | ADR-033                               | Single-binary shipping with embedded frontend + Font Awesome bundle                                 |
| 2026-05-01 | ADR-034                               | Database path in config.toml — `[database].url` with shellexpand + env var fallback                 |
| 2026-05-01 | ADR-035                               | macOS Launch Agent — `install-launch-agent`, `uninstall-launch-agent`, `service-status`             |
| 2026-05-01 | ADR-036                               | Dynamic redirect URI for OAuth — `--public-url` flag, fallback chain                                |
| 2026-05-01 | ADR-037                               | Clean startup logging — consolidated banner, poller subscription count, actual bound port           |
| 2026-05-01 | ADR-038                               | Deemix download service integration — web-UI-only config, cookie-auth, playlist download queue      |
| 2026-06-10 | ADR-039                               | Column resize switched to pixel-based sizing (30–500px) with `columnConfig_v2_` key                 |
| 2026-06-10 | ADR-040                               | Server-side filtering for Tracks and Files pages — all filters in SQL, pagination-aware             |
| 2026-06-10 | ADR-041                               | Import/Export web UI — `GET /api/dump`, `POST /api/restore`, `#data` page with preview              |
| 2026-06-10 | ADR-042                               | Bulk write comments — multi-select checkboxes + "WRITE COMMENTS (X)" on Tracks and Files pages      |
| 2026-06-10 | ADR-043                               | Spotify rate-limit retry — parse `Retry-After` header from 429, retry with backoff (max 3 attempts) |
| 2026-06-10 | ADR-044                               | Tag parent resolution — Setlist tags resolve to parent tags via `tag_parents` table + views         |
| 2026-06-10 | ADR-045                               | Tag curation workflow page — sequential prev/next + typeahead + "Create & Add" inline flow          |

---

## ADR-039: Pixel-Based Column Resize

**Date**: 2026-06-10
**Status**: Accepted (implemented)

**Context**: Column resize on CRUD pages used percentage-based widths (e.g. `width: 18%`). When dragging a resize handle, the percentage recalculated on each mouse-move event, causing a feedback loop where columns shrank/grew uncontrollably.

**Decision**: Switch to pixel-based sizing: each column has an explicit pixel width (30–500px range). New `columnConfig_v2_` localStorage key avoids stale percentage data from v1. Default widths are scaled from old percent-based values (e.g. 18% → 180px).

**Consequences**: Smooth, predictable column resizing. Old config is silently ignored — users see default widths on first v0.2.0 load.

---

## ADR-040: Server-Side Filtering

**Date**: 2026-06-10
**Status**: Accepted (implemented)

**Context**: Client-side filtering on Tracks and Files pages (JavaScript `Array.filter()` after API fetch) broke pagination — the total count from the server didn't match the filtered set. Page numbers were wrong, and some pages had fewer items than expected.

**Decision**: Move all filters to server-side SQL. Extended `TracksQuery` with `services`, `fileTypes`, `fileTypeAgg` params. Extended `FilesQuery` with PMV, file type, and comment-status filters. Client-side `applyClientFilters` blocks removed. All pagination counts now come from filtered queries.

**Consequences**: Correct pagination with filters active. Slightly more complex SQL queries with dynamic WHERE clauses, but more accurate results. Removed dead PMV filter from Tracks page (service tracks have no comment data).

---

## ADR-041: Import/Export Web UI

**Date**: 2026-06-10
**Status**: Accepted (implemented)

**Context**: Database dump/restore was only available via CLI (`cargo run -- dump` / `cargo run -- restore`). Users wanted a web UI for backup and migration.

**Decision**: Add `GET /api/dump` (returns JSON with `Content-Disposition` header for browser download) and `POST /api/restore?confirm=true` (multipart JSON upload, replaces entire DB). New `#data` page with Export section (download button with spinner) and Import section (file picker → preview with row counts per table + timestamp → confirm → restore). Destructive restore button styled red with warning banner.

**Consequences**: Easy backup and migration without CLI access. Body limit increased to 100MB for large dumps. Safety gate: restore requires `?confirm=true` query param.

---

## ADR-042: Bulk Write Comments

**Date**: 2026-06-10
**Status**: Accepted (implemented)

**Context**: Writing comments to files was a per-file operation (click pencil icon on each row). For large libraries, this was tedious. Users wanted to select multiple tracks/files and write comments in bulk.

**Decision**: Add multi-select checkboxes (select-all header + per-row) to Tracks and Files pages. Selection state uses `Set` for efficient membership checks, persists across page navigation. An ACTIONS panel shows "WRITE COMMENTS (X)" where X = count of selected items needing comment updates (computed server-side via `POST /api/{entity}/needs-comment-count`). Clicking the button queues write-comment tasks via `POST /api/{entity}/write-comments[-by-ids]`.

**Consequences**: Efficient bulk operations. Shared `actions-panel.js` module for reusability. Tracks → files resolution goes through `v_file_track_link`; files are direct. Selection cleared after successful write. Toast notifications for success/error/up-to-date.

---

## ADR-043: Spotify Rate-Limit Retry

**Date**: 2026-05-22
**Status**: Accepted (implemented, expanded in 0.3.2)

**Context**: Spotify's API returns HTTP 429 with a `Retry-After` header when rate limits are exceeded. The sync worker was firing all playlist syncs in a tight loop with no delay or retry, causing failures during large sync batches. Additionally, the subscription and global pollers had no retry logic — every 429 was an instant failure, creating a feedback loop of burst requests hitting the rate limit persistently.

**Decision**: Two phases:

_Phase 1 (v0.3.0)_: Parse the `Retry-After` header from 429 responses by walking the error chain (`rspotify::ClientError::Http` → `ReqwestError::StatusCode(response)`). Wrap API calls in a retry loop (max 3 attempts per playlist) with the `Retry-After` duration + 1s sleep. Add 300ms `tokio::sleep` between successful playlist syncs. Non-429 errors fail immediately.

_Phase 2 (v0.3.2)_: Extract `extract_retry_after_secs` and `client_error_retry_after_secs` into a shared `src/spotify/retry.rs` module used by all three consumers (sync worker, subscription poller, global poller). Fix the global poller which had a broken string-parsing implementation that read the HTTP status code `429` as the retry duration. Add retry loops to the subscription poller (for both `get_playlist` and `get_playlist_tracks`) and the global poller (for `get_user_playlists`). Reuse the Spotify client across all subscriptions within a poll cycle (eliminating unnecessary token refresh calls). Add 300ms inter-subscription delay to prevent burst traffic.

**Consequences**: All Spotify API consumers now use the same reliable retry logic. Persistent 429 feedback loops are broken by proper backoff and client reuse. `warn!` logging on each retry with the actual `Retry-After` duration. Max 3 retries prevents infinite loops on persistent rate limits.

---

## ADR-044: Tag Parent Resolution

**Date**: 2026-06-10
**Status**: Accepted (implemented)

**Context**: Setlist-category tags are long playlist names (e.g. `Dark Techno/2026/Hardtechno/Some Event`). File comments using these tags were unwieldy. Users wanted shorter, meaningful tags with proper PMV categorization to appear in comments instead.

**Decision**: Allow Setlist tags to have "parent" tags that replace them in file comments. New `tag_parents` table (UNIQUE on tag_id + parent_tag_id) with validation: only Setlist tags can have parents, no self-references, parents must exist. Two new views: `v_resolved_tags` (returns parents if they exist, otherwise the tag itself) and `v_file_resolved_tags` (like `v_file_tags` but through `v_resolved_tags`). `compute_target_comment()` queries `v_file_resolved_tags` instead of `v_file_tags`. Migration: `002_playlist_fetch_tracking.sql` (merged).

**Consequences**: Clean, categorized comments. A Setlist tag with parents `dark` (Mood), `techno` (Vibe), `hard` (Merkmal) produces `[PMV] dark techno hard` instead of `[S--] Dark Techno/2026/...`. Tags without parents work as before (backward compatible). Parent editing available in Tags page Edit modal and Tag Curation page.

---

## ADR-045: Tag Curation Workflow Page

**Date**: 2026-06-10
**Status**: Accepted (implemented)

**Context**: Assigning parent tags to Setlist tags one-by-one via the Tags page Edit modal was tedious. Users needed a dedicated workflow to go through the entire Setlist tag queue efficiently.

**Decision**: New `#tag-curation` page with a sequential workflow (prev/next with keyboard shortcuts ←/→ or p/n, progress bar), a tag card showing the full name + metadata, a parent chip editor with typeahead search, and an inline "Create & Add" flow (pick category → create new tag → immediately add as parent). A collapsible "Browse All" mini table supports search, has_parents filter, sort (name/length/files/parents), and click-to-jump. Auto-save: every add/remove immediately PUTs parents to the API; navigation waits for in-flight saves. Backend endpoint: `GET /api/tags/curation-queue` with filtering/sorting params.

**Consequences**: Efficient batch curation. No manual save button needed — changes persist immediately. Keyboard shortcuts speed up navigation. The "Create & Add" flow eliminates the need to pre-create parent tags.

---

## ADR-046: Incremental Folder Scan

**Date**: 2026-05-20
**Status**: Accepted (implemented)

**Context**: Full folder rescans reprocessed every file on every scan, which was slow for large libraries. Users needed a faster option that only picked up new or changed files.

**Decision**: New `ScanMode` enum with `Full` and `Incremental { since: Option<i64> }` variants. Incremental mode checks file mtimes against the folder's `last_scanned` timestamp — files older than the cutoff are skipped. On first scan (no `last_scanned`), falls back to full scan. FolderWatcher auto-starts in `serve()` with a 5-minute polling interval using incremental mode. Frontend: two scan buttons — Quick Scan (⚡ incremental) and Full Rescan (🔄). API: `POST /api/folders/{id}/scan?mode=full|incremental`.

**Consequences**: Subsequent scans are dramatically faster — only new/changed files are processed. Zero configuration needed for users. The FolderWatcher runs automatically on server start. First scan is always full.

---

## ADR-047: Tracks Playlist Filter

**Date**: 2026-05-20
**Status**: Accepted (implemented)

**Context**: Users wanted to view all tracks belonging to a specific playlist (or multiple playlists) directly from the Tracks page. Previously, the only way was to navigate to a playlist and view its tracks there.

**Decision**: Add a playlist typeahead filter to the Tracks page toolbar (LEFT column, between Tags and Date), following the same pattern as the existing Tags typeahead. The user types a playlist name, gets suggestions from `/api/playlists?search=...`, clicks to add chips, and the track list filters server-side. Multiple playlists are OR'd together (tracks in ANY selected playlist). Backend: new `playlists` param on `TracksQuery` → `SELECT DISTINCT st.* JOIN service_playlist_tracks JOIN service_playlists WHERE LOWER(sp.name) IN (LOWER(?),...)`. Case-insensitive matching. When multi-playlist filter is active, the single-playlist context badge is hidden.

**Consequences**: Fast, intuitive playlist-based track filtering. Reuses the familiar typeahead + chips UI pattern. Server-side filtering ensures correct pagination. Works with all other filters (tags, PMV, type, date, service) simultaneously.

---

## ADR-048: New Playlists Sync & Remote Count Tracking

**Date**: 2026-05-20
**Status**: Accepted (implemented)

**Context**: Syncing all playlists was slow when only a few new playlists needed to be discovered. Additionally, remote track counts were not updated during playlist-list sync or polling, causing stale stats on the Playlists page.

**Decision**: New `SyncType::NewPlaylists` — fetches the full playlist list from Spotify, diffs against existing DB playlist IDs, and only syncs metadata + tracks for playlists that don't yet exist. "Sync New" button on the Playlists page. Remote track counts are now updated in two places: during playlist-list sync (from `SimplifiedPlaylist.tracks.total`) via `update_playlist_remote_count()`, and during subscription polling (after streaming all tracks) via `update_playlist_fetch_tracking()`. A new "Stale" filter on the Playlists page shows playlists where `localTrackCount ≠ remoteTrackCount`.

**Consequences**: Discovering new playlists is much faster — only net-new playlists get full track syncs. Playlist stats stay accurate without a full rescan. The Stale filter helps users quickly find playlists that need attention.

---

## ADR-049: Digging Multi-Seed Suggestion Engine

**Date**: 2026-05-22
**Status**: Accepted (implemented)

**Context**: ADR-015 described a single-seed digging workflow, but users needed to seed suggestions from an entire tag (multiple tracks) to explore a curated musical space. A single seed track is too narrow — suggestions vary wildly between seeds from the same tag.

**Decision**: New `POST /api/digging/suggest` endpoint accepting either `seedFileIds` or `seedTag` to resolve multiple seeds. Algorithm: resolve seeds → detect BPM outliers (deviation >20 from median) → compute BPM range [min(bpm)-range, max(bpm)+range] from non-outliers → query eligible files within BPM range → filter by Camelot compatibility against any seed (OR logic) → score each candidate (play_count, recency, BPM diff, Camelot bonus, shared tag bonus) → deduplicate by ISRC (prefer stem.m4a over flac) → sort by score ascending. Response includes seed metadata, BPM range, suggestions with `score_breakdown`, and `candidatesConsidered`. New `GET /api/files/{id}/stream` endpoint for browser-native audio playback with Range header support (HTTP 206 partial content).

**Consequences**: Multi-seed suggestions produce much more relevant results than single-seed. Outlier detection prevents far-off-BPM tracks from polluting the pool. ISRC dedup ensures unique suggestions. Scoring transparency helps users understand why a suggestion was made.

---

## ADR-050: Digging Frontend (Audio Player + Staging Area)

**Date**: 2026-05-22
**Status**: Accepted (implemented)

**Context**: The digging workflow needed a visual interface for seed selection, suggestion browsing, and session building. Users needed to audition tracks and accumulate candidates before committing.

**Decision**: New `#digging` SPA page with a split-panel layout: LEFT panel (40%) for tag-based seed selection + track cards + config (BPM range slider, Camelot jump toggles), RIGHT panel (60%) for scored suggestions with embedded `<audio>` players using Web Audio API waveform visualization. Staging area accumulates tracks across multiple "Find Similar" rounds — clicking "Add" moves a suggestion to staging, and a "Refine" button re-seeds from the original seeds + all staged tracks, creating an expanding seed pool. Key coverage indicator shows which Camelot keys are covered in the staging area. "Save as Playlist" persists staging as a local playlist via `POST /api/playlists/local`. Audio player: single active stream, waveform rendered client-side from PCM peaks, seekable progress bar.

**Consequences**: Iterative exploration — each refinement round expands the seed pool and narrows suggestions toward the target musical space. The staging area avoids premature commitment. Key coverage helps DJs visualize harmonic gaps.

---

## ADR-051: Local Playlists (Digging Persistence)

**Date**: 2026-05-22
**Status**: Accepted (implemented)

**Context**: Saving digging sessions required creating new playlist entities that could store any service track (Spotify, local, YouTube). Existing service_playlists required a valid service type. The user needed a lightweight way to persist discovered tracks without creating Spotify playlists.

**Decision**: Add `'local'` as a valid service value in the `service_tracks` CHECK constraint (migration 006). A local playlist is a `service_playlists(service='local')` containing any `service_tracks` — including Spotify tracks identified by ISRC. `v_file_track_link` extended to match `service='local' AND service_id = CAST(f.id AS TEXT)`. New `POST /api/playlists/local` endpoint: for each file ID, finds or creates a `service_track(service='local', service_id=file.id)`, creates a playlist with the given name, and links all resolved tracks. Existing `v_tag_playlist` automatically creates a Setlist tag from the playlist name, making new local playlists immediately available as tags for filtering and comment writing.

**Consequences**: Digging sessions persist immediately as playlists (and thus tags). No Spotify API calls needed. The automatic tag creation feeds into the existing comment-writing pipeline. Future feature: mirror local playlists to Spotify via the existing OAuth flow.

---

## ADR-052: Global Playlist Polling

**Date**: 2026-05-22
**Status**: Accepted (implemented)

**Context**: The subscription poller only covers explicitly-subscribed playlists. Unsubscribed playlists go stale until a manual full sync. With 200+ playlists, users needed automatic change detection for all playlists without fetching every playlist's tracks every cycle.

**Decision**: New `src/global_poller.rs` background task running at a configurable interval (default 900s/15min). Uses Spotify's `snapshot_id` for cheap change detection: fetches all user playlists (paginated), compares each `snapshot_id` against the stored DB value, and only fetches tracks for playlists where the snapshot changed or the playlist is new. New `snapshot_id` column on `service_playlists` (migration 006, consolidated). Configurable via `[polling].global_interval_secs` in `config.toml` or `MOMOS_GLOBAL_POLL_INTERVAL_SECS` env var (0 = disabled). Detects deleted playlists and logs them with `warn!`. 429 rate limits handled with retry + backoff. 200ms delay between playlist syncs.

**Consequences**: All playlists stay up-to-date automatically. Snapshot-based detection minimizes API traffic (~14 calls per cycle for 200 playlists with 5 changes). New playlists are auto-discovered. Deleted playlists are detected and logged. Rate-limit safe by design.

---

## ADR-053: Download Guarantor (100% File Coverage)

**Date**: 2026-08-29
**Status**: Accepted (implemented)

**Context**: Subscribed Spotify playlists could end up with tracks that had no linked local file — download queues stalled, deemix entries went zombie, and there was no single place that guaranteed every track eventually had a file.

**Decision**: New `src/download_guarantor.rs` background task running every 10 minutes. Two phases: (1) poll the deemix-pyweb API and UPSERT real download status into `deemix_downloads`, detecting stuck/zombie entries; (2) for every track in every subscribed playlist without a linked file, re-queue via deemix first, then fall back to spotDL (YouTube). Ships alongside the standalone `download-service/` Python pipeline (FastAPI + deemix/spotDL/Spotify clients) that performs the actual downloads.

**Consequences**: File coverage converges to 100% without manual intervention. Failed downloads are automatically retried across two independent sources. Adds a runtime dependency on the deemix-pyweb service and spotDL CLI.

---

## ADR-054: Telemetry Analytics Push

**Date**: 2026-08-29
**Status**: Accepted (implemented)

**Context**: There was no visibility into the single production instance's health — failed tasks, error logs, and table sizes were invisible without SSHing in and poking at the SQLite DB.

**Decision**: The prod instance periodically pushes a self-describing telemetry bundle over HTTPS to a small receiver on the LAN server. The core payload is a consistent SQLite full snapshot via `VACUUM INTO`, plus logs, `task_history`, aggregated metrics, and redacted instance metadata. Implemented in `src/telemetry/` (metrics, receiver client, scheduler) with a systemd unit (`deploy/momos-telemetry.service`) and Caddy snippet for the receiver. No schema migration required (reuses `task_history` from migration 022).

**Consequences**: The SQLite snapshots double as an off-machine DB backup. A LAN-side analyzer (openclaw) can read the latest snapshot directly for error/task/orphan statistics. Requires the receiver to be reachable; push is best-effort and does not block startup.

---

## ADR-055: macOS Menu Bar Tray Icon

**Date**: 2026-08-29
**Status**: Accepted (implemented)

**Context**: The `.app` runs headless (`LSUIElement = true`, no Dock icon), so there was no way to see server status or quit the app without killing the process.

**Decision**: Add a menu bar tray icon via `tray-icon` + `tao` (native AppKit bindings, no WebKit). The Tao event loop owns the main thread (required for `NSStatusBar`), and the Axum server + all background tasks are moved onto a Tokio runtime in a spawned thread. Tray menu provides "Open Dashboard" (opens `http://localhost:3000`) and "Quit".

**Consequences**: Users can now see status and cleanly quit from the menu bar. `main()` is restructured so the runtime no longer owns the main thread — background tasks must be spawned explicitly. Adds ~3.5 MB binary size, no GPU overhead.

---

## ADR-056: File↔Track Corrections

**Date**: 2026-08-29
**Status**: Accepted (implemented)

**Context**: Automatic file↔track linking (`v_file_track_link`) matched on ISRC/service IDs, but users sometimes needed to override it — e.g. force-link a file to a specific track, or prevent a wrong automatic link.

**Decision**: Add explicit `include`/`exclude` corrections that override automatic linking, stored in a new `file_track_corrections` table (migration 023). New endpoints: `GET/PUT /api/files/{id}/track-corrections`, `GET/PUT /api/tracks/{id}/file-corrections`, and `DELETE /api/file-track-corrections/{id}`. The Track/File detail pages gain a "disconnect" button and a link-file typeahead.

**Consequences**: Users can correct mis-links without editing the DB. Corrections win over automatic ISRC matching. Requires migration 023 (additive).

---

## ADR-057: External Tool Path Resolution

**Date**: 2026-08-29
**Status**: Accepted (implemented)

**Context**: The app shells out to `metaflac`, `exiftool`, `ffmpeg`, and `ffprobe`. When launched as a macOS GUI `.app` (Finder/Dock), the process inherits a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`) that omits Homebrew's `/opt/homebrew/bin`, so `Command::new("metaflac")` failed with "No such file or directory (os error 2)" even though the tools were installed.

**Decision**: Add `src/external_tools.rs` with `resolve_tool()`, which resolves a tool name to an absolute path across common install locations (`/opt/homebrew/bin`, `/usr/local/bin`, `/opt/local/bin`, and their sbin variants), falling back to the bare name. Use it for `metaflac`/`exiftool` in `src/db/files.rs` and `ffmpeg`/`ffprobe` in `src/api/files.rs`.

**Consequences**: Comment writing, metadata extraction, and stem streaming work regardless of how the app was launched. No longer relies on `PATH` for Homebrew tools. System tools (`rsync`, `ssh`) remain on the minimal `PATH` and are unchanged.

---

## ADR-058: Per-Format Comment Writers (lofty for MP3 only)

**Date**: 2026-08-29
**Status**: Accepted (implemented)

**Context**: `write_comment_to_file` shelled out to `exiftool` for everything except FLAC. The Homebrew exiftool build reports `Writing of MP3 files is not yet supported`, so MP3 comment writes always failed. We considered unifying all comment writes on `lofty` (already used to read tags during scans), which does support writing FLAC/WAV/AIFF/M4A/MP3.

**Decision**: Keep the mixed writer, add `lofty` only for MP3:

- FLAC → `metaflac` (`--remove-tag=COMMENT --set-tag COMMENT=...`) — lossless, touches only the `COMMENT` field.
- MP3 → `lofty` (ID3v2 `COMM` frame) — the only in-tree option, since exiftool cannot write MP3 and ffmpeg writes a `TXXX:comment` frame that lofty doesn't read as `ItemKey::Comment`.
- M4A/WAV/AIFF → `exiftool` (unchanged).

MP3 comment _reads_ also go through `lofty` (`read_comment_from_file` branches on `.mp3`). This is required for consistency: lofty writes the ID3v2 `COMM` frame with language `"XXX"` (its MPEG write path ignores the language field), which exiftool renders as `Comment-xxx` rather than `Comment`.

**Consequences**: Do **not** unify on `lofty` for writes — its Vorbis-comment round-trip is not lossless (it remaps `ORGANIZATION` → `LABEL`), which would corrupt FLAC tags. MP3 is an accepted exception because there is no lossless in-tree alternative and testing confirmed other ID3v2 frames are preserved.
