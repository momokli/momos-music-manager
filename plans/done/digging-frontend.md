## Plan: digging-frontend

**Status**: done ✅
**Branch**: `feat/digging-frontend`
**Ready for review**: yes
**Depends on**: `feat/digging-multi-seed` (Phase 1)
**Migration needed**: no

### Description

Build the `#digging` SPA page — a split-view Digging/Curator workflow. Left panel: tag-based seed selection with track cards showing BPM/Key/tags, config controls (BPM range, Camelot jumps). Right panel: scored & ranked suggestions with embedded `<audio>` players, tag overview, and action buttons (add to tag).

### Design

```
┌─────────────────────────────────────────────────────┐
│ DIGGING                                    [Config]│
├───────────────────────┬─────────────────────────────┤
│ SEEDS                 │ SUGGESTIONS                 │
│                       │                             │
│ [Collapse-capital  ✕] │ +-+ +-+ +-+ +-+ +-+ +-+  │
│ [Find Similar]        │ |#1| | Games People Play |  │
│                       │ |  | | Paula van Klar    |  │
│ Config:               │ |  | | 140BPM 3m perfect |  │
│ BPM: [====8====] ±8   │ |  | | [+▶] [+ Add]     |  │
│ Jumps: [+1][-1][+2]   │ +-+ +-+ +-+ +-+ +-+ +-+  │
│        [-2][+7][A↔B]  │                             │
│                       │ +-+ +-+ +-+ +-+ +-+ +-+  │
│ +-+ +-+ +-+ +-+     │ |#2| | The Void          |  │
│ | Games People Play  | │ |  | | Maite Dedecker    |  │
│ | Paula van Klar    | │ |  | | 141BPM 8m perfect |  │
│ | 140 BPM · 3m      | │ |  | | [+▶] [+ Add]     |  │
│ | ⚠ OUTLIER         | │ +-+ +-+ +-+ +-+ +-+ +-+  │
│ +-+ +-+ +-+ +-+     │                             │
│                       │ [Load More]                 │
└───────────────────────┴─────────────────────────────┘
```

### Frontend: `frontend/pages/digging.js`

#### State

```javascript
const state = {
  selectedTag: null, // { id, name }
  seeds: [], // DiggingSeed[]
  bpmRange: 8,
  camelotJumps: {
    "+1": true,
    "-1": true,
    "+2": true,
    "-2": true,
    "+7": true,
    "-7": true,
    a_to_b: true,
    same: true,
  },
  limit: 10,
  suggestions: [],
  bpmMin: null,
  bpmMax: null,
  candidatesConsidered: 0,
  loading: false,
  configOpen: false,
  activeAudio: null,
};
```

#### Functions

| Function                        | Purpose                                              |
| ------------------------------- | ---------------------------------------------------- |
| `init(container)`               | Entry point: renders layout + wires events           |
| `renderLayout(container)`       | Renders split-panel HTML                             |
| `renderSeeds(container)`        | Renders seed cards into `#digging-seeds`             |
| `renderSuggestions(container)`  | Renders suggestion cards into `#digging-suggestions` |
| `wireEvents(container)`         | Wires all click/keyboard events                      |
| `buildRequest()`                | Builds `POST /api/digging/suggest` body from state   |
| `doSearch(container)`           | Calls API, updates state, re-renders suggestions     |
| `setupAudioPlayers(container)`  | Wires Play/Pause for `<audio>` elements              |
| `loadConfig()` / `saveConfig()` | localStorage persistence                             |

#### Audio Player

- One `<audio>` element per suggestion card pointing to `/api/files/{id}/stream`
- Clicking Play stops any currently playing audio, starts new one
- Button toggles ▶ / ⏸
- `onended` resets button

### Files to modify

- `frontend/pages/digging.js` — new file (~400 lines)
- `frontend/app.js` — register `"digging": "digging"` in PAGE_MAP
- `frontend/shared/nav.js` — add `{ href: "#digging", icon: "fa-magnifying-glass", label: "Digging" }` to TOOLS_ITEMS
- `frontend/style.css` — digging-specific styles (~100 lines)

### CSS (key classes to add)

```css
.digging-layout {
  display: flex;
  gap: 1.5rem;
  height: calc(100vh - 180px);
}
.digging-seeds {
  width: 40%;
  overflow-y: auto;
}
.digging-suggestions {
  width: 60%;
  overflow-y: auto;
}
.seed-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 0.75rem;
}
.seed-card.outlier {
  opacity: 0.5;
  border-style: dashed;
}
.suggestion-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 0.75rem;
  display: flex;
  gap: 1rem;
  align-items: flex-start;
}
.sugg-rank {
  font-size: 1.5rem;
  font-weight: 700;
  color: var(--muted);
  min-width: 2rem;
  text-align: center;
}
.sugg-body {
  flex: 1;
}
.badge.camelot.perfect {
  background: #2e7d32;
  color: #fff;
}
.badge.camelot.good {
  background: #1565c0;
  color: #fff;
}
.badge.camelot.ok {
  background: #666;
  color: #fff;
}
.btn-play {
  background: var(--primary);
  color: #fff;
  border: none;
  border-radius: 50%;
  width: 32px;
  height: 32px;
  cursor: pointer;
  font-size: 0.9rem;
  flex-shrink: 0;
}
.digging-config {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 1rem;
}
.jump-toggle {
  padding: 0.2rem 0.6rem;
  border: 1px solid var(--border);
  border-radius: 4px;
  cursor: pointer;
  font-size: 0.8rem;
  background: var(--bg);
}
.jump-toggle.active {
  background: var(--primary);
  color: #fff;
  border-color: var(--primary);
}
```

### Acceptance Criteria

- [x] `#digging` route loads the digging page
- [x] Nav link "Digging" in TOOLS section
- [x] Tag typeahead finds tags from `/api/tags?search=...`
- [x] Selecting a tag enables "Find Similar" button
- [x] "Find Similar" calls `POST /api/digging/suggest`, renders results
- [x] Seeds render as cards with BPM/Key/outlier warning
- [x] Suggestions render as ranked cards with score breakdown
- [x] Camelot compatibility badge (perfect=green, good=blue, ok=grey)
- [x] `<audio>` player: Play/Pause works, only one plays at a time
- [x] BPM range slider (2–20) triggers re-fetch
- [x] Camelot jump toggles trigger re-fetch
- [x] Config persists in localStorage
- [x] Loading spinner during API calls
- [x] Error states: toast for API errors, empty state when no tag selected
- [x] Responsive: stacks vertically on narrow screens
- [x] No regressions: other pages still load and function
- [x] Frontend compiles (ES modules load without errors)

---

