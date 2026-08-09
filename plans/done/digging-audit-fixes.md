## Plan: digging-audit-fixes

**Status**: done ✅
**Branch**: `fix/digging-audit`
**Ready for review**: no
**Depends on**: `feat/digging-filter-row`
**Migration needed**: no

### Description

Fix issues discovered during digging page audit: playback, card tag display, rating data, filter wiring verification.

### Issue 1: Playback

`pickAudioFile()` only accepted `location === "local"`. All production files are `location: "backup"`. Fixed to accept any file (prefers FLAC > stem.m4a). Verify `/api/files/{id}/stream` works for backup files.

### Issue 2: Card tags

Tags split into PHASE (with ⚡), MOOD, VIBE, TAGS rows by category prefix. Removed playlist badges (duplicated tag names). Removed averaged ⚡ badge.

### Issue 3: Rating

All ratings are 0. Traktor RANKING may not be in collection.nml. Show stars only when > 0.

### Issue 4: Filter wiring audit

Verify all filters (PMV, KEY, Phase, Energy, BPM, Tags, Search, Sort) work end-to-end with curl tests.

### Acceptance Criteria

- [ ] Playback works for tracks with any file (any location)
- [ ] Card tags organized by category with PHASE/MOOD/VIBE/TAGS rows
- [ ] No duplicate tag display
- [ ] Rating stars when > 0
- [ ] All filters verified end-to-end
- [ ] `cargo build` passes

---

