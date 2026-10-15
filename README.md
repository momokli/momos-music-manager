# Music stack — `music-hub` · `music-api` · `kellner`

> **Scope change (2026-10-12).** This repository is no longer "one app". It is the
> monorepo for a **music stack**: two always-on servers and a local companion.
> **Momo's Music Manager (MMM)** — the original single-user Mac app — is
> **decommissioned / sunset**; its code lives under `legacy/mmm/`
> (see [Legacy](#legacy-mmm--sunset)).

---

## The stack

| Project                             | Path                                                                  | Role                                                                                                                                               | Runs on          | Docs                                                                             |
| ----------------------------------- | --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------- | -------------------------------------------------------------------------------- |
| **music-hub** (codename _spelunke_) | [`music-hub/`](music-hub/)                                            | Multi-user web UI + DB over Spotify / SoundCloud / YouTube / Traktor. Tags, playlists, collectives, digging, discovery, now-playing.               | always-on server | [`music-hub/AGENT.md`](music-hub/AGENT.md)                                       |
| **music-api**                       | [`music-api/`](music-api/)                                            | The **authority**: ISRC/order → download service (Deezer / deemix / yt-dlp / spotDL), object store, and the **BPM/key/metadata catalogue**.        | always-on server | [`music-api/README.md`](music-api/README.md)                                     |
| **kellner**                         | [`kellner/`](kellner/)                                                | Local **cross-platform companion**: Backpack upkeep, **backups of the music library → music-api**, Traktor `collection.nml` **upload**, downloads. | each Mac / PC    | [`kellner/AGENT.md`](kellner/AGENT.md)                                           |
| ~~MMM~~                             | `legacy/mmm/` (`src/`, `frontend/`, `migrations/`, `tests/`, `site/`) | The original single-user Mac app. **Sunset.**                                                                                                      | —                | [`docs/legacy/mmm-README.md`](docs/legacy/mmm-README.md), [`legacy/mmm/FROZEN.md`](legacy/mmm/FROZEN.md) |

**How they relate:** the hub _orders_ and _reads_; **music-api downloads, analyses,
remembers and expands**; kellner keeps the local machines in sync with music-api.
Architecture plan: [`plans/proposed/hub-crawl-everything.md`](plans/proposed/hub-crawl-everything.md).

---

## Repo layout

```
.
├── music-hub/          # music-hub  — web app (axum + askama + sqlx/SQLite)
├── music-api/          # music-api  — order/download + metadata catalogue
├── kellner/            # local companion (skeleton)
├── docs/
│   ├── ROADMAP.md      # milestones + sequencing (source of truth: GitHub)
│   ├── ARCHITECTURE.md # stack-level architecture
│   └── legacy/         # MMM-era docs (ARCHITECTURE, DECISIONS, mmm-README)
├── plans/              # one file per plan; index in plans/README.md
├── legacy/
│   ├── mmm/            # legacy MMM app (sunset)
│   └── download-service/  # legacy download service
└── AGENT.md            # repo-wide agent guidance + legacy MMM
```

---

## Status

- **music-hub** — active (primary). Live at <https://hub.zukkafabrik.de>.
- **music-api** — active. Metadata/BPM authority; download service.
- **kellner** — **new**; skeleton only (stubbed CLI).
- **MMM** — **sunset**. Feature freeze; data migrates to the hub. Still runs on the
  Mac until the migration completes.

See [`docs/ROADMAP.md`](docs/ROADMAP.md) for milestones and sequencing.

---

## Working in this repo

- **Repo-wide rules** (branches, PRs, conventional commits, release): [`AGENT.md`](AGENT.md).
- **Per-project rules win for their subtree** — read `music-hub/AGENT.md` /
  `music-api/README.md` / `kellner/AGENT.md` before touching those crates.
- **Plans**: one file per plan under [`plans/`](plans/); index + lifecycle in
  [`plans/README.md`](plans/README.md).
- **Deploy**: each server has its own runbook (see `music-hub/AGENT.md` for the hub's
  rsync loop; music-api's is in its README).

---

## Legacy: MMM — sunset

The original **Momo's Music Manager** (single-user, local files + Spotify, Traktor)
was the starting point of everything above. It is **decommissioned**:

- code: `legacy/mmm/` (`src/`, `frontend/`, `migrations/`, `tests/`, `site/`)
- docs: [`docs/legacy/mmm-README.md`](docs/legacy/mmm-README.md), [`AGENT.md`](AGENT.md)
- migration target: **music-hub** (+ **music-api** for downloads/metadata)

Treat MMM as read-only unless a fix is needed to keep it running through the
transition — see [`legacy/mmm/FROZEN.md`](legacy/mmm/FROZEN.md) (frozen; escape label `mmm:unfreeze`).
