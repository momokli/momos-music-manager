# UI Plan: Smart Search Results

> Initial prompt from user (2026-07-19):
> "consider a proper design thought to the way you display the search results
> i think we want to have a filter bar in between which gives toggle to each service, default all on
> search results are individually fetched per service and displayed immediately, will display placeholder
> result '1. soundcloud-icon greyed out' type where the result will be placed to not have sudden
> re-arranges of the flow in the ui. could have the search animation shrink/resize effect.
> we want it be fast and snappy. searches should be cached 5-10 min if not even hours."

---

## Architecture

```
User types "ABBA"
       │
       ▼
┌─────────────────────────────────────────────────────┐
│               BACKEND: GET /search?q=ABBA           │
│                                                     │
│  Cache check: query in memory?                      │
│    YES → return immediately (<5ms)                  │
│    NO  → fetch ALL services, cache, return          │
│                                                     │
│  Response:                                          │
│  {                                                  │
│    "spotify":   [{...}, {...}, {...}],              │
│    "youtube":   [{...}, {...}, {...}],              │
│    "soundcloud":[{...}, {...}, {...}],              │
│    "cached": false,                                 │
│    "took_ms": 1200                                  │
│  }                                                  │
│                                                     │
│  Cache: TTL = 30 minutes, in-memory dict            │
│  Key: normalized query (lowercase, trimmed)         │
└─────────────────────────────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────────┐
│               FRONTEND: wish.zukkafabrik.de          │
│                                                     │
│  [🔍 ABBA________________________]                  │
│                                                     │
│  [● SPOTIFY] [● YOUTUBE] [● SOUNDCLOUD]            │
│                                                     │
│  ┌─ SPOTIFY ──────────────────────────────────┐    │
│  │ ┌─────────┐ ┌─────────┐ ┌─────────┐        │    │
│  │ │ ░░░░░░░░ │ │ ░░░░░░░░ │ │ ░░░░░░░░ │  ← skeleton  │
│  │ │ ░░░░░░░░ │ │ ░░░░░░░░ │ │ ░░░░░░░░ │    │    │
│  │ │ SPOTIFY  │ │ SPOTIFY  │ │ SPOTIFY  │    │    │
│  │ └─────────┘ └─────────┘ └─────────┘        │    │
│  └──────────────────────────────────────────────┘    │
│  ┌─ YOUTUBE ──────────────────────────────────┐    │
│  │ ┌─────────┐ ┌─────────┐ ┌─────────┐        │    │
│  │ │ Dancing  │ │ ░░░░░░░░ │ │ ░░░░░░░░ │        │    │
│  │ │ Queen    │ │ ░░░░░░░░ │ │ ░░░░░░░░ │        │    │
│  │ │ YOUTUBE  │ │ YOUTUBE  │ │ YOUTUBE  │        │    │
│  │ └─────────┘ └─────────┘ └─────────┘        │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

## Key Design Decisions

### 1. One API call, not three

The backend fetches all services and returns them together with cache.
This is simpler than three parallel frontend fetches and avoids CORS/complexity.
The response time with cache is <5ms, without cache ~1-2s.

### 2. Skeleton placeholders prevent layout shift

When user submits search:

1. Immediately render skeleton grid (grey placeholder cards)
2. Fetch results from API
3. Fill in skeleton slots with real data
4. Smooth transition (CSS opacity animation)

No layout shift because the skeleton occupies the same space as the final result.

### 3. Caching strategy

- Key: `normalize(query)` = lowercase, trimmed, max 100 chars
- TTL: 30 minutes (configurable via env `SEARCH_CACHE_TTL_SECONDS`)
- Storage: in-memory Python dict (fast, no DB overhead)
- Max cache entries: 500 (LRU eviction)

### 4. Layout per service group

Each service gets its own horizontal row of 3 cards.
Services can be toggled on/off via filter bar.
Default: all three on.

```
[● SPOTIFY] [● YOUTUBE] [● SOUNDCLOUD]

