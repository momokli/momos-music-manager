# Momo's Music Manager — Architecture Overview

> Rust backend (Axum/SQLx/SQLite) + modular vanilla JS SPA frontend.

---

## Tech Stack

### Backend

| Layer          | Choice                                 |
| -------------- | -------------------------------------- |
| Runtime        | Rust (2024 edition) + Tokio            |
| Web Framework  | Axum 0.8.x                             |
| Database       | SQLite 3.x via SQLx                    |
| Auth           | OAuth 2.0 (Spotify)                    |
| Logging        | `tracing` crate                        |
| Audio Metadata | `lofty` (Rust) + `exiftool` (fallback) |
| ML             | `candle` — all-MiniLM-L6-v2 embeddings |

### Frontend (Modular SPA)

- **Vanilla JS with ES modules** in `frontend/`
- **Hash-based router** in `app.js` — listens on `hashchange`, dynamically imports page modules
- **`pages/`** — one module per route (dashboard, files, tracks, playlists, tags, tag-categories, services, folders, tasks, auto-categorize, traktor-import)
- **`shared/`** — reusable modules (api.js, components.js, format.js, nav.js, search-filter.js)
- **No build tool, no framework**
- Frontend is embedded in the Rust binary via `rust-embed`; no separate dev server needed
- Backend communication via `fetch()` + REST API
- **Standalone page**: `digging.html` (not part of SPA)

---

## Database Schema

**Single Migration**: `migrations/001_initial_schema.sql`

### Tables (12)

| Table                     | Purpose                                 | BPM/Key? |
| ------------------------- | --------------------------------------- | -------- |
| `tag_categories`          | Setlist, Phase, Mood, Vibe, Merkmal     | —        |
| `tags`                    | Tag catalog, UNIQUE name                | —        |
| `service_tracks`          | Tracks from Spotify/SoundCloud/YouTube  | ❌ No    |
| `service_playlists`       | Playlists from services                 | —        |
| `service_playlist_tracks` | Many-to-many playlist ↔ track           | —        |
| `files`                   | **Local files** with all metadata       | ✅ Yes   |
| `service_config`          | OAuth tokens (access, refresh, expiry)  | —        |
| `folders`                 | Watched folders with scan config        | —        |
| `subscriptions`           | Playlist subscriptions for polling      | —        |
| `tag_embeddings`          | ML embedding vectors for tags           | —        |
| `tag_energy_levels`       | Energy level assignments for Phase tags | —        |
| `tag_similarities`        | Precomputed tag similarity scores       | —        |

### Views (7)

| View                     | Purpose                                     |
| ------------------------ | ------------------------------------------- |
| `unified_tracks`         | Union of service tracks and local files     |
| `v_file_track_link`      | Links local files to matched service tracks |
| `v_tag_playlist`         | Tags matched to playlists by name           |
| `v_file_tags`            | Tags matched to files via playlists         |
| `v_subscriptions`        | Active subscriptions with poll status       |
| `v_tag_categories`       | Tag categories with tag counts              |
| `v_tags_with_categories` | Tags joined with their category info        |

### Patterns

- **File vs ServiceTrack**: Two separate types. `files` have BPM/Key, `service_tracks` don't. No junction tables.
- **Service IDs directly on `files`**: `spotify_id`, `soundcloud_id`, `youtube_id` as direct columns (no matches table).
- **Tags = Playlists**: Tags linked to playlists via case-insensitive name matching. Setlist is the default category.
- **Sync State in-memory**: No sync fields in DB — everything via `TaskManager` in RAM.
- **Comment stored on file metadata**: Written via exiftool into the file's comment tag.

---

## Configuration

**Priority** (highest wins):

1. Environment variables (`.env` file or shell exports)
2. `~/.config/momos-music-manager/config.toml`
3. Built-in defaults

### Config.toml format

