## Plan: fix-filter-visual-feedback

**Status**: done ✅
**Branch**: `fix/filter-visual-feedback`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Fix filter button visual feedback bugs across all CRUD pages. The root cause is the "render toolbar once, patch DOM imperatively" pattern: when an event handler mutates state but forgets to toggle `.active` on the button, the button appears frozen. Additionally, some filters never reach the backend (placebo buttons), and some UI elements (playlist badge, Create Tags spinner) have lifecycle bugs.

### Architecture context

All four CRUD pages use the same pattern:

- `renderToolbar(state)` generates HTML with `${condition ? " active" : ""}` inline — runs **once** in `init()`
- `fetchAndRender()` only replaces `#page-content` div, NOT the toolbar
- Event handlers must imperatively update DOM (`.classList.toggle`, `.innerHTML`, `.style.display`)
- If a handler mutates state but skips DOM update → visual freeze

### Issues found

#### A. files.js — 3 button groups with no visual toggle

| Button group                  | Lines     | Symptom                                                                                      |
| ----------------------------- | --------- | -------------------------------------------------------------------------------------------- |
| Key buttons (24 Camelot keys) | 823–866   | `state.keys` mutates, `.active` never toggled. ALL/NONE actions also skip `.active` updates. |
| Linked / Unlinked toggle      | 1091–1125 | `state.linkedOnly`/`state.unlinked` mutate, buttons never toggle.                            |
| Non-Default Only toggle       | 1129–1141 | `state.nonDefaultOnly` toggles, button never updates.                                        |

**Fix for key buttons**: Add `btn.classList.toggle("active")` in the regular key toggle handler. For ALL/NONE actions, re-sync all 24 button classes from `state.keys`.

**Fix for Linked/Unlinked**: Add `linkedBtn.classList.toggle("active", state.linkedOnly)` and `unlinkedBtn.classList.toggle("active", state.unlinked)` in each handler. Also update the sibling button (mutual exclusion).

**Fix for Non-Default Only**: Add `btn.classList.toggle("active", state.nonDefaultOnly)`.

#### B. tracks.js — Playlist context badge doesn't disappear

| Element                         | Lines     | Symptom                                                                                                                         |
| ------------------------------- | --------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Playlist context badge × button | 1838–1843 | `state.playlistId = null`, navigate to `#tracks`, `fetchAndRender` called — but toolbar was rendered once, badge HTML persists. |

**Fix**: Add DOM manipulation in the clear handler: `badge.style.display = "none"` or `badge.remove()`. Also, the `updatePlaylistBadge()` function at line 1132-1140 already exists and hides the badge when `selectedPlaylists` has items — extend it to also hide when `playlistId` is null.

#### C. playlists.js — Service filter is placebo (never sent to backend)

| Element                      | Lines            | Symptom                                                                                                                                                             |
| ---------------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Multi-select service buttons | 548–572, 418–434 | `state.selectedServices` toggles correctly, `syncServiceFilterUI()` works, but `buildParams()` never sends it and backend `PlaylistsQuery` has no `services` field. |

**Fix (option A, simpler)**: Convert to single-select using the existing `state.service` + `service` param. Change the multi-select button group to radio-style (only one active at a time).

**Fix (option B, more work)**: Add `services: Option<String>` to `PlaylistsQuery`, implement SQL `IN` filter in `playlists_handler`, add to `buildParams()`. Worth it if multi-service filtering is genuinely useful.

**Recommendation**: Option A (single-select). The existing `service` dropdown in the filter panel already provides single-service filtering. The multi-select buttons are redundant and broken.

#### D. playlists.js — "Create Tags" button stays spinning

| Element            | Lines     | Symptom                                                                                      |
| ------------------ | --------- | -------------------------------------------------------------------------------------------- |
| Create Tags button | 1147–1172 | On success, button HTML is set to spinner but never restored. Only re-enabled on error path. |

**Fix**: Add `finally` block that always restores the button: `createTagsBtn.disabled = false; createTagsBtn.innerHTML = '<i class="fas fa-tag"></i> Create Tags';`.

### Additional minor fixes

#### E. files.js — Filter panel collapse not persisted

Lines 774–787: The collapse toggle works but never calls `localStorage.setItem()`. Add it (pattern already exists in tracks.js and tags.js).

#### F. tags.js — Duplicate `wireActionsRefresh` call

Lines 895–902 and 1025–1032: Called twice. Second overwrites first. Delete the first instance (the second has the `refresh` button comment).

#### G. playlists.js + tags.js — Filter row toggle states not persisted

Both pages have `[data-filter]` toggle labels (Service, Category, etc.) whose enabled/disabled state resets on page re-entry. Add `localStorage` read on init + write on toggle. Pattern: `filterRowState_{page}_{filterName}`.

#### H. files.js + playlists.js — `untaggedOnly` has no UI button

`untaggedOnly` exists in `HASH_DEFAULTS`, `HASH_SCHEMA`, and `buildParams()` on playlists.js, but `renderToolbar()` has no button for it. Either add a button or remove the dead state.

### Files to modify

- `frontend/pages/files.js` — Key buttons `.active` toggle, Linked/Unlinked `.active`, Non-Default `.active`, filter collapse localStorage
- `frontend/pages/tracks.js` — Playlist badge clear DOM update
- `frontend/pages/playlists.js` — Service filter (convert to single-select), Create Tags `finally` block, filter row toggle localStorage, untaggedOnly UI
- `frontend/pages/tags.js` — Remove duplicate wireActionsRefresh, filter row toggle localStorage

### Acceptance Criteria

- [ ] Key buttons toggle `.active` visually on click
- [ ] ALL m / NONE m / ALL d / NONE d actions update all 24 key button states
- [ ] Linked/Unlinked buttons show active state, mutual exclusion works
- [ ] Non-Default Only button shows active state
- [ ] Playlist context badge disappears when × is clicked
- [ ] Service filter on playlists page actually filters results
- [ ] Create Tags button re-enables after success
- [ ] Filter panel collapse state persists across page navigations (files.js)
- [ ] No duplicate wireActionsRefresh in tags.js
- [ ] Filter row toggle states persist across page navigations (playlists.js, tags.js)
- [ ] No regressions: sort, pagination, search, column config, layout mode, bulk comments still work
- [ ] `cargo build` passes (no backend changes unless service filter chosen as option B)

---

