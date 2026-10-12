# Progress — Issue #330: Backpack resolver (union, dedup, score-sorted, limit)

- Repo: momokli/momos-music-manager
- Branch: feat/issue-330-backpack-resolver
- Base: origin/main (bd5afd8)
- Milestone: hub-backpacks-0.24.0
- PR target: main (independent, freely mergeable)

## Goal

Resolver layer for hub backpacks: given a `backpack_id`, union the tracks of all
its sources (tag / playlist / lens / track), dedup, score-sort by relevance, and
apply a limit.

Predecessors: #327 schema (`hub_backpacks` + `hub_backpack_items`), #329 sources
layer (`mmm-hub/src/backpacks.rs`). #330 adds resolution only — no UI (#328), no
active-scope (#331).

## Stage log

- [x] planner: spec
- [x] setup: branch + build baseline
- [x] developer: impl + tests
- [ ] verifier: diff/security
- [ ] tester: integration
- [ ] developer: PR
- [ ] reviewer: final

## Facts

- `mmm-hub/src/backpacks.rs` (186 lines) — sources layer; module doc explicitly
  defers resolution to #330.
- `mmm-hub/migrations/044_hub_backpacks.sql` — schema.
- `curate::resolve_track_ids` (mmm-hub/src/curate.rs) — existing resolver for
  curated playlists (tag/lens); reuse patterns.
- Track mapping:
  - tag: `hub_track_resolved_tags(tag_id, track_id)`
  - playlist: `hub_playlist_tracks(playlist_id, track_id)`
  - lens: curate::LENSES = newest | liked | unheard
  - track: literal `hub_tracks.id`
- Tests in `mmm-hub/tests/` (backpack_schema.rs, backpack_sources.rs).

## Plan

Scope: resolution only, inside `mmm-hub/src/backpacks.rs` (module `backpacks`).
No UI (#328), no active-scope (#331). Reuse `curate` style: `anyhow::Result`,
`&SqlitePool`, raw `sqlx::query_scalar` / `query_as`.

### 1. Public API

```rust
/// One resolved track: its hub id, relevance score, and the distinct backpack
/// sources that contributed it (ordered by source position, ascending).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedTrack {
    pub track_id: i64,
    pub score: i64,
    pub sources: Vec<ResolvedSource>,
}

/// A contributing source of a resolved track. Mirrors `SourceRef` (#329) so the
/// UI (#328) can label "why is this track here" without re-querying items.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ResolvedSource {
    Tag { tag_id: i64 },
    Playlist { playlist_id: i64 },
    Lens { lens: String },
    Track { track_id: i64 },
}

/// Resolve a backpack to a scored, deduped, sorted, limited track list.
/// Unknown/empty backpack -> `Ok(vec![])`. `limit` is clamped to `1..=500`.
pub async fn resolve(
    pool: &SqlitePool,
    backpack_id: i64,
    limit: i64,
) -> Result<Vec<ResolvedTrack>>
```

Field justification:
- `track_id: i64` — the only hub identifier the caller (UI/active-scope) needs;
  stable sort key, avoids leaking join rows.
- `score: i64` — number of distinct contributing sources; the whole point of
  "score-sorted" and of exposing why a track ranks where it does.
- `sources: Vec<ResolvedSource>` — provenance/badges; deduped and position-ordered
  so rendering is deterministic and a track's contributing kinds are inspectable.
- `ResolvedSource` re-uses the four `SourceKind`s and their ids/lens key, so it
  serializes directly to the UI contract without a second lookup.

Internal-only helper (private):

```rust
// (track_id, first_position) accumulation per contributing source target.
fn fold_sources(items: &[BackpackItem], resolved: Vec<(ResolvedSource, Vec<i64>)>,
                limit: i64) -> Vec<ResolvedTrack>
```
SQL stays per-kind in private async fns `tracks_for_tag`, `tracks_for_playlist`,
`tracks_for_lens`, returning `Vec<i64>` (order = the kind's own ranking).

### 2. Union + dedup semantics (exact)

- Each `hub_backpack_items` row is resolved to an ordered `Vec<i64>` of hub
  track ids (its "contribution").
- **Source identity for scoring = the source *target*, not the row:** two rows
  pointing at the same `(kind, id)`/`(kind, lens)` count once. Canonical key is
  `ResolvedSource` equality. (Duplicate rows are possible; they add no signal.)
- **Dedup:** a track id appears at most once in the output, regardless of how
  many sources/rows/repetitions contain it.
- **Score:** `score = number of distinct contributing source targets` that
  contain the track. A track in a tag *and* a playlist scores 2; in one source
  only, 1. Range: `1..=items.len()`.
- No source-position weighting in v1 (kept simple/deterministic); `score` is a
  plain distinct-source count. (Position is used only as a tie-break, §4.)
- `sources` lists exactly the distinct targets that contributed, each once.

### 3. Lens handling

- `user_id` is read from the backpack itself:  
  `SELECT user_id FROM hub_backpacks WHERE id = ?1` via `fetch_optional`.
  `None` -> return `Ok(vec![])` (unknown backpack). No user param on the API.
- Reuse the exact `curate::resolve_track_ids` lens SQL, re-scoped to that
  `user_id`:
  - `newest`: `SELECT id FROM hub_tracks ORDER BY first_seen_at DESC, id DESC`
    (hub-wide, not user-scoped — matches curate).
  - `liked`: `SELECT track_id FROM hub_liked_tracks WHERE user_id = ?1
    ORDER BY liked_at DESC`.
  - `unheard`: `SELECT t.id FROM hub_tracks t LEFT JOIN hub_v_track_traktor v
    ON v.track_id = t.id ORDER BY COALESCE(v.play_count,0) ASC,
    t.first_seen_at DESC` (hub-wide).
  - Unknown/empty lens key (shouldn't occur — validated in #329): fall back to
    `newest`, mirroring curate's `_` arm. Never errors.
- `LENSES` keys are reused verbatim; no new lens vocabulary.

### 4. Deterministic ordering + tie-break

Stable total order for the final `Vec<ResolvedTrack>`:
1. `score` **descending** (more sources first).
2. `first_position` **ascending** — the smallest `hub_backpack_items.position`
   among the track's contributing sources (earlier-listed source wins).
3. `track_id` **ascending** — final deterministic tie-break so identical
   score+position never depends on row scan order.

`first_position` is an internal sort key only (not exposed on `ResolvedTrack`).
Within `sources`, order by `position` ascending, then canonical kind order
(`tag < playlist < lens < track`) for full stability. Items are read via the
existing `list_sources` (already `ORDER BY position, id`).

### 5. Limit semantics

- Applied **after** union + dedup + sort: `out.truncate(limit)` on the sorted vec.
- Clamp exactly like curate: `let limit = limit.clamp(1, 500);`.
  - So `limit <= 0` behaves as `1` (mirrors `curate::resolve_track_ids`;
    documented in the doc-comment). `limit > 500` becomes `500`.
- No per-source `LIMIT` is applied (would corrupt cross-source scores: a track
  at rank `limit+1` in source A but also in source B must still be able to
  outrank). Per-source queries fetch the full contribution set.

### 6. Empty / unknown backpack

- `hub_backpacks` row missing -> `Ok(vec![])` (no error, via `fetch_optional`).
- Backpack exists with zero items -> each per-source loop is empty -> `Ok(vec![])`.
- A source whose target table is empty/absent still contributes nothing; never
  errors.

### 7. Test list — `mmm-hub/tests/backpack_resolver.rs`

Reuse the `setup()`/seed-helper conventions from `backpack_sources.rs`
(tempdir SQLite, `foreign_keys(true)`, `sqlx::migrate!`), adding helpers:
`insert_resolved_tag(tag_id, track_id)`, `insert_playlist_track(playlist_id,
 track_id, position)`, `insert_liked(user_id, track_id, liked_at)`,
`insert_traktor_play(track_id, play_count)` (or rely on missing view rows = 0).
`TS = "2026-01-01T00:00:00+00:00"`.

1. `union_across_all_four_kinds` — seed tag(T1,T2), playlist(P: T3), lens
   `newest` (tracks T4,T5), literal track T6; `resolve(bp, 100)` returns the
   union; assert set equality `{T1..T6}` and len 6.
2. `track_in_two_sources_is_deduped_and_scored_two` — T1 in tag *and* playlist;
   assert T1 appears exactly once, `score == 2`, and `sources` contains both the
   `Tag` and `Playlist` variants.
3. `score_ordering_desc` — T1 in 2 sources, T2 in 1 source; assert T1 precedes
   T2 and `scores == [2, 1]`.
4. `limit_truncates_after_sort` — 5 tracks, `limit = 2`; assert exactly 2
   tracks and they are the two highest scores (not insertion order).
5. `limit_is_clamped` — `limit = 0` -> len <= 1 (clamped to 1); `limit = 9999`
   with 3 tracks -> len == 3 (clamp high).
6. `deterministic_tie_break_by_position_then_id` — two tracks each from exactly
   one source with equal score; assert the earlier-`position` source's track is
   first; with equal score+position, assert ascending `track_id`. Run `resolve`
   twice and assert byte-identical `Vec<ResolvedTrack>`.
7. `lens_liked_is_user_scoped` — two users, each with likes; assert the
   backpack's `user_id` (from `hub_backpacks`) governs `liked` results — other
   user's likes are absent.
8. `lens_unheard_orders_by_play_count` — T_low play_count 0, T_high play_count
   9; assert T_low precedes T_high.
9. `empty_and_unknown_backpack_yield_empty` — `resolve(999, 50)` -> empty;
   real backpack with no items -> empty.
10. `unknown_lens_key_falls_back_to_newest` — force-insert a lens item with a
    bogus key directly (bypassing #329 validation); assert `resolve` does not
    error and returns newest tracks (documents the defensive fallback).
11. `sources_listed_once_each_in_position_order` — tag(pos 1) + playlist(pos 2)
    both contain T1; assert `sources == [Tag, Playlist]` in that order and no
    duplicates.

Exact assertions use `Vec<ResolvedTrack>` equality and `iter().map(|r|
 r.track_id).collect::<Vec<_>>()` comparisons.

### 8. Files to add/modify + definition of done

Add / modify:
- `mmm-hub/src/backpacks.rs` — add `ResolvedTrack`, `ResolvedSource`, `resolve`,
  and private per-kind helpers (`tracks_for_tag/playlist/lens`, `fold_sources`).
  Update the module doc to drop the "no resolution" footnote for #330.
- `mmm-hub/tests/backpack_resolver.rs` — new integration test file (§7).
- `progress-feat-issue-330-backpack-resolver.md` — this plan + stage log ticks.

No migration, no schema change (read-only over #327/#329 tables). No new deps
(`serde` already used via `serde_json`; add `serde::{Serialize}` only if the
crate already depends on `serde` — otherwise keep plain derives and skip
`serde::Serialize`).

Definition of done:
- `cargo build -p mmm-hub` succeeds (warnings clean for new code).
- `cargo test -p mmm-hub` passes, including all §7 tests.
- No public API beyond §1; nothing touches #328 UI or #331 active-scope.
- `resolve` never panics and returns `Ok(vec![])` for unknown/empty backpacks.

### Summary

Add one read-only resolver `backpacks::resolve(pool, backpack_id, limit) ->
Result<Vec<ResolvedTrack>>` in `mmm-hub/src/backpacks.rs`. It reads the
backpack's `user_id` (for lens scoping), resolves each source item to hub track
ids (tag via `hub_track_resolved_tags`, playlist via `hub_playlist_tracks`, lens
via the reused `curate` rules, track literally), unions + dedups by `track_id`,
scores by distinct contributing source targets, sorts by `score desc,
first-position asc, track_id asc`, clamps `limit` to `1..=500`, and returns
`ResolvedTrack { track_id, score, sources }`. Unknown/empty backpack -> empty
`Vec`. Covered by 11 integration tests in `mmm-hub/tests/backpack_resolver.rs`.

## Dev

Implemented issue #330 in the `mmm-hub` crate. Resolution now lives in
`mmm-hub/src/backpacks.rs` (module doc updated to drop the "no resolution (#330)"
footnote).

### Files changed
- `mmm-hub/src/backpacks.rs` — added `ResolvedTrack`, `ResolvedSource`
  (`serde::Serialize`; `Hash` also derived so `ResolvedSource` can key the
  source-dedup `HashSet`), `pub async fn resolve(pool, backpack_id, limit)`, and
  private helpers `fold_sources`, `tracks_for_tag`, `tracks_for_playlist`,
  `tracks_for_lens`, `kind_rank`. Reads `user_id` via
  `SELECT user_id FROM hub_backpacks WHERE id=?1` `fetch_optional`
  (`None -> Ok(vec![])`). tag -> `hub_track_resolved_tags`; playlist ->
  `hub_playlist_tracks ORDER BY position`; lens -> reused `curate` rules
  (newest = hub-wide `hub_tracks` by `first_seen_at DESC, id DESC`; liked =
  user-scoped `hub_liked_tracks ORDER BY liked_at DESC`; unheard =
  `COALESCE(play_count,0) ASC, first_seen_at DESC` via `hub_v_track_traktor`;
  unknown -> newest, never errors); literal track -> the id. Union + dedup by
  `track_id`; score = distinct contributing source targets; sort by
  `score desc, first_position asc, track_id asc`; `limit` clamped to `1..=500`
  (applied post-sort via `truncate`). `sources` ordered by item `position` asc
  then kind rank `tag < playlist < lens < track`.
- `mmm-hub/tests/backpack_resolver.rs` — new; all 11 tests from §7 of the plan,
  using the `setup()`/seed-helper conventions from `backpack_sources.rs`
  (`insert_resolved_tag`, `insert_playlist_track`, `insert_liked`,
  `insert_traktor_play`, plus `insert_raw_lens_item` for the bogus-lens case).
- `progress-feat-issue-330-backpack-resolver.md` — stage log + this section.

### Test result

`cargo test -p mmm-hub` (run as `cd mmm-hub && cargo test`, see deviation):

```
Running tests/backpack_resolver.rs
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Full `cargo test` for the crate: **278 passed, 0 failed** across all targets
(lib unit 45; resolver 11; backpack_schema 7; backpack_sources 18; curate 13;
ui 53; and all other suites green). `cargo build` succeeds with no warnings from
the new code (only a pre-existing `dead_code` warning in `src/web.rs`).

### Deviation from the plan
- Build/test invocation: from the repo root `cargo build -p mmm-hub` does **not**
  resolve — `mmm-hub` is a standalone crate (own `Cargo.toml`/`Cargo.lock`), not
  a member of the root workspace. Ran `cargo build` / `cargo test` from inside
  `mmm-hub/` instead; results equivalent.
- `ResolvedSource` additionally derives `Hash` (needed to dedup source targets
  in a `HashSet`); plan only listed `Serialize`/`Clone`/`PartialEq`/`Eq`.
- `fold_sources` takes the private contribution tuple
  `&[(ResolvedSource, i64, Vec<i64>)]` (source, item position, track ids) rather
  than the plan's illustrative `(&[BackpackItem], Vec<(ResolvedSource, Vec<i64>)>)`
  signature — the position is needed for the specified tie-break and folding it
  in at read time avoids a second lookup. Semantics unchanged.
- `tracks_for_tag` adds `ORDER BY track_id` for deterministic contribution order
  (plan left the tag query unordered; output order is fixed by the global sort
  regardless).
