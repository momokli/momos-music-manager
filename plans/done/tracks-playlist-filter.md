## Plan: tracks-playlist-filter

**Status**: done ✅
**Branch**: `feat/playlist-sync-enhancements`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Add a playlist filter to the Tracks page — a typeahead search box with chips, matching the existing Tags filter pattern. Users type a playlist name, get suggestions from `/api/playlists`, click to add playlist chips, and the track list filters to only tracks belonging to any of the selected playlists. Multiple playlists are OR'd together (tracks in ANY selected playlist).

### Current State

- Backend `TracksQuery` has `playlist_id: Option<i64>` for single-playlist scoping (used by playlist context badge, not exposed as a user filter)
- Frontend toolbar LEFT column has: Tags (typeahead + chips), Date
- No way to filter tracks by playlist name(s) from the Tracks page

### Backend Changes (`src/api.rs`)

1. **Extend `TracksQuery`**: add `playlists: Option<String>` — comma-separated playlist names

2. **Modify `get_tracks()`**: when `playlists` is set, add JOIN + IN filter:

   ```sql
   SELECT DISTINCT st.* FROM service_tracks st
   JOIN service_playlist_tracks spt ON spt.track_id = st.id
   JOIN service_playlists sp ON sp.id = spt.playlist_id
   WHERE 1=1
     AND LOWER(sp.name) IN (?,?,...)
   ```

   Use `DISTINCT` to avoid duplicates when a track belongs to multiple selected playlists.

3. **Modify `get_tracks_count()`**: same JOIN + IN filter with `COUNT(DISTINCT st.id)`.

4. **Conflict handling**: when both `playlist_id` (single) and `playlists` (multi) are set, `playlists` takes precedence (multi-select replaces single-playlist scoping). The `playlist_id` param is used by the playlist context badge — when the user adds playlist chips, the badge should be cleared on the frontend side.

### Frontend Changes (`frontend/pages/tracks.js`)

#### 1. State additions

```javascript
selectedPlaylists: [],  // array of playlist name strings
playlistEnabled: true,
```

#### 2. Hash schema additions

```javascript
selectedPlaylists: { type: "array", default: [] },
```

#### 3. Toolbar HTML (LEFT column, between Tags and Date)

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="playlist">Playlists</span>
  <div class="typeahead-wrap" style="flex:1">
    <div class="tag-search-wrap">
      <i class="fas fa-list"></i>
      <input
        type="text"
        class="input-text input-search"
        id="tracks-playlist-search"
        placeholder="filter by PLAYLIST"
        autocomplete="off"
      />
      <div class="tag-dropdown" id="tracks-playlist-dropdown"></div>
    </div>
  </div>
  <div class="tag-chips" id="tracks-playlist-chips">${playlistChipsHtml}</div>
</div>
```

#### 4. `buildParams`

```javascript
if (state.selectedPlaylists && state.selectedPlaylists.length > 0) {
  params.set("playlists", state.selectedPlaylists.join(","));
}
```

#### 5. Wire typeahead (in `wireToolbarEvents`)

Same pattern as tags typeahead (already present in the same file for `#tracks-tag-search`):

- Debounced input → `fetchJSON("/api/playlists?search=...&page_size=20")`
- Dropdown with playlist names (+ service icon? optional: service badge for clarity)
- Keyboard nav (ArrowDown/Up, Enter, Escape)
- Click outside closes dropdown
- Click item → add to `state.selectedPlaylists`, clear input, close dropdown, re-fetch

#### 6. Wire chip removal

Delegate click on `.tag-chip-x` inside `#tracks-playlist-chips` → remove from `state.selectedPlaylists`, re-fetch.

#### 7. `updateFilterUI`

Include `.tag-chips` and `.typeahead-wrap` in the disable/enable toggle for `[data-filter="playlist"]`.

#### 8. Toggle handler

Add `playlistEnabled` to the generic toggle handler (click on disabled filter row re-enables it).

#### 9. `wireContentEvents` / `updateFilterUI`

Include playlist chip container + typeahead in filter UI state syncing.

### Files to modify

- `src/api.rs` — extend `TracksQuery`, update `get_tracks()`, `get_tracks_count()`
- `frontend/pages/tracks.js` — state, hash, toolbar HTML, typeahead wiring, chips, buildParams

### Acceptance Criteria

- [x] Playlist typeahead appears in LEFT column between Tags and Date
- [x] Typing searches playlists via `/api/playlists?search=...` with debounce
- [x] Dropdown shows matching playlist names; keyboard nav works
- [x] Clicking a dropdown item adds a playlist chip and filters tracks server-side
- [x] Multiple chips supported (OR logic — tracks in any selected playlist)
- [x] Removing a chip removes the filter and refreshes
- [x] Playlist filter is toggleable (collapsible, localStorage persistence)
- [x] Pagination works correctly with playlist filter active
- [x] Count query matches filtered result count
- [x] When playlist filter is active, the single-playlist context badge is cleared
- [x] No regressions: tags, PMV, type, date, service filters still work
- [x] No regressions: sort, pagination, column config, layout mode, bulk comments
- [x] Backend compiles (`cargo build`)
- [x] Test with `curl` first

---

