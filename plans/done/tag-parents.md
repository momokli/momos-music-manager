## Plan: tag-parents

**Status**: done ✅
**Branch**: `feat/tag-parents`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — merged into `002_playlist_fetch_tracking.sql`

### Description

Allow Setlist-category tags (long playlist names) to have "parent" tags that replace them in file comments. A Setlist tag like `Dark Techno/2026/Hardtechno/...` resolves to parent tags `dark` (Mood), `techno` (Vibe), `hard` (Merkmal). Comments use the parent tag names and categories instead of the long original. Only Setlist tags can have parents; P/M/V/E tags cannot.

### Schema

- **`tag_parents`** table: `(id, tag_id, parent_tag_id, created_at)` with UNIQUE(tag_id, parent_tag_id)
- **`v_resolved_tags`** view: for each tag, returns parent tags if they exist, otherwise the tag itself
- **`v_file_resolved_tags`** view: like `v_file_tags` but resolves through `v_resolved_tags`

### Backend Changes

- **`src/db.rs`**: `get_tag_parents()`, `get_tag_children()`, `set_tag_parents()` (with validation: Setlist-only, no self-ref, parents must exist)
- **`src/db.rs`**: `compute_target_comment()` now queries `v_file_resolved_tags` instead of `v_file_tags`
- **`src/api.rs`**: `GET /api/tags/{id}/parents`, `PUT /api/tags/{id}/parents`, `GET /api/tags/{id}/children`

### Frontend Changes

- **`frontend/pages/tags.js`**: Edit modal shows "Parent Tags" section for Setlist tags with typeahead search, chip management, and save

### Acceptance Criteria

- [x] Setlist tags can be assigned parent tags via API and frontend
- [x] Non-Setlist tags rejected with clear error
- [x] Self-reference prevented
- [x] Non-existent parent tags rejected
- [x] `compute_target_comment` uses resolved parent tags (names + categories)
- [x] Comment PMV indicators reflect parent tag categories
- [x] Tags without parents work as before (backward compatible)
- [x] Backend compiles (`cargo build`)
- [x] Migration runs cleanly
- [x] Tested with curl

---

