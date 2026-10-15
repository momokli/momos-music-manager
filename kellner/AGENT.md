# kellner — Agent Handover

> **Last updated**: 2026-10-15 · §5 decisions locked (ADR-081, #361)
> Keep this file current. It is the entry point for any agent working on `kellner/`.
>
> **Status: pairing implemented (#362).** `src/config.rs` (config + device identity,
> `0600`, redacted) and `src/pairing.rs` (token gen, hub register, approval URL,
> persist) exist, plus `kellner pair`. The remaining commands (`backpack`, `backup`,
> `traktor-upload`, `downloads`) are still stubs.

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
- **uploads** — Traktor `collection.nml` → `music-api` (playcount / rating /
  playlists).
- **downloads** — pull ordered ISRCs / Backpack contents back down.
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
    ├── main.rs    # clap CLI: pair (+ backpack / backup / traktor-upload / downloads stubs)
    ├── config.rs  # ~/.config/kellner/config.toml + persisted device identity
    └── pairing.rs # token gen, hub register, approval URL, browser open, confirm
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
