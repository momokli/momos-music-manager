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

# 🎯 IMPLEMENTATION PLAN

This plan is organized as sequential phases. Each phase has clear deliverables
and can be implemented independently by an agent without regressions.

---

## Phase 0: Quick Fixes & Diagnostics

### 0.1 — Tracks search ✅ (resolved)

**Status**: Verified working. The search across title/artist/album on the tracks page functions correctly.

No action needed.

### 0.2 — Clean up cargo warnings ✅ (resolved)

**Status**: Removed global `#![allow(dead_code)]` and all `#[allow(unused_imports)]` annotations. Added targeted `#[allow(dead_code)]` on individual items that are intentionally unused (future use, dev-only, etc.).

`cargo build` and `cargo clippy` both produce zero warnings.

No action needed.

### 0.3 — Clean up startup log ✅ (resolved)

**Status**: Consolidated startup log messages into a clean banner:

- Poller now shows subscription count (`idle, 0 subscriptions` or `{N} subscription(s)`)
- Uses `listener.local_addr()` to show actual bound port
- Added startup banner: `🚀 Momo's Music Manager v0.1.0 started`

No action needed.

---

## Phase 1: Deployment & Database Path

### 1.1 — Database path configuration ✅ (resolved)

**Status**: Config.toml now supports `[database]` section with `url` field. Priority chain:
`DATABASE_URL` env var > `[database].url` in config.toml > `sqlite:~/.local/share/momos-music-manager/library.db`

The default path uses `shellexpand` for `~` resolution. Parent directory is auto-created on startup.

No action needed.

### 1.2 — macOS Launch Agent (auto-start) ✅ (resolved)

**Status**: Added `src/launch_agent.rs` with `install()`, `uninstall()`, and `status()` functions.
Conditionally compiled for macOS only via `#[cfg(target_os = "macos")]`.

CLI commands:

- `cargo run -- install-launch-agent` — creates plist + loads via `launchctl bootstrap`
- `cargo run -- uninstall-launch-agent` — unloads + removes plist
- `cargo run -- service-status` — shows whether agent is loaded/running

No action needed.

### 1.3 — Config.toml usage documented in README ✅ (resolved)

**Status**: README now has a full "Configuration" section with config.toml format, priority chain,
and env var reference. Also has a "Deployment" section covering database path, launch agent, and reverse proxy.

No action needed.

---

## Phase 2: Spotify Auth Improvements

### 2.1 — Dynamic redirect URI ✅ (resolved)

**Status**: Added `--public-url` CLI flag to `serve` command. Added `public_url` field to `AppState`.
Fallback chain: CLI `--public-url` > `PUBLIC_URL` env var / `[server].public_url` in config.toml > `None`.

No action needed.

### 2.2 — Callback redirects should go to frontend URL ✅ (resolved)

**Status**: Both `service_callback_handler` and `legacy_callback_handler` now redirect to `state.public_url`
or fall back to `http://{server_host}:{server_port}`. No more hardcoded `:3000` / `:8000` split.

No action needed.

---

## Phase 3: Documentation & README Cleanup

### 3.1 — README cleanup ✅ (resolved)

**Status**: README is now fully English, up-to-date with the current single-binary setup.
Includes configuration docs, deployment section, launch agent docs, and clean CLI reference.

No action needed.

### 3.2 — Update docs/FRONTEND_NEXT_PLAN.md ✅ (resolved)

**Status**: Document marked as historical reference. `frontend_next/` references removed,
wiring status table updated to reflect reality, only genuinely unfinished items remain.

No action needed.

### 3.3 — Update docs/ARCHITECTURE.md ✅ (resolved)

**Status**: Updated to reflect 12 tables/views, added views section, updated frontend section,
added `[database]`/`[server]` config, updated API listing and dev commands.

No action needed.

### 3.4 — Update docs/DECISIONS.md ✅ (resolved)

**Status**: Added ADR-034 through ADR-037 covering database path, launch agent,
dynamic redirect URI, and startup logging.

No action needed.

---

## Phase 4: Code Quality & Polish

### 4.1 — Remove `#![allow(dead_code)]` and fix individually

**Problem**: The global `#![allow(dead_code)]` hides legitimate dead code issues.

**Approach**:

1. Remove the global attribute
2. Build and see what's flagged
3. For each dead code:
   - If truly unused and should be removed: remove it
   - If needed for future use: add `#[allow(dead_code)]` on the specific item with a comment
   - If used conditionally (e.g. behind feature flags): add `#[cfg_attr(not(feature_x), allow(dead_code))]`

**Target**: Zero warnings without global suppression.

### 4.2 — Fix `#[allow(unused_imports)]` instances

Same approach as 4.1 but for imports in `api.rs`, `spotify/client.rs`, `spotify/mod.rs`.

### 4.3 — Clean up TODO comments ✅ (resolved)

**Status**: All TODOs audited. Addressed TODO (dynamically redirect) removed.
Remaining TODOs given meaningful context. Compiles cleanly.

No action needed.

### 4.4 — Add config.toml schema for `[database]` and `[server]` ✅ (resolved)

**Status**: Added `DatabaseToml`, `ServerToml` structs and extended `TomlConfig` in `config.rs`.
`ServiceCredentials` now has `database_url`, `server_host`, `server_port`, `server_public_url`
fields with proper fallback chain (env > toml > default).

No action needed.

---

## Phase 5: Testing & Verification

### 5.1 — Test tracks search

**Action items**:

1. Add test tracks to the database
2. Verify `GET /api/tracks?search=test` returns correct results
3. Verify `GET /api/tracks/count?search=test` returns correct count
4. Test empty search returns all tracks
5. Test case-insensitive search

### 5.2 — Test deployment flow

**Action items**:

1. Build release binary: `cargo build --release`
2. Copy to `/usr/local/bin/`
3. Set up config.toml at `~/.config/momos-music-manager/config.toml`
4. Run install-launch-agent
5. Verify server starts on login
6. Test Spotify OAuth with deployed setup

### 5.3 — Test Spotify OAuth with `--public-url`

**Action items**:

1. Start with `--public-url https://mmm.mydomain.de`
2. Trigger auth flow
3. Verify redirect URI matches
4. Verify callback redirects to the public URL
5. Verify token exchange works

---

## Phase 6: Future Considerations (not for this round)

### 6.1 — Configuration validation on startup

- Validate that database path is writable
- Validate that Spotify credentials exist if using Spotify features
- Print config summary on startup (without secrets)

### 6.2 — Health check endpoint enhancement

- Add database connectivity check
- Add OAuth token validity check
- Add service status check

### 6.3 — Docker support

- Recreate Docker setup for server deployment
- Document Docker usage in README

### 6.4 — Reverse proxy documentation

- Add nginx/Caddy example configs
- Document WebSocket support for reverse proxy

---

## Implementation Order Summary

| Phase                      | Priority    | Effort | Status     |
| -------------------------- | ----------- | ------ | ---------- |
| 0.1 Tracks Search          | 🔴 Critical | Small  | ✅ Done    |
| 0.2 Cargo Warnings         | 🟡 Medium   | Medium | ✅ Done    |
| 0.3 Startup Log            | 🟢 Low      | Small  | ✅ Done    |
| 1.1 DB Path Config         | 🔴 Critical | Medium | ✅ Done    |
| 1.2 Launch Agent           | 🟡 Medium   | Medium | ✅ Done    |
| 1.3 README Deployment Docs | 🟡 Medium   | Small  | ✅ Done    |
| 2.1 Dynamic Redirect URI   | 🟡 Medium   | Medium | ✅ Done    |
| 2.2 Callback Redirect Fix  | 🟢 Low      | Small  | ✅ Done    |
| 3.1 README Cleanup         | 🟡 Medium   | Small  | ✅ Done    |
| 3.2-3.4 Docs Update        | 🟢 Low      | Medium | ✅ Done    |
| 4.1-4.2 Code Cleanup       | 🟡 Medium   | Medium | ✅ Done    |
| 4.3 TODO Cleanup           | 🟢 Low      | Small  | ✅ Done    |
| 4.4 Config Schema          | 🔴 Critical | Medium | ✅ Done    |
| 5.1-5.3 Testing            | 🟡 Medium   | Medium | 🔜 Pending |

---

## Phase 7: Deemix Service Integration & Tracks Page Overhaul

### 7.0 — Overview

Introduce "deemix" as a new streaming service alongside Spotify, SoundCloud, YouTube.
Deemix auth uses ARL (cookie value) + a local deemix web API host (default `http://localhost:6596`).
The deemix service can receive Spotify playlist share URLs into an internal download queue,
and individual queue items can be retried/refreshed.

On the frontend side, the TRACKS page gets overhauled to match the FILES page's filter&search+table
UI/UX, becoming the canonical CRUD interface pattern for all list views. A playlist-context badge
in the toolbar shows when tracks are scoped to a specific playlist.

---

### 7.1 — Web-UI-Only Configuration

