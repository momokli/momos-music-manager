# kellner — Agent Handover

> **Last updated**: 2026-10-15 · downloads implemented (#366); backup (#365);
> traktor-upload (#364); §5 decisions locked (ADR-081, #361)
> Keep this file current. It is the entry point for any agent working on `kellner/`.
>
> **Status: pairing (#362), traktor-upload (#364), backup (#365) implemented.**
> `src/config.rs` (config + device identity + `music_api_token`, `0600`, redacted) and
> `src/pairing.rs` (token gen, hub register, approval URL, persist) exist, plus `kellner pair`.
> `kellner traktor-upload` detects the local Traktor `collection.nml`
> (`src/sync/traktor.rs`), watches it for **mtime** changes (`src/state.rs` skips an
> unchanged file) and uploads it to the **hub** via the device-token-gated
> `POST /api/hub/traktor/import` (`src/client/hub.rs`).
> `kellner backup` walks the collection (`src/sync/backup.rs`), canonicalises each audio
> file (Comment cleared via `lofty`), and mirrors missing objects to music-api's object
> store (`src/client/api.rs`): one bulk `POST /objects/check`, then idempotent
> `PUT /objects/{sha256}` for the missing ones; `--dry-run` / `--root` / `--once`
> supported.
> `kellner downloads` (`src/sync/downloads.rs`) places one `POST /orders` for the
> requested ISRCs (`--isrc`/`--isrcs`), polls `GET /tracks` until every ISRC is
> terminal or `--timeout-secs` elapses, then streams each ready file
> (`GET /isrc/{isrc}/{format}`) and writes it **flat** into `collection_dir` as
> `<isrc>.<ext>` (temp + rename). Format pref `--format` (flac default) with
> flac→320→128 fallback; provenance (path/format/order) + idempotency live in
> `state.json`; no arguments → read-only status report (no network). The only
> remaining stub is `backpack`.

---

## 1. What this is

`kellner` is the **local, cross-platform companion** of the music stack: a small
binary that runs on each Mac / PC and keeps the local machine in sync with the
always-on servers. It is a separate cargo crate in `kellner/` (**not** a workspace
member — it declares its own empty `[workspace]`).

| Project                  | Role                                                                 | Runs on          |
| ------------------------ | -------------------------------------------------------------------- | ---------------- |
| **music-hub** (spelunke) | multi-user web UI + DB                                               | always-on server |
| **music-api**            | download + metadata authority (ISRC/order, object store, BPM/key)    | always-on server |
| **kellner**              | local companion: Backpack upkeep, backups, Traktor upload, downloads | each Mac / PC    |

## 2. Scope boundaries (important)

- kellner talks **only to `music-api`** (and, where needed, the hub's API). It
  **never** talks to Deezer / Spotify / SoundCloud / YouTube providers directly —
  `music-api` is the single authority for downloads and metadata.
- No provider credentials (ARL, Spotify tokens, …) live in kellner. kellner pairs with
  the **hub** (device token) and talks to `music-api` with that delegated credential
  (see §5).
- kellner is **not** a server: it is a short-lived CLI plus (later) a background
  service/tray. It owns no shared database.

## 3. Planned architecture (sketch)

- **pairing** — one-time link of a local machine to the **hub**: kellner creates a device
  token and opens `https://hub.zukkafabrik.de/kellner/anheuern`; a logged-in user answers
  “diesen kellner anheuern?” and stores the token locally.
- **backups** — walk the local music library, hash files, upload to `music-api`'s
  object store; content-hash dedup.
- **uploads** — kellner uploads the local Traktor `collection.nml` to the **hub**
  (`POST /api/hub/traktor/import`, device-token-gated; ADR-081 **Entscheidung 6**);
  the hub calls `traktor::import_str` internally to persist playcount / rating /
  playlists. (Not a direct `music-api` call.)
- **downloads** — **implemented (#366)**: `kellner downloads --isrc …` orders the
  ISRCs at music-api, polls until ready and files the delivered bytes flat into
  the collection (see §4). The Backpack-driven variant (diff/tag) is **#367**.
- **service/tray** — runs as a **tray / menu-bar widget** (v1, battery-friendly) on top of
  launchd (macOS) / systemd (Linux) / Task Scheduler (Windows); right-click shows exactly
  “feierabend” (quit) and “re-auth”.

## 4. Source layout (current)

```
kellner/
├── Cargo.toml     # package kellner, edition 2024, own empty [workspace]
├── README.md
├── AGENT.md       # this file
└── src/
    ├── main.rs    # clap CLI: pair, traktor-upload (+ backpack / backup / downloads stubs)
    ├── config.rs  # ~/.config/kellner/config.toml + persisted device identity
    ├── pairing.rs # token gen, hub register, approval URL, browser open, confirm
    ├── state.rs   # minimal JSON sync state (state.json: `last_upload_mtime`, `orders`, `downloads`)
    ├── sync/
    │   ├── mod.rs
    │   ├── traktor.rs  # collection.nml detection (macOS/Windows defaults + override) + mtime watch
    │   ├── backup.rs   # walk + canonicalise (Comment cleared) + SHA-256 + dedup/upload (#365)
    │   └── downloads.rs # order → poll → stream → flat file + provenance (#366)
    └── client/
        ├── mod.rs
        ├── hub.rs     # traktor upload via `Authorization: Bearer <device_token>` + bounded retry/backoff
        └── api.rs     # music-api: object store (#365) + orders/tracks/file delivery (#366)
```

## 5. Decisions (locked 2026-10-15 — ADR-081, #361)

All open points below are now **resolved**; treat them as intent to implement, not forks.

- **Pairing/auth** — **implemented (#362).** kellner creates a crypto-random
  **device token** and a **pairing token**, registers both via
  `POST {hub_url}/api/hub/devices` (raw hex; the hub hashes), and opens the returned
  `approval_path` (`/kellner/anheuern?token=…`, default host `hub.zukkafabrik.de`). A
  logged-in user answers “diesen kellner anheuern?” Ja/Nein; “Ja” binds the device and
  the hub rotates the pairing hash to the device-token hash (single-use). The link
  _is_ the capability. The device token is stored `0600`, never logged, and is later
  presented to the hub **and** for delegated `music-api` calls. No provider credentials
  ever live here.
  - **Binding confirmation is user-driven** (open URL + press Enter / `--wait`): the
    merged hub (#363) exposes **no device-gated route**, so kellner cannot poll.
    Auto-poll is deferred to **#364** — do not invent a hub endpoint.
- **Traktor-upload transport** — **implemented (#364).** kellner watches the local
  `collection.nml` (mtime; unchanged files are skipped via `state.json`) and `POST`s it
  to the hub's token-gated `POST /api/hub/traktor/import` (ADR-081 **Entscheidung 6**);
  the hub runs `traktor::import_str` itself. The body is capped at **16 MiB**
  (`MAX_NML_BYTES` in `music-hub/src/traktor_api.rs`; larger collections get `413`).
- **Config location** — `~/.config/kellner/config.toml` exists **only** for the hub-host
  override (default `hub.zukkafabrik.de`). **No feature config on the device**: the
  Traktor-upload / backpack-sync toggles and the per-device backpack set live in the Hub
  (#369).
- **Comment write + hash** — Comment via `lofty` into the file **and** mirrored into
  `collection.nml` (guard: only when Traktor is closed; atomic temp+rename). Backup hash =
  SHA-256 of the **canonicalised** file (comment cleared) → dedup-safe.
- **Backup transport** — v1 whole-file idempotent `PUT /objects/{sha256}` + `POST
/objects/check` skip; chunked/resumable is v2.
- **Tray/service** — tray/menu-bar widget is **v1** (battery-friendly), not a later
  add-on; keep battery-awareness in mind (cf. `plans/proposed/battery-friendly-tray.md`).
- **Repo rules** — one issue = one branch = one PR; see the root
  [`AGENT.md`](../AGENT.md) and [`docs/ROADMAP.md`](../docs/ROADMAP.md)
  (milestone **kellner-0.1.0**).
