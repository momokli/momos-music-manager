# Frontend Build Plan — Mock → Real App

## Motivation

Convert `frontend_rethought/mock.html` (a single 2300-line mock) into a modular, backend-connected SPA.  
Replace the old `frontend/` folder (monolithic, duplicated code) with a clean structure.

---

## Target Structure

```
frontend_next/
├── index.html                # Shell: sidebar + main content area + <script type="module" src="app.js">
├── style.css                 # All styles from mock.html (CSS variables + utility classes)
├── shared/                   # Reusable modules (imported by page modules)
│   ├── api.js                # API_BASE, fetchJSON(url, opts), checkBackend()
│   ├── components.js         # renderLoading(), renderEmpty(), renderErrorBlock(), Pagination class, td(), renderBadge()
│   ├── format.js             # formatDate(), formatDuration(), formatBPM(), formatNumber()
│   └── nav.js                # renderNav(currentPageId) — sidebar HTML builder
├── pages/                    # One JS module per page, each exports init(container)
│   ├── dashboard.js          # Stats cards + recent activity
│   ├── files.js              # Local files table with Diff annotations (page-2)
│   ├── tracks.js             # Service tracks table (page-3)
│   ├── playlists.js          # Playlists table (page-4) — checkboxes, Local/Remote, Last Synced, bulk action
│   ├── tags.js               # Tags flat table (page-5)
│   ├── tag-categories.js     # Tag categories table (page-6)
│   ├── services.js           # Service status/config table (page-7)
│   ├── tasks.js              # Task manager + sync status (page-8)
│   ├── folders.js            # Folders table (page-9)
│   ├── auto-categorize.js    # Wizard with AI suggestions (page-10)
│   └── bulk-import.js        # Bulk import — per-category textareas + results (page-11)
└── app.js                    # Router: listens on hashchange, dynamically imports pages/**, calls init()
```

---

## Architecture

### Routing (SPA, hash-based)

```
hashchange → app.js reads window.location.hash
           → dynamic import('./pages/playlists.js')
           → page.init(document.getElementById('main-content'))
           → page renders DOM + attaches event listeners
```

### Data Flow

```
page.init(container)
  ├── container.innerHTML = renderLoading()
  ├── fetchJSON('/api/playlists?limit=50&offset=0')
  │     ├── success → container.innerHTML = tableHtml
  │     └── error   → container.innerHTML = renderErrorBlock({ title, detail, retryFn })
  └── event listeners bound to container (delegated)
```

### API Client (`shared/api.js`)

```js
export const API_BASE = "http://localhost:3000/api";

export async function fetchJSON(url, options = {}) {
  const fullUrl = url.startsWith("http") ? url : `${API_BASE}${url}`;
  const res = await fetch(fullUrl, {
    headers: { "Content-Type": "application/json", ...options.headers },
    ...options,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({}));
    throw new Error(err.error || `HTTP ${res.status}`);
  }
  return res.json();
}
```

### Component Helpers (`shared/components.js`)

| Function | Purpose |
|----------|---------|
| `renderLoading(label)` | Spinner + text |
| `renderEmpty({ icon, title, message, actionHtml })` | Empty state |
| `renderErrorBlock({ title, detail, retryFn })` | Error state with retry |
| `renderBadge(text, color, opts)` | Inline color badge |
| `td(content, opts)` | Table cell HTML |
| `class Pagination` | Pagination logic + UI binding |

### Navigation (`shared/nav.js`)

Builds sidebar from data array. Highlights active page.  
Dropdown for Tools (Auto-Categorize, Bulk Import).

---

## Step-by-Step Implementation

### Phase 1: Scaffold

- [ ] Create `frontend_next/` directory tree
- [ ] Copy `shared/` from `frontend_rethought/shared/`
- [ ] Create `style.css` — extract CSS variables + utility classes from mock.html
  - CSS vars: `--bg`, `--surface`, `--border`, `--text`, `--accent`, `--green`, `--red`, `--yellow`, `--purple`
  - Utility: `.badge`, `.btn`, `.input-text`, `.table-wrap`, `.data-table`, `.pagination`, `.modal`, `.loading`, `.empty-state`, `.error-block`
- [ ] Create `index.html` — sidebar nav + `<main id="main-content">` + script module

### Phase 2: Router + Core

- [ ] Create `app.js` — hashchange listener, dynamic import, nav highlight
- [ ] `shared/nav.js` — render sidebar from mock.html (Dashboard, Files, Tracks, Playlists, Tags, Tag Categories, Services, Tasks, Folders, Auto-Categorize, Bulk Import)