Deemix is configured **entirely via the web UI** — no config.toml section, no env vars.

**Config storage**: ARL + host are stored in the `service_config` table:

- `service_config.access_token` → ARL value
- `service_config.metadata_json` → `{"host": "http://localhost:6596"}` (or similar JSON blob)
- `service_config.is_connected` → set to `true` after successful test

**No config.rs changes needed** for deemix. The `ServiceCredentials` struct stays untouched.
The backend reads deemix config exclusively from the `service_config` DB table.

**Services page UI states**:

| State                  | What the user sees                                                                                      |
| ---------------------- | ------------------------------------------------------------------------------------------------------- |
| Not configured         | ARL input (password field) + HOST input + "Test & Save" button                                          |
| Configured + connected | Status badge "✅ Connected" + "Reconfigure" button + "Test" button (calls `/api/services/deemix/queue`) |
| Configured but failing | Status badge "❌ Error" with message + "Reconfigure" button + "Test" button                             |

**Test button** calls `GET /api/services/deemix/queue` — if it returns data, the connection works.
**Reconfigure** clears the stored config and shows the ARL/HOST input form again.

**File**: `src/api.rs` (new deemix handlers) + `frontend/pages/services.js` (updated)

---

### 7.2 — Database Schema Changes

This requires schema changes, so we modify `migrations/001_initial_schema.sql` and delete all old `.db` files.

**Changes**:

1. `service_config.service` CHECK constraint: add `'deemix'` → `CHECK (service IN ('spotify', 'soundcloud', 'youtube', 'deemix'))`
2. `service_tracks.service` CHECK constraint: add `'deemix'` → `CHECK (service IN ('spotify', 'soundcloud', 'youtube', 'deemix'))`
3. New table `deemix_downloads`:
   ```sql
   CREATE TABLE deemix_downloads (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       spotify_playlist_url TEXT NOT NULL,
       playlist_name TEXT,
       status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'downloading', 'completed', 'failed')),
       track_count_total INTEGER DEFAULT 0,
       track_count_downloaded INTEGER DEFAULT 0,
       error_message TEXT,
       created_at INTEGER DEFAULT (unixepoch()),
       updated_at INTEGER DEFAULT (unixepoch()),
       UNIQUE(spotify_playlist_url)
   );
   ```

**File**: `migrations/001_initial_schema.sql`

---

### 7.3 — New Backend Module: `src/deemix/`

#### 7.3.1 — `src/deemix/mod.rs`

- Module declaration, re-exports

#### 7.3.2 — `src/deemix/client.rs`

HTTP client for the deemix web API. Default port is `6595` (not `6596`).

**Auth flow**:

1. `POST /api/loginArl` with body `{"status": 1, "arl": "<ARL>"}`
2. Server returns user data + sets session cookie (`connect.sid`)
3. Use the session cookie jar for all subsequent requests

The reqwest client must use a cookie store to retain the `connect.sid` session cookie.

**Endpoints** (confirmed from live deemix-pyweb v4.5.2):

| Method | Path                 | Body                                      | Description                                          |
| ------ | -------------------- | ----------------------------------------- | ---------------------------------------------------- |
| `POST` | `/api/loginArl`      | `{"status": 1, "arl": "..."}`             | Authenticate with ARL, get session cookie            |
| `GET`  | `/api/connect`       | —                                         | Check connection status + get settings + queue items |
| `GET`  | `/api/getQueue`      | —                                         | Get all queue items as a map keyed by `uuid`         |
| `POST` | `/api/addToQueue`    | JSON body (TBD, likely `{"url": "..."}`)  | Add a Spotify playlist URL to the download queue     |
| `POST` | `/api/retryDownload` | JSON body (TBD, likely `{"uuid": "..."}`) | Retry a failed download                              |

**Cookie lifecycle + auto-re-auth**:

- The `connect.sid` session cookie lives **only in the reqwest cookie jar** (in-memory)
- It expires when: our server restarts, OR the deemix server restarts (Express in-memory session)
- **Don't store the cookie in the DB** — store only the ARL
- **Auto-re-auth on 401**: Every API call that returns HTTP 401 triggers a re-auth:
  1. Load ARL from `service_config` DB
  2. Call `POST /api/loginArl` to get a fresh session cookie
  3. Retry the original request once
  4. If re-auth also fails → mark `is_connected = false` in DB + surface error

**Struct**:

```rust
pub struct DeemixClient {
    http_client: reqwest::Client,      // with cookie store
    base_url: String,                   // e.g. "http://localhost:6595"
    db: Pool<Sqlite>,                   // to read ARL for re-auth
}

impl DeemixClient {
    pub fn new(base_url: &str, db: Pool<Sqlite>) -> Self { ... }
    pub async fn login_arl(&self, arl: &str) -> Result<DeemixUser> { ... }  // POST /api/loginArl
    pub async fn test_connection(&self) -> Result<bool> { ... }              // GET /api/getQueue (fails if not authed)
    pub async fn get_queue(&self) -> Result<HashMap<String, DeemixQueueItem>> { ... }  // GET /api/getQueue
    pub async fn add_to_queue(&self, url: &str) -> Result<()> { ... }       // POST /api/addToQueue
    pub async fn retry_download(&self, uuid: &str) -> Result<()> { ... }    // POST /api/retryDownload

    // Internal: wraps every API call with auto-re-auth on 401
    async fn authed_request(&self, method: Method, path: &str) -> Result<Response> { ... }
}
```

#### 7.3.3 — `src/deemix/models.rs`

```rust
/// Response from POST /api/loginArl
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixLoginResponse {
    pub status: i64,
    pub arl: String,
    pub user: DeemixUser,
    pub childs: Vec<DeemixUser>,
    pub current_child: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixUser {
    pub id: i64,
    pub name: String,
    pub picture: String,
    pub license_token: String,
    pub can_stream_hq: bool,
    pub can_stream_lossless: bool,
    pub country: String,
    pub language: String,
}

/// Single queue item from GET /api/getQueue (keyed by uuid in response)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixQueueItem {
    #[serde(rename = "type")]
    pub item_type: String,               // "album" | "spotify_playlist"
    pub id: String,                      // Deezer/Spotify ID
    pub bitrate: i64,
    pub uuid: String,                    // unique key: e.g. "spotify_playlist_xxx_1"
    pub title: String,                   // playlist/album title
    pub artist: String,                  // creator name
    pub cover: Option<String>,
    pub explicit: bool,
    pub size: i64,                       // total track count
    pub downloaded: i64,                 // downloaded track count
    pub failed: i64,                     // failed track count
    pub progress: i64,                   // percentage (0-100)
    pub errors: Vec<DeemixDownloadError>,
    pub files: Vec<DeemixDownloadedFile>,
    #[serde(rename = "__type__")]
    pub collection_type: String,
    pub status: String,                  // "completed" | "withErrors" | "queued" | "downloading"
    pub extras_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixDownloadError {
    pub message: String,
    pub data: DeemixErrorData,
    pub stack: String,
    #[serde(rename = "type")]
    pub error_type: String,              // "track"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixErrorData {
    pub id: String,
    pub title: String,
    pub artist: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixDownloadedFile {
    pub album_urls: Option<Vec<DeemixAlbumUrl>>,
    pub album_path: Option<String>,
    pub album_filename: Option<String>,
    pub filename: String,
    pub data: DeemixTrackData,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixAlbumUrl {
    pub url: String,
    pub ext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixTrackData {
    pub id: serde_json::Value,           // can be string or number
    pub title: String,
    pub artist: String,
}

/// Top-level response from GET /api/getQueue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeemixQueueResponse {
    pub queue: HashMap<String, DeemixQueueItem>,
    pub queue_order: Vec<String>,
}
```

### 7.4 — API Endpoints

All deemix-specific endpoints in `src/api.rs`:

| Method   | Path                                    | Handler                  | Description                                                |
| -------- | --------------------------------------- | ------------------------ | ---------------------------------------------------------- |
| `POST`   | `/api/services/deemix/auth`             | `deemix_auth_handler`    | Validate ARL, store in DB, test connection                 |
| `GET`    | `/api/services/deemix/queue`            | `deemix_queue_handler`   | List local + remote queue combined                         |
| `POST`   | `/api/services/deemix/queue`            | `deemix_enqueue_handler` | Add Spotify playlist URL (store in DB + forward to deemix) |
| `POST`   | `/api/services/deemix/queue/{id}/retry` | `deemix_retry_handler`   | Retry a failed download                                    |
| `DELETE` | `/api/services/deemix/queue/{id}`       | `deemix_delete_handler`  | Remove queue item                                          |

**Generic service wiring**: Add `'deemix'` to all existing service-iterating endpoints:

