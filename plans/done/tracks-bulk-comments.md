## Plan: tracks-bulk-comments

**Status**: done ✅
**Branch**: `feat/tracks-bulk-comments`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Add multi-select checkboxes to the Tracks page with an ACTIONS panel containing a "WRITE COMMENTS (X)" button. X is the number of selected tracks whose linked files have outdated comments (need a comment string update).

### Backend Changes

- New `POST /api/tracks/needs-comment-count` endpoint — takes `{ trackIds: [1,2,3] }`, finds linked files via `v_file_track_link`, computes which need updates, returns `{ totalTracks, tracksNeedingUpdate, filesNeedingUpdate }`
- New `POST /api/tracks/write-comments` endpoint — takes track IDs, finds linked files needing updates, queues write-comment task via `start_write_comment_task`

### Frontend Changes

- `frontend/shared/actions-panel.js` — updated with configurable button rendering and selection count badge
- `frontend/shared/crud.js` — added `Set` instance skip in `updateHash` to avoid serialization issues
- `frontend/pages/tracks.js` — checkbox column (select-all + per-row), selection state (`selectedTrackIds` Set), `computeNeedsCount` to query backend, `writeCommentsForSelected` to trigger bulk writes, `updateSelectionUI` to keep panel in sync
- `frontend/style.css` — `.col-checkbox` styles for checkbox column

### Acceptance Criteria

- [x] Checkbox column with select-all in header
- [x] Selection persists across page navigation (Set-based)
- [x] Actions panel shows selection count badge
- [x] "WRITE COMMENTS (X)" button shows count of selected tracks needing updates
- [x] Clicking button queues write-comment task for linked files
- [x] Toast notifications for success/error/up-to-date
- [x] Selection cleared after successful write
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