### Phase 3: Pages (one by one)

Each page follows the same pattern:
1. `init(container)` — render loading, fetch data, render table, bind events
2. Error/empty/loading states handled uniformly
3. Table headers + rows match mock.html exactly

**Order** (easiest → hardest):
1. `dashboard.js` — stats from `/api/health` + `/api/stats`
2. `services.js` — from `/api/services`
3. `folders.js` — from `/api/folders`
4. `tag-categories.js` — from `/api/tag-categories`
5. `tasks.js` — from `/api/tasks`
6. `tags.js` — from `/api/tags`
7. `tracks.js` — from `/api/tracks`
8. `playlists.js` — from `/api/playlists` (with checkbox + bulk action)
9. `files.js` — from `/api/files` (with diff annotations)
10. `auto-categorize.js` — from `/api/tags/unreviewed` + `/api/tags/{id}/suggest`
11. `bulk-import.js` — from `/api/tags/bulk-import` + `/api/tags/bulk-resolve`

### Phase 4: Polish

- [ ] Keyboard shortcuts (Ctrl+F search, Escape clear, arrow pagination)
- [ ] Polling for sync status
- [ ] Debug panel (optional)
- [ ] Responsive sidebar (mobile hamburger)

---

## Backend API Endpoints (Reference)

| Method | Endpoint | Used by |
|--------|----------|---------|
| GET | `/api/health` | dashboard, all pages |
| GET | `/api/stats` | dashboard |
| GET | `/api/files` | files |
| GET | `/api/files/{id}` | files (single) |
| GET | `/api/tracks` | tracks |
| GET | `/api/tracks/{id}` | tracks (single) |
| GET | `/api/tags` | tags |
| POST | `/api/tags` | tags (create) |
| PUT | `/api/tags/{id}` | tags (update) |
| DELETE | `/api/tags/{id}` | tags (delete) |
| GET | `/api/tags/unreviewed` | auto-categorize |
| GET | `/api/tags/{id}/suggest` | auto-categorize |
| PUT | `/api/tags/{id}/categorize` | auto-categorize |
| POST | `/api/tags/bulk-import` | bulk-import |
| POST | `/api/tags/bulk-resolve` | bulk-import |
| GET | `/api/tag-categories` | tag-categories |
| POST | `/api/tag-categories` | tag-categories (create) |
| PUT | `/api/tag-categories/{id}` | tag-categories (update) |
| DELETE | `/api/tag-categories/{id}` | tag-categories (delete) |
| GET | `/api/playlists` | playlists |
| GET | `/api/playlists/{id}/tracks` | playlists (detail) |
| GET | `/api/services` | services |
| POST | `/api/services/{service}/sync` | services |
| GET | `/api/services/{service}/sync/{task_id}` | tasks |
| GET | `/api/services/{service}/sync-status` | services |
| GET | `/api/services/{service}/config` | services |
| PUT | `/api/services/{service}/config` | services |
| GET | `/api/tasks` | tasks |
| GET | `/api/tasks/{id}` | tasks |
| DELETE | `/api/tasks/{id}` | tasks (cancel) |
| GET | `/api/folders` | folders |
| POST | `/api/folders` | folders (add) |
| DELETE | `/api/folders/{id}` | folders (delete) |
| POST | `/api/folders/{id}/watch` | folders (toggle watch) |
| POST | `/api/folders/{id}/scan` | folders (scan) |

---

## Key Design Decisions

1. **SPA not multi-page** — mock.html already has hash navigation; avoids duplicating sidebar in every HTML file
2. **ES modules** — no bundler needed, native `import`/`export` works in modern browsers
3. **CSS variables + utility classes** — keeps styling consistent without Tailwind build step
4. **Dynamic import per page** — pages load on demand, not all at once
5. **Event delegation** — attach one listener per page container, not per element
6. **No framework** — vanilla JS keeps it simple for POC phase

---

## Mock HTML → JS Module Translation Guide

For each page in mock.html:

```
<div id="page-X" class="hidden">     → pages/xxx.js  (module, not hidden)
  Header (h1 + p)                    → render once in init()
  Toolbar (search + selects + btns)   → render + bind events
  Table/Content                       → renderLoading → fetch → renderTable
  Pagination                          → Pagination class
</div>
```

The `hidden` class is replaced by the router: only the active page's container is rendered into `#main-content`.
