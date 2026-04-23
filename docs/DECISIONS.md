# Momo's Music Manager — Architectural Decision Records

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

**Decision**: Use the TaskManager for WriteComment operations. The API returns a `task_id` immediately (202 Accepted pattern), and the frontend polls for completion.

**Consequences**:

- `POST /api/files/{id}/write-comment` — returns task_id
- `POST /api/files/write-comments` — batch write all files needing update
- Frontend shows spinner while task is running
- File-level granularity — errors in one file don't stop the batch
- Edge cases handled: already-up-to-date skip, missing file error, DB-update-after-write warning

---

## Revision History

| Date       | Decision                              | Description                                                                    |
| ---------- | ------------------------------------- | ------------------------------------------------------------------------------ |
| Initial    | ADR-001, 003, 004, 006, 011, 013, 014 | Core architecture decisions                                                    |
| 2026-04-17 | ADR-015                               | Structured comment format                                                      |
| 2026-04-19 | ADR-016, 017, 018                     | Service tracks API, POC strategy, Folder CRUD                                  |
| 2026-04-20 | ADR-019                               | Folder scanning configuration                                                  |
| 2026-04-23 | ADR-022                               | Target comment computation                                                     |
| 2026-04-24 | ADR-023, 024                          | TaskManager, WriteComment                                                      |
| 2026-04-25 | —                                     | Cleanup: removed outdated ADRs (React, Docker, design.html, presets, bugfixes) |
