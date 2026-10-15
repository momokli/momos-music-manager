# Changelog

All notable changes to **music-api**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `api-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

## [0.4.0] — 2026-10-15

**music-api-0.4.0** — the long-term backup store becomes a permanent source for
`music-api` (an ISRC resolves to `stems`/`flac`/`mp3` and streams directly), and a
whole order can be fetched as a single ZIP archive.

### Added

- **Backup store as permanent source** (#359, PR #446): the long-term backup store
  becomes a permanent source — an ISRC now resolves to `stems`/`flac`/`mp3` and
  streams directly. New `GET /isrc/{isrc}/objects` lists the resolvable objects for
  an ISRC; the `get_file` fallback becomes local → store (local file first, else
  store); `PUT` accepts the `X-Group-Key` / `X-Stem-Type` upload headers; `/objects`
  and `/tracks` metadata are fed from the store. Purely additive, no migration;
  scope `music-api/`. `cargo test`: **100 passed / 0 failed**.
- **Whole-playlist download as an archive** (#358, PR #447):
  `GET /orders/{id}/archive[?format=flac|320|128]` returns the complete order as a
  single ZIP (`application/zip`,
  `Content-Disposition: attachment; filename="order-<id>.zip"`). Non-ready or
  missing tracks are skipped (no hard fail); without `format` the best-available tier
  is chosen per track (`flac` → `320` → `128`); audio is stored (no recompression)
  and streamed from an unlinked temp file. Status codes: `400` invalid `format`,
  `404` unknown order, `409` no servable tracks. Shares `resolve_servable()` with
  `get_file`. `cargo test`: **36 + 77 passed, 0 failed**.

### Notes

- **Priority + control panel** (#360) shipped **no deliverable**: high-prio orders,
  pause/resume and retry from the UI are already implemented on `main`
  (`POST /orders/{id}/priority`, `POST /isrc/{isrc}/retry`,
  `POST /worker/pause|resume`, `GET /worker`, `GET /queue`, plus the
  `/music-api` control console in the Hub; commit `714a668`, #259). The issue was
  closed by triage as `triage:no-action` — no new change in this release.
- Version note: the milestone is **music-api-0.4.0**, so the release tag is
  `api-v0.4.0`. A seed tag `api-v0.4.0` already exists (created on the PR #393
  commit, not on an api feature release). This branch is stacked on the open
  `release/music-api-0.3.0` PR (#445), which records the crate as `0.3.0`; this PR
  bumps it to `0.4.0`. See #445 for the tag-line decision.

## [0.3.0] — 2026-10-15

**music-api-0.3.0** — music-api becomes the metadata authority: an ISRC→BPM/key
backfill feeds `hub_track_features`, a content-addressed object store gains explicit
pruning, and YouTube joins SoundCloud as an acquisition source.

### Added

- **BPM/key backfill from the metadata authority** (#354, PR #442): the Hub client
  queries music-api's metadata range API and writes BPM/key back into
  `hub_track_features` via `backfill_from_music_api` with `source='music-api'`
  (migration 050 adds `metadata_checked_at`). New CLI subcommand `metadata-backfill`
  and a console button „BPM/Key holen". music-api itself stays unchanged (read-only
  client in the Hub).
- **Object store prune** (#355, PR #443): `DELETE /objects/{sha256}` (idempotent,
  204/404/400), consumer-driven `POST /objects/prune` (`{hashes,dryRun}` →
  `{deleted,missing,freedBytes,dryRun}`), `POST /objects/prune/orphans` (reconcile
  files-without-rows / rows-without-files, optional stale-tmp sweep) and
  `GET /objects/stats` (`{count,totalBytes}`). Explicit only — no auto/time/size-based
  deletion. Purely additive, no migration; existing object routes untouched.
- **YouTube source** (#356, PR #444): tracks can be ordered as
  `{"source":"youtube","externalId":"<videoId>"}` and are resolved via yt-dlp
  (new `ytdlp::youtube_url`), symmetric to SoundCloud. The worker dispatches by source
  (`run_non_deezer`; shared finalizer renamed `finalize_soundcloud` → `finalize_external`).
  YouTube is a lossy source — the download lands as `mp3-320`, never FLAC. README
  documents the non-Deezer order shape.

### Notes

- **Native metadata range route for the Hub** (#357) shipped **no deliverable**: the
  route already exists (metadata range API + Hub push, #302) and the issue was closed
  by triage as `triage:no-action`.
- Version note: the milestone is **music-api-0.3.0**, so the release tag is
  `api-v0.3.0`. `music-api/Cargo.toml` had been pre-set to `0.4.0` (PR #393) and a
  seed tag `api-v0.4.0` exists; this release records the crate as `0.3.0` — see the
  release PR discussion.
