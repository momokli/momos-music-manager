# Changelog

All notable changes to **music-api**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `api-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

### Added

- **`track_files` catalogue + `archive-backfill` + `/storage/stats`** (#700) — one
  additive table maps every physical file to `(isrc, role, stem_part)`; the
  `music-api archive-backfill <--dry-run|--apply>` CLI fills it from
  `tracks.path_*`, the object store (bundle ISRC from the tag/column, stem WAVs
  inherit it by base name) and legacy `store_objects.isrc` copies. Read-only on
  media files and idempotent: a second `--apply` writes nothing, `--dry-run`
  writes no rows and lists unattributable files as `unmanaged`.
  `GET /storage/stats` now aggregates `track_files` only (files/bytes per role,
  `tracksWithoutFiles`, `incompleteTracks`, `unmanagedFiles`) instead of
  scanning per request; `servableButNotReady` (#699) stays as a DB-only counter.
- **Album length in the artist album list** (#621) — `GET /deezer/artist/{id}/albums`
  now carries `seconds` per album, filled from the own catalogue
  (`catalogue_albums.seconds`) in one batched query. `null` while the album is
  not catalogued, and never baked into the page cache (the hint is re-read per
  request, so it cannot go stale).
- **`GET /storage/stats` and `repair-states` CLI** (#699) — visibility and repair
  for the format-completeness fix: `servableButNotReady` (must be `0`) and
  `pathsMissingOnDisk`, plus `music-api repair-states [--dry-run|--apply]`.

### Fixed

- **Format completeness instead of a quality rank** (#699) — a track is
  `ready` while any requested format is servable; paths are additive and a
  failed attempt never degrades a servable row.
  - `tracks.state` is now derived by a single writer, `db::recompute_state`,
    from the on-disk formats (`path_*`) and the additive demand set
    (`tracks.wanted_formats`, new column).
  - `mark_ready` writes each `path_*` additively (`COALESCE`) — a download at
    another quality no longer clears a path already on disk.
  - `mark_terminal` records the outcome in the new `tracks.terminal_state` and
    keeps `error`; a row with a servable format stays `ready` instead of being
    degraded to `failed`/`absent` (~2,500 rows in production).
  - Demand is a **format set** (`wanted_formats`, additive across orders) rather
    than a scalar tier: re-arm happens only for a genuinely new, not-yet-servable
    format, and never again for a format that terminally failed.
  - `kellner` and `music-hub` gate availability on the delivered `formats`, not
    on `state == "ready"`, so those tracks are fetched and shown again.
  - `repair-states --apply` stats every path first and only then re-derives the
    state; `error` is preserved.

## [0.7.0] - 2026-11-19

Native Deezer download is the only path — the deemix sidecar is removed
entirely (#518).

### Added

- **Deezer metadata read endpoints** for the hub — `GET /deezer/artist/{id}/albums`,
  `GET /deezer/album/{id}` and `GET /deezer/search/artist`. Each handler makes
  exactly one upstream call; unknown ids answer `404`, transport/quota failures
  `502`. Tracks without an ISRC are kept (`isrc: null`).
- **Native download is the only path** (#518) — one download implementation
  (`deezer_dl`); the ISRC tag is written before the file is stored.
- New `DOWNLOAD_QUALITY` env (`flac` | `mp3-320` | `mp3-128`, default `flac`),
  fail-fast on an invalid value.

### Changed

- **Env rename** — `DEEMIX_ARL` → `DEEZER_ARL`. The old name is still read as a
  fallback with a one-time warning (variable name only, never the value).
- `GET /queue` is a native counts view; `GET /health` and `GET /worker` no
  longer carry `backend`/`sidecarConfigured` (the cached `arlTier` stays).
- DB column `tracks.deemix_uuid` renamed to `download_ref` via an idempotent
  in-place migration (no `DROP`); existing values are preserved.
- `POST /work` rejects `kind` values with a `crawl:` prefix with `409` (there is
  no crawl worker any more).

### Removed

- **The deemix sidecar, entirely** — `DEEMIX_URL`, `DEEMIX_BITRATE`,
  `DEEMIX_DOWNLOAD_DIR` and `DOWNLOAD_BACKEND`; `src/deemix.rs`; the
  `deploy/deemix-api.compose.yml` container and every deploy step around it.
- **Artist-order API** — `POST/GET /artist-orders` and the underlying
  `artist_orders` persistence. Pre-existing `artist_orders` /
  `hub_deemix_downloads` tables stay in old databases (never dropped) but are no
  longer read or written.
- The `backend`/`sidecarConfigured` fields and the hub ops "Quelle" badge.

## [0.5.0]

Native Deezer downloads — music-api fetches the audio itself (128/320/FLAC). No
deemix sidecar, no Node, no Docker, no `/api/getQueue` polling on the native path.

### Added

- **Native Deezer download in the worker + ISRC tag** (#510) — `deezer_dl` drives
  the download directly; the ISRC tag is written into the file via `lofty` (MP3:
  ID3 `TSRC`, FLAC: Vorbis `ISRC`) since Rusteer writes none. Per-track run-log
  `"{isrc}: ready (native, {flac|mp3-320|mp3-128}, {Xs})"`.
- **`deezer_dl` — vendored Rusteer (MIT)** (#509) — new `music-api/src/deezer_dl/`
  module: `crypto/` + gateway, trimmed to the download set (no tagging/lofty).
  Small public API (`login`, `download_track`, `Quality { Mp3_128, Mp3_320, Flac }`,
  explicit errors). MIT notice + upstream URL in `music-api/src/deezer_dl/LICENSE`
  and `music-api/THIRD_PARTY.md`.

### Changed

- **deemix sidecar optional via `DEEMIX_URL`, no queue poll in native mode** (#511)
  — `DEEMIX_URL` has no default any more; new `DOWNLOAD_BACKEND` default is `auto`
  (no URL ⇒ native only; URL without ARL ⇒ sidecar; URL + ARL ⇒ native). Explicit
  `sidecar` without a URL fails fast. Native mode early-returns in
  `advance_downloads`/`advance_artist_orders`; `GET /queue` answers from the DB
  (`backend:"native"`). `GET /health` + `GET /worker` expose `backend`,
  `sidecarConfigured`, `arlTier` (no `userId`, no network in the health path); the
  music-hub ops console shows the source/ARL badges.
- Upgrade note: hosts that left `DEEMIX_URL` unset ran the old default sidecar and
  now run native (intended #511 semantics).

### Fixed

- **deemix stack mismatch** (`DEEMIX_URL :6599` missing from the Compose) (#514) —
  the stopgap is superseded by the native download path: downloads no longer depend
  on the masked Docker/deemix stack.
