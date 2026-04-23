# Momo's Music Manager - Comment System Documentation

## Overview

The comment system provides a structured way to store and display metadata about music files, linking them to tags derived from streaming service playlists. Comments follow a standardized format that enables both human readability and machine parsing.

## Comment Format

### Standard Format

```
[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}
```

### Components

1. **PMV Indicators** (in brackets):
   - `P`: Phase tag present
   - `M`: Mood tag present
   - `V`: Vibe tag present
   - `_`: Category not represented
   - Example: `[PMV]`, `[P__]`, `[__V]`

2. **Tags**: Space-separated tag names (lowercase)
   - Derived from playlist names via tag matching
   - Sorted by category priority (Phase > Mood > Vibe > Merkmal > Setlist)
   - Within same category, sorted alphabetically

3. **Source IDs**: Service identifiers in format:
   - `sp:xxx` - Spotify track ID
   - `sc:xxx` - SoundCloud track ID
   - `yt:xxx` - YouTube video ID
   - Multiple source IDs allowed (space-separated)

### Examples

```
[PMV] build jazzy warehouse sp:1WSF0LJGwJkYejuMtyJVuA
[P__] opener peak yt:dQw4w9WgXcQ
[__V] chill sunset sc:890123
```

## Parsing Logic (`src/comment.rs`)

The `parse_comment` function extracts structured data from comment strings:

```rust
pub struct ParsedComment {
    pub phase: char,        // 'P', 'M', 'V', or '_'
    pub mood: char,         // 'P', 'M', 'V', or '_'
    pub vibe: char,         // 'P', 'M', 'V', or '_'
    pub tags: HashSet<String>,
    pub source_ids: Vec<String>,
}
```

### Parsing Rules

1. **Empty comments**: Return empty `ParsedComment` with all `'_'` indicators
2. **Bracket detection**: Look for `[XXX]` pattern at start (3 characters)
3. **PMV validation**: Each character must be 'P', 'M', 'V', or '\_'
4. **Tag/source separation**: Split remaining content, identify source IDs by prefix
5. **Case handling**: Tags are converted to lowercase for consistency

## Tag Association Chain

Tags are associated with files through an indirect chain without junction tables:

```
File ──(isrc/service_id)──▶ ServiceTrack(s) ──(playlist_tracks)──▶ Playlist(s)
                                                                        │
                                                                   name match (COLLATE NOCASE)
                                                                        │
                                                                        ▼
                                                                    Tag(s) in `tags` table
                                                                        │
                                                                   has category_id
                                                                        │
                                                                        ▼
                                                              TagCategory (sort_order)
```

### Key Principles

- **Tags as single source of truth**: All categorization flows through the `tags` table
- **No junction tables**: File-tag relationships are computed, not stored
- **Name-based matching**: Playlist names match tag names case-insensitively
- **Category-driven**: Tag categories determine PMV indicators and sort order

## Target Comment Computation Algorithm

For each file, compute what the comment _should_ be based on current service track associations:

### Step 1: Find Matching Service Tracks

```sql
SELECT st.id FROM service_tracks st
WHERE st.isrc = ?
   OR (st.service = 'spotify' AND st.service_id = ?)
   OR (st.service = 'soundcloud' AND st.service_id = ?)
   OR (st.service = 'youtube' AND st.service_id = ?)
```

### Step 2: Find Associated Playlists

```sql
SELECT DISTINCT sp.name FROM service_playlists sp
JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
WHERE spt.track_id IN (...)
```

### Step 3: Find Matching Tags with Categories

```sql
SELECT t.name, tc.prefix, tc.sort_order
FROM tags t
JOIN tag_categories tc ON tc.id = t.category_id
WHERE t.name IN (...)
ORDER BY tc.sort_order, t.name
```

### Step 4: Determine PMV Characters

- Check if any tag belongs to `Phase` category → `P` else `_`
- Check if any tag belongs to `Mood` category → `M` else `_`
- Check if any tag belongs to `Vibe` category → `V` else `_`