```toml
[database]
# Default: sqlite:~/.local/share/momos-music-manager/library.db
# Override with DATABASE_URL env var
url = "sqlite:~/.local/share/momos-music-manager/library.db"

[server]
# Override with SERVER_HOST env var
host = "127.0.0.1"
# Override with SERVER_PORT env var
port = 3000
# Override with PUBLIC_URL env var or --public-url CLI flag
# public_url = "https://mmm.mydomain.de"

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

### Dev-only env vars (not in config.toml)

- `DATABASE_URL` — overrides `[database].url` from config.toml; defaults to `sqlite:~/.local/share/momos-music-manager/library.db`

---

## API Structure

All endpoints under `/api/`. No version prefix.

### Files

```
GET    /api/files                    # Paginated list (50/page) with comment status
GET    /api/files/count              # Total count
GET    /api/files/{id}               # Detail with comment_target/comment_current
POST   /api/files/{id}/sync-comment  # WriteComment task for single file
POST   /api/files/bulk-sync          # Bulk WriteComment task
```

### Tracks (Service tracks)

```
GET    /api/tracks                    # Paginated list
GET    /api/tracks/count              # Total count
GET    /api/tracks/{id}               # Detail
```

### Playlists + Tags

```
GET    /api/playlists                          # All playlists (supports service, search, untagged, mismatch filters)
GET    /api/playlists/{id}/tracks              # Tracks in a playlist
GET/POST /api/playlists/subscriptions          # List / create subscription
DELETE /api/playlists/subscriptions/{id}       # Delete subscription

GET/POST   /api/tags                            # List / create
PUT/DELETE /api/tags/{id}                       # Update / delete

GET/POST   /api/tag-categories                  # List / create
PUT/DELETE /api/tag-categories/{id}             # Update / delete

GET    /api/tags/from-playlists                 # Wizard: playlists without tags
POST   /api/tags/create-from-playlists          # Create tags from playlist names
```

### Services (OAuth + Sync)

```
GET  /api/services                               # Status of all services
POST /api/services/{service}/auth                # Start OAuth flow
GET  /api/services/{service}/callback            # OAuth callback handler
POST /api/services/{service}/sync                # Trigger sync
GET  /api/services/{service}/sync/{task_id}      # Sync task status
```

### Folders

```
GET/POST   /api/folders                          # List / create
GET/PUT/DELETE /api/folders/{id}                 # Get / update / delete
POST /api/folders/{id}/watch                     # Toggle active
POST /api/folders/{id}/scan                      # Trigger scan → { "taskId": "uuid" }
```

### Tasks

```
GET    /api/tasks                   # Paginated list (with percent + subItems)
GET    /api/tasks/{id}              # Single task detail
DELETE /api/tasks/{id}              # Cancel a running task
```

### Tags (AI categorization)

```
GET    /api/tags/unreviewed                     # Tags pending review
POST   /api/tags/{id}/categorize                # Assign category
POST   /api/tags/bulk-review                    # Accept/reject AI suggestions
POST   /api/tags/bulk-update                    # Bulk category changes
POST   /api/tags/bulk-check                     # Check which tags have categories
GET    /api/tags/embeddings                     # All embeddings
GET    /api/tags/embeddings/category/{id}       # Embeddings for a category
POST   /api/tags/embeddings/recompute           # Recompute all embeddings
```

### Digging (Curator)

```
GET    /api/digging/sessions                    # List digging sessions
POST   /api/digging/sessions                    # Create new session
POST   /api/digging/chain                       # Build session chain
GET    /api/digging/energy-levels               # Tag energy levels
POST   /api/digging/energy-levels               # Set energy level
DELETE /api/digging/energy-levels/{id}          # Delete energy level
POST   /api/digging/reorder-tags                # Batch reorder tags
```

### Health

```
GET    /api/health
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
| (standalone)       | `digging.html`                      | Curator/session-builder page       |

### Shared modules

| Module                             | Purpose                         |
| ---------------------------------- | ------------------------------- |
| `frontend/shared/api.js`           | `API_BASE`, `fetchJSON()`       |
| `frontend/shared/components.js`    | Loading, empty, error, paginate |
| `frontend/shared/format.js`        | Format date, duration, BPM      |
| `frontend/shared/nav.js`           | Sidebar navigation builder      |
| `frontend/shared/search-filter.js` | Generic search/filter UI        |

