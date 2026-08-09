## Plan: tag-bundles

**Status**: done ✅
**Branch**: `feat/tag-bundles`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `017_tag_bundles.sql`

### Description

New concept: a "bundle tag" aggregates multiple member tags. Files with any member tag also get the bundle tag. This is ADDITIVE (members stay visible, bundle appears additionally). Used so the user can filter by a single tag in Traktor + add a BPM range on top — solving Traktor's smartlist limitation (can't do OR-of-multiple-tags AND BPM).

Unlike `tag_parents` (which does SUBSTITUTION — Setlist tag replaced by its P/M/V/E parents for comment writing), tag bundles are purely aggregative and work for any tag category.

### New table

```sql
CREATE TABLE tag_bundles (
    id INTEGER PRIMARY KEY,
    bundle_tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    member_tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE (bundle_tag_id, member_tag_id)
);
```

### Resolution

Extended `refresh_file_resolved_tags()` and `refresh_track_resolved_tags()` with transitive bundle resolution loop (fixed-point iteration). After the existing view-based INSERT, repeatedly finds bundle tags whose members are present in resolved tags, inserts the bundle tag too. Repeats until no new rows (handles multi-level: A→B→C). 20-iteration safety limit.

### Backend endpoints

- `GET /api/tags/bundles?search=X` — list bundle tags with member counts
- `GET /api/tags/{id}/bundle-members` — member tags of a bundle
- `PUT /api/tags/{id}/bundle-members` — set members with validation (existence, self-ref, cycle detection via DFS)
- `GET /api/tags/{id}/bundle-of` — which bundles is this tag a member of?

### Frontend

New `#tag-bundles` SPA page with two-panel layout:

- **Left**: searchable list of bundle tags with member counts
- **Right**: selected bundle → member chips with category badges, typeahead search to add, × to remove, auto-save on every change
- "New Tag" button → creates Setlist tag, opens for member assignment
- File preview section showing first 10 files with this bundle tag

### Comment output

Bundle tags are Setlist category, so they appear in the tags section of the comment automatically (via `file_resolved_tags` which now includes bundle resolution):

```
[PMV] spät afterhour schnell afterhour-jonas sp:xxx
```

In Traktor: filter comment → contains `afterhour-jonas` AND BPM 120-130 ✅

### Files created/modified

| File                             | Change                                                                                                                            |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `migrations/017_tag_bundles.sql` | New migration                                                                                                                     |
| `src/db/tags.rs`                 | 5 new functions: `get_bundle_members`, `get_bundle_of`, `check_bundle_cycle`, `set_bundle_members`, `get_bundle_tags_with_counts` |
| `src/db/playlists.rs`            | Extended `refresh_file_resolved_tags` + `refresh_track_resolved_tags` with bundle transitive closure                              |
| `src/api/tags.rs`                | 4 new handlers + 3 new routes                                                                                                     |
| `frontend/pages/tag-bundles.js`  | New SPA page (~680 lines)                                                                                                         |
| `frontend/app.js`                | Register route                                                                                                                    |
| `frontend/shared/nav.js`         | Add nav link                                                                                                                      |
| `frontend/style.css`             | Bundle page styles                                                                                                                |
| `tests/api_tags.rs`              | 6 integration tests                                                                                                               |

### Acceptance Criteria

- [x] Migration 017 runs cleanly
- [x] Bundle member CRUD works (set, get, reverse lookup)
- [x] Cycle detection rejects circular bundles
- [x] Self-reference rejected with 400
- [x] `refresh_file_resolved_tags` includes bundle resolution transitively
- [x] Multi-level bundles resolve correctly (A→B→C)
- [x] `#tag-bundles` page renders with two-panel layout
- [x] Typeahead search + chipping works for adding/removing members
- [x] Auto-save on member changes
- [x] `cargo build` passes
- [x] All 379 lib + 41 API tags integration tests pass

---

