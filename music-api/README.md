# music-api

An **ISRC order/consume API** in front of deemix. Consumers (Momo's Music
Manager) place _orders_ of ISRCs; this service resolves them on Deezer,
downloads the best available quality through a dedicated deemix instance,
derives the lossy variants and serves the files back by ISRC.

```
MMM ──POST /orders {isrc…}──▶ music-api ──▶ api.deezer.com  (ISRC → track, no auth)
                                  │
                                  ├──▶ deemix (FLAC, bitrate fallback)  ──▶ /downloads
                                  │
                                  └──▶ ffmpeg  → flac + 320 + 128  ──▶ GET /isrc/{isrc}/{format}
```

Why this shape: **deemix cannot ingest Spotify links** any more (its
client-credentials Spotify app gets the playlist name but no songs, plus an
app-wide quota), and Spotify URL ingestion was the fragile part of the old flow.
MMM already knows the ISRCs of every track it cares about, so the whole Spotify
problem disappears if orders are keyed by ISRC. Deezer's public ISRC lookup needs
no credentials at all.

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
file (ffprobe), not from what was requested, because deemix's bitrate fallback
lands on whatever the ARL's Deezer tier allows.

### `GET /orders?status=open|done|partial&limit=50`

### `GET /isrc/{isrc}` — single-ISRC state.

### `GET /isrc/{isrc}/{flac|320|128}` — stream the file.

### Queue control (transparent, hub-facing)

| Method | Path                      | Use                                                        |
| ------ | ------------------------- | ---------------------------------------------------------- |
| `GET`  | `/queue`                  | live deemix queue (`items`) + per-state `counts`           |
| `GET`  | `/worker`                 | `{paused, intervalSecs, maxAttempts}`                      |
| `POST` | `/worker/pause` `/resume` | pause/resume the worker                                    |
| `POST` | `/isrc/{isrc}/retry`      | reset a failed/stranded ISRC to `pending` (`409` if ready) |
| `GET`  | `/logs?limit=n`           | recent service events, newest first                        |
| `GET`  | `/tracks?isrcs=a,b,c`     | bulk metadata/state for many ISRCs                         |

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

| Method | Path                      | Use                                                      |
| ------ | ------------------------- | -------------------------------------------------------- |
| `GET`  | `/vault/tracks?isrcs=a,b` | resolved analysis for many ISRCs (unknown ISRCs omitted) |
| `GET`  | `/vault/track/{isrc}`     | resolved analysis for one ISRC (`404` if unknown)        |

Each row is `{isrc, bpm, key, camelot, energy, source, updatedAt}`. BPM/key/camelot
are picked **per field by precedence** over the latest per-analyzer satellite rows
(default `traktor > essentia > reccobeats`, configurable via `ANALYSIS_PRECEDENCE`);
`source` is the analyzer that provided the chosen BPM. A trash value stays in the
model — it is simply not chosen.

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
| `downloading` | submitted to deemix, waiting                                     |
| `ready`       | at least one format is on disk (`formats` lists them)            |
| `absent`      | Deezer has no match, or it is not streamable for the ARL account |
| `failed`      | transport/timeout/deemix rejection                               |

Order status is derived: `open` while any item is pending/downloading, `done`
when all are `ready`, otherwise `partial`.

Transient failures are retried with exponential backoff up to `QUEUE_MAX_ATTEMPTS`
(default 3); only then does a track become `failed`. `absent` (no streamable match)
stays terminal. A row in `downloading` holds a **lease** that the worker refreshes
while deemix reports progress; if the lease lapses (dead process, evicted item) the
row is re-queued by the reaper at the start of the next tick.

## Pipeline (one worker, no external queue)

The `tracks.state` column _is_ the queue; the worker runs a flat tick (skipped while
globally paused):

0. **reap** — re-queue any `downloading` row whose lease lapsed (crash/restart recovery).
1. **resolve+enqueue** — for each _due_ `pending` ISRC (past its backoff window):
   look it up on Deezer, then submit `https://www.deezer.com/track/<id>` to deemix
   at `DEEMIX_BITRATE`. `CantStream` → `absent`; other rejections become a bounded,
   backoff retry.
