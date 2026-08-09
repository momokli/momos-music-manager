## Plan: digging-enrichment

**Status**: done ✅
**Branch**: `feat/digging-enrichment`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Enrich the Digging track browser with play count, rating, last played from linked files. Add server-side sorting. Add absolute BPM filter. Auto-load tracks on open.

### Backend: `src/digging.rs`

#### 1. Add play_count, rating, last_played to DiggingTrackResult

```rust
pub play_count: i32,
pub rating: i32,
pub last_played: Option<i64>,
```

#### 2. Add sort params to DiggingTracksQuery

```rust
pub sort_by: Option<String>,   // "relevance","playCount","rating","bpm","energy","lastPlayed","tagCount"
pub sort_order: Option<String>, // "asc" or "desc"
```

Default sort when no filters: `rating desc → playCount desc`. With filters: `fileMatchCount desc → then rating desc`.

#### 3. Update TrackDiggingRow + SQL

Add subqueries for play_count, rating, last_played from linked files (MAX aggregate). Add tag_category_count computation in Rust.

### Frontend: `frontend/pages/digging.js`

- Add ▶7 plays, ★4 rating, "3d ago" badges to track cards
- Add sort dropdown (Relevance, Plays, Rating, BPM, Energy, Tags) + ↑/↓ toggle
- Add BPM from/to number inputs (absolute filter, independent of ladder)
- Auto-load tracks on page open

### Files to modify

- `src/digging.rs`
- `frontend/pages/digging.js`
- `frontend/style.css`

### Acceptance Criteria

- [ ] `playCount`, `rating`, `lastPlayed` in API response
- [ ] Sort by playCount/rating/bpm/energy/tagCount all work
- [ ] Default sort (empty page): rating desc, playCount desc
- [ ] Card badges: plays, rating stars, last played
- [ ] Sort dropdown + direction toggle in filter bar
- [ ] BPM from/to inputs work independently
- [ ] Auto-load on page open
- [ ] Backend compiles

---

