# kellner — Agent Handover

> **Last updated**: 2026-10-12 · branch `feat/docs-roadmap`
> Keep this file current. It is the entry point for any agent working on `kellner/`.
>
> **Status: skeleton.** Only `src/main.rs` (a stubbed clap CLI) exists. Everything
> below is a *sketch* — treat it as intent, not as implemented behaviour.

---

## 1. What this is

`kellner` is the **local, cross-platform companion** of the music stack: a small
binary that runs on each Mac / PC and keeps the local machine in sync with the
always-on servers. It is a separate cargo crate in `kellner/` (**not** a workspace
member — it declares its own empty `[workspace]`).

| Project | Role | Runs on |
| --- | --- | --- |
| **music-hub** (spelunke) | multi-user web UI + DB | always-on server |
| **music-api** | download + metadata authority (ISRC/order, object store, BPM/key) | always-on server |
| **kellner** | local companion: Backpack upkeep, backups, Traktor upload, downloads | each Mac / PC |

## 2. Scope boundaries (important)

- kellner talks **only to `music-api`** (and, where needed, the hub's API). It
  **never** talks to Deezer / Spotify / SoundCloud / YouTube providers directly —
  `music-api` is the single authority for downloads and metadata.
- No provider credentials (ARL, Spotify tokens, …) live in kellner. Auth is against
  `music-api` (see §5).
- kellner is **not** a server: it is a short-lived CLI plus (later) a background
  service/tray. It owns no shared database.

## 3. Planned architecture (sketch)

- **pairing** — one-time link of a local machine to `music-api`; stores a token
  locally.
- **backups** — walk the local music library, hash files, upload to `music-api`'s
  object store; content-hash dedup.
- **uploads** — Traktor `collection.nml` → `music-api` (playcount / rating /
  playlists).
- **downloads** — pull ordered ISRCs / Backpack contents back down.
- **service/tray** — run as launchd (macOS) / systemd (Linux) / a tray app; poll
  for work.

## 4. Source layout (current)

```
kellner/
├── Cargo.toml     # package kellner, edition 2024, deps: clap (derive) + anyhow
├── README.md
├── AGENT.md       # this file
└── src/
    └── main.rs    # stubbed clap CLI (backpack / backup / traktor-upload / downloads)
```

## 5. Open points / assumptions

- **Auth against music-api** — assumed to reuse `music-api`'s
  `Authorization: Bearer <MUSIC_API_TOKEN>` model, but the *pairing* flow (how a
  machine obtains its own token) is undecided.
- **Config location** — not decided (likely `~/.config/kellner/config.toml`,
  mirroring the stack's config conventions).
- **Backup transport** — object-store upload protocol (chunked? resumable?) is open.
- **Tray/service** — cross-platform tray (e.g. `tray-icon`) vs. a plain
  launchd/systemd unit is undecided; keep the core CLI-first.
- **Repo rules** — one issue = one branch = one PR; see the root
  [`AGENT.md`](../AGENT.md) and [`docs/ROADMAP.md`](../docs/ROADMAP.md)
  (milestone **kellner-0.1.0**).
