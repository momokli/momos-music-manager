## Plan: storage-holistic-cleanup

**Status**: proposed
**Branch**: `fix/storage-holistic-cleanup`
**Ready for review**: no
**Depends on**: `feat/file-lifecycle-management` (already merged)
**Migration needed**: no

### Audit: Current state

| Data point           | Value                                           |
| -------------------- | ----------------------------------------------- |
| Total files          | 5,006 (1,770 stems + 2,104 FLACs + 1,132 WAVs)  |
| Total size           | 196.8 GB                                        |
| Backed up            | 3,167 files (1,762 stems + 1,405 FLACs)         |
| ISRCs with stem+FLAC | 682 (redundant FLACs ~60 GB)                    |
| WAVs from subdirs    | 1,132 indexed, 0 backed up, 0 with source_of    |
| Prune candidates     | 2,962 (too high — includes 682 redundant FLACs) |

### Problem #1: No format preference for pruning

When a track (same ISRC) has a `.stem.m4a` version, other formats (FLAC, MP3, WAV) are redundant locally. The nuo-stems workflow is: convert FLAC to stem, keep stem, archive FLAC to NAS. Currently 682 FLACs have a corresponding stem but both count as "kept".

**Fix**: Global "Prefer stem files" toggle in Storage page. When on, the prune query excludes FLACs/MP3s/WAVs whose same-ISRC stem exists. This converts 682 redundant FLACs into valid prune candidates.

**Storage**: Toggle persisted as `stem_preferred` in a config store (service_config table or new column on Settings).

**Prune query change** — add AND NOT clause:

```
AND NOT (
    f.file_type != 'stem.m4a'
    AND EXISTS (
        SELECT 1 FROM files f2
        WHERE f2.isrc = f.isrc AND f2.isrc IS NOT NULL
        AND f2.file_type = 'stem.m4a'
    )
)
```

### Problem #2: WAV source tracking incomplete

1,132 WAVs are indexed (from subdirs, since scan_recursive=true reached them), but:

- `source_of` is never populated (no linking to parent stem)
- `wav_source_dirs` in StorageStatus counts 0 because it queries `source_of IS NOT NULL`
- WAVs aren't tracked as source files vs independent files

**Fix**: After scanner indexes WAVs from subdirs, post-process to set `source_of`. Match: directory name (without extension) → stem filename in parent dir.

### Problem #3: Storage page layout is messy

Current layout mixes file types oddly (FLACs as subtitle of Stems card), WAV Sources card is confusing, and there's no size breakdown per file type.

**Fix**: Clean card layout:

- Row 1: Local Files | Backed Up | Prune Candidates (summary)
- Row 2: Per-type breakdown with sizes (stems, FLACs, WAVs, MP3s)
- Stem preference toggle section
- Folders section (keep as-is, already nice)

Add size fields to StorageStatus: `local_stems_size`, `local_flacs_size`, `local_wavs_size`, `local_mp3s_size`.

### Files to modify

- `src/db.rs` — add `stem_preferred` config, per-type size fields, update prune query, fix wav_source_dirs
- `src/api.rs` — add `GET/PUT /api/storage/settings`, update StorageStatus construction
- `frontend/pages/storage.js` — overhaul layout, stem preference toggle, per-type sizes
- `frontend/style.css` — storage layout styles

### Acceptance Criteria

- [ ] Stem preference toggle shows in Storage page, persists correctly
- [ ] With stem_preferred=true, 682 FLACs with same-ISRC stem become prune candidates
- [ ] With stem_preferred=false, current behavior preserved
- [ ] WAV source_of populated by scanner for subdir WAVs
- [ ] StorageStatus includes per-type size breakdown
- [ ] Clean card layout — no format treated as subtitle
- [ ] `cargo build` passes
- [ ] No regression to backup/reconcile/prune

### Problem #4: Tag file counts don't include parent-resolved files

`v_tag_file_counts` uses `v_file_tags` (direct tag→playlist matching). But `v_file_resolved_tags` already exists and correctly resolves parent tags. The fix: either update `v_tag_file_counts` to use `v_file_resolved_tags`, or create a new `v_resolved_tag_file_counts` view and use it in the Tags page.

Similarly, `get_tags_count` and `get_all_tags` use `v_tag_file_counts`. Change to `v_file_resolved_tags`.

**Example**: "Droid House" has parent "house". Currently "house" shows 0 files. After fix: "house" shows 571+ files (sum of all child tags).

**Fix**:

- Create or update `v_tag_file_counts` to join through `v_file_resolved_tags`
- Update `get_all_tags` SQL to use the new count source

### Problem #5: Tag edit modal doesn't show parent tags

The modal in the Tags page (tag edit flow) only shows name + category selector. It should also show:

- Current parent tags as chips with category badges
- Button to navigate to Tag Curation page for full parent management

**Fix**: Add parent tag chips section to `showEditTagModal`, populated from `GET /api/tags/{id}/parents`.

---

