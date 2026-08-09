## Plan: files-bulk-comments

**Status**: done ✅
**Branch**: `feat/files-bulk-comments`
**Ready for review**: yes
**Depends on**: `feat/tracks-bulk-comments` (already merged into `review/all-features`)
**Migration needed**: no

### Description

Port the checkbox-selection + "WRITE COMMENTS (X)" bulk-action pattern from Tracks to Files. On the Files page: multi-select checkboxes, an ACTIONS panel button showing how many selected files actually need a comment update (have a `needsUpdate` delta), click to queue write-comment tasks for all selected files needing updates.

### What exists already

- Per-row "write-comment" button (pencil icon) — calls `POST /api/files/{id}/write-comment`
- Actions panel skeleton in `init()` — div#files-sel-count badge, refresh button, `wireActionsRefresh` import
- `POST /api/files/write-comments` (filter-based: linked_only/tags/non_default_only) + `GET /api/files/needs-update-count` (same filters)
- File data model already has `needsUpdate` (bool), `comment`, `commentTarget`, `diffOld`, `diffNew` — comment diff is already rendered per-row
- Shared `actions-panel.js` already supports configurable buttons + selection count badge
- `.col-checkbox` CSS already exists (from tracks)

### What's missing (files-specific)

#### Backend

Unlike tracks, files don't need a join — they ARE the comment-bearing entity. So the endpoints are simpler:

1. **`POST /api/files/needs-comment-count`** — takes `{ fileIds: [1,2,3] }`, fetches those files, runs `compute_target_comment` for each, returns `{ totalFiles, filesNeedingUpdate }`
2. **`POST /api/files/write-comments-by-ids`** — takes `{ fileIds: [1,2,3] }`, fetches files, filters to those needing updates, calls `start_write_comment_task`, returns `{ taskId, fileCount }`

Router additions (in `src/api.rs`, near existing file routes):

- `.route("/api/files/needs-comment-count", post(files_needs_comment_count_by_ids_handler))`
- `.route("/api/files/write-comments-by-ids", post(files_write_comments_by_ids_handler))`

#### Frontend (`frontend/pages/files.js`)

Same pattern as tracks, adapted to the files page structure:

1. **State**: add `selectedFileIds: new Set()`, `needsCommentCount: 0`
2. **renderBody**: prepend checkbox `<th>` + `<td>` to each row (outside column-config system — same as tracks)
3. **renderEmptyBody**: add checkbox header + increment colspan
4. **wireContentEvents**: wire select-all + individual row checkboxes (same logic as tracks)
5. **init**: replace inline actions panel HTML with `renderActionsPanel([{ id: "write-comments", label: "WRITE COMMENTS", ... }])` — same call as tracks
6. **init**: wire `#files-actions-write-comments` button to `writeCommentsForSelected(container, state)`
7. **add helpers**: `updateSelectionUI`, `computeNeedsCount`, `writeCommentsForSelected` — same pattern as tracks but using `/api/files/needs-comment-count` and `/api/files/write-comments-by-ids`
8. **fetchAndRender**: call `updateSelectionUI` after each render
9. **handle null signal**: `if (signal && signal.aborted) return;` — already present in tracks, replicate in files

#### Key differences from tracks

| Aspect                      | Tracks                                                     | Files                                                                              |
| --------------------------- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Endpoint entity             | tracks → joined to files via `v_file_track_link`           | files directly                                                                     |
| needs-update count response | `{ totalTracks, tracksNeedingUpdate, filesNeedingUpdate }` | `{ totalFiles, filesNeedingUpdate }`                                               |
| needsUpdate field           | computed server-side per-request                           | already in API response (`needsUpdate`), but still verify server-side for accuracy |
| Import/state                | `showToast`, `updateSelectionCount` already imported       | `showToast` already imported, `updateSelectionCount` needs adding                  |
| renderBody params           | `(data, state)` — already has `state`                      | `(data, state)` — already has `state`                                              |

#### Potential client-side optimization

Files already return `needsUpdate` from the API. We _could_ compute X client-side (count `selectedFileIds ∩ files.where(f => f.needsUpdate)`), avoiding the `/api/files/needs-comment-count` round-trip. But the server-side check is more accurate (recomputes target comment fresh), so stick with the backend endpoint for consistency with tracks.

### Files to modify

- `src/api.rs` — add `FilesBulkRequest` struct + 2 handlers + 2 routes
- `frontend/pages/files.js` — checkbox column, selection state, actions panel wiring, helper functions

### Acceptance Criteria

- [ ] Checkbox column with select-all in header
- [ ] Selection persists across page navigation (Set-based)
- [ ] Actions panel shows selection count badge
- [ ] "WRITE COMMENTS (X)" button shows count of selected files needing updates (X = files with comment delta)
- [ ] Clicking button queues write-comment task for selected files that need updates
- [ ] Toast notifications for success/error/up-to-date
- [ ] Selection cleared after successful write
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

