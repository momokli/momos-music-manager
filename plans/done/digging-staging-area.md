## Plan: digging-staging-area

**Status**: done ✅
**Branch**: `feat/digging-staging-area`
**Ready for review**: yes
**Depends on**: `feat/digging-frontend`, `feat/local-playlists`
**Migration needed**: no

### Description

Add a "staging area" to the left panel of the Digging page. Users click "Add" on suggestions, which moves tracks into staging. Tracks accumulate there until the user is happy, then they can persist the entire staging area as a new local playlist (using existing `POST /api/playlists/local`). Camelot key coverage indicator shows which keys are covered.

### State additions to `frontend/pages/digging.js`

```javascript
staging: [],          // DiggingSuggestion[] — accumulated tracks
showSaveDialog: false,
playlistName: "",
```

### Key functions

| Function                    | Purpose                                           |
| --------------------------- | ------------------------------------------------- |
| `addToStaging(suggestion)`  | Move from suggestions[] to staging[]              |
| `removeFromStaging(fileId)` | Move back from staging[] to suggestions[]         |
| `renderStaging()`           | Render staging cards + key coverage + save button |
| `clearStaging()`            | Empty staging (on new tag selection)              |
| `saveStagingAsPlaylist()`   | POST /api/playlists/local → toast                 |
| `getCoveredKeys()`          | Return sorted unique Camelot keys from staging    |

### Key coverage

Show which keys (1m–12m, 1d–12d) are present in staging. Gaps visible.

### Behavior

- Clicking "Add" on a suggestion moves it from suggestions list to staging area
- "Find Similar" is now a **Refine** button when staging is non-empty:
  - Uses `seedFileIds` = all original seed file IDs + all staging file IDs
  - Returns fresh suggestions based on the expanded seed pool
  - Staging tracks are NOT removed — they persist as seeds for the next round
- "Remove" returns track from staging to suggestions
- "Save as Playlist" opens name input → POST /api/playlists/local → clears staging
- Staging cleared on new tag selection
- Staging persists across "Find Similar" / "Load More" on same tag

### Why this is powerful

```
Round 1: Collapse-capital (6 seeds) → 10 suggestions → pick 3 → staging
Round 2: 6 + 3 = 9 seeds → 10 suggestions → pick 2 more → staging
Round 3: 6 + 5 = 11 seeds → 10 suggestions → pick 1 → staging
 → Saves as "collapse-capital-v2" playlist (6 seeds + 6 staging = 12 tracks)
```

Each round brings you closer to the musical space you're exploring.

### Files: `frontend/pages/digging.js` + `frontend/style.css`

### Acceptance Criteria

- [ ] "Add" moves suggestion to staging, removes from suggestion list
- [ ] "Remove" returns track to suggestions
- [ ] Key coverage indicator shows covered Camelot keys
- [ ] "Save as Playlist" → name input → POST → success toast
- [ ] Staging cleared on new tag / new search
- [ ] Staging survives multiple "Load More" calls
- [ ] No regressions: seeds, suggestions, audio, config all still work

---

