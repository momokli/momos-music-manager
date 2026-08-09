## Plan: relax-prune-safety-gates

**Status**: proposed
**Branch**: `feat/fix-backpack-local-tracking` (current)
**Depends on**: auto-prune done ✅
**Migration needed**: no

### Description

Remove two overly-strict safety gates from `get_prune_candidates()` that block
legitimate prunes. The user trusts backup — if a file is on the NAS, it's safe
to delete locally.

### Problem

| Gate                                                 | Blocks               | Why                         |
| ---------------------------------------------------- | -------------------- | --------------------------- |
| `source_of IS NOT NULL` for WAVs                     | 1,242 backed-up WAVs | Never linked to parent stem |
| `bpm IS NOT NULL OR comment IS NOT NULL` for non-WAV | ~533 backed-up FLACs | Audiobooks, unscanned files |

### Fix

**`src/db/storage.rs`** — simplify `get_prune_candidates()` step 1 query.

Before:

```sql
WHERE fl.location_type = 'backup'
  AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL))
  AND (f.file_type = 'wav' OR f.bpm IS NOT NULL OR (f.comment IS NOT NULL AND f.comment != ''))
  AND EXISTS (file_locations WHERE location_type = 'local')
```

After:

```sql
WHERE fl.location_type = 'backup'
  AND EXISTS (file_locations WHERE location_type = 'local')
```

A file is safe to delete if: backed up + local + not in backpack. No other gates.

### TDD: Agent Decomposition

Single agent — one file, one change:

**Agent: Relax prune gates**

1. Read `src/db/storage.rs:520-540` to see the current query
2. Update the `get_prune_candidates` test in `src/db/storage.rs` `#[cfg(test)]` to expect more candidates (add a file without metadata but with backup+local)
3. Remove the two safety gates from the SQL
4. Verify: `cargo test --lib -- db::storage` + `cargo build`

### Acceptance Criteria

- [ ] 1,242 WAVs become prune candidates (pending `source_of`)
- [ ] ~533 metadata-less FLACs become prune candidates
- [ ] Backpack-protected files still excluded
- [ ] Non-backed-up files still excluded
- [ ] Files without `file_locations.local` still excluded
- [ ] `cargo build` + `cargo test` pass

---