- `services_handler` / `get_service_connections` — include deemix in the expected services list
- `service_auth_handler` — handle `'deemix'` case (store ARL in service_config instead of OAuth tokens)
- `service_config_handler` (GET + PUT) — read/write `arl` + `host` in service_config.metadata_json
- `service_reset_handler` — handle `'deemix'` (clear config, don't poll)
- `add_track_to_playlist_handler` — maybe not needed for deemix initially

**Important**: Deemix does not use OAuth. Auth = store ARL in `service_config` with `access_token = arl` and `is_connected = true` after successful test. No refresh token flow needed.

---

### 7.5 — Tasks Integration

New `SyncType` variant in `src/tasks/mod.rs`:

```rust
pub enum SyncType {
    ServiceSync(String),   // existing
    WriteComment,          // existing
    RecomputeEmbeddings,   // existing
    ScanFolder,            // existing
    DeemixSync,            // NEW — periodic poll of deemix queue
}
```

A `DeemixSync` task polls the deemix queue status periodically and updates local `deemix_downloads` table with progress information.

---

### 7.6 — Tracks Page Overhaul (frontend)

#### 7.6.1 — Goal

The TRACKS page (`frontend/pages/tracks.js`) currently has a simple toolbar (search + service filter)
with a table below. We need to upgrade it to the same pattern as the FILES page.

**Files page pattern**:

1. **Filter panel** (left side): Tag picker, key grid, BPM range, write-comment sidebar
2. **Stats row**: Refresh button + total count badge
3. **Table**: Paginated, sortable-ish via pre-defined columns
4. **Pagination**: Prev/Next with page number
5. **URL state**: Hash params for page/search/filters
6. **Stable toolbar**: Rendered once, preserves focus

**Tracks page after overhaul**:

1. **Stable toolbar** (rendered once, preserves search focus):
   - Search input (existing, keep)
   - Service filter dropdown/button group (existing, keep)
   - NEW: **Playlist context badge** — shown when viewing tracks scoped to a specific playlist
     - Badge shows playlist name + icon
     - Has an X button to clear the filter
     - Badge is a chip similar to `tag-chip` in comment-writer
2. **Content area** (re-rendered):
   - Stats row: Refresh btn + total track count + current playlist info if scoped
   - Table: Same columns as now but with playlist column added
   - Pagination: Prev/Next

#### 7.6.2 — Playlist Context

**URL param**: `#tracks?playlistId=123` or `#tracks?playlistName=...`

**Init flow**:

1. `app.js` already parses hash params via `getHashParams()`
2. `tracks.js` init receives container + signal + hash params from app.js (or parses them itself)
3. If `playlistId` param exists, fetch playlist name via `/api/playlists/{id}` and set state
4. API calls include `?playlistId=...` to filter tracks

**Backend API changes**:

- `GET /api/tracks` and `GET /api/tracks/count` — add optional `playlistId` query param
- Join `service_playlist_tracks` to filter by playlist
- `GET /api/playlists/{id}` — returns playlist name, service, track count

**Visual**:

```html
<!-- In the toolbar -->
<div class="playlist-context-badge">
  <i class="fa-solid fa-list"></i>
  <span>Playlist: Summer Vibes</span>
  <button class="playlist-context-clear" title="Clear playlist filter">&times;</button>
</div>
```

#### 7.6.3 — Link from Playlists to Tracks

Each playlist row in `playlists.js` already has action buttons. We add two new elements:

**"View Tracks" link**: Navigates to `#tracks?playlistId=42` with a playlist context badge:

```html
<a href="#tracks?playlistId=42" class="btn btn-sm btn-icon" title="View tracks">
  <i class="fa-solid fa-list-music"></i>
</a>
```

**Deemix column**: A new column between existing columns showing deemix queue status per playlist, with conditional buttons:

| State                 | Action                                                                                |
| --------------------- | ------------------------------------------------------------------------------------- |
| Not in deemix queue   | ➕ Plus button — adds this playlist URL to deemix (`POST /api/services/deemix/queue`) |
| Queued or downloading | ⏳ Spinner or "Queued" label                                                          |
| Completed             | ✅ Checkmark (or no action needed)                                                    |
| Failed                | 🔄 Retry button — retries the download (`POST /api/services/deemix/queue/{id}/retry`) |

**Backend adds**:

- `GET /api/playlists` SQL extended to LEFT JOIN `deemix_downloads` to include status per playlist
- New field on playlist response: `deemixStatus: "queued" | "downloading" | "completed" | "failed" | null`

**playlists.js changes**:

- Add `Deemix` column header with icon `fa-solid fa-download`
- Each row: render plus/retry/status depending on deemix queue state
- Wire click handlers for add/retry actions that call the API

#### 7.6.4 — Updated Playlist Columns

| Change                | Detail                                                             |
| --------------------- | ------------------------------------------------------------------ |
| NEW Deemix column     | Between "Tags" and "Actions" — shows queue status + action buttons |
| Keep existing columns | Name, Service, Tracks, Tags, Actions (with View Tracks added)      |

---

### 7.7 — Future: Per-Track Download Status

_Not implemented in this phase — outlined for future reference._

Deemix provides per-track download status within each playlist (e.g. whether a specific track
was found on Deezer and downloaded successfully). This status will eventually be stored
in a new `deemix_track_status` column or table, and surfaced on the TRACKS page via:

- A new "Download" column showing ✅/❌/⏳ per service track
- Interaction with the `v_file_track_link` view to link downloaded files back to tracks

For now, we only track playlist-level queue status in `deemix_downloads`.

---

### 7.8 — Updated Tracks Table Columns

| Column      | Style | Notes                                                            |
| ----------- | ----- | ---------------------------------------------------------------- |
| Title       | 22%   | Same as before                                                   |
| Artist      | 16%   | Same as before                                                   |
| Service     | 8%    | Badge with icon                                                  |
| Album       | 14%   | Same as before                                                   |
| Playlists   | 18%   | NEW — comma-separated playlist names (only if not scoped to one) |
| Local Files | 10%   | Same as before                                                   |
| Duration    | 8%    | Same as before                                                   |
| ISRC        | 4%    | Same as before                                                   |

When scoped to a playlist, the Playlists column is hidden (redundant).

---

### 7.7 — Shared CRUD Interface Pattern

This phase establishes a canonical pattern for all list/table CRUD views.

**Pattern components**:

1. **Module structure**:
   - `renderToolbar(search, extraFilters)` — stable HTML, rendered once
   - `renderBody(data, state)` — content HTML, re-rendered on changes
   - `buildParams(state)` → URLSearchParams for API calls
   - `fetchAndRender(container, signal, state)` — fetch + render loop
   - `wireContentEvents(container, signal, state)` — event wiring after render
   - `init(container, signal, hashParams)` — entry point, reads URL hash for initial state

2. **State object**:

   ```js
   const state = {
     page: 0,
     search: "",
     service: "all",
     // view-specific filters...
     // playlistId: null,     // (for tracks)
     // playlistName: null,   // (for tracks)
   };
   ```

3. **URL hash state** (via `app.js`):
   - `page`, `search`, `service`, etc. are stored in hash params
   - On state change → update `window.location.hash` without reload
   - On init → parse hash params into state

4. **Files page** → Will be refactored to use the same shared patterns (future phase, not this one)

---

### 7.9 — AppState & Service Wiring

**`main.rs` AppState** — no new fields needed; AppState already has `db`, `config`, `task_manager`, `embeddings`, `public_url`.

**Cargo.toml** — no new dependencies; `reqwest` is already used in the spotify module.

**Service polling** — deemix does NOT need a poller for subscriptions. The queue is polled on demand
or via the services page connection check.

---

### 7.10 — Service Connections (Services Page)

**`frontend/pages/services.js`**:

- Add `deemix` to `SERVICE_META` with icon `fa-solid fa-download`
- Add `deemix` color to `SERVICE_COLORS`
- Service row shows: name, configured, connected, queue item count
- Config modal shows: ARL input (password field), host input, "Test Connection" button
- Auth flow: POST to `/api/services/deemix/auth` with `{ arl, host }` → backend validates → stores

---

### 7.11 — Implementation Order

| Sub-phase                | Priority    | Effort | What                                             |
| ------------------------ | ----------- | ------ | ------------------------------------------------ |
| 7.1 Config               | 🔴 Critical | Small  | Web-UI-only — store ARL+host in service_config   |
| 7.2 Schema               | 🔴 Critical | Small  | `001_initial_schema.sql` — add CHECK + new table |
| 7.3 Backend Module       | 🔴 Critical | Medium | `src/deemix/` — client, models                   |
| 7.4 API Endpoints        | 🔴 Critical | Medium | `src/api.rs` — deemix handlers + generic wiring  |
| 7.5 Tasks                | 🟡 Medium   | Small  | `src/tasks/mod.rs` — DeemixSync variant          |
| 7.6 Tracks Overhaul      | 🔴 Critical | Large  | `frontend/pages/tracks.js` — full rewrite        |
| 7.7 CRUD Pattern         | 🟡 Medium   | Small  | Documentation + shared patterns                  |
| 7.8 Services page wiring | 🟡 Medium   | Small  | Update services.js for deemix config/status      |
| 7.9 Playlists column     | 🟡 Medium   | Medium | Add Deemix column with add/retry buttons         |

---

### 7.12 — Detailed Technical Notes

#### Deemix Client Auth

Deemix uses cookie-based auth. The ARL (Account Registration Link) is a persistent session cookie.

The `connect.sid` cookie lives **only in the reqwest cookie jar** (in-memory).
**Don't store the cookie in the DB** — store only the ARL.
Auto-re-auth on HTTP 401: load ARL from DB, call `/api/loginArl`, retry once.

```rust
impl DeemixClient {
    pub fn new(base_url: &str, db: Pool<Sqlite>) -> Self {
        let http_client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("Failed to build reqwest client");

        Self {
            http_client,
            base_url: base_url.trim_end_matches('/').to_string(),
            db,
        }
    }

    pub async fn login_arl(&self, arl: &str) -> Result<DeemixLoginResponse> {
        let body = serde_json::json!({"status": 1, "arl": arl});
        let resp = self.http_client
            .post(format!("{}/api/loginArl", self.base_url))
            .json(&body)
            .send()
            .await?;
        // The session cookie (connect.sid) is automatically stored in the cookie jar
        Ok(resp.json().await?)
    }

    /// Make an API call with auto-re-auth on 401.
    /// Loads ARL from DB, re-authenticates, retries once.
    async fn authed_request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http_client.request(method.clone(), &url).send().await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            // Session expired — re-auth from stored ARL
            let arl = self.load_arl_from_db().await?;
            self.login_arl(&arl).await?;
            // Retry original request
            return self.http_client.request(method, &url).send().await;
        }

        Ok(resp)
    }

    async fn load_arl_from_db(&self) -> Result<String> {
        let row = sqlx::query("SELECT access_token FROM service_config WHERE service = 'deemix'")
            .fetch_one(&self.db)
            .await?;
        Ok(row.try_get::<String, _>("access_token")?)
    }
}
```

#### Deemix Web API Endpoints (confirmed)

The deemix web server exposes (confirmed from live deemix-pyweb v4.5.2):

- `POST /api/loginArl` — body `{"status": 1, "arl": "..."}` to authenticate, returns user data + sets `connect.sid` cookie
- `GET /api/connect` — check connection status + get settings + queue items
- `GET /api/getQueue` — returns `{"queue": {uuid: item, ...}, "queueOrder": []}`
- `POST /api/addToQueue` — add a Spotify playlist URL to the download queue
- `POST /api/retryDownload` — retry a failed or cancelled item
- Default host port is `6595` (not `6596`)

#### Tracks Page `renderBody()` Return Type

Current `get_tracks()` returns `Vec<ApiServiceTrack>` with `local_files: Vec<String>` (file type strings).
The overhaul adds playlist info:

- Add optional `playlist_names: Vec<String>` field to `ApiServiceTrack`
- Or add a separate endpoint for playlist-track associations

Preference: Extend `ApiServiceTrack` with `playlist_names: Vec<String>` (computed in the SQL query via GROUP_CONCAT on playlist names).

#### Playlist API Endpoint

Add a `GET /api/playlists/{id}` endpoint that returns:

```json
{
  "data": {
    "id": 1,
    "service": "spotify",
    "name": "Summer Vibes",
    "trackCount": 42
  }
}
```

Also add `GET /api/playlists/{id}/tracks` with proper implementation (currently a TODO stub).

#### URL Hash State for Tracks

```
#tracks?playlistId=42&search=summer&service=spotify&page=0
```

`app.js` already parses these. The tracks page `init()` should accept them as a second argument,
or parse `window.location.hash` directly. Since `app.js` currently passes only `(container, signal)`,
we should update the contract: `init(container, signal, hashParams)`.

---

### 7.13 — Files to Modify / Create

| File                                | Action                                                                                                |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `migrations/001_initial_schema.sql` | ✏️ Edit — add `'deemix'` to CHECK constraints, add `deemix_downloads` table                           |
| `src/deemix/mod.rs`                 | ✨ Create                                                                                             |
| `src/deemix/client.rs`              | ✨ Create                                                                                             |
| `src/deemix/models.rs`              | ✨ Create                                                                                             |
| `src/api.rs`                        | ✏️ Edit — add deemix handlers, wire routes, extend get_tracks with playlistId filter + playlist_names |
| `src/tasks/mod.rs`                  | ✏️ Edit — add DeemixSync variant                                                                      |
| `src/main.rs`                       | ✏️ Edit — add `mod deemix;`                                                                           |
| `frontend/pages/tracks.js`          | ✏️ Edit — full rewrite following FILES page pattern                                                   |
| `frontend/pages/playlists.js`       | ✏️ Edit — add Deemix column (+/retry/status) + "View Tracks" link in each row                         |
| `frontend/pages/services.js`        | ✏️ Edit — add deemix metadata + config modal                                                          |
| `frontend/shared/search-filter.js`  | ✏️ Edit — add renderPlaylistBadge helper (optional)                                                   |
| `frontend/app.js`                   | ✏️ Edit — update init contract to pass hashParams to page modules                                     |
| `frontend/style.css`                | ✏️ Edit — add .playlist-context-badge styles (optional)                                               |
| `docs/DECISIONS.md`                 | ✏️ Edit — add ADR for deemix integration                                                              |
| `docs/ARCHITECTURE.md`              | ✏️ Edit — update tables with deemix                                                                   |

---

---

## Phase 8: Deemix Queue CRUD Page

### 8.0 — Overview

Create a new frontend page `#deemix-queue` that lists all entries from the combined deemix queue
(GET /api/services/deemix/queue), following the TRACKS page pattern (stable toolbar + horizontal-scroll table).

### 8.1 — Backend

No backend changes needed. `GET /api/services/deemix/queue` already returns:

```typescript
interface DeemixCombinedQueueItem {
  id: number | null; // local DB id
  uuid: string | null; // deemix queue UUID
  spotifyPlaylistUrl: string | null;
  playlistName: string | null;
  status: string; // queued | downloading | completed | failed
  trackCountTotal: number;
  trackCountDownloaded: number;
  errorMessage: string | null;
  createdAt: number | null; // unix timestamp
  updatedAt: number | null; // unix timestamp
  title: string | null; // from deemix queue
  artist: string | null; // from deemix queue
  progress: number; // 0-100
}
```

### 8.2 — Frontend: `frontend/pages/deemix-queue.js`

Pattern: TRACKS page (`tracks.js`) — stable toolbar + body area with stats, table, pagination.

#### Toolbar (stable, rendered once)

- Search input (filters by title/artist/playlistName/spotifyPlaylistUrl)
- Status filter dropdown: All, queued, downloading, completed, failed
- Refresh button in stats row

#### Table (horizontal scroll)

| Column        | Width | Notes                                    |
| ------------- | ----- | ---------------------------------------- |
| Status        | 8%    | Badge with color + icon                  |
| Title         | 18%   | From deemix queue title                  |
| Artist        | 14%   | From deemix queue artist                 |
| Playlist Name | 16%   | playlistName field                       |
| Progress      | 8%    | Bar + percentage (0-100)                 |
| Downloaded    | 10%   | trackCountDownloaded / trackCountTotal   |
| Status Detail | 10%   | UUID (truncated) + error message tooltip |
| Created       | 8%    | Formatted date                           |
| Updated       | 8%    | Formatted date                           |
| Actions       | 10%   | Retry (failed), Delete                   |

#### Table container: horizontal scroll via `overflow-x: auto` on `.table-wrap`

#### Pagination: simple prev/next

#### Status badges:

- queued: yellow/grey
- downloading: blue with spinner icon
- completed: green with checkmark
- failed: red with exclamation

#### Actions per row:

- **Retry** (only for failed items): POST /api/services/deemix/queue/{id}/retry
- **Delete**: DELETE /api/services/deemix/queue/{id}

#### init(container, signal, hashParams):

- Parse search, statusFilter, page from hashParams
- Render toolbar once
- Fetch & render loop

### 8.3 — Frontend: `frontend/app.js`

Add `"deemix-queue": "deemix-queue"` to PAGE_MAP.

### 8.4 — Frontend: `frontend/shared/nav.js`

Add to Services section:

```js
{ id: "deemix-queue", label: "Deemix Queue", icon: "fa-download" },
```

### 8.5 — Implementation Order

| Step | File                             | What                     |
| ---- | -------------------------------- | ------------------------ |
| 8.3  | `frontend/app.js`                | Add route                |
| 8.4  | `frontend/shared/nav.js`         | Add nav link             |
| 8.2  | `frontend/pages/deemix-queue.js` | Full page implementation |

---

## Phase 9: CLI Interface for Deemix Actions

### 9.0 — Overview

Add CLI subcommands for direct deemix actions without needing the web UI.
Useful for debugging, scripting, and automation.

### 9.1 — CLI Commands (in `src/main.rs`)

```
cargo run -- deemix auth <ARL> [host]     # Test ARL + save to DB
cargo run -- deemix status                # Show config status + queue count
cargo run -- deemix queue                 # List queue items (JSON)
cargo run -- deemix add <url>            # Add playlist URL to queue
cargo run -- deemix retry <id>           # Retry a failed download
cargo run -- deemix delete <id>          # Remove from queue
```

### 9.2 — Implementation

1. Add `Deemix` subcommand to `Commands` enum in `main.rs`
2. Create `src/deemix/cli.rs` with handler functions that reuse `DeemixClient`
3. Each subcommand:
   - Connects to the DB (same pool setup as serve)
   - Loads deemix config from `service_config` table
   - Instantiates `DeemixClient`
   - Executes the action
   - Prints JSON or human-readable output to stdout

### 9.3 — Files to Modify

| File                | Action                                       |
| ------------------- | -------------------------------------------- |
| `src/main.rs`       | Add `deemix` subcommand with sub-subcommands |
| `src/deemix/cli.rs` | ✨ Create — CLI handler functions            |
| `src/deemix/mod.rs` | Add `pub mod cli;`                           |

### 9.4 — Example Output

```bash
$ cargo run -- deemix status
Deemix: connected
  Host: http://localhost:6596
  User: Matheo Klimke
  Queue: 13 items

$ cargo run -- deemix queue
[{"uuid": "album_738903131_9", "title": "Folge 43: ...", "status": "completed", "progress": 90}, ...]

$ cargo run -- deemix add "https://open.spotify.com/playlist/..."
Added to queue: https://open.spotify.com/playlist/...
```

---

## Recommended Agent Slices

Since phases have limited overlap, you can parallelize:

- **Agent A**: Phase 0 (all three items) — independent, no config changes
- **Agent B**: Phase 4.4 (config schema) + Phase 1.1 (DB path) — config core changes
- **Agent C**: Phase 1.2 (launch agent) + Phase 2 (Spotify auth) — deployment + OAuth
- **Agent D**: Phase 3 (docs) + Phase 4.1-4.3 (cleanup) — documentation + code quality
- **Agent E**: Phase 5 (testing) — after all others complete

Start with Agent A (quick wins) + Agent B (foundation) in parallel,
then Agent C + Agent D, then Agent E.

---

## Phase 10: CRUD Pages Unified Pattern

### 10.0 — Decisions (from rubberducking)

| #   | Question           | Decision                                                                                               |
| --- | ------------------ | ------------------------------------------------------------------------------------------------------ |
| 1   | Sort cycle         | **none → asc → desc → none** (three-state, allows resetting to default)                                |
| 2   | Page-size storage  | **Global via `localStorage`** — one setting across all pages, stored under key `crudPageSize`          |
| 3   | Tag file count     | **Include it!** Add a `v_tag_file_counts` SQL view for efficient querying without duplicate JOIN logic |
| 4   | Tasks polling      | **Keep 5s polling** — useful for watching running tasks complete                                       |
| 5   | Folders UI         | **Keep modal-based** add/edit — too many fields for inline editing                                     |
| 6   | FILES filter panel | **Refactor to stable toolbar** — separate filter panel from re-rendering table body to preserve focus  |
| 7   | Hash sync          | **`history.replaceState`** (silent) — avoids scroll jumps and page re-init loops                       |
| 8   | Approach           | **Build FILES page first as reference** — it's the richest page, serves as blueprint for all others    |

---

### 10.0b — Strategy: Build One Reference Page First

Instead of doing backend + all frontend in one big batch, we:

**Step 1 — Backend foundation**: Add `sort`/`order`/`pageSize` support, query structs, new SQL views.
**Step 2 — Shared frontend**: Create `shared/crud.js` helpers, CSS, `localStorage` page-size.
**Step 3 — Reference page (FILES)**: Fully retrofit FILES into the stable-toolbar + body pattern with sort, page-size, hash sync. This becomes the canonical blueprint.
**Step 4 — Spawn agents in parallel**: Each remaining page follows the exact same pattern. Agents have disjoint write scopes so they can run simultaneously.

---

### 10.1 — Overview: Target State

Unify ALL list/table pages under a consistent CRUD interface pattern:

| Page         | Current Pattern                | Target State                                  |
| ------------ | ------------------------------ | --------------------------------------------- |
| Files        | Filter panel + table           | Stable toolbar + sort + page size + hash sync |
| Tracks       | Stable toolbar + table         | Sort + page size + hash sync                  |
| Playlists    | Inline render                  | Full retrofit                                 |
| Tags         | Client-side filter             | Full retrofit (server-side pagination)        |
| Deemix Queue | Stable toolbar + client filter | Sort + page size + server-side filter         |
| Tasks        | Polling inline                 | Full retrofit (polling stays)                 |
| Folders      | Modal-based, own pattern       | Full retrofit (modals stay)                   |

**Non-targets** (special UIs that stay as-is):

- Tag Categories (drag-and-drop, energy levels)
- Services (card-based config)
- Dashboard (stats cards)
- Auto-categorize (wizard)

---

### 10.2 — Database: New SQL View for Tag File Counts

Add to `migrations/001_initial_schema.sql`:

```sql
-- Tag file counts (efficient view to avoid duplicate JOIN logic)
CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;
```

This view uses the existing `v_file_tags` view chain (files → tracks → playlists → tags)
and simply counts distinct files per tag. The tags API endpoint LEFT JOINs this view
for the `file_count` field.

---

### 10.3 — Backend: Unified Sort & Pagination Support

#### 10.3.1 — Add `sort` + `order` + `pageSize` to all list query structs

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub key: Option<String>,
    pub tags: Option<String>,
    pub search: Option<String>,
    pub linked_only: Option<bool>,
    pub unlinked: Option<bool>,
    pub non_default_only: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

// Same additions to: TracksQuery, PlaylistsQuery, TasksQuery
// New: TagsQuery, FoldersQuery, DeemixQueueQuery
```

`pageSize` is a convenience hint (overrides `limit`). If both are absent, defaults remain.

#### 10.3.2 — Shared `apply_sort` helper in `api.rs`

```rust
/// Append ORDER BY clause with whitelist validation.
/// Only allows known column names — safe from SQL injection.
pub fn apply_sort(
    sql: &mut String,
    sort: Option<&str>,
    order: Option<&str>,
    whitelist: &[&str],
    default: &str,
) {
    let sort_col = sort
        .filter(|s| whitelist.contains(&s))
        .unwrap_or(default);
    let ord = match order {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    sql.push_str(&format!(" ORDER BY {} {}", sort_col, ord));
}
```

#### 10.3.3 — Sort whitelists per endpoint

| Endpoint           | Whitelist                                                                                                                           | Default      |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| `get_files`        | `title`, `artist`, `bpm`, `key`, `isrc`, `play_count`, `last_played`, `created_at`, `duration_ms`, `file_type`                      | `id`         |
| `get_tracks`       | `title`, `artist`, `service`, `album`, `duration_ms`, `isrc`, `imported_at`                                                         | `id`         |
| `get_playlists`    | `name`, `service`, `track_count`, `imported_at`, `updated_at`                                                                       | `name`       |
| `get_tags`         | `name`, `category`, `created_at`, `file_count`                                                                                      | `name`       |
| `get_deemix_queue` | `title`, `artist`, `playlist_name`, `status`, `progress`, `created_at`, `updated_at`, `track_count_total`, `track_count_downloaded` | `created_at` |
| `get_tasks`        | `type`, `status`, `created_at`, `updated_at`, `progress`                                                                            | `created_at` |
| `get_folders`      | `path`, `file_count`, `watch_enabled`, `scan_recursive`, `last_scanned`, `max_depth`                                                | `path`       |

#### 10.3.4 — New/updated API endpoints

| Method | Path                         | Change                                                                   |
| ------ | ---------------------------- | ------------------------------------------------------------------------ |
| `GET`  | `/api/tags`                  | Add `limit`, `offset`, `search`, `category`, `sort`, `order`, `pageSize` |
| `GET`  | `/api/tags/count`            | Add `search`, `category` filters (existed as stub, implement fully)      |
| `GET`  | `/api/folders`               | Add `sort`, `order`, `search`, `limit`, `offset`, `pageSize`             |
| `GET`  | `/api/folders/count`         | NEW — total count with `search` filter                                   |
| `GET`  | `/api/services/deemix/queue` | Add `sort`, `order`, `search`, `status`, `limit`, `offset`, `pageSize`   |

---

### 10.4 — Frontend: Canonical Component Structure

Every CRUD page module follows this contract:

```js
// Exported entry point (called by app.js)
export async function init(container, signal, hashParams) {
  // 1. Parse hashParams into state
  // 2. Render stable toolbar once (preserves focus)
  // 3. Render content wrapper
  // 4. Wire toolbar events (search, filters, sort headers)
  // 5. Fetch + render initial data
}
```

**Internal functions**:

| Function                                      | Responsibility                                                   | Rendered           |
| --------------------------------------------- | ---------------------------------------------------------------- | ------------------ |
| `renderToolbar(state)`                        | Search input + filter controls                                   | Once (stable)      |
| `renderBody(data, state)`                     | Stats row + table rows + pagination + page-size selector         | On data change     |
| `buildParams(state)` → URLSearchParams        | Serialises state → API query params                              | —                  |
| `fetchAndRender(container, signal, state)`    | Fetches data, calls renderBody                                   | —                  |
| `wireContentEvents(container, signal, state)` | Pagination, row actions                                          | After renderBody   |
| `updateHash(state)`                           | Syncs state to `window.location.hash` via `history.replaceState` | After state change |
| `parseHash(hashParams)` → state               | Parses incoming hashParams                                       | On init            |

**State shape** (canonical fields + page-specific extras):

```js
const state = {
  page: 0, // 0-based
  pageSize: 25, // from localStorage (set in init)
  search: "",
  sort: "", // column name, e.g. "title"
  order: "asc", // "asc" | "desc"
  // page-specific filters...
};
```

---

### 10.5 — Global Page Size via localStorage

**Key**: `crudPageSize`

**Defaults**: `25` (available options: `10`, `25`, `50`, `100`)

**How it works**:

```js
// In init():
const saved = localStorage.getItem("crudPageSize");
const pageSize = saved ? parseInt(saved, 10) : 25;

// On page size change:
localStorage.setItem("crudPageSize", String(newSize));
state.pageSize = newSize;
state.page = 0;
fetchAndRender(...);
```

Page size is NOT stored in URL hash — it's the same across all pages.
The hash has `sort`, `order`, `search`, `page`, and page-specific filters.

---

### 10.6 — URL Hash State (Linkable Views)

**Canonical hash format**:

```
#page?sort=bpm&order=desc&search=house&page=0&key=1m,2m
```

**Rules**:

1. On INIT — read all state from `hashParams` (already provided by `app.js`)
2. On STATE CHANGE — call `updateHash(state)` which silently updates `window.location.hash`
3. Only meaningful values serialised: skip defaults (`page=0`, `sort=""`, `order="asc"`, `search=""`)
4. Page size is NOT in hash (it's global via localStorage)

**`updateHash(state)` helper** in `shared/crud.js`:

```js
/**
 * Update window.location.hash from canonical CRUD state.
 * Uses history.replaceState so no hashchange event fires.
 * The page module handles re-fetching itself after state changes.
 *
 * @param {string} pageId — e.g. "files", "tracks"
 * @param {object} state — the mutable state object
 * @param {object} [defaults] — default values to skip (e.g. {sort: "", order: "asc"})
 */
export function updateHash(pageId, state, defaults = {}) {
  const params = new URLSearchParams();
  for (const [key, val] of Object.entries(state)) {
    if (key === "pageSize") continue; // global, not in hash
    if (val === defaults[key] || val === undefined || val === null) continue;
    if (Array.isArray(val) && val.length === 0) continue;
    params.set(key, Array.isArray(val) ? val.join(",") : String(val));
  }
  const qs = params.toString();
  const hash = qs ? `#${pageId}?${qs}` : `#${pageId}`;
  if (window.location.hash !== hash) {
    history.replaceState(null, "", hash);
  }
}
```

---

### 10.7 — Page Size Selector (in `shared/crud.js`)

Lives in the stats row (re-renders with body, not stable toolbar):

```js
export function renderPageSizeSelector(currentSize, available = [10, 25, 50, 100]) {
  const opts = available
    .map(
      (s) =>
        `<option value="${s}"${s === currentSize ? " selected" : ""}>${s} per page</option>`,
    )
    .join("");
  return `<select class="page-size-select" data-page-size="true">${opts}</select>`;
}

/**
 * Wire page size selector changes.
 * Storage is global (localStorage), state is per-page.
 */
export function wirePageSizeSelector(container, state, onChange) {
  const sel = container.querySelector("[data-page-size]");
  if (!sel) return;
  sel.addEventListener("change", () => {
    const val = parseInt(sel.value, 10);
    localStorage.setItem("crudPageSize", String(val));
    state.pageSize = val;
    state.page = 0;
    onChange();
  });
}
```

---

### 10.8 — Sortable Column Headers

**Frontend pattern**:

Column `<th>` elements get click handlers. Click cycles: **none → asc → desc → none**.

```html
<th data-sort="title" class="sortable sort-asc">Title <i class="fas fa-sort-up"></i></th>
```

CSS classes:

- `.sortable` — cursor pointer, hover highlight
- `.sort-asc` / `.sort-desc` — active sort indicator

**Shared helper** in `shared/crud.js`:

```js
/**
 * Render a sortable table header.
 * @param {string} label — display label
 * @param {string} column — sort key sent to the API
 * @param {object} state — current { sort, order }
 * @param {object} [opts] — { style, width }
 * @returns {string} HTML
 */
export function sortableTh(label, column, state, opts = {}) {
  let icon = "fa-sort";
  let cls = "sortable";
  if (state.sort === column) {
    cls += state.order === "asc" ? " sort-asc" : " sort-desc";
    icon = state.order === "asc" ? "fa-sort-up" : "fa-sort-down";
  }
  const style = opts.style ? ` style="${opts.style}"` : "";
  return `<th class="${cls}" data-sort="${column}"${style}>${label} <i class="fas ${icon}"></i></th>`;
}

/**
 * Wire sortable header clicks on a table.
 * Cycle: none → asc → desc → none (three-state).
 * Calls onChange(newSort, newOrder) after updating state.
 */
export function wireSortableHeaders(tableEl, state, onChange) {
  tableEl.querySelectorAll("th.sortable[data-sort]").forEach((th) => {
    th.addEventListener("click", () => {
      const col = th.dataset.sort;
      if (state.sort === col) {
        if (state.order === "asc") {
          state.order = "desc";
        } else {
          state.sort = "";
          state.order = "asc";
        }
      } else {
        state.sort = col;
        state.order = "asc";
      }
      state.page = 0;
      onChange();
    });
  });
}
```

---

### 10.9 — CSS Additions (`frontend/style.css`)

```css
/* Page size selector */
.page-size-select {
  padding: 2px 6px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-surface);
  color: var(--text);
  font-size: 0.75rem;
  font-family: var(--font-mono);
  margin-left: 8px;
  cursor: pointer;
}

/* Sortable column headers */
th.sortable {
  cursor: pointer;
  user-select: none;
}
th.sortable:hover {
  background: rgba(255, 255, 255, 0.05);
}
th.sortable i {
  opacity: 0.3;
  margin-left: 4px;
  font-size: 0.7rem;
}
th.sort-asc i.fa-sort-up,
th.sort-desc i.fa-sort-down {
  opacity: 1;
}
th.sort-asc i.fa-sort,
th.sort-desc i.fa-sort,
th:not(.sort-asc):not(.sort-desc) i.fa-sort-up,
th:not(.sort-asc):not(.sort-desc) i.fa-sort-down {
  display: none;
}
th:not(.sort-asc):not(.sort-desc) i.fa-sort {
  display: inline;
}
```

---

### 10.10 — Reference Page: FILES (the blueprint)

FILES is the richest page with the most features, making it the ideal reference implementation.

#### Current issues to fix:

1. **Whole-container re-render**: The filter panel, stats row, table, and pagination are all in one `innerHTML` call. Changing BPM slider re-renders EVERYTHING, losing search focus.
2. **No sort**: Column headers are `<th>` with no click handlers.
3. **No page size**: Hardcoded `PAGE_SIZE = 50`.
4. **Hash read-only**: Reads hash on init but never writes it back.
5. **Toast duplication**: Has its own `showToast` function duplicated from `components.js`.
6. **Duplicated `escapeHtml`**: Has its own instead of importing from `components.js`.
7. **Comment writer sidebar**: Currently rendered inline. Should be stable in the toolbar area.

#### Target structure for FILES:

```
┌──────────────────────────────────────────────────────┐
│  TOOLBAR (stable, rendered once)                      │
│  ┌──────────────┐ ┌──────────┐ ┌───────────────────┐ │
│  │ 🔍 Search…   │ │BPM slider│ │ Key grid + tags   │ │
│  └──────────────┘ └──────────┘ └───────────────────┘ │
│  Comment writer sidebar (stable)                      │
├──────────────────────────────────────────────────────┤
│  CONTENT (re-rendered on changes)                    │
│  Stats: 🔄 127 files │ 25 per page ▼                 │
│  ┌────────────────────────────────────────────────┐  │
│  │ Title ↑ │ Artist │ BPM ↓ │ Key │ Linked │ … │  │  │
│  ├────────────────────────────────────────────────┤  │
│  │ row 1                                        │  │  │
│  │ row 2                                        │  │  │
│  └────────────────────────────────────────────────┘  │
│  ◀ Page 1 of 6 ▶                                     │
└──────────────────────────────────────────────────────┘
```

**Key changes**:

- Filter panel (BPM, key grid, tag filter) moves INTO the toolbar (preserved across re-renders)
- Comment writer sidebar stays in toolbar area
- Stats row, table, pagination, page size selector → `renderBody()`
- Sortable column headers
- `updateHash()` after each state change
- Import shared helpers from `shared/crud.js`

**File**: `frontend/pages/files.js` — full rewrite following the canonical pattern.

---

### 10.11 — Per-Page Details (for agent briefs)

#### 10.11.1 — TRACKS (minor additions)

**Already has**: stable toolbar, playlist context badge, hash params.

**Needs**:

- Sortable column headers (10.8)
- Page size selector in stats row (10.7)
- `updateHash()` on state change (10.6)
- `pageSize` from localStorage instead of hardcoded `PAGE_SIZE = 10`

**Sortable columns**: `title`, `artist`, `service`, `album`, `duration_ms`, `isrc`, `imported_at`

**File**: `frontend/pages/tracks.js`

---

#### 10.11.2 — PLAYLISTS (full retrofit)

**Needs**:

- Stable toolbar: search + service filter
- Sortable column headers
- `updateHash()` + init from hash
- Stats row: refresh + count + page-size selector
- Backend: add `sort`/`order` to `PlaylistsQuery`

**Table columns**:

| Column    | Width | Sortable | Notes                            |
| --------- | ----- | -------- | -------------------------------- |
| Name      | 22%   | ✅       |                                  |
| Service   | 8%    | ✅       | Badge                            |
| Tracks    | 8%    | ✅       | Numeric                          |
| Tags      | 16%   | ❌       | Tag badges inline                |
| Deemix    | 10%   | ❌       | Queue status + action buttons    |
| Sync      | 8%    | ❌       | Sync status badge                |
| Subscribe | 8%    | ❌       | Subscription toggle              |
| View      | 6%    | ❌       | Link to #tracks?playlistId=...   |
| Actions   | 14%   | ❌       | Edit, Delete, Create Tag buttons |

**File**: `frontend/pages/playlists.js`

---

#### 10.11.3 — TAGS (full retrofit, server-side pagination)

**Needs (backend)**:

- `GET /api/tags` — add `limit`, `offset`, `search`, `category`, `sort`, `order`, `pageSize`
- `GET /api/tags/count` — add `search`, `category` filters
- New `TagsQuery` struct
- SQL LEFT JOIN to `v_tag_file_counts` for file_count field

**Needs (frontend)**:

- Stable toolbar: search + category filter
- Sortable column headers
- Server-side paginated fetching
- `updateHash()` + init from hash

**Table columns**:

| Column   | Width | Sortable | Notes                             |
| -------- | ----- | -------- | --------------------------------- |
| Tag Name | 35%   | ✅       |                                   |
| Category | 25%   | ✅       | Badge with category icon          |
| Files    | 15%   | ❌       | File count from v_tag_file_counts |
| Created  | 15%   | ✅       | Formatted date                    |
| Actions  | 10%   | ❌       | Edit, Delete                      |

**File**: `frontend/pages/tags.js`

---

#### 10.11.4 — DEEMIX QUEUE (minor additions)

**Already has**: stable toolbar, status filter, content pattern.

**Needs**:

- Sortable column headers
- Page size selector
- `updateHash()` on state change
- Move filter + pagination to server-side
- Backend: add `sort`/`order`/`search`/`status`/`limit`/`offset` to `GET /api/services/deemix/queue`

**File**: `frontend/pages/deemix-queue.js`

---

#### 10.11.5 — TASKS (full retrofit)

**Needs**:

- Stable toolbar: search + status filter
- Sortable column headers
- `updateHash()` + init from hash
- Polling stays but re-renders only body (not toolbar)
- Backend: add `sort`/`order` to `TasksQuery`, add search support

**File**: `frontend/pages/tasks.js`

---

#### 10.11.6 — FOLDERS (full retrofit)

**Needs**:

- Stable toolbar: search input
- Table rows (not card-style render)
- Sortable column headers
- `updateHash()` + init from hash
- Keep modal-based add/edit
- Backend: add `sort`/`order`/`search`/`limit`/`offset` to `GET /api/folders`, add `GET /api/folders/count`

**File**: `frontend/pages/folders.js`

---

### 10.12 — Shared Module: `frontend/shared/crud.js`

```js
// shared/crud.js — Shared CRUD page building blocks

/**
 * Render sortable table header with current state indicator.
 */
export function sortableTh(label, column, state, opts = {}) { ... }

/**
 * Render page size selector dropdown.
 * Stored in localStorage, not in hash.
 */
export function renderPageSizeSelector(currentSize, available) { ... }

/**
 * Wire page size selector changes.
 */
export function wirePageSizeSelector(container, state, onChange) { ... }

/**
 * Wire sortable header clicks (three-state: none→asc→desc→none).
 */
export function wireSortableHeaders(tableEl, state, onChange) { ... }

/**
 * Update window.location.hash via history.replaceState.
 * Skips defaults and pageSize (which is global).
 */
export function updateHash(pageId, state, defaults) { ... }

/**
 * Initialize pageSize from localStorage with fallback.
 */
export function getPageSize(fallback = 25) {
  const saved = localStorage.getItem("crudPageSize");
  return saved ? parseInt(saved, 10) : fallback;
}
```

---

### 10.13 — Step-by-Step Execution Order

| Phase | What                                                                                  | Who                        | Dependencies  |
| ----- | ------------------------------------------------------------------------------------- | -------------------------- | ------------- | --- |
| **A** | Add `v_tag_file_counts` view to `001_initial_schema.sql`                              | Single agent               | None          | ✅  |
| **B** | Backend: sort/order/pageSize in query structs + `apply_sort` helper + update handlers | Single agent               | A             | ✅  |
| **C** | Backend: add pagination to tags + folders endpoints, TagsQuery, FoldersQuery          | Single agent               | B             | ✅  |
| **D** | Backend: add sort/order to deemix queue endpoint                                      | Single agent               | B             | ✅  |
| **E** | Create `shared/crud.js` with all helpers                                              | Single agent               | None          | ✅  |
| **F** | Add CSS for sort + page size to `style.css`                                           | Single agent               | None          | ✅  |
| **G** | **Reference page: FILES** — full retrofit                                             | **🔴 Most critical agent** | E, F, B       | ✅  |
| **H** | Retrofits for TRACKS + DEEMIX QUEUE (minor additions)                                 | Parallel agents            | G (blueprint) | ✅  |
| **I** | Retrofits for PLAYLISTS + TAGS + TASKS + FOLDERS (full)                               | Parallel agents            | G (blueprint) | ✅  |
| **J** | Remove duplicated toast/notification code                                             | Single agent (future)      | H, I          | ⏳  |
| **K** | Update ARCHITECTURE.md + DECISIONS.md                                                 | Single agent (future)      | All           | ⏳  |
| **L** | Visual upgrade: wrap toolbars in collapsible .filter-panel on all CRUD pages          | Parallel agents            | All           | ✅  |

---

## Phase 11: Traktor-like Column Customization

### 11.0 — Overview

Add full column customization to all CRUD table pages: visibility toggles,
drag-to-reorder, and drag-to-resize — persisted in localStorage per-page.

| Feature         | How It Works                                                           |
| --------------- | ---------------------------------------------------------------------- |
| **Visibility**  | "Columns" button in stats row → modal with checkbox per column         |
| **Reorder**     | Drag column headers directly in the table, OR drag in the config modal |
| **Resize**      | Drag handle on right edge of each `<th>` — updates width in real-time  |
| **Persistence** | Per-page config saved to `localStorage` key `columnConfig_{pageId}`    |
| **Reset**       | "Reset to defaults" button in the config modal                         |

---

### 11.1 — Shared Module: `frontend/shared/column-config.js`

**Exports**:

```js
/**
 * Column config system — Traktor-like table customization.
 *
 * Usage in a page module:
 *
 *   const COLUMNS = [
 *     { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 22 },
 *     { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 16 },
 *     { id: "comment", label: "Comment Diff", sortable: false, defaultWidth: 25 },
 *   ];
 *
 *   const config = loadColumnConfig("files", COLUMNS);
 *
 *   // In renderBody:
 *   const headerHtml = renderColumnHeaders(config, COLUMNS, state, sortableTh);
 *   const cellsHtml = renderColumnCells(config, COLUMNS, cellRenderers, row);
 *
 *   // After renderBody:
 *   wireColumnResize(container, "files", COLUMNS, config);
 *   wireColumnReorder(container, "files", COLUMNS, config, () => fetchAndRender(...));
 *   wireConfigButton(container, "files", COLUMNS, config, () => fetchAndRender(...));
 */
```

**Key functions**:

- `loadColumnConfig(pageId, columns)` — load from localStorage or create defaults
- `saveColumnConfig(pageId, config)` — save to localStorage
- `renderColumnConfigTrigger()` — HTML for the "Columns" button
- `renderColumnHeaders(config, columns, state, sortableTh)` — render `<th>` elements in order
- `renderColumnCells(config, columns, cellRenderers, row)` — render `<td>` elements in order
- `wireColumnResize(container, pageId, columns, config)` — drag handles on `<th>` edges
- `wireColumnReorder(container, pageId, columns, config, onSave)` — drag headers to reorder
- `wireConfigButton(container, pageId, columns, config, onSave)` — open config modal

---

### 11.2 — Column Config Schema

```js
// localStorage key: "columnConfig_files"
[
  { id: "title", visible: true, width: 22 },
  { id: "artist", visible: true, width: 16 },
  { id: "bpm", visible: true, width: 8 },
  ...
]
```

The config is a flat array. **Order in the array = display order.**
Visibility and width are the only per-column settings.
Sortability comes from the column model (static).

---

### 11.3 — Column Model (per page)

Each page defines a `COLUMNS` array:

```js
const FILES_COLUMNS = [
  { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 18 },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 6 },
  { id: "bpm", label: "BPM", sortable: true, sortKey: "bpm", defaultWidth: 8 },
  { id: "key", label: "Key", sortable: true, sortKey: "key", defaultWidth: 3 },
  { id: "linked", label: "Linked", sortable: false, defaultWidth: 2 },
  { id: "isrc", label: "ISRC", sortable: true, sortKey: "isrc", defaultWidth: 3 },
  { id: "plays", label: "Plays", sortable: true, sortKey: "play_count", defaultWidth: 3 },
  {
    id: "duration",
    label: "Duration",
    sortable: true,
    sortKey: "duration_ms",
    defaultWidth: 5,
  },
  { id: "album", label: "Album", sortable: false, defaultWidth: 5 },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 7,
  },
  {
    id: "lastPlayed",
    label: "Last Played",
    sortable: true,
    sortKey: "last_played",
    defaultWidth: 7,
  },
  { id: "comment", label: "Comment Diff", sortable: false, defaultWidth: 25 },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 12 },
];
```

---

### 11.4 — Cell Renderers (per page)

Each page provides a map of `{ columnId: (rowData) => HTML string }`:

```js
const FILES_CELL_RENDERERS = {
  title: (f) => escapeHtml(f.title),
  artist: (f) => escapeHtml(f.artist),
  bpm: (f) => `<span class="font-mono">${formatBPM(f.bpm)}</span>`,
  key: (f) => renderKeyBadge(f.key),
  linked: (f) => renderLinkBadge(f.matchedServices),
  isrc: (f) =>
    f.isrc ? `<code>${escapeHtml(f.isrc)}</code>` : '<span class="text-muted">—</span>',
  plays: (f) => `<span class="font-mono text-sm">${escapeHtml(f.playCount || 0)}</span>`,
  duration: (f) =>
    f.duration > 0
      ? `<span class="font-mono text-sm">${formatDuration(f.duration)}</span>`
      : '<span class="text-muted">—</span>',
  album: (f) => (f.album ? escapeHtml(f.album) : '<span class="text-muted">—</span>'),
  created: (f) =>
    f.createdAt ? formatTimestamp(f.createdAt) : '<span class="text-muted">—</span>',
  lastPlayed: (f) =>
    f.lastPlayed ? formatTimestamp(f.lastPlayed) : '<span class="text-muted">—</span>',
  comment: (f) => renderCommentDiff(f),
  actions: (f) => renderFileActions(f),
};
```

---

### 11.5 — Execution Plan

| Step   | What                                        | Files                                 |
| ------ | ------------------------------------------- | ------------------------------------- |
| 11.5.1 | Create shared column-config.js              | `frontend/shared/column-config.js` ✨ |
| 11.5.2 | Add CSS for resize handles + config trigger | `frontend/style.css` ✏️               |
| 11.5.3 | Retrofit FILES page (reference)             | `frontend/pages/files.js` ✏️          |
| 11.5.4 | Retrofit TRACKS page                        | `frontend/pages/tracks.js` ✏️         |
| 11.5.5 | Retrofit PLAYLISTS page                     | `frontend/pages/playlists.js` ✏️      |
| 11.5.6 | Retrofit TAGS page                          | `frontend/pages/tags.js` ✏️           |
| 11.5.7 | Retrofit TASKS page                         | `frontend/pages/tasks.js` ✏️          |
| 11.5.8 | Retrofit FOLDERS page                       | `frontend/pages/folders.js` ✏️        |
| 11.5.9 | Retrofit DEEMIX QUEUE page                  | `frontend/pages/deemix-queue.js` ✏️   |

---

### 11.6 — CSS Additions

```css
/* ─── Column resize handles ─── */
th {
  position: relative;
}
.col-resize-handle {
  position: absolute;
  right: 0;
  top: 0;
  bottom: 0;
  width: 5px;
  cursor: col-resize;
  z-index: 2;
  user-select: none;
}
.col-resize-handle:hover,
.col-resize-handle.resizing {
  background: var(--accent);
  opacity: 0.5;
}

