# Spelunke Roadmap

> Consolidated roadmap across the projects in this repository. **Source of truth
> for the milestones/issues** is GitHub; this document explains the logic and
> sequencing. Last restructured: 2026-10-12.

## Projects

The repository now hosts three-and-a-bit products. The centre of gravity has
moved **away from MMM** toward the Hub + music-api, with a small local agent
(“Kellner”) on each machine.

| Project                                             | What it is                                                                                                                                               | Status           |
| --------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------- |
| **music-api**                                       | ISRC/order → Deezer/SoundCloud download service, object store, metadata authority (BPM/key). Rust, systemd, port 8710.                                   | active           |
| **music-hub** (codename _spelunke_, dir `music-hub/`) | Multi-user web UI + DB over Spotify/SoundCloud/YouTube/Traktor: tagging, digging, clusters, curated playlists, now-playing. Rust/Axum/SQLite, port 8080. | active (primary) |
| **kellner**                                         | Local agent on Mac/PC: Traktor upload, backups, downloads, backpack upkeep. launchd/systemd.                                                             | **new**          |
| **mmm**                                             | Momo’s Music Manager (original local app).                                                                                                               | **sunset**       |

## Milestones (GitHub) and their intent

GitHub milestones are the unit of _release slicing_; each milestone is a
coherent, shippable slice. Labels (`area:*`, `research`) tag the work domain.

### music-api

- **music-api-0.2.0** — durable queue, bounded retries, download lease, pause/resume, control API.
- **music-api-0.3.0** — metadata authority (BPM/key), object store + prune, SoundCloud/YouTube sources, native Hub metadata route.
- **music-api-0.4.0** — ops: playlist download, backup-store as source, priority/control panel.

### spelunke Hub (web)

0.1–0.23 already shipped (walking skeleton → now-playing, tag search, smart playlists).
Next slices:

- **hub-backpacks-0.24.0** — _new_: multiple backpacks per user, active set, backpack = collection of playlists/tags/smart-filters.
- **hub-recs-0.25.0** — cluster-driven recommendations, embedding provider, per-user scope, similarity v2.
- **hub-artist-album-0.26.0** — artist depth (collabs, b2b, reference scoring) + album entity/view.
- **hub-tagging-0.27.0** — daily/weekly tag task, completeness flag, tag-page insights polish.
- **hub-ops-0.28.0** — engine/scoring settings UI, admin/ops console, import-history polish, music-api native surface.

### kellner (new)

- **kellner-0.1.0** — pairing, Traktor upload, backups, downloads, backpack upkeep, service/tray.

### mmm (sunset)

- **mmm-sunset-1.19.0** — feature freeze, data migration to Hub, deprecation docs.

### infra

- **infra-deploy-1.0.0** — deploy/runbook, CI expansion, secrets/config.

## Sequencing (logical order, one slice at a time)

1. **Hub foundations already shipped**: ingest, tag layer/types, digging, tag-queue, now-playing, curated playlists, uniform track actions, clusters.
2. **hub-backpacks-0.24.0** — backpacks are the _selection_ primitive the rest leans on (Hub views, Kellner, exports).
3. **music-api-0.3.0** — metadata authority + sources unlock scoring/ripeness/backfill.
4. **hub-recs-0.25.0** — build on clusters (#261) + backpacks for “why recommended”.
5. **hub-artist-album-0.26.0** — explorer depth.
6. **hub-tagging-0.27.0** — completeness + daily/weekly task.
7. **kellner-0.1.0** — local automation on top of a stable Hub API.
8. **hub-ops-0.28.0** + **infra-deploy-1.0.0** — configurability & runbooks.
9. **music-api-0.4.0** + **mmm-sunset-1.19.0** — finish the move off MMM.

## Working the roadmap

- One issue = one branch = one PR against the current integration branch
  (`feat/music-hub` for Hub work). Conventional-Commit title, `Closes #<n>`.
- `epic` issues collect sub-issues via checklists; they are never “done” on
  their own.
- `research` issues gate the implementation issues that depend on a decision.
