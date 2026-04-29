# Momo's Music Manager — Agent Guidance

## Project Context

Music library management for DJs. Rust backend (Axum/SQLx/SQLite) + modular SPA frontend (vanilla JS, ES modules).
Single developer, no production data, no backward compatibility needed.

## Key Principles

1. **Schema**: 9 tables — `tag_categories`, `tags`, `service_tracks`, `service_playlists`, `service_playlist_tracks`, `files`, `service_config`, `folders`, `subscriptions`
2. **Single Migration**: Only `migrations/001_initial_schema.sql` — replace it and delete all DB files if schema changes
3. **Separate Types**: `File` (local files with BPM/Key) vs `ServiceTrack` (service entries, no BPM/Key) — no junction tables
4. **Tags = Playlists**: Via name matching (case-insensitive). Setlist is default category.
5. **Comment Format**: `[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}` — e.g. `[PMV] build jazzy warehouse sp:xxx`
6. **Service IDs**: Direct columns on `files` (spotify_id, soundcloud_id, youtube_id)
7. **Key Matching**: Rust-only (Camelot wheel, no DB table)
8. **Task Manager**: In-memory task tracking — 4 operation types (ServiceSync, WriteComment, RecomputeEmbeddings, ScanFolder)
9. **Sync State**: In-memory `TaskManager` — tasks auto-pruned 5 min after completion
10. **Config Priority** (highest wins): Env vars > `~/.config/momos-music-manager/config.toml` > built-in defaults

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
This is useful for quick dev switching.

Dev-only env vars (not in config.toml):

- `DATABASE_URL` — default `sqlite:app.db`
- `SPOTIFY_API_CACHE` — `record`/`replay` for dev
- `SCAN_CACHE` — `record`/`replay` for dev

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

## Important Gotchas

- **Before testing**: Always delete old DB files (`app.db`, `compile_check.db`, `test.db`)
- **If you see "migration 27" errors**: DELETE ALL DB files and start fresh
- **No SoundCloud/YouTube OAuth yet** — framework is ready, actual flow not implemented
- **Frontend is an SPA** — modular vanilla JS with ES modules in `frontend/`. Hash-based router (`app.js`), shared modules in `shared/`, pages in `pages/`. Serve with `python3 -m http.server` (no `file://`).
- **Docker** was removed — will be recreated later. Use `cargo run` for now.
- **digging.html** is a standalone HTML page (not part of the SPA) for the digging/curation workflow
- **Playlist subscriptions** poll every 30s in the background — managed in `poller.rs`

## Tag Categories (Defaults)

| Category | Prefix | Icon      | Sort        |
| -------- | ------ | --------- | ----------- |
| Setlist  | (none) | ListMusic | 0 (default) |
| Phase    | P      | Activity  | 1           |
| Mood     | M      | Heart     | 2           |
| Vibe     | V      | Sparkles  | 3           |
| Merkmal  | (none) | Hash      | 4           |

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

## Docs

- `docs/ARCHITECTURE.md` — System design
- `docs/DECISIONS.md` — ADRs
- `docs/COMMENT_SYSTEM.md` — Comment format spec
- `docs/TASK_MANAGER.md` — Task manager details
- `docs/FRONTEND_BUILD_PLAN.md` — SPA migration history
- `docs/FRONTEND_NEXT_PLAN.md` — Remaining frontend work

## Handover

1. Document progress and decisions in `docs/DECISIONS.md`
2. Leave TODO comments in code
3. Ensure backend compiles (`cargo build`) before handing over
4. Test with `curl` commands first, then frontend