SPOTIFY
┌──────┐ ┌──────┐ ┌──────┐
│ Card │ │ Card │ │ Card │
└──────┘ └──────┘ └──────┘

YOUTUBE
┌──────┐ ┌──────┐ ┌──────┐
│ Card │ │ Card │ │ Card │
└──────┘ └──────┘ └──────┘

SOUNDCLOUD
┌──────┐ ┌──────┐ ┌──────┐
│ Card │ │ Card │ │ Card │
└──────┘ └──────┘ └──────┘
```

### 5. Skeleton card design

Grey placeholder with service icon dimmed:

```
┌──────────────────────────┐
│ ░░░░░░░░░░░░░░░░░░░░░░░░ │  ← grey shimmer
│ ░░░░░░░░░░░░░░░░░░░░░░░░ │
│ [◌ SPOTIFY]          [·] │  ← dimmed icon + badge
└──────────────────────────┘
```

CSS animation: subtle shimmer/pulse effect while loading.

## Implementation Plan

### Phase 1: Backend Cache

**File:** `download-service/main.py`

```python
import time
from collections import OrderedDict

_search_cache: OrderedDict[str, tuple[float, dict]] = OrderedDict()
CACHE_TTL = int(os.environ.get("SEARCH_CACHE_TTL_SECONDS", 1800))
CACHE_MAX = 500

def _cached_search(q: str, limit: int) -> dict:
    key = q.strip().lower()[:100]
    now = time.time()
    if key in _search_cache:
        ts, data = _search_cache[key]
        if now - ts < CACHE_TTL:
            _search_cache.move_to_end(key)
            return {**data, "cached": True, "took_ms": 0}
        del _search_cache[key]
    # Fetch all services
    t0 = time.time()
    spotify = _fetch_spotify(q, limit)
    youtube = _search_youtube(q, limit)
    soundcloud = _search_soundcloud(q, limit)
    data = {"spotify": spotify, "youtube": youtube, "soundcloud": soundcloud}
    took = int((time.time() - t0) * 1000)
    result = {**data, "cached": False, "took_ms": took}
    _search_cache[key] = (now, data)
    if len(_search_cache) > CACHE_MAX:
        _search_cache.popitem(last=False)
    return result
```

### Phase 2: Frontend Skeleton + Filter Bar

**File:** `download-service/static/request.html`

**HTML structure:**

```html
<!-- Filter bar -->
<div id="filter-bar" style="display:none">
  <button class="filt active" data-svc="spotify">● Spotify</button>
  <button class="filt active" data-svc="youtube">● YouTube</button>
  <button class="filt active" data-svc="soundcloud">● SoundCloud</button>
</div>

<!-- Results grouped by service -->
<div id="results-grid"></div>
```

**CSS:**

```css
.filter-bar {
  display: flex;
  gap: 0.5rem;
  padding: 0.5rem 0;
}
.filter-bar button {
  padding: 0.3rem 0.6rem;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--muted);
  border-radius: 4px;
  cursor: pointer;
  font-size: 0.65rem;
  font-family: "Courier New", monospace;
}
.filter-bar button.active {
  color: var(--text);
  border-color: var(--text);
}
.filter-bar button[data-svc="spotify"].active {
  border-color: #1db954;
  color: #1db954;
}
.filter-bar button[data-svc="youtube"].active {
  border-color: #ff0000;
  color: #ff0000;
}
.filter-bar button[data-svc="soundcloud"].active {
  border-color: #fc3d0a;
  color: #fc3d0a;
}

.service-row {
  margin-bottom: 1rem;
}
.service-row h3 {
  font-size: 0.65rem;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  color: var(--muted);
  margin-bottom: 0.4rem;
  font-family: "Courier New", monospace;
  display: flex;
  align-items: center;
  gap: 4px;
}
.service-row h3 svg {
  width: 12px;
  height: 12px;
}
.service-cards {
  display: flex;
  gap: 0.5rem;
  overflow-x: auto;
}

