# Changelog

All notable changes to **music-api**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `api-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

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