2. **advance** — poll the deemix queue for the in-flight UUIDs; on a terminal
   success status, locate the file, copy it into the store and transcode. While the
   item is still downloading its lease is refreshed.
3. **analyze** — for each `ready` track with a FLAC and no `essentia` metadata
   yet, call the analyzer (bounded, off the tick loop) and upsert BPM/key. Skipped
   entirely when `ANALYZER_URL` is unset.

Lossless source → `flac/` + `320/` + `128/`. When deemix had to fall back to
MP3 (no lossless master, or a Deezer tier that caps quality), the delivered file
is probed with ffprobe: a ≥256 kbps MP3 becomes the 320 source (and 128 is
derived from it), a lower one is stored as 128 only. The dedicated deemix
download dir is ours, so the source file is removed after it has been collected.

## Configuration

See [`.env.example`](.env.example). Everything is env-driven; `MUSIC_API_TOKEN`
and `DEEMIX_ARL` are the only required secrets. Response bodies from
`loginArl` are never logged — deemix echoes the ARL back.

Queue tuning: `QUEUE_MAX_ATTEMPTS` (default `3`), `QUEUE_RETRY_BASE_SECS` (`60`)
and `QUEUE_RETRY_MAX_SECS` (`1800`) shape the bounded retry backoff. The download
lease reuses `DOWNLOAD_TIMEOUT_SECS` (refreshed as a heartbeat).

Analysis: `ANALYZER_URL` (fallback `HUB_ANALYZER_URL`) enables the BPM/key job;
unset/blank disables it. `ANALYSIS_CONCURRENCY` (default `1`) caps concurrent
analyzer calls. `ANALYSIS_PRECEDENCE` (default `traktor,essentia,reccobeats`) sets
the vault resolver's per-field precedence. The one-shot backfill reads
`VAULT_BACKFILL_BATCH` (default `500`).

## Deploy

Build (a static Linux binary; the target host has no Rust toolchain):

```bash
# musl cross-build from macOS needs a cross toolchain; alternatively build on
# the host or in a rust:1 container:
cargo build --release --target x86_64-unknown-linux-musl
```

On the host:

```bash
install -d /opt/music-api/{data,deemix-config}
install -d /data/public/media/music-api/{incoming,flac,320,128}   # the BIG volume
install -m755 music-api /opt/music-api/music-api
install -m600 .env.example /opt/music-api/music-api.env   # then edit + chmod
install -m644 deploy/music-api.service /etc/systemd/system/music-api.service
systemctl daemon-reload && systemctl enable --now music-api
```

**Storage**: the file store (`flac/320/128/incoming`) must sit on a large
volume — a full library is hundreds of GB. `/opt/music-api/data` holds only the
small ledger DB. `deploy/relocate-data.sh` moves an existing store and rewrites
the stored paths.

docker compose -f deploy/deemix-api.compose.yml up -d

# once: SSH-tunnel to :6599, log in with the ARL, set Quality = FLAC + Bitrate fallback ON

# (deploy/setup.sh seeds this config and picks a usable ARL from an existing instance)

# .149: add deploy/Caddyfile.snippet to the Caddyfile, export MUSIC_API_TOKEN, reload Caddy

````

**ARL tier matters.** FLAC and 320 need a Deezer Premium/HiFi account; a Free ARL
makes deemix fall back to what the account may stream (128) — the service still
serves the track, but `sourceFormat` is `mp3-128` and there is no `flac`/`320`.
The tier is a property of the ARL, not of this service.

`/health` is the only unauthenticated route and is safe for Caddy/uptime checks.

## Tests

```bash
cargo test
````

53 tests: unit tests for ISRC normalisation, order-status derivation, retry
backoff, deemix and Deezer payload parsing, constant-time token compare,
download-file matching and the vault hashing/normalisation primitives; plus
integration tests over the real router (auth, order create/read/list, `/isrc`
status, file delivery, order flipping to `done`, the durable queue: retry, lease
recovery, worker status and queue counts, the per-ISRC metadata upsert/merge/range
read, and the Data Vault: precedence resolution, hub/link/satellite ingest and the
batched, idempotent backfill).

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
