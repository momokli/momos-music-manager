## Plan: cleanup-feat-current-wip

**Status**: done ✅
**Branch**: `feat/current-wip`
**Migration needed**: no

### Description

Preserved all unstaged work from the old `main` into a feature branch for cherry-picking later.

### Files modified

All 27 files that had uncommitted changes + deleted modules (`src/scan_cache.rs`, `src/spotify/replay.rs`).

### Completed

- [x] `git stash --include-untracked` on `main`
- [x] `git stash branch feat/current-wip` — creates branch + applies stash
- [x] `git add -A && git commit -m "wip: current working state before workflow reset"`
- [x] `git checkout main` — working tree clean, ready for new feature branches

---

---

