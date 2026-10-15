# Progress — Issue #367 · `feat(kellner): backpack maintenance locally`

- **Branch**: `feature/issue-367-kellner-backpack` (off `origin/main` @ 17f625a)
- **PR target**: `main` (independent, freely mergeable) · **PR body**: `Closes #367`
- **Milestone**: kellner-0.1.0 (#37)
- **Plan leaves**: `P2-5` (prune soft-move), `P2-6` (integration), `P3-1…P3-4` (comment templating) — plus the
  mandatory hub read (`/api/hub/backpacks` + `/export`, plan §3 / `P2-1`) which no other GH issue delivered.
- **Issue body**: "Sync active backpacks to disk."

## Ist-Stand (RE, 2026-10-15)

- `kellner` has `pair` (#362), `traktor-upload` (#364), `backup` (#365), `downloads` (#366).
  `downloads` orders/polls/files **explicit** ISRCs — it deliberately does **no** backpack diffing/tagging/prune.
- `kellner backpack` is still a **stub** (`not_yet("backpack")`).
- The hub has **no** JSON backpack read endpoint for devices — only session-gated HTML pages
  (`/backpacks`, `/backpacks/{id}/export?format=m3u8|traktor|zip`). Device-token auth primitive already
  exists (`devices::find_bound_by_token_hash`, used by `traktor_api.rs`).
- `backpack_membership`/`sync_state` manifest (plan `P0-2`) was never built; kellner uses a JSON `state.json`.

## Scope of this PR

**A) music-hub — device-token-gated backpack read API** (new `music-hub/src/backpack_api.rs`)
- `GET /api/hub/backpacks` (Bearer device token) →
  `{"backpacks":[{"id":1,"name":"…","is_active":true}, …]}`
- `GET /api/hub/backpacks/{id}/export` (Bearer device token) →
  `{"backpack":{"id":1,"name":"…"},"tracks":[
     {"track_id":9,"isrc":"DE…","title":"…","artists":"…","tags":["a","b"],
      "bpm":128.0,"key":"A-Minor","energy":7.0}, …]}`
  - `tags` = `hub_tags.name` via `hub_track_resolved_tags` (owner-scoped), alphabetical.
  - `bpm`/`key`/`energy` from `hub_track_audio` (nullable → omitted/`null`).
  - `isrc`/`title`/`artists` from `hub_tracks`.
  - Owner check via `user_id` from the bound device + `backpack_owned_by` (foreign/unknown → `404`).
  - Route mounted in `api.rs` next to `devices::router` / `traktor_api::router`.

**B) kellner — `backpack` command** (new `src/sync/backpack.rs`, `src/tag/comment.rs`)
- `kellner backpack [--root DIR] [--format flac|320|128] [--priority N] [--dry-run] [--no-tags]`
- Fetch active backpacks from hub → export each → union desired rows by ISRC.
- Diff against local state/disk → order only missing ISRCs (`POST /orders`) → poll → fetch ready files
  → write flat `<isrc>.<ext>` (reuse `downloads::write_atomic`).
- Render the resolved tag set into the file's **Comment** field per format (lofty), canonical content hash
  (Comment cleared) recorded (`P3-3`); unchanged render ⇒ no rewrite (mtime preserved).
- **Prune** (`P2-5`): `backpack_membership` ISRCs no longer in any active backpack are **soft-moved** to
  `<root>/.kellner-trash/` — never hard-deleted, never touch user `downloads` entries.
- `state.json` gains `backpack_membership: {isrc → {path, backpack_id, tags_rendered, content_hash, downloaded_at}}`.
- Integration test `kellner/tests/backpack_sync.rs` (mock hub + mock music-api): fetch-once, record, prune.

## Akzeptanzkriterien
- [ ] `cd music-hub && cargo test` green (incl. new backpack_api tests: auth, shape, scoping, empty→[]).
- [ ] `cd kellner && cargo build && cargo test` green (unit + backpack_sync integration).
- [ ] No hard delete anywhere; `user`-sourced files never pruned.
- [ ] Comment write is format-correct and does not change the canonical (dedup) hash.
- [ ] `closes #367` in the PR body; Conventional-Commit title.

## Log
- 2026-10-15: RE done, branch created, spec written. → developer (hub A).
