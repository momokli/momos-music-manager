# Changelog

All notable changes to Momo's Music Manager.

---

## [0.3.2] — 2026-05-22

### Changed

- **Rate-Limit Retry (Phase 2)**: Extracted shared `extract_retry_after_secs` into `src/spotify/retry.rs`, used by sync worker, subscription poller, and global poller. Fixed global poller's broken string-parsing implementation that mistook HTTP status `429` for 429 seconds of backoff.

### Fixed

- **Subscription poller 429 loop**: Added retry logic (3 attempts, proper `Retry-After` backoff) for `get_playlist` and `get_playlist_tracks` calls. Spotify client now reused across subscriptions in a cycle (eliminates unnecessary token refresh calls). 300ms inter-subscription delay prevents burst traffic.
- **Global poller 429 loop**: Added retry logic (3 attempts) for `get_user_playlists` call. Uses proper header-parsing instead of string-scraping.

### Migration Notes

- Migrations 006 and 007 consolidated into a single `006_local_service.sql` (includes `snapshot_id` column for global poller). If upgrading from 0.3.1, delete `app.db` and re-run.

---

## [0.3.0] — 2026-05-21

### Added

- **Digging Multi-Seed Engine**: New `POST /api/digging/suggest` endpoint with BPM outlier detection, Camelot compatibility, ISRC dedup, and scored suggestions. Audio streaming via `GET /api/files/{id}/stream` with Range header support.
- **Digging Frontend** (`#digging` page): Split-panel workflow with tag-based seed selection, scored suggestion cards, embedded `<audio>` player with waveform visualization, staging area, and key coverage indicator.
- **Local Playlists**: `service='local'` playlists persist digging sessions without Spotify API calls. Automatic Setlist tag creation via `v_tag_playlist`. New `POST /api/playlists/local` endpoint.
- **Global Playlist Poller**: Background task that checks ALL Spotify playlists via snapshot-based change detection (default 15min interval). Auto-discovers new playlists, detects deleted ones.
- **Auto-Deemix Subscriptions**: Subscription poller automatically triggers deemix downloads on first poll and when new tracks are found.

### Changed

- Consolidated migrations 006 + 007 into `006_local_service.sql`.

---

## [0.2.0] — 2026-05-20

### Added

- **Tracks Playlist Filter**: New playlist typeahead + chips on the Tracks page (LEFT column, between Tags and Date). Type a playlist name, get suggestions from `/api/playlists`, click to add chips, and the track list filters to tracks belonging to any selected playlist (OR logic). Multiple playlists supported. Case-insensitive matching.

- **Incremental Folder Scan**: New `ScanMode` enum (Full/Incremental). Incremental mode checks file mtimes and skips unchanged files. FolderWatcher now auto-starts in `serve()` with a 5-minute polling interval. Folders page has two scan buttons: Quick Scan (⚡ incremental, new/changed files only) and Full Rescan (🔄 reprocess all).

- **New Playlists Sync**: `SyncType::NewPlaylists` — fetches the full playlist list from Spotify, diffs against existing DB entries, and only syncs metadata + tracks for playlists that don't yet exist. "Sync New" button on the Playlists page.

- **Playlist Stale Filter**: New "Stale" toggle on the Playlists page filter panel — shows only playlists where `localTrackCount ≠ remoteTrackCount`.

- **Remote Track Count Tracking**: Remote track counts are now updated during playlist-list sync (from `SimplifiedPlaylist.tracks.total`) and during subscription polling (after streaming all tracks). Keeps the playlist page stats accurate.

- **Playlist Category ID Filtering**: Category filter on Playlists page switched from prefix letters (`p,m,v,e,s`) to category IDs. Backed by new `v_playlist_tag_category` view (migration 005). Category buttons are rendered dynamically from the tag-categories table.

- **App Version Embedding**: Version from `Cargo.toml` is now displayed in the CLI (`--version`), available via `GET /api/version`, and shown as a subtle `v0.2.0` badge in the web UI's top navigation bar.

- **Tag Parent Resolution**: Setlist-category tags can now have "parent" tags that replace them in file comments. A long Setlist tag like `Dark Techno/2026/Hardtechno/...` resolves to shorter parent tags (`dark`, `techno`, `hard`) with their own categories (Mood, Vibe, Merkmal). Comments use parent tag names and categories instead of the original. Backed by new `tag_parents` table + `v_resolved_tags` / `v_file_resolved_tags` views.

