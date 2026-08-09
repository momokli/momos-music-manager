## Plan: digging-flat-ladder

**Status**: done ✅
**Branch**: `feat/digging-flat-ladder`
**Ready for review**: no
**Depends on**: `feat/digging-enrichment`
**Migration needed**: no

### Description

Redesign the Digging page: swap panes (browser left, ladder right), remove energy curve/steps concept, make the ladder a flat ordered list of identical track cards. Filters derive from ALL ladder tracks (not selected steps). Add drag-to-reorder, session persistence. Unified card design used identically in both panes.

### Layout

```
┌──────────────────────────────┬───────────────────────────────┐
│ BROWSER (left, 55%)          │ LADDER (right, 45%)           │
│                              │                               │
│ [search]  sort: [▾] ↑↓     │ #1 ██ Full track card         │
│ BPM from/to inputs           │    with waveform, play, ×    │
│                              │                               │
│ Filters (all toggleable):    │ #2 ██ Full track card         │
│ ☑ ⚡Energy 1-4              │    ...                        │
│ ☐ 🔑Keys (±1▾)             │                               │
│ ☐ 🎵BPM (±5▾)              │ #3 ██ Full track card         │
│ ☑ 🏷️Tags + chips           │    ...                        │
│                              │                               │
│ Track cards (paginated)      │ Computed from ladder:         │
│ ┌──────────────────────────┐│ BPM: 119-133 · Keys: 4m,5m   │
│ │ ⠿ Title · Artist         ││ Tags: deep, dark, house      │
│ │ 122BPM·4m·⚡3.2·▶7·★3  ││                              │
│ │ tags: deep dark house    ││ [Save Session] [Load]        │
│ │ [▶────waveform────]     ││ [Save as Playlist]           │
│ │ FLAC ✓ STEM ✓            ││                              │
│ └──────────────────────────┘│                               │
│                              │                               │
│ [Prev] Page N [Next]        │                               │
└──────────────────────────────┴───────────────────────────────┘
```

### Key changes from current

| Aspect              | Current                                        | New                                     |
| ------------------- | ---------------------------------------------- | --------------------------------------- |
| Panes               | Ladder left, browser right                     | Browser left, ladder right              |
| Ladder structure    | Energy curve steps (⚡1,⚡2...) with selection | Flat ordered list #1,#2,#3...           |
| Filter source       | Selected steps' energy/keys                    | ALL ladder tracks combined              |
| Ladder items        | Minimal text (title, BPM, energy, ×)           | Full track cards (identical to browser) |
| Reorder             | None                                           | Drag handle to reorder within ladder    |
| Session persistence | None                                           | Save/Load to localStorage               |
| Curve selector      | Sawtooth, Peak Hour, etc.                      | Removed                                 |

### Track card (unified, used in both panes)

```
┌──────────────────────────────────────────────────┐
│ ⠿  Title                                   [▶]  │
│    Artist                                        │
│                                                  │
│    122 BPM · 4m · ⚡3.2 · ▶7 · ★★★★            │
│    house  deep  dark  warehouse  +3 more         │
│                                                  │
│    ▂▃▄▅▆▇██▇▆▅▄▃▂▁▁▂▃▄▅▆▇██▇▆▅▄▃▂  0:45/5:32  │
│                                                  │
│    FLAC ✓(💾)  STEM ✓(💻)  |  Spotify · 3 lists  │
└──────────────────────────────────────────────────┘
```

In browser: ⠿ = drag handle (drag to ladder). In ladder: ⠿ = reorder handle.

### Filter logic

When ladder has tracks, filters derive from ALL tracks:

- ⚡Energy: all unique energy levels (±0.5 each), OR'd → `energyLevels=1,3,4`
- 🔑Key: all keys, expanded by user's range (±1/±2/A↔B) → `keyList=4m,5m,3d&keyRange=+1,-1,same`
- 🎵BPM: median BPM of all ladder tracks ± user slider → `bpmMin=...&bpmMax=...`
- 🏷️Tags: all non-Phase tags from ladder (OR) + user chips → `tags=deep,dark,house`

Each filter toggleable independently. Default: Energy ON, Tags ON, Keys OFF, BPM OFF.

### Session persistence

Save/Load to localStorage under key `diggingSession`:

```javascript
{
  ladder: [{ id, title, artist, bpm, musicalKey, energyLevel, ... }],
  filters: { energyEnabled, keyEnabled, bpmEnabled, tagsEnabled, keyRange },
  bpmRange, sortBy, sortOrder,
  savedAt: epochMs
}
```

Two buttons: "Save Session" (writes), "Load Session" (reads + restores). Auto-save on every change (debounced). Load on page open if session exists.

### Backend

No changes needed. `GET /api/digging/tracks` already supports all filter params.

### Files to modify

- `frontend/pages/digging.js` — major rewrite (~400 lines changed)
- `frontend/style.css` — layout adjustments

### Acceptance Criteria

- [ ] Browser on left, ladder on right
- [ ] Ladder is flat numbered list (no energy curve/steps)
- [ ] Identical track cards in both panes
- [ ] Drag from browser ⠿ to ladder adds at drop position
- [ ] Drag ⠿ within ladder reorders
- [ ] × on ladder card removes from ladder
- [ ] Filters derive from ALL ladder tracks (not selected steps)
- [ ] Energy, Key, BPM, Tags filters all toggleable
- [ ] Key range dropdown (±1, ±2, A↔B, etc.)
- [ ] BPM range slider adjusts ±N from ladder median
- [ ] Tag chips work (add/remove, OR with ladder tags)
- [ ] Search, sort, BPM from/to inputs all work
- [ ] Save Session / Load Session via localStorage
- [ ] Auto-save on every change (debounced 2s)
- [ ] Auto-restore session on page open
- [ ] Save as Playlist still works (collects all ladder track IDs)
- [ ] `cargo build` passes (no backend changes)

---