### Step 5: Sort Tags

1. By category `sort_order` (Phase=0, Mood=1, Vibe=2, Merkmal=3, Setlist=4)
2. Within same category, alphabetically

### Step 6: Collect Service IDs

- `sp:xxx` if `files.spotify_id` is not null
- `sc:xxx` if `files.soundcloud_id` is not null
- `yt:xxx` if `files.youtube_id` is not null

### Step 7: Format Target Comment

```
"[{pmv}] {sorted_tags} {service_ids}"
```

## API Integration

### Extended ApiFile Struct

```rust
pub struct ApiFile {
    // ... existing fields ...
    pub comment_current: Option<String>,   // Current comment from file metadata
    pub comment_target: String,            // Computed target comment
    pub comment_needs_update: bool,        // True if current != target
}
```

### Endpoints

- `GET /api/files` - Includes comment status in list responses
- `GET /api/files/{id}` - Includes comment status in detail response
- `POST /api/files/{id}/sync-comment` - Write target comment to file (future)

## Frontend Display

### Comment Status Column

Files page shows visual diff between current and target comments:

| Status          | Display                                          |
| --------------- | ------------------------------------------------ |
| ✅ Up to date   | Green checkmark + current comment                |
| ❌ Needs update | ~~strikethrough current~~ → green target comment |

### Visual Indicators

- **Current comment**: Muted text, strikethrough + red tint if stale
- **Target comment**: Green text
- **OK status**: Green checkmark icon

## Template System

### Current Template (Hardcoded)

```
"[{prefixes}] {sorted_tags} {service_ids}"
```

### Template Tokens

- `{prefixes}`: PMV characters (e.g., "PMV", "P**", "**V")
- `{sorted_tags}`: Tags sorted by category priority then alphabetically
- `{service_ids}`: Space-separated source IDs for non-null service references

### Future Configuration

Planned template configuration options:

- Custom template string
- Include/exclude service IDs
- Custom separators
- Prefix/suffix text

## Performance Considerations

### Batch Query Optimization

For list endpoints (`GET /api/files`):

1. Fetch all files in page
2. Collect their service IDs
3. Execute single batch query for matching tracks
4. Execute single batch query for playlists
5. Execute single batch query for tags
6. Map results back to files

### Single File Optimization

For detail endpoints (`GET /api/files/{id}`):

- Direct computation with parameterized queries
- No batching needed

## Error Handling

### Edge Cases

1. **No matching tracks**: Return empty/just service IDs comment
2. **No matching tags**: Return comment with only PMV/service IDs
3. **Invalid PMV in current comment**: Use computed PMV from tags
4. **Missing service IDs**: Omit from target comment

### Validation

- PMV characters validated during parsing
- Source ID prefixes validated (`sp:`, `sc:`, `yt:`)
- Tag names normalized to lowercase

## Related Components

### `src/comment.rs`

- `parse_comment()` - Parse comment string to structured data
- `generate_comment()` - Generate comment from structured data
- `generate_target_comment()` - Generate with all service IDs
- `extract_tags_from_comment()` - Extract tags from comment

### `src/db.rs`

- `compute_target_comment()` - Core computation algorithm
- `update_file_comment()` - Update comment in database
- `write_comment_to_file()` - Write comment to file metadata via exiftool

### `src/api.rs`

- `sync_comment_handler()` - API endpoint for comment sync
- `ApiFile` struct extensions - Comment status fields

## Future Enhancements

### Phase 2 (Planned)

- **Write to file action**: Button to update file metadata
- **Template configuration**: UI for customizing comment format
- **Bulk operations**: Update comments for multiple files
- **Reverse parsing**: Extract tags from comments and create associations

### Phase 3 (Future)

- **Conflict resolution**: Handle manual vs automatic tag conflicts
- **Version history**: Track comment changes over time
- **Export/import**: Backup and restore comment configurations
- **Advanced templates**: Conditional formatting, custom fields

---

_Last Updated: 2026-04-23_  
_Related Decisions: ADR-015, ADR-022_
