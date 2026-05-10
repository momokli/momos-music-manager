# Changelog

All notable changes to Momo's Music Manager.

---

## [0.2.0] — Unreleased

### Added

- **Tag Parent Resolution**: Setlist-category tags can now have "parent" tags that replace them in file comments. A long Setlist tag like `Dark Techno/2026/Hardtechno/...` resolves to shorter parent tags (`dark`, `techno`, `hard`) with their own categories (Mood, Vibe, Merkmal). Comments use parent tag names and categories instead of the original. Backed by new `tag_parents` table + `v_resolved_tags` / `v_file_resolved_tags` views (migration `003_tag_parents.sql`).

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

- **New migrations**: `002_playlist_fetch_tracking.sql` (playlist sync tracking columns), `003_tag_parents.sql` (tag_parents table + resolved-tag views).
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
