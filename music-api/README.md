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

## Pipeline (one worker, no external queue)

The `tracks.state` column _is_ the queue; the worker runs a flat two-phase tick:

1. **resolve+enqueue** — for each `pending` ISRC: look it up on Deezer, then
   submit `https://www.deezer.com/track/<id>` to deemix at `DEEMIX_BITRATE`.
   `CantStream` → `absent`; other rejections → `failed`.
2. **advance** — poll the deemix queue for the in-flight UUIDs; on a terminal
   success status, locate the file, copy it into the store and transcode.

Lossless source → `flac/` + `320/` + `128/`. When deemix had to fall back to
MP3 (no lossless master, or a Deezer tier that caps quality), the delivered file
is probed with ffprobe: a ≥256 kbps MP3 becomes the 320 source (and 128 is
derived from it), a lower one is stored as 128 only. The dedicated deemix
download dir is ours, so the source file is removed after it has been collected.

## Configuration

See [`.env.example`](.env.example). Everything is env-driven; `MUSIC_API_TOKEN`
and `DEEMIX_ARL` are the only required secrets. Response bodies from
`loginArl` are never logged — deemix echoes the ARL back.

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
install -m755 music-api /opt/music-api/music-api
install -m600 .env.example /opt/music-api/music-api.env   # then edit + chmod
install -m644 deploy/music-api.service /etc/systemd/system/music-api.service
systemctl daemon-reload && systemctl enable --now music-api

docker compose -f deploy/deemix-api.compose.yml up -d
# once: SSH-tunnel to :6599, log in with the ARL, set Quality = FLAC + Bitrate fallback ON
# (deploy/setup.sh seeds this config and picks a usable ARL from an existing instance)

# .149: add deploy/Caddyfile.snippet to the Caddyfile, export MUSIC_API_TOKEN, reload Caddy
```

**ARL tier matters.** FLAC and 320 need a Deezer Premium/HiFi account; a Free ARL
makes deemix fall back to what the account may stream (128) — the service still
serves the track, but `sourceFormat` is `mp3-128` and there is no `flac`/`320`.
The tier is a property of the ARL, not of this service.

`/health` is the only unauthenticated route and is safe for Caddy/uptime checks.

## Tests

```bash
cargo test
```

17 tests: unit tests for ISRC normalisation, order-status derivation, deemix and
Deezer payload parsing, constant-time token compare and download-file matching;
plus integration tests over the real router (auth, order create/read/list,
`/isrc` status, file delivery, order flipping to `done`).

## v1 scope / known limits

- Delivery **streams through the process**; large FLACs are streamed, not
  buffered, but there is no HTTP range support yet.
- The MP3 fallback is classified by an ffprobe bit-rate read; a container that
  reports none is treated as 320.
- No on-disk garbage collection; a track is stored once (keyed by ISRC) and kept.
- The worker holds no per-track retry counter — `failed` is terminal. Re-order
  the ISRC (after clearing the row) to retry.
- MMM-side consumption (order the Backpack's ISRCs, then link delivered files)
  is the next step and not part of this crate.
