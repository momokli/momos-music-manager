# Changelog

All notable changes to **music-api**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `api-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

## [0.6.0] - 2026-11-19

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
