## Plan: incremental-folder-scan

**Status**: done ✅
**Branch**: `feat/incremental-folder-scan`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Add incremental "Quick Scan" mode to folder scanning — only process files whose `mtime` is newer than `folders.last_scanned`. Also activate the dormant `FolderWatcher` on server startup so it polls active folders every 5 minutes.

### Backend: db.rs

1. Add `ScanMode` enum: `Full | Incremental { since: Option<i64> }`
2. `scan_directory_with_config` accepts `scan_mode`, skips files with `mtime <= since` in walk loop
3. `scan_folder` accepts `ScanMode`, passes folder's `last_scanned` as `since` when Incremental

### Backend: api.rs + tasks/mod.rs

4. `scan_folder_handler` accepts `?mode=incremental|full` query param (default: `incremental`)
5. `start_scan_folder_task` passes `ScanMode` through to `scan_directory_with_config`

### Backend: watch.rs + main.rs

6. In `serve()`, create `FolderWatcher` + call `.start()` so it polls active folders
7. `scan_active_folders` uses `ScanMode::Incremental` (polling should be lightweight)

### Frontend: folders.js

8. Replace single rescan button with two: Quick Scan (fa-bolt, yellow) and Full Rescan (fa-sync)
9. `scanFolder(id, btnEl, mode)` sends `?mode=` query param

### Files to modify

- `src/db.rs` — ScanMode + scan_directory_with_config + scan_folder
- `src/api.rs` — scan_folder_handler query param
- `src/tasks/mod.rs` — start_scan_folder_task ScanMode
- `src/watch.rs` — scan_active_folders uses Incremental
- `src/main.rs` — start FolderWatcher
- `frontend/pages/folders.js` — two buttons + mode param

### Acceptance Criteria

- [x] Quick Scan skips files with mtime ≤ folder.last_scanned
- [x] Fresh folder (last_scanned = NULL) does full scan regardless of mode
- [x] Full scan preserves current behavior
- [x] FolderWatcher starts at boot, polls active folders every 5 min
- [x] FolderWatcher uses incremental mode for its polls
- [x] Two buttons in UI: Quick Scan (bolt) + Full Rescan (sync)
- [x] Backend compiles (`cargo build`)
- [x] Tested with curl

---

