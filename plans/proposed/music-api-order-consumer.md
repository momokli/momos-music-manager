# Plan: music-api order consumer (Backpack downloads without deemix-Spotify)

**Status**: in-progress
**Branch**: `feat/music-api-order-consumer`
**Ready for review**: yes
**Depends on**: `feat/music-api` (the `music-api` service — API contract + deployed instance on `.200`)
**Migration needed**: yes (one additive migration, `027_music_api_imports.sql`)

### Description

MMM stops depending on deemix being able to read Spotify links (it no longer
can: a client-credentials Spotify app gets the playlist name but no songs, plus
an app-wide quota) and instead **orders ISRCs** from `music-api`, which resolves
them on Deezer, downloads through deemix and serves the files back. MMM imports
the delivered files into its library and links them like any other file.

MMM already stores every track's ISRC (`service_tracks.isrc`), so the whole
Spotify round-trip disappears from the download path.

### Design

**Contract (already built, `music-api/README.md`)**

| Method | Path | Use |
|---|---|---|
| `POST` | `/orders` | `{"items":[{"isrc":"…"}]}` → `{orderId,status,count}` |
| `GET` | `/orders/{id}` | per-ISRC `{state,deezerId,title,artist,formats[],error}` |
| `GET` | `/orders?status=open` | outstanding orders |
| `GET` | `/isrc/{isrc}` | single ISRC state |
| `GET` | `/isrc/{isrc}/{flac\|320\|128}` | the file |

Auth: `Authorization: Bearer <token>`.

**Consumer cycle** (new `src/music_api.rs` + a background task, shaped like
`poller.rs` / `download_guarantor.rs`):

1. **Compute demand** — Backpack tracks (`backpack::get_backpack_track_ids`)
   that have no linked file (`get_file_ids_for_track_ids`), whose
   `service_tracks.isrc` is set, and that are **not already imported**
   (`music_api_imports`, see below). Cap per cycle.
2. **Order** — `POST /orders` in batches (≤100 ISRCs). Persist the `orderId`.
3. **Poll** — for open orders, `GET /orders/{id}` each cycle. For `ready` items,
   download the best offered format and place it; for `absent`/`failed`, record
   the reason.
4. **Link** — after placing files, trigger a targeted incremental scan of the
   destination folder so the existing matcher links them (ISRC match already
   works via `v_file_track_link`).

**Placement & format**

- `flac` available → write into the existing FLAC library dir (`flacs_dir`).
- only `320`/`128` → write into the existing MP3 dir (`mp3_dir`).
- Filename: the same `Artist - Title.ext` convention the scanner already keys
  on, so fuzzy/ISRC linking behaves exactly as with deemix/spotDL downloads.
- Formats are per-track, not per-run: a later `flac` becoming available upgrades
  the import (re-download + replace, keeping the 128/320 only if no flac exists).

**Persistence** — one additive migration:

```
music_api_imports(
  isrc TEXT PRIMARY KEY,
  state TEXT NOT NULL,        -- ordered | ready | imported | absent | failed
  format TEXT,                -- flac | mp3-320 | mp3-128 (what was placed)
  file_path TEXT,
  deezer_id TEXT,
  error TEXT,
  order_id TEXT,
  updated_at INTEGER NOT NULL
)
```

This is MMM's own ledger: it prevents re-downloading (`music_api_imports` PK on
ISRC) and lets the UI show progress without re-deriving from the remote.

**Config** (`config.toml`, env overrides win):

```toml
[music_api]
base_url = "https://music.example.com"   # MUSIC_API_URL
token    = "…"                           # MUSIC_API_TOKEN
enabled  = true
batch_size = 100
interval_secs = 900
```

**UI**

- Backpack page: a small "music-api" card — demand count, open orders, imported
  / absent / failed, and a manual **Pull now** button (mirrors
  `POST /api/backpack/push`).
- Tasks page: one task per consumer cycle, like the other background jobs.

**Supersede (same plan, so the system stays coherent)**

- The Backpack **Spotify playlist** stays (useful as a transport/back-up and for
  the user's own listening), but MMM stops submitting it to deemix.
- `DownloadGuarantor`'s deemix coupling (zombie re-submit) and the deemix
  auto-download paths become redundant once music-api owns downloads; they are
  retired or reduced to the spotDL fallback for `absent` ISRCs.
- `deemix_downloads` / the deemix queue API rows lose their purpose; keep the
  table (additive rule) and stop writing it from the download path.

### Files to modify

| File | Change |
|------|--------|
| `migrations/027_music_api_imports.sql` | new table (additive) |
| `src/music_api.rs` | client (orders/status/download) + config struct |
| `src/music_api_consumer.rs` | the background cycle |
| `src/config.rs` | `[music_api]` section + env overrides |
| `src/main.rs` | spawn the consumer, wire the manual-pull handler |
| `src/api/backpack.rs` | "music-api" status + `POST …/pull` handler |
| `src/download_guarantor.rs` | retire the deemix auto path (spotDL fallback stays) |
| `frontend/pages/backpack.js` | music-api card + Pull button |
| `frontend/tests/backpack.spec.js` | card renders, pull posts, errors surface |
| `tests/api_music_api.rs` | client + consumer contract, mocked upstream |
| `docs/DECISIONS.md` | ADR (music-api is the download authority) |
| `CHANGELOG.md` | Added/Changed |

### Acceptance Criteria

- [x] `cargo build` passes
- [x] `cargo test` passes (new `tests/api_music_api.rs`, `tests/music_api_client.rs`)
- [x] `migration_integrity` passes with the new migration
- [x] `cd frontend && npx playwright test` passes
- [ ] A Backpack track with no local file is ordered, imported and linked
      end-to-end against the deployed `music-api` (manual verification)
- [ ] An `absent` ISRC is recorded and never re-ordered on the next cycle
- [x] The old deemix-submit path no longer runs (no `addToQueue` from MMM)

### Open questions (need a decision before implementation)

1. **Formats to keep per track**: FLAC only (derive nothing), or also keep 320
   for device compatibility? Proposed default: FLAC when available, else 320.
2. **Destination dirs**: reuse the current `flacs_dir` / `mp3_dir`, or a
   dedicated `music-api/` dir that the scanner also indexes? Proposed default:
   reuse existing dirs so nothing about linking changes.
3. **Retire timing**: fold the deemix-path retirement into this plan, or land
   the consumer first and retire afterwards in a small follow-up?
4. **TLS/edge**: is `music-api` reached through Caddy on `.149` from the Mac, or
   directly over the LAN (`http://192.168.178.200:8710`)? Affects whether the
   token travels in clear text on the wire.
