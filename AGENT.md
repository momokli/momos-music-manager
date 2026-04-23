# Momo's Music Manager — Agent Guidance

## Project Context

Music library management for DJs. Rust backend (Axum/SQLx/SQLite) + simple HTML/JS frontend (POC phase).
Single developer, no production data, no backward compatibility needed.

## Key Principles

1. **Schema**: 9 tables — `tag_categories`, `tags`, `service_tracks`, `service_playlists`, `service_playlist_tracks`, `files`, `service_config`, `folders`, ~~`explorer_presets`~~ (removed)
2. **Single Migration**: Only `migrations/001_initial_schema.sql` — replace it and delete all DB files if schema changes
3. **Separate Types**: `File` (local files with BPM/Key) vs `ServiceTrack` (service entries, no BPM/Key) — no junction tables
4. **Tags = Playlists**: Via name matching (case-insensitive). Setlist is default category.
5. **Comment Format**: `[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}` — e.g. `[PMV] build jazzy warehouse sp:xxx`
6. **Service IDs**: Direct columns on `files` (spotify_id, soundcloud_id, youtube_id)
7. **Key Matching**: Rust-only (Camelot wheel, no DB table)
8. **Task Manager**: In-memory task tracking (SyncManager refactored), no DB sync fields
9. **Sync State**: In-memory `SyncManager` — 4 operation types (playlists, tracks, single playlist, full)
10. **.env Only**: Service credentials in `.env` file, never in DB or UI

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
```

## Important Gotchas

- **Before testing**: Always delete old DB files (`app.db`, `compile_check.db`, `test.db`)
- **If you see "migration 27" errors**: DELETE ALL DB files and start fresh
- **No SoundCloud/YouTube OAuth yet** — framework is ready, actual flow not implemented
- **Frontend is POC** — pure HTML/JS in `frontend/`. React (`frontend_react/`) is gone.
- **Docker** was removed — will be recreated later. Use `cargo run` for now.

## Tag Categories (Defaults)

| Category | Prefix | Icon      | Sort        |
| -------- | ------ | --------- | ----------- |
| Setlist  | (none) | ListMusic | 0 (default) |
| Phase    | P      | Activity  | 1           |
| Mood     | M      | Heart     | 2           |
| Vibe     | V      | Sparkles  | 3           |
| Merkmal  | (none) | Hash      | 4           |

## Docs

- `docs/ARCHITECTURE.md` — System design
- `docs/DECISIONS.md` — ADRs
- `docs/COMMENT_SYSTEM.md` — Comment format spec
- `docs/TASK_MANAGER.md` — Task manager details
- `docs/schema.sql` — Full DB schema reference

## Handover

1. Document progress and decisions in `docs/DECISIONS.md`
2. Leave TODO comments in code
3. Ensure backend compiles (`cargo build`) before handing over
4. Test with `curl` commands first, then frontend
