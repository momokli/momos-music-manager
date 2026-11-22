# music-api

An **ISRC order/consume API** that downloads Deezer audio natively. Consumers
(Momo's Music Manager) place _orders_ of ISRCs; this service resolves them on
Deezer, downloads the best available quality **itself** (tagging the ISRC),
derives the lossy variants and serves the files back by ISRC. Non-Deezer sources
(SoundCloud, YouTube) go through yt-dlp.

```
MMM ──POST /orders {isrc…}──▶ music-api ──▶ api.deezer.com  (ISRC → track, no auth)
                                  │
                                  ├──▶ native Deezer download (deezer_dl, ISRC tag)  ──▶ store
                                  │
                                  └──▶ ffmpeg  → flac + 320 + 128  ──▶ GET /isrc/{isrc}/{format}
```

Why this shape: **Spotify URLs cannot be ingested** any more (the old
client-credentials app gets the playlist name but no songs, plus an app-wide
quota), and Spotify URL ingestion was the fragile part of the old flow. MMM
already knows the ISRCs of every track it cares about, so the whole Spotify
problem disappears if orders are keyed by ISRC. Deezer's public ISRC lookup needs
no credentials at all, and the download is a single Rust path — no sidecar, no
Node, no Docker.

## Contract (v1)

All routes except `/health` require `Authorization: Bearer <MUSIC_API_TOKEN>`.

### `POST /orders`

```json
{ "items": [{ "isrc": "USQX91201487" }, { "isrc": "US-QX9-12-01487" }] }
```

ISRCs are normalised (hyphens/spaces stripped, uppercased) and de-duplicated.
Ordering an ISRC that is already present is a cheap no-op — the track keeps its
state, so a re-order never re-downloads.

Response: `{ "orderId": "<uuid>", "status": "open", "count": 1 }`

#### Non-Deezer sources (SoundCloud, YouTube)

Tracks that only exist off Deezer can be ordered by naming their acquisition
`source` and the `externalId` within that source (the key becomes
`<source>:<id>`):

```json
{ "items": [{ "source": "soundcloud", "externalId": "1837565340" }] }
```

```json
{ "items": [{ "source": "youtube", "externalId": "dQw4w9WgXcQ" }] }
```

`externalId` is the SoundCloud numeric track id or the YouTube video id — the
final file is resolved by yt-dlp (`api.soundcloud.com/tracks/<id>` /
`youtube.com/watch?v=<id>`). These are lossy sources: the download lands as
`mp3-320` (the 128 is derived by the transcode queue), never FLAC. Omitting
`source` (or setting `deezer`) keeps the ISRC path above.

### `GET /orders/{orderId}`

```json
{
  "orderId": "…",
  "status": "open",
  "createdAt": 0,
  "updatedAt": 0,
  "items": [
    {
      "isrc": "USQX91201487",
      "state": "ready",
      "deezerId": "836932812",
      "title": "Sudno",
      "artist": "Molchat Doma",
      "formats": ["flac", "320", "128"],
      "error": null
    }
  ]
}
```

`sourceFormat` is `flac`, `mp3-320` or `mp3-128` — classified from the delivered
file (ffprobe), not from what was requested, because the tier fallback lands on
whatever the ARL's Deezer tier allows.

### `GET /orders/{orderId}/archive[?format=flac|320|128]` — whole order as one ZIP (#358)

One request → one `application/zip` attachment (`Content-Disposition:
attachment; filename="order-<orderId>.zip"`), instead of pulling `GET
/isrc/{isrc}/{format}` per track. The ZIP is built into a temp file under
`data_dir/tmp/`, then streamed (`Content-Length` set); audio entries are stored
**without recompression** (`CompressionMethod::Stored`).

Tracks that are not currently servable (not ready, file missing) are **skipped
silently** — the archive never fails because of one missing track.

| Query `format`         | Behaviour                                         |
| ---------------------- | ------------------------------------------------- |
| `flac` / `320` / `128` | Only that format; tracks without it are skipped.  |
| _(omitted)_            | Best-available per track: `flac` → `320` → `128`. |

Entry names are cosmetic: `"<artist> - <title>"` sanitised (path/control
characters stripped, whitespace collapsed, ~150 chars max), falling back to the
track key when both are empty; collisions get a ` (n)` suffix. The extension
follows the format used (`.flac` / `.mp3`). The client keeps its own
ISRC↔file mapping.

| Status | When                                                                                                                          |
| ------ | ----------------------------------------------------------------------------------------------------------------------------- |
| `200`  | Order exists and ≥1 track is servable → ZIP.                                                                                  |
| `400`  | `format` is set but not `flac`/`320`/`128`.                                                                                   |
| `404`  | Order id is unknown.                                                                                                          |
| `409`  | Order exists but no track is servable in the selected (or best-available) format. Body: `{"error":"no downloadable tracks"}`. |
| `500`  | DB/IO error (temp file, ZIP write).                                                                                           |

### `GET /orders?status=open|done|partial&limit=50`

### `GET /isrc/{isrc}` — single-ISRC state.

### `GET /isrc/{isrc}/{flac|320|128}` — stream the file.

**Local-first, store-backed (#359).** An openable data_dir file is streamed as
before with `X-Source: local`. When no local file is present (or no path is
recorded), the file is served from the permanent, content-addressed object
store with `X-Source: store` — same `Content-Type` (`audio/flac` / `audio/mpeg`)
and single-range support. Neither local file nor store object → `404`; an
unknown format is still a `400`. `320` and `128` both resolve to the newest
stored `mp3` object (the store keeps no bitrate).

### `GET /isrc/{isrc}/objects` — resolve an ISRC to its object variants (#359).

Read-only view of the backup store for one ISRC. The ISRC is normalised first.

```json
{
  "isrc": "AEA0D1846146",
  "count": 3,
  "variants": [
    {
      "hash": "…",
      "kind": "flac",
      "size": 123,
      "contentType": "audio/flac",
      "name": "AEA0D1846146.flac",
      "originalPath": null,
      "stemType": null,
      "groupKey": null,
      "createdAt": 1700000000
    }
  ],
  "groups": [
    {
      "groupKey": "grp-1",
      "stems": [
        /* … */
      ]
    }
  ]
}
```

`kind` is derived read-only: `stem` when the object carries a `stem_type`,
otherwise from the extension of `name`/`original_path` (`flac`, `mp3`, `audio`
for m4a/wav/aiff/ogg, else `other`). `groups` links stems best-effort via the
`group_key` of this ISRC's own objects (no filename heuristics). `404` only when
the ISRC is entirely unknown (no track **and** no object); a known-but-empty
ISRC answers `200` with `count: 0`.

### Object store upload metadata

`PUT /objects/{sha256}` accepts optional `X-ISRC`, `X-Original-Path`, `X-Name`
and (since #359) `X-Group-Key` / `X-Stem-Type` for stem grouping. Re-uploading an
existing object never wipes these: group/stem columns are merged with `COALESCE`.

### Object store deletion policy

Deletion is **consumer-driven and explicit**: the only way to remove objects is
`POST /objects/prune` with `{"hashes": ["<sha256>", …]}` (plus `dryRun` to get
the `deleted`/`missing`/`freedBytes` report without touching anything) or the
single-key `DELETE /objects/{sha256}`. There is no automatic or scheduled
reconcile — nothing deletes files that were not named in a request (#703).
Storage drift is only *reported*: `GET /storage/stats` carries
`orphanCandidates` with `filesWithoutRow` (object files with no
`store_objects` row), `rowsWithoutFile` (rows whose file is missing) and
`total`; a consumer that wants those gone passes their hashes to
`POST /objects/prune` itself.

### Queue control (transparent, hub-facing)

| Method | Path                      | Use                                                        |
| ------ | ------------------------- | ---------------------------------------------------------- |
| `GET`  | `/queue`                  | native queue view: `items` (empty hull) + per-state `counts` |
| `GET`  | `/worker`                 | `{paused, intervalSecs, maxAttempts, maxInflight}`         |
| `POST` | `/worker/pause` `/resume` | pause/resume the worker                                    |
| `POST` | `/isrc/{isrc}/retry`      | reset a failed/stranded ISRC to `pending` (`409` if ready) |
| `GET`  | `/logs?limit=n`           | recent service events, newest first                        |
| `GET`  | `/tracks?isrcs=a,b,c`     | bulk metadata/state for many ISRCs                         |
| `GET`  | `/account`                | Deezer account tier behind `DEEZER_ARL`                    |

### Deezer account tier (`GET /account`)

Returns the tier of the account behind `DEEZER_ARL`, so the hub can warn when
FLAC is impossible:

```json
{
  "offer": "Deezer HiFi",
  "lossless": true,
  "high": true,
  "low": true,
  "userId": 1234567,
  "canStreamFlac": true
}
```

The tier is probed once at startup (spawned, off the boot path) and cached; the
handler re-probes on demand if the cache is still empty. FLAC downloads require
a Premium/HiFi ARL — a Free ARL (`lossless: false`) silently falls back to a
lower quality, and `DOWNLOAD_QUALITY=flac` then logs a startup warning. Responses:
`409` when `DEEZER_ARL` is unset, `502` when the tier probe fails.

### Deezer metadata (`/deezer/*`)

Read-only lookups against the public Deezer API for the hub: artist release
lists, album detail with ISRCs, and artist id resolution by name. They use the
same `DEEZER_BASE` config as the ISRC lookup, need no ARL, and make **exactly
one** upstream request each (no fan-out, no extra requests per album).

| Method | Path                                       | Use                                                                     |
| ------ | ------------------------------------------ | ----------------------------------------------------------------------- |
| `GET`  | `/deezer/artist/{id}/albums?limit=&index=` | the artist's releases (`limit` default 50, max 100)                     |
| `GET`  | `/deezer/album/{id}`                       | album metadata + its tracks (`isrc`, `title`, `durationMs`, `position`) |
| `GET`  | `/deezer/search/artist?q=&limit=`          | artist id by name (`limit` default 10, max 100)                         |

```json
// GET /deezer/artist/15167501/albums?limit=50&index=0
{
  "albums": [
    {
      "albumId": "1137915301",
      "title": "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)",
      "releaseDate": "2026-11-13",
      "recordType": "album",
      "coverUrl": "https://cdn-images.dzcdn.net/images/cover/…/500x500-000000-80-0-0.jpg",
      "nbTracks": null
    }
  ],
  "total": 417,
  "next": "https://api.deezer.com/artist/15167501/albums?limit=50&index=50"
}
```

`nbTracks` is always `null`: Deezer's artist-albums list carries no track
count, and the field exists so clients keep a stable shape. `coverUrl` prefers
`cover_big`, then `cover_medium`, then `cover_xl`. `next` is Deezer's own paging
link, passed through verbatim (may be `null`).

`GET /deezer/album/{id}` adds `seconds` (the sum of every track's `duration`)
and `tracks`, sorted by Deezer's 1-based `track_position`. Tracks without an
ISRC are **kept** with `isrc: null` so the caller decides.

Responses: `404` when Deezer has no such artist/album (Deezer reports that as an
HTTP 200 `error` object with code 800, and a plain HTTP 404 means the same),
`502` on transport/quota failures and unparseable payloads, `400` for a
non-numeric id or a missing `q`.

### Per-ISRC metadata (BPM/key authority)

The service is also the ISRC → {bpm, key, camelot} authority for a **range** of
ISRCs (fed by the hub's Traktor/analysis data).

| Method | Path                    | Use                                                              |
| ------ | ----------------------- | ---------------------------------------------------------------- |
| `POST` | `/metadata`             | bulk upsert `{items:[{isrc,bpm,key,camelot,…}]}` (non-null wins) |
| `GET`  | `/metadata?isrcs=a,b,c` | metadata for a range; unknown ISRCs come back in `missing`       |
| `GET`  | `/metadata/{isrc}`      | metadata for one ISRC (`404` if unknown)                         |

Merge rule: a non-null field wins; a `null` field keeps the stored value.

### Analysis (BPM/key authority)

When `ANALYZER_URL` (fallback `HUB_ANALYZER_URL`) points at an Essentia analyzer
(e.g. `http://127.0.0.1:8711`), music-api analyses its **own** downloads: once a
track is `ready` with a FLAC on disk, the worker posts the FLAC path to
`{ANALYZER_URL}/analyze` (`{"path":…,"embed":false}`) and upserts the returned
`bpm`/`key`/`camelot` into `track_metadata` with **`source='essentia'`**. This
makes music-api the analysis authority — the hub never analyses itself and just
reads the result back via `GET /metadata/{isrc}`.

It is additive to (and coexists with) the hub's `POST /metadata` push: the merge
rule is `source`-agnostic (non-null wins), so whichever side has a value, it is
kept. The analysis job is **idempotent** (a track with an `essentia` row is never
re-analysed), runs **bounded/asynchronously** off the download tick
(`ANALYSIS_CONCURRENCY`, default `1`), and a failing/absent analyzer is **backed
off** (60s→1800s) instead of retried every tick. A blank URL disables analysis.

### Data Vault 2.0 (`/vault/*`)

music-api is the **data vault of music**: a Data Vault 2.0 model (hubs, links,
satellites) over every entity it touches. Insert-only, hash-keyed, auditable —
multiple sources' truths coexist and consumers pick the value they trust. The
schema lives in `src/vault.rs` (additive DDL, created on every boot via
`db::init`); the design is [`plans/proposed/music-api-data-vault.md`](../plans/proposed/music-api-data-vault.md).

| Layer      | Tables                                                                                                                                   |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Hubs       | `hub_track` (ISRC), `hub_artist` (name), `hub_album` (title+date)                                                                        |
| Links      | `link_track_artist`, `link_track_album`                                                                                                  |
| Satellites | `sat_track_meta`, `sat_track_analysis`, `sat_track_download`, `sat_artist_meta`, `sat_artist_extid`, `sat_album_meta`, `sat_album_extid` |

Conventions: `*_hk = UPPER(HEX(SHA256(normalized_bk)))`, `*_lk = SHA256(hub keys)`,
`hash_diff = SHA256(descriptive cols)`, `load_date` UTC ISO-8601, `record_source`
the producer. Satellites are **insert-only**: a new row is written only when the
`hash_diff` changes. The operational queue (`tracks.state`, `orders`) is _not_ the
vault — the worker writes _loads_ into it.

**Consumer surface** (the hub is only a consumer):

| Method | Path                      | Use                                                                         |
| ------ | ------------------------- | --------------------------------------------------------------------------- |
| `GET`  | `/vault/tracks?isrcs=a,b` | resolved analysis for many ISRCs (unknown ISRCs omitted)                    |
| `GET`  | `/vault/track/{isrc}`     | the **atomic** track view — hub + links + all satellites (`404` if unknown) |

Every `/vault/tracks` row is `{isrc, bpm, key, camelot, energy, source, updatedAt}`.
BPM/key/camelot are picked **per field by precedence** over the latest per-analyzer
satellite rows (default `traktor > essentia > reccobeats`, configurable via
`ANALYSIS_PRECEDENCE`); `source` is the analyzer that provided the chosen BPM. A
trash value stays in the model — it is simply not chosen.

`GET /vault/track/{isrc}` returns the fully-assembled view, so a consumer can render
HUB + LINKS + SATELLITES and pick the truth it trusts:

```json
{
  "isrc": "GBKQU2538819",
  "hub": { "title": "…", "durationMs": 343414, "explicit": false },
  "resolved": {
    "bpm": 124.0,
    "key": "8A",
    "camelot": "8A",
    "energy": 0.8,
    "source": "traktor"
  },
  "links": {
    "artists": [{ "name": "BINARYH", "externalIds": { "deezer": "10725209" } }],
    "album": {
      "title": "HOZHO NAASHA",
      "releaseDate": "2025-05-09",
      "externalIds": { "deezer": "747377661" }
    }
  },
  "satellites": {
    "analysis": [
      {
        "analyzer": "traktor",
        "bpm": 124.0,
        "key": "8A",
        "camelot": "8A",
        "energy": null,
        "loadDate": "…"
      }
    ],
    "download": {
      "state": "ready",
      "sourceFormat": "flac",
      "formats": ["flac", "320", "128"],
      "loadDate": "…"
    },
    "meta": [
      {
        "recordSource": "deezer",
        "title": "…",
        "artist": "…",
        "album": "…",
        "durationMs": 343414,
        "loadDate": "…"
      }
    ]
  }
}
```

- `hub` — from `hub_track` + the **newest** `sat_track_meta`.
- `resolved` — the same precedence resolution as `/vault/tracks` (envelope fields dropped).
- `links` — from `link_track_artist`/`link_track_album` + the `*_extid` satellites.
  Names are the hubs' **normalised** business keys (collapsed, uppercased).
- `satellites.analysis` — **all** analyzers (the newest row per analyzer).
- `satellites.download` — the newest `sat_track_download` (`null` if none).
- `satellites.meta` — every `record_source` (the newest row per source).

**Ingest** happens on the resolve path (Deezer → hubs/links/extids/`sat_track_meta`),
on `ready` (`sat_track_download`), on the analysis job (`sat_track_analysis`,
`analyzer='essentia'`), and on `POST /metadata` (analysis + metadata satellites).
`track_metadata` is kept as a **compat projection** so the legacy `/metadata`
surface and the idempotency check keep working unchanged.

**Backfill** existing rows off the boot path (never blocks startup):

```bash
# batched, transactional, idempotent — safe to re-run
DATABASE_URL=sqlite:… VAULT_BACKFILL_BATCH=500 music-api backfill-vault
```

## States

| state         | meaning                                                          |
| ------------- | ---------------------------------------------------------------- |
| `pending`     | ordered, not yet resolved                                        |
| `downloading` | native download dispatched, waiting on its background task       |
| `ready`       | at least one format is on disk (`formats` lists them)            |
| `absent`      | Deezer has no match, or it is not streamable for the ARL account |
| `failed`      | transport/timeout/hard download error                            |

Order status is derived: `open` while any item is pending/downloading, `done`
when all are `ready`, otherwise `partial`.

Transient failures are retried with exponential backoff up to `QUEUE_MAX_ATTEMPTS`
(default 3); only then does a track become `failed`. `absent` (no streamable match)
stays terminal. A row in `downloading` holds a **lease**; if the lease lapses (dead
process) the row is re-queued by the reaper at the start of the next tick.

**In-flight cap.** Orders are accepted without limit, but only `MAX_INFLIGHT`
(default 5) rows are dispatched at once — the `downloading` rows _are_ the
in-flight set. Each tick the worker pulls at most `max_inflight - inflight` rows
from the priority-ordered due set, so priority decides _which_ pending rows fill
the free slots.

## Pipeline (one worker, no external queue)

The `tracks.state` column _is_ the queue; the worker runs a flat tick (skipped while
globally paused):

0. **reap** — re-queue any `downloading` row whose lease lapsed (crash/restart recovery).
1. **resolve + download** — pull at most `max_inflight - inflight` _due_ `pending`
   ISRCs (highest order priority first, then oldest): look each up on Deezer, then
   start a native `deezer_dl` download at `DOWNLOAD_QUALITY` on a background task
   (the ISRC tag is written before the file is stored). A non-Deezer source is
   handed to a bounded yt-dlp task instead.
2. **transcode** — derive the missing lossy formats (`320`/`128`) from the primary
   file on the bounded `transcode` queue.
3. **analyze** — for each `ready` track with a FLAC and no `essentia` metadata
   yet, call the analyzer (bounded, off the tick loop) and upsert BPM/key. Skipped
   entirely when `ANALYZER_URL` is unset.

Lossless source → `flac/` + `320/` + `128/`. When the tier caps quality (no
lossless master, or a Free ARL), the delivered file is probed with ffprobe: a
≥256 kbps MP3 becomes the 320 source (and 128 is derived from it), a lower one is
stored as 128 only.

## Configuration

See [`.env.example`](.env.example). Everything is env-driven; `MUSIC_API_TOKEN`
and `DEEZER_ARL` are the only required secrets. The ARL is only ever handed to
the download module — it never reaches a log line or an error message (warnings
name the variable, never its value).

Download: `DOWNLOAD_QUALITY` selects the requested quality — `flac` (default),
`mp3-320` or `mp3-128`. An invalid value is a startup error (fail-fast, never a
silent default). The tier fallback steps down FLAC → 320 → 128 when the ARL's
account cannot stream the wanted tier. There is no download backend switch: the
native `deezer_dl` path is the only one. `DEEZER_ARL` is the ARL variable; the
pre-0.6 name is still accepted as a fallback with a one-time warning.

> **Upgrade note (0.6.0).** The old sidecar (a deemix instance behind
> `DEEMIX_URL`/`DEEMIX_ARL`/`DEEMIX_BITRATE`) is gone. Existing hosts that still
> export the old ARL variable keep working (it is read as a fallback), but should
> rename it to `DEEZER_ARL`; `DEEMIX_URL`/`DEEMIX_BITRATE`/
> `DEEMIX_DOWNLOAD_DIR`/`DOWNLOAD_BACKEND` are dropped and no longer read.

Queue tuning: `QUEUE_MAX_ATTEMPTS` (default `3`), `QUEUE_RETRY_BASE_SECS` (`60`)
and `QUEUE_RETRY_MAX_SECS` (`1800`) shape the bounded retry backoff. The download
lease reuses `DOWNLOAD_TIMEOUT_SECS` (refreshed as a heartbeat). `MAX_INFLIGHT`
(default `5`) caps how many downloads are dispatched at once; order intake
itself is unbounded.

Analysis: `ANALYZER_URL` (fallback `HUB_ANALYZER_URL`) enables the BPM/key job;
unset/blank disables it. `ANALYSIS_CONCURRENCY` (default `1`) caps concurrent
analyzer calls. `ANALYSIS_PRECEDENCE` (default `traktor,essentia,reccobeats`) sets
the vault resolver's per-field precedence. The one-shot backfill reads
`VAULT_BACKFILL_BATCH` (default `500`).

Deezer tier probe: `DEEZER_GW_BASE` (default
`https://www.deezer.com/ajax/gw-light.php`) is the gw-light endpoint used to read
the account tier behind `DEEZER_ARL`. FLAC requires a Premium/HiFi ARL; with a
Free ARL and `DOWNLOAD_QUALITY=flac` the service logs a warning at startup. See
[`GET /account`](#deezer-account-tier-get-account).

## Deploy

Build (a static Linux binary; the target host has no Rust toolchain):

```bash
# musl cross-build from macOS needs a cross toolchain; alternatively build on
# the host or in a rust:1 container:
cargo build --release --target x86_64-unknown-linux-musl
```

On the host:

```bash
install -d /opt/music-api/data
install -d /data/public/media/music-api/{flac,320,128}   # the BIG volume
install -m755 music-api /opt/music-api/music-api
install -m600 .env.example /opt/music-api/music-api.env   # then edit + chmod (set DEEZER_ARL)
install -m644 deploy/music-api.service /etc/systemd/system/music-api.service
systemctl daemon-reload && systemctl enable --now music-api
# or: deploy/setup.sh <binary> does the above idempotently
# .149: add deploy/Caddyfile.snippet to the Caddyfile, export MUSIC_API_TOKEN, reload Caddy
```

**Storage**: the file store (`flac/320/128`) must sit on a large volume — a full
library is hundreds of GB. `/opt/music-api/data` holds only the small ledger DB.
`deploy/relocate-data.sh` moves an existing store and rewrites the stored paths.

**ARL tier matters.** FLAC and 320 need a Deezer Premium/HiFi account; a Free ARL
makes the service fall back to what the account may stream (128) — the service
still serves the track, but `sourceFormat` is `mp3-128` and there is no
`flac`/`320`. The tier is a property of the ARL, not of this service.

`/health` is the only unauthenticated route and is safe for Caddy/uptime checks.

## Tests

```bash
cargo test
```

Unit tests for ISRC normalisation, order-status derivation, retry backoff,
Deezer payload parsing, constant-time token compare, quality parsing, the ARL
alias resolver, the vault hashing/normalisation primitives and the `ffprobe` tag
parser; plus integration tests over the real router (auth, order create/read/
list, `/isrc` status, file delivery, order flipping to `done`, the durable
queue: retry, lease recovery, worker status and queue counts, the native
download path end-to-end with a loopback Deezer mock, the per-ISRC metadata
upsert/merge/range read, the `deemix_uuid`→`download_ref` migration, the
ffprobe-fixture ingest into the vault, and the Data Vault: precedence
resolution, hub/link/satellite ingest and the batched, idempotent backfill).

## v1 scope / known limits

- Delivery **streams through the process**; large FLACs are streamed, not
  buffered, but there is no HTTP range support yet.
- The MP3 fallback is classified by an ffprobe bit-rate read; a container that
  reports none is treated as 320.
- No on-disk garbage collection; a track is stored once (keyed by ISRC) and kept.
- Terminal rows (`failed`/`absent`) are re-queued via `POST /isrc/{isrc}/retry`;
  a plain re-order stays a cheap no-op.
- MMM-side consumption (order the Backpack's ISRCs, then link delivered files)
  is the next step and not part of this crate.