.skeleton {
  background: var(--card);
  border: 1px solid var(--border);
  padding: 0.75rem;
  min-width: 140px;
  flex: 1;
}
.skeleton .shimmer {
  height: 8px;
  background: var(--border);
  border-radius: 4px;
  margin-bottom: 6px;
  animation: shimmer 1.5s infinite;
}
.skeleton .shimmer:nth-child(2) {
  width: 60%;
}
.skeleton .shimmer:nth-child(3) {
  width: 80%;
  margin-top: 8px;
  height: 6px;
}
@keyframes shimmer {
  0%,
  100% {
    opacity: 0.3;
  }
  50% {
    opacity: 0.6;
  }
}
```

**JS flow:**

```javascript
async function doSearch(q) {
  // 1. Show skeleton immediately
  showSkeleton();
  // 2. Fetch (cached if available)
  var data = await fetch(API + "/search?q=" + encodeURIComponent(q) + "&limit=3").then(
    (r) => r.json(),
  );
  // 3. Fill skeleton with real data
  fillResults(data);
}

function showSkeleton() {
  var svcs = getActiveServices();
  var h = "";
  for (var i = 0; i < svcs.length; i++) {
    h +=
      '<div class="service-row"><h3>' +
      svgIcon(svcs[i]) +
      " " +
      svcs[i].toUpperCase() +
      '</h3><div class="service-cards">';
    for (var j = 0; j < 3; j++) {
      h +=
        '<div class="skeleton"><div class="shimmer"></div><div class="shimmer"></div><div class="shimmer"></div></div>';
    }
    h += "</div></div>";
  }
  resultsEl.innerHTML = h;
}

function fillResults(data) {
  var svcs = getActiveServices();
  var h = "";
  for (var i = 0; i < svcs.length; i++) {
    var svc = svcs[i];
    var tracks = data[svc] || [];
    h +=
      '<div class="service-row"><h3>' +
      svgIcon(svc) +
      " " +
      svc.toUpperCase() +
      '</h3><div class="service-cards">';
    if (tracks.length === 0) {
      h +=
        '<div class="card" style="opacity:.4"><div class="info"><div class="title">No results</div></div></div>';
    } else {
      for (var j = 0; j < 3 && j < tracks.length; j++) {
        h += renderCard(tracks[j]);
      }
    }
    h += "</div></div>";
  }
  resultsEl.innerHTML = h;
  wireCardButtons();
}
```

### Phase 3: Service Toggle Buttons

Clicking a filter button toggles that service's visibility.
State stored in localStorage per session.

```javascript
var activeServices = ["spotify", "youtube", "soundcloud"];
// Load from localStorage
var saved = localStorage.getItem("search_services");
if (saved) activeServices = JSON.parse(saved);

// Filter bar click handler
document.getElementById("filter-bar").addEventListener("click", function (e) {
  var btn = e.target.closest("button");
  if (!btn) return;
  var svc = btn.dataset.svc;
  btn.classList.toggle("active");
  if (btn.classList.contains("active")) {
    if (activeServices.indexOf(svc) < 0) activeServices.push(svc);
  } else {
    activeServices = activeServices.filter(function (s) {
      return s !== svc;
    });
  }
  localStorage.setItem("search_services", JSON.stringify(activeServices));
  // Re-render current results
  if (lastResults) fillResults(lastResults);
});
```

---

## Files to modify

| File                                   | Change                                                             |
| -------------------------------------- | ------------------------------------------------------------------ |
| `download-service/main.py`             | Add in-memory cache with TTL, OrderedDict LRU                      |
| `download-service/static/request.html` | New: skeleton, filter bar, service-group layout, shimmer animation |

---

## Acceptance Criteria

- [ ] Search results appear as skeleton first, then fill in (<100ms to skeleton visible)
- [ ] Cached searches return in <5ms
- [ ] Filter bar toggles services on/off, state persists in localStorage
- [ ] Each service gets its own row with 3 cards
- [ ] No layout shift during loading (skeleton same size as cards)
- [ ] Shimmer animation while loading
- [ ] Cache TTL 30 minutes via env var
- [ ] Max 500 cache entries, LRU eviction
- [ ] All 14 existing E2E tests still pass
