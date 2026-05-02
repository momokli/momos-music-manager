# Frontend Next — Interactive Wiring Plan

> **Status: Historical Reference** — Most items completed as of 2026-05-01.
>
> This document describes the plan that guided the wiring of interactive features
> across all frontend pages. All 11 pages now have full CRUD, event wiring, and
> API mutations. The document is kept for reference but is no longer an active
> TODO list.

---

## Current State (2026-05-01)

All 11 pages load real data from the backend API and have interactive features
(buttons, forms, modals, OAuth flows) fully wired.

### Wiring Status Overview

| Page            | Data Load | Event Wiring                                                  | API Mutations                       |
| --------------- | --------- | ------------------------------------------------------------- | ----------------------------------- |
| Dashboard       | ✅        | ✅ bulk sync, service resync, navigation                      | ✅                                  |
| Files           | ✅        | ✅ search/filter/paginate, view modal, similar tracks         | ✅ write comment, bulk tag          |
| Tracks          | ✅        | ✅ search/filter/paginate/refresh                             | N/A (read-only)                     |
| Playlists       | ✅        | ✅ search/filter/paginate, subscribe/unsubscribe, sync        | ✅ create/edit tag, subscribe, sync |
| Tags            | ✅        | ✅ filter/search, CRUD modals                                 | ✅ create/edit/delete               |
| Tag Categories  | ✅        | ✅ drag-and-drop, inline edits, icon picker, energy levels    | ✅ create/edit/delete, reorder      |
| Services        | ✅        | ✅ auth, resync, reset, fetch counts, config modal, polling   | ✅ auth, config, sync, reset        |
| Folders         | ✅        | ✅ add/edit/delete modals, scan, toggle watch                 | ✅ CRUD, scan, watch                |
| Tasks           | ✅        | ✅ filter/refresh, cancel, retry, view logs, polling          | ✅ cancel, retry                    |
| Auto-Categorize | ✅        | ✅ full wizard with keyboard shortcuts & speculative prefetch | ✅ categorize, skip                 |
| Traktor Import  | ✅        | ✅ full flow with path detection, continuous polling          | ✅ import, status                   |

---

## Shared Components (all implemented)

| Module                             | Status                                                                                     |
| ---------------------------------- | ------------------------------------------------------------------------------------------ |
| `frontend/shared/components.js`    | ✅ `renderModal`, `showModal`, `showToast`, `useErrorBanner`, `Pagination`, render helpers |
| `frontend/shared/api.js`           | ✅ `API_BASE`, `fetchJSON`                                                                 |
| `frontend/shared/format.js`        | ✅ date, duration, BPM formatters                                                          |
| `frontend/shared/nav.js`           | ✅ Sidebar navigation builder                                                              |
| `frontend/shared/search-filter.js` | ✅ Generic search/filter UI                                                                |

---

## Implementation Patterns Used

### Pattern 1: Folder CRUD (modal + mutations)

Implemented in `frontend/pages/folders.js` with full add/edit/delete modals, scan, and watch toggle.

### Pattern 2: Services OAuth flow

Implemented in `frontend/pages/services.js` with authorize, resync, reset, fetch counts, config modal, and polling.

### Pattern 3: Shared modal component

Implemented in `frontend/shared/components.js` as `showModal()` with action callbacks and Escape-key-to-close.

---

## API Reference (all mutation endpoints implemented in frontend)

| Endpoint                           | Method | Purpose                         | Frontend Usage                |
| ---------------------------------- | ------ | ------------------------------- | ----------------------------- |
| `/api/folders`                     | POST   | Add folder                      | `folders.js`                  |
| `/api/folders/{id}`                | PUT    | Update folder                   | `folders.js`                  |
| `/api/folders/{id}`                | DELETE | Delete folder                   | `folders.js`                  |
| `/api/folders/{id}/scan`           | POST   | Scan folder                     | `folders.js`                  |
| `/api/folders/{id}/watch`          | POST   | Toggle watch                    | `folders.js`                  |
| `/api/services/{svc}/auth`         | POST   | OAuth start                     | `services.js`                 |
| `/api/services/{svc}/reset`        | POST   | Reset connection                | `services.js`                 |
| `/api/services/{svc}/sync`         | POST   | Trigger sync                    | `services.js`, `playlists.js` |
| `/api/services/{svc}/config`       | GET    | Get config                      | `services.js`                 |
| `/api/services/{svc}/config`       | PUT    | Save config                     | `services.js`                 |
| `/api/services/{svc}/fetch-counts` | GET    | Fetch counts                    | `services.js`                 |
| `/api/services/{svc}/sync-status`  | GET    | Sync status                     | `services.js`                 |
| `/api/tags`                        | POST   | Create tag                      | `tags.js`, `playlists.js`     |
| `/api/tags/{id}`                   | PUT    | Update tag                      | `tags.js`                     |
| `/api/tags/{id}`                   | DELETE | Delete tag                      | `tags.js`                     |
| `/api/tag-categories`              | POST   | Create category                 | `tag-categories.js`           |
| `/api/tag-categories/{id}`         | PUT    | Update category                 | `tag-categories.js`           |
| `/api/tag-categories/{id}`         | DELETE | Delete category                 | `tag-categories.js`           |
| `/api/tasks/{id}`                  | DELETE | Cancel task                     | `tasks.js`                    |
| `/api/tags/create-from-playlists`  | POST   | Create tags from playlist names | `playlists.js`                |

---

## Remaining Work (minor polish)

- [ ] **Dashboard**: Service cards could be clickable to navigate to services page
- [ ] **Dashboard**: Task polling with auto-refresh when tasks are running
- [ ] **Global**: Keyboard shortcuts (Ctrl+F for search focus, Escape to clear/close)
- [ ] **Global**: Mobile responsive — verify sidebar collapse, table scroll, modal sizing
- [ ] **Global**: Loading spinners on buttons during API calls (some pages already do this)
- [ ] **Tracks**: Read-only by design (no mutations needed for service tracks)
