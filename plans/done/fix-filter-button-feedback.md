## Plan: fix-filter-button-feedback

**Status**: done ✅
**Branch**: `fix/filter-button-feedback`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Fix two bugs with the Files page filter toolbar buttons:

1. **No visual active state**: After clicking any filter button, its `.active` class was never toggled because `fetchAndRender` only re-renders `#files-content`, not the toolbar. Fixed by adding `btn.classList.toggle("active")` inline in all 5 button click handlers (Service, PMV Category, PMV Aggregate, Comment Status, File Type).

2. **Comment status pagination broken**: The `comment_statuses` filter was applied in Rust AFTER `LIMIT/OFFSET` in SQL, meaning a page expecting 100 results could return 5. Fixed by fetching ALL matching rows (no LIMIT/OFFSET) when comment status filter is active, computing `comment_needs_update` pre-filtering in Rust, then applying offset/limit in Rust. Cached `target_comment` results are reused in the downstream ApiFile conversion loop to avoid recomputation.

### Files modified

- `frontend/pages/files.js` — 5x `btn.classList.toggle("active")` added in filter button handlers
- `src/api.rs` — `get_files()` conditionally skips SQL LIMIT/OFFSET when comment_statuses is active, fetches all rows, filters in Rust, then slices for pagination

### Acceptance Criteria

- [x] All 5 filter button groups toggle `.active` class immediately on click
- [x] Multi-select buttons (Service, Comment, FileType) properly toggle on/off
- [x] Single-select (PMV Aggregate) properly clears sibling buttons
- [x] Comment status filter returns correct page sizes (LIMIT rows, not fewer)
- [x] Count query returns correct total for comment status filter
- [x] Cached target comments avoid recomputation in downstream conversion
- [x] Backend compiles (`cargo build`)
- [x] No regressions to other filters or pagination without comment status filter

---

