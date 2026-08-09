## Plan: backpack-system

**Status**: done ✅
**Branch**: feat/backpack-system
**Depends on**: fix/local-file-tracking + Maintainer
**Migration needed**: yes — rename tags.followed to tags.backpack (migration 014)

### Description

Overhaul "follow" into proper "Backpack" system. Rename everywhere (DB column + all code + UI), make tracks inherit backpack status from their tags, add active sync task that pulls missing files from backup and cleans up redundant formats. WAV source files excluded (Ableton only, metadata from linked track).

### Part 1: Rename followed → backpack everywhere

Migration 014: ALTER TABLE tags RENAME COLUMN followed TO backpack.

Rust: Tag.followed → Tag.backpack, set_tag_followed() → set_tag_backpack(), get_followed_tags() → get_backpack_tags(), is_file_followed() → is_in_backpack(). All SQL queries updated. get_prune_candidates() WHERE t.followed = 1 → WHERE t.backpack = 1.

Frontend: Tags page "Follow" → "Backpack", icon fa-eye → fa-backpack. Labels: "files are kept locally" → "files are kept offline".

### Part 2: Track inherits backpack from tags

A track is "in backpack" if ANY of its tags has backpack = true, OR if individually in the "backpack" playlist.

Add pub in_backpack: bool to track API responses (TrackDetail, Tracks list). Tracks page shows backpack icon for in_backpack tracks.

### Part 3: Backpack Sync task

New TaskType: BackpackSync { tag_ids }. When a tag is toggled to backpack, spawns background task:

- For each track in backpack tags: find best local file (stem > FLAC > MP3)
- If missing: pull from backup
- If redundant: mark safe-to-delete (stem exists → FLAC can be pruned)
- Skips WAV source files (Ableton only)
- Ensures exactly one version per track is local

### Part 4: Backpack Page (new #backpack route)

New SPA page showing: active backpack tags with track counts, per-track file status (stem ✓ / FLAC only / needs pull / nothing available), "Sync All" and "Pull Missing" bulk buttons.

### Files to modify

- migrations/014_backpack_rename.sql — new
- src/db.rs — rename all followed→backpack, add in_backpack computation, BackpackSync logic
- src/api.rs — in_backpack on track responses, backpack sync handler
- src/tasks/mod.rs — new BackpackSync task type + worker
- frontend/pages/tags.js — Follow→Backpack UI
- frontend/pages/tracks.js — backpack icon for in_backpack tracks
- frontend/pages/backpack.js — NEW page
- frontend/app.js + shared/nav.js — register route
- frontend/style.css — backpack page styles

### Acceptance Criteria

**Part 1:**

- [ ] Migration 014 runs cleanly (001→014)
- [ ] All code references: followed → backpack
- [ ] Tags page shows "Backpack" not "Follow"
- [ ] cargo build passes

**Part 2:**

- [ ] Track detail + tracks list return inBackpack: bool
- [ ] Track inherits backpack from tags
- [ ] Tracks page shows backpack icon
- [ ] cargo build passes

**Part 3:**

- [ ] BackpackSync task spawns on tag toggle
- [ ] Pulls missing files (stem > FLAC)
- [ ] Cleans redundant formats
- [ ] WAV source files excluded
- [ ] cargo build passes

**Part 4:**

- [ ] #backpack page renders tags + track status
- [ ] Sync/Pull buttons work
- [ ] Registered in app.js + nav.js
- [ ] cargo build passes

#### Background maintenance scheduler

The current system has multiple polling mechanisms with different intervals:

- Folder watcher: polls active folders every 5 min (incremental scan)
- Global playlist poller: every 15 min
- Subscription poller: every 30s
- Auto-backup poller: every 10 min
- **No** automatic full scan or stale-local cleanup

But there's no unified "housekeeping" that:

- Triggers a full scan when incremental scans have been running too long (files may have been deleted locally)
- Automatically cleans up stale `file_locations.local` entries for files no longer on disk
- Detects unbacked-up files and triggers backup for folders with `auto_backup` enabled
- Discovers backup-only files (via `discover_backup_files`) on a schedule
- Keeps all counts (`localFileCount`, `pruneCandidateCount`) current without manual button clicks

**Idea: A `Maintainer` background task** — single loop, configurable interval (default: 1h), does:

1. **Quick health check**: For each active folder, check if `last_scanned` is > 24h old → trigger a full scan
2. **Stale local cleanup**: For each scanned folder, remove `file_locations.local` for files that disappeared (already implemented in `scan_folder`, but only runs when a scan happens — if no scan triggers, stale entries persist)
3. **Unbacked-up check**: For folders with `auto_backup = true`, check if any files lack backup → log warning (backup sync is handled by the existing auto-backup poller)
4. **Backup discovery**: On a longer interval (e.g. weekly), trigger `discover_backup_files` to sync NAS inventory

This would replace the need for a manual "Run Full Scan" button — the maintainer just runs it when needed.

#### Backup-only file metadata extraction

When `discover_backup_files` finds files on the NAS that don't exist in the local DB, it currently creates a bare record with no metadata:

```json
{ "title": null, "artist": null, "isrc": null, "bpm": null, "fileSize": 0 }
```

**What the user wants**: Extract metadata (ISRC, title, artist, BPM, file size) directly from the backup location. This enables:

- Matching to service tracks (Spotify/SoundCloud) via ISRC → track-detail shows files even if never local
- Better prune/discovery workflows — knowing "this backup-only FLAC has ISRC X"

**How to extract remotely**: Via SSH we can run `exiftool` or `ffprobe` on the NAS:

```bash
ssh backup "ffprobe -v quiet -print_format json -show_format /volume1/media/flacs/Artist - Title.flac"
```

This returns: duration, format, tags (title, artist, album, ISRC, BPM if embedded). For WAVs there might be no metadata, but for FLACs and stems there usually is.

**What would change**:

- `discover_backup_files()` accepts an optional reference to a BackupEngine (for SSH)
- For new files, after creating the DB record, SSH to NAS to extract metadata
- Update the `files` row with: `title`, `artist`, `isrc`, `file_size`, `duration_ms`
- The `file_hash` stays as sentinel `"backup-only-{size}"` (can't hash remotely easily)
- **Result**: Track-detail can now show "this track has a FLAC on backup (+ ISRC matched)" even if you've never had the file locally

**Risk**: Running `ffprobe` on 6000+ FLACs over SSH would be slow — should be batched and only for newly discovered files, not on every maintainer cycle.

**TODO**: Decide if this is part of the Maintainer or a separate "enrich backup files" task.

---

