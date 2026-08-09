## Plan: digging-filter-row

**Status**: done ✅
**Branch**: `feat/digging-filter-row`
**Ready for review**: no
**Depends on**: `feat/digging-flat-ladder`
**Migration needed**: no

### Description

Add three persistent filter rows to the browser pane (PMV, KEY, Phase) that filter server-side alongside the ladder-derived toggles. These are independent AND filters — a track must match all active groups.

### Layout

```
┌─────────────────────────────────────────────────┐
│ PMV: [P] [M] [V]  |  Full  Partial  None       │
│ KEY: 1m 2m ... 12m  |  ALL m  NONE m            │
│      1d 2d ... 12d  |  ALL d  NONE d            │
│ Phase: End Start Release Build Sustain Peak     │
├─────────────────────────────────────────────────┤
│ ☑ ⚡Energy 1-4  ☑ 🔑Ladder keys  ☐ BPM  ☑ Tags │
└─────────────────────────────────────────────────┘
```

### Filter details

| Row   | Behavior                                                                                                                     | Backend param                        |
| ----- | ---------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| PMV   | Multi-select P/M/V + single-select Full/Partial/None. Picking category clears aggregate, picking aggregate clears categories | NEW: `pmvCategories`, `pmvAggregate` |
| KEY   | 24 toggle buttons. ALL m = select all minor. ALL/NONE per mode                                                               | Existing: `keyList`                  |
| Phase | 6 multi-select buttons. Adds phase tag names to OR tag filter                                                                | Existing: `tags`                     |

### Updated ladder-derived energy

Energy now uses range ±1 from ALL ladder tracks' energy levels:

```
Ladder: Start(⚡1) + Build(⚡4) + Release(⚡2)
→ 1±1 = 1,2,3; 4±1 = 3,4,5; 2±1 = 1,2,3
→ union: 1,2,3,4,5
→ energyLevels=1,2,3,4,5
```

### Backend: `src/digging.rs`

Add to `DiggingTracksQuery`:

```rust
pub pmv_categories: Option<String>,  // comma P,M,V
pub pmv_aggregate: Option<String>,   // "full", "partial", "none"
```

Add to `search_digging_tracks`:

- Parse `pmv_categories` into `Vec<String>`
- PMV category filter (OR): EXISTS subquery joining v_file_tags → tag_categories.prefix IN (...)
- PMV aggregate full (AND): 3 EXISTS subqueries for p, m, v prefixes
- PMV aggregate partial (OR): same as categories with all three
- PMV aggregate none (NOT): NOT EXISTS subquery for any PMV prefix

### Frontend: `frontend/pages/digging.js`

Add three filter rows above the existing toggle bar in `renderFilterBar()`. Update `loadTracks()` to send new params and compute energy range ±1.

### Acceptance Criteria

- [ ] P, M, V buttons multi-select; clicking toggles active
- [ ] Full/Partial/None mutually exclusive, clear categories on select
- [ ] KEY: all 24 buttons toggleable, ALL/NONE per mode work
- [ ] Phase: 6 buttons append Phase tag names to tags param
- [ ] Energy: ladder-derived now uses ±1 range from each track's energy (union)
- [ ] Filters compose: PMV AND key AND phase AND ladder-energy AND ladder-tags
- [ ] Backend compiles

---