/* ─── Column drag reorder ─── */
th.dragging {
  opacity: 0.5;
  border: 1px dashed var(--accent);
}
th.drop-target {
  border-left: 2px solid var(--accent);
}

/* ─── Column config trigger button ─── */
.col-config-trigger {
  padding: 2px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.75rem;
  cursor: pointer;
  margin-left: 8px;
}
.col-config-trigger:hover {
  border-color: var(--accent);
  color: var(--accent);
}

/* ─── Column config modal ─── */
.col-config-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 0;
  cursor: grab;
}
.col-config-item:active {
  cursor: grabbing;
}
.col-config-item.dragging {
  opacity: 0.4;
}
.col-config-drag-handle {
  cursor: grab;
  color: var(--text-muted);
  font-size: 0.85rem;
  padding: 0 4px;
}
.col-config-checkbox {
  margin: 0;
}
.col-config-label {
  flex: 1;
  font-size: 0.85rem;
}
.col-config-width {
  width: 60px;
  padding: 2px 4px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-surface);
  color: var(--text);
  font-size: 0.75rem;
  text-align: center;
  font-family: var(--font-mono);
}
.col-config-reset {
  margin-top: 12px;
  width: 100%;
}
```

---

### 11.7 — Key Behaviours

**Resize**:

- Drag handle appears as a 5px vertical strip on the right edge of each `<th>`
- On hover, the handle highlights with accent color
- Drag left/right to change width (min 30px, max 500px)
- Width is stored in localStorage as percentage
- On mouseup, save to localStorage and re-render headers

**Reorder (header drag)**:

- Drag a `<th>` to a new position between other headers
- Visual drop indicator shows where the column will land
- On drop, reorder the config array and save to localStorage
- Then re-render the table

**Config modal**:

- Triggered by a "Columns" button in the stats row
- Shows all columns (visible + hidden) in current order
- Checkbox to toggle visibility
- Drag handle to reorder within the modal
- Width input (number) per column
- "Reset to defaults" button
- "Close" button saves + re-renders

---

### 11.8 — Implementation Order

Build shared module + reference page first, then parallel agents for the rest.

| Phase | What                                        | Who             |
| ----- | ------------------------------------------- | --------------- |
| **A** | Create `shared/column-config.js` + CSS      | Single agent    |
| **B** | Retrofit FILES (reference)                  | Agent           |
| **C** | Retrofit TRACKS + DEEMIX QUEUE              | Parallel agents |
| **D** | Retrofit PLAYLISTS + TAGS + TASKS + FOLDERS | Parallel agents |

### 10.14 — Key UX/UI Details

**Stable toolbar focus preservation**:

- `renderToolbar()` is called ONCE in `init()` via `container.innerHTML = toolbarHtml + contentWrap`
- Only `contentWrap` gets replaced on re-render (via `document.getElementById`)
- `wireSearchFilter` from `search-filter.js` already handles focus restoration

**Page size selector placement**:

- Lives in the stats row (right-aligned), not the toolbar
- Re-renders with the body so it reflects current state
- Available sizes: 10, 25, 50, 100

**Sort indicator priority**:

- At rest: show neutral `fa-sort` icon on all sortable columns
- Active asc: show `fa-sort-up` (filled)
- Active desc: show `fa-sort-down` (filled)
- Hide the inactive icon direction to keep it clean

**Hash update timing**:

- After any filter change (search, sort, page, BPM, key, etc.)
- Before `fetchAndRender()` resolves — so hash is already correct when data arrives

**Empty states**:

- Zero results matching filters: show stats row with `0 files` + "No results match your filters" in table body
- Zero data in DB at all: same but with suggestion to add data first time

**Loading states**:

- Content area shows spinner while fetching
- Toolbar stays fully interactive (preserves search input, filters)
- Previous data is replaced on fetch start (don't keep stale data visible)