- **Tag Curation Page** (`#tag-curation`): A dedicated curation workflow page for going through Setlist tags and assigning parent tags. Features a sequential workflow (prev/next with keyboard shortcuts), tag card with metadata, parent chip editor with typeahead search, inline "Create & Add" flow (category picker → create → add as parent), and a browsable mini table. Auto-save persists changes immediately.

- **Import/Export Web UI** (`#data` page): Database dump and restore via the web UI. Export downloads the full DB as JSON. Import uploads a JSON dump with preview (row counts per table, timestamp) before confirming the destructive restore. Backend endpoints: `GET /api/dump`, `POST /api/restore?confirm=true`.

- **Tracks Bulk Write Comments**: Multi-select checkboxes on the Tracks page with an ACTIONS panel "WRITE COMMENTS (X)" button. X counts how many selected tracks have linked files with outdated comments. Backend endpoints: `POST /api/tracks/needs-comment-count`, `POST /api/tracks/write-comments`.

- **Files Bulk Write Comments**: Same checkbox-selection + bulk WRITE COMMENTS pattern on the Files page. Backend endpoints: `POST /api/files/needs-comment-count`, `POST /api/files/write-comments-by-ids`.

- **Spotify Rate-Limit Retry**: The Spotify sync worker now parses the `Retry-After` header from 429 responses and retries with backoff (up to 3 attempts). A 300ms delay between successful playlist syncs helps stay under Spotify's soft rate limit (~3 req/s).

- **Playlist Sync Tracking** (migration `002_playlist_fetch_tracking.sql`): The "Sync Stale" operation now catches all local != remote mismatches (missing tracks, extra tracks, missing playlists), not just the stale ones.

- **Server-Side Filtering**: Client-side filters on Tracks and Files pages moved to server-side. Tracks: `services`, `fileTypes`, `fileTypeAgg` params. Files: PMV, file type, and comment-status filters. Fixes broken pagination when filters were active.

- **Tags Page Filter Box**: Retrofitted the Tags page toolbar into the canonical 2-column filter-panel pattern (category multi-select buttons + search + New Tag button) matching Files/Playlists pages.

- **Dropdown Navigation**: Top nav now uses dropdown menus for Tools and Library sections, reducing horizontal clutter.

- **Actions Panel** (`shared/actions-panel.js`): Generic, reusable actions panel with configurable buttons and selection count badge, shared across Tracks and Files pages.

### Changed

- **Column Resize**: Switched from percentage-based to pixel-based column sizing (30–500px range). New `columnConfig_v2_` localStorage key avoids stale percentage data. Eliminates the resize feedback loop.

- **Playlist Filter UI**: Fixed filter button state synchronization — clicking service/PMV buttons now correctly toggles the active CSS class. PMV filter refactored to use tag category lookups server-side.

### Fixed

- Playlist service and PMV filter buttons now properly sync their visual state after clicks.
- Column resize no longer enters a feedback loop when dragging handles.
- Pagination on Tracks and Files pages works correctly when filters are active.

### Migration Notes

- **New migrations**: `002_playlist_fetch_tracking.sql` (playlist sync tracking columns + tag_parents table + resolved-tag views), `003_remote_unique_count.sql`, `004_unique_tags_nocase.sql`, `005_v_playlist_tag_category.sql` (playlist→tag→category resolution view).
- Delete old `app.db*` files and restart to run migrations from scratch.
- Column config localStorage uses `columnConfig_v2_` prefix — old percentage-based config is ignored.

---

## [0.1.0] — Initial Release

- Rust backend (Axum/SQLx/SQLite) with embedded SPA frontend (vanilla JS, ES modules).
- Local file management with BPM/Key detection and Camelot wheel key matching.
- Spotify OAuth + playlist/track sync.
- Tag system with 5 categories (Setlist, Phase, Mood, Vibe, Merkmal).
- Structured ID3 comment format `[{P}{M}{V}] tags source_id`.
- Traktor collection.nml import.
- Semantic tag embeddings for auto-categorization.
- Playlist subscriptions with 30s background polling.
- macOS Launch Agent deployment.
- Config priority: env vars > config.toml > defaults.