---

## Comment System

### Format

```
[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}
```

Example: `[PMV] build jazzy warehouse sp:1WSF0LJGwJkYejuMtyJVuA`

- **PMV**: Phase/Mood/Vibe — `P`, `M`, `V` or `_` when no tag of that category
- **Tags**: Space-separated, sorted by category priority
- **Source IDs**: `sp:xxx`, `sc:xxx`, `yt:xxx`

### Target Comment Computation

For each file, the system computes what the comment **should be**:

1. Match service tracks via ISRC + service IDs
2. Find playlists of those tracks
3. Find tags via name matching (case-insensitive)
4. Derive PMV chars from tag categories
5. Sort tags (Phase > Mood > Vibe > Merkmal > Setlist)
6. Collect service IDs from `files.spotify_id` / `soundcloud_id` / `youtube_id`
7. Format → Target Comment

The API returns `comment_current`, `comment_target`, `comment_needs_update`.

---

## Task Manager

In-memory task tracking with background workers.

**Task types:**

| Type                                 | Description                          |
| ------------------------------------ | ------------------------------------ |
| `ServiceSync { service, operation }` | Sync playlists/tracks from a service |
| `WriteComment { file_ids }`          | Write target comments to file tags   |
| `RecomputeEmbeddings`                | Recompute ML embeddings for all tags |
| `ScanFolder { folder_id }`           | Scan a folder for audio files        |

**Conflict prevention:** One task per type at a time (e.g. one Spotify sync, one scan per folder).

- Tasks reside in RAM, auto-pruned 5 min after completion
- Pruner runs every 60 seconds in the background
- Progress: `percent` (0–100) + `sub_items` for detail

---

## Background Services

### Subscription Poller (`poller.rs`)

- Runs in a background tokio task
- Every 30 seconds checks due playlist subscriptions
- Fetches tracks via Spotify API, upserts new ones
- Can be cancelled via `CancellationToken`

### Folder Watcher (`watch.rs`)

- Optional — not auto-started
- Polls active folders every 5 minutes for new/changed files
- Configurable scan interval

---

## Key Matching (Rust-only)

Camelot Wheel — **no DB table**.

- Only number counts (±1, wrapping 12↔1)
- A/B (Major/Minor) ignored
- Implemented in `src/digging.rs`

---

## Source Directory

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

## DJ Workflow

```
Spotify URL → Download tool → FLACs → NUO-STEMS → STEMs → Taggen → Traktor
```

1. **Download**: Playlist URL into download queue
2. **Convert**: NUO-STEMS converts FLACs into STEM files
3. **Import**: Backend scans folder, extracts metadata (BPM, key, comment)
4. **Tag**: Comment system writes playlist info into ID3 comment (via exiftool)
5. **Sync**: Spotify OAuth sync fetches playlists → tags → comments
6. **Traktor**: Import collection.nml → updates play counts / last played

---

## Dev Commands

```bash
# Start backend (also serves frontend via rust-embed)
cargo run -- serve --host 127.0.0.1 --port 3000

# Or with a public URL for OAuth callbacks
cargo run -- serve --host 127.0.0.1 --port 3000 --public-url https://mmm.mydomain.de

# Scan single file for metadata debugging
cargo run -- scan-file /path/to/file.stem.m4a

# DB dump/restore (save before deleting app.db)
cargo run -- dump
cargo run -- restore

# Launch agent (macOS only)
cargo run -- install-launch-agent
cargo run -- uninstall-launch-agent
cargo run -- service-status

```

---

## Not Yet Implemented

- SoundCloud OAuth (framework ready, flow not wired)
- YouTube OAuth (framework ready, flow not wired)
- Docker Compose (to be recreated later)
- Advanced harmonic matching (relative keys, extended intervals)
- Preset management
- Explorer/curator preset system (endpoints defined, handlers stubbed)
- WebSocket support for real-time task updates (endpoint stubbed)

---

_Last Updated: 2026-05-01_
