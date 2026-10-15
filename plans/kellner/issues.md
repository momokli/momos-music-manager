# kellner — Epic, Iteration Templates & Leaf-Issue Backlog

> Companion to [`plans/proposed/kellner-0.1.0.md`](../proposed/kellner-0.1.0.md) — the
> plan is authoritative; on any conflict the plan wins. Endpoint names, table names and
> the authority split are copied verbatim from it.
>
> **GitHub (authoritative)**: milestone [`kellner-0.1.0`](https://github.com/momokli/momos-music-manager/milestone/37)
> (**#37**, approved, `Freigabe: ja`) holds the operative leaf issues **#361–#370**; its
> description links back to this plan. The `P0`–`P6` leaves below are the **detailed
> sub-spec** for those issues — they are **not** created as separate GitHub issues.
>
> **Status**: draft for review · **Last updated**: 2026-10-14
>
> **Rules for every leaf issue** (from `AGENT.md` + `.github/workflows/pr-quality.yml`):
> one issue = one branch = one PR; the PR title is a Conventional Commit; the PR body
> carries `Closes #<n>`; **never** commit to `main`.

---

## 1. GitHub wiring & epic context

**Milestone**: [`kellner-0.1.0`](https://github.com/momokli/momos-music-manager/milestone/37)
(**#37**, approved, `Freigabe: ja`). The runner's focus is the lowest-numbered approved
open milestone with work — currently `hub-ops-0.28.0` (#34); kellner is picked up once
#34 / `music-api-0.3.0` (#35) / `music-api-0.4.0` (#36) close.

**Operative leaf issues** (order = runner dispatch order):

| GH # | Title                                                           | Plan phase     | Detailed leaves below           |
| ---- | --------------------------------------------------------------- | -------------- | ------------------------------- |
| #361 | `[Research] ADR: Kellner architecture (local agent)` ← **Gate** | §1 / §10       | `P0-4`                          |
| #363 | `feat(hub): hub_devices registry (Hub side)`                    | P0 (hub)       | plan §3 (hub-side)              |
| #362 | `feat(kellner): device pairing / token with the Hub`            | P0             | `P0-3`                          |
| #364 | `feat(kellner): Traktor collection.nml auto-upload`             | P1             | `P1-1` … `P1-5`                 |
| #365 | `feat(kellner): backups — pull/mirror the library`              | P2 (pull) + P4 | `P2-4`, `P4-1` … `P4-3`         |
| #366 | `feat(kellner): downloads — place music-api orders locally`     | P2             | `P2-3`                          |
| #367 | `feat(kellner): backpack maintenance locally`                   | P2 + P3        | `P2-5`, `P2-6`, `P3-1` … `P3-4` |
| #368 | `feat(kellner): service (launchd/systemd) + tray/menu-bar`      | P5             | `P5-1`                          |
| #369 | `feat(hub): Kellner settings in the Hub`                        | (hub)          | plan §3 (hub-side)              |
| #370 | `test(kellner): packaging + tests`                              | P5 + P6        | `P5-2`, `P6-1`, `P6-2`          |

Two GH issues are **Hub-side** and were only implicit in the first `P0`–`P6` draft:
**#363** (`hub_devices` registry — the Hub half of pairing) and **#369** (per-device
Kellner settings in the Hub). They belong to the hub crate, not `kellner/`; the plan now
covers them in **§3 (Hub-side surface)** and resolves **§10.2** toward a `hub_devices`
device token.

**Order change (ADR-081 §7).** The per-device backpack assignment is v1-critical: **#369**
(+ **#363**) now precede the backpack sync **#365** → `#361 → #363 → #369 → #362 → #364 →
#365 → #366 → #367 → #368 → #370`.

### Epic context

**Title**: `kellner` · **Label**: `epic` (none exists yet — the milestone #37 description
is the tracker) · **Dispatched**: never (context only)

### Body

`kellner` is the local, cross-platform (macOS/Windows/Linux) companion of the music
stack. It keeps each Mac/PC in sync with the always-on servers: it uploads Traktor
`collection.nml` (playcount / rating / playlists) to the hub, pulls the user's active
**Backpacks** down (ordering missing tracks via music-api, writing them flat into the
collection folder, and rendering the user's tags into each file's Comment field —
Traktor-visible), backs up the local library to music-api's object store, and reports
state. It talks **only** to `music-hub` and `music-api` — never to Deezer / Spotify /
SoundCloud / YouTube directly.

**Authority split (resolved — plan §1).** Intrinsic facts (audio bytes + BPM/key/energy
analysis) live in the **music-api Vault** as insert-only satellites with
`record_source`/`analyzer` and precedence `traktor > essentia > reccobeats`. Attributed,
per-user facts (tags, playlists, Backpacks) live in the **hub** — never in the Vault.
kellner reads both and merges locally.

**Milestone**: `kellner-0.1.0`. **Iterations**: `P0` … `P6`.

| Iteration | Outcome                                                                                                     | New server surface                            |
| --------- | ----------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| `P0`      | Crate builds; config + local `state.db` manifest + pairing skeleton. No sync yet.                           | —                                             |
| `P1`      | Traktor `collection.nml` is detected and uploaded to the hub every few minutes.                             | hub `POST /api/hub/traktor/import`            |
| `P2`      | Active Backpacks are diffed, missing tracks ordered + fetched, files land flat, removed tracks soft-pruned. | hub `GET /api/hub/backpacks`, `…/{id}/export` |
| `P3`      | Resolved tags (+BPM/key/energy) are rendered and written into each file's Comment field.                    | —                                             |
| `P4`      | The local library is walked, hashed and backed up to music-api's object store.                              | — (existing object store)                     |
| `P5`      | Background loop runs as launchd / systemd / Task Scheduler; active backpacks are selectable.                | —                                             |
| `P6`      | End-to-end harness (mock hub + music-api) green; README/AGENT/roadmap/CHANGELOG closed out.                 | —                                             |

**Leaf issues**: `P0-1` … `P0-4`, `P1-1` … `P1-5`, `P2-1` … `P2-6`, `P3-1` … `P3-4`,
`P4-1` … `P4-3`, `P5-1` … `P5-2`, `P6-1` … `P6-2` (22 total). The per-iteration work
order is the leaf checklist in each template below, not this table.

**Out of scope (later 2.x line)**: the mascot, the smart-lense engine, group/phase/energy
templating, SoundCloud/YouTube provider work, multi-user on one device.

---

## 2. Iteration templates (P0–P6)

Each iteration is a slice of milestone `kellner-0.1.0`. The template body is the
iteration's GitHub description: outcome statement, leaf-issue checklist, unlock line.
Only the iteration currently being worked carries `Freigabe: ja` — that is **only P0**
for now.

### P0 — foundation: crate + config + state + pairing skeleton

**Outcome**: The `kellner` crate builds standalone (root `cargo build` unaffected);
`~/.config/kellner/config.toml` (env-overridable) resolves the collection dir and the
server URLs; a local `state.db` manifest (`local_files`, `backpack_membership`,
`sync_state`) is created and migrated; a pairing skeleton records a paired identity. No
network sync yet.

**Leaf checklist**

- [ ] `P0-1` — `feat(kellner): scaffold crate, CLI surface and config`
- [ ] `P0-2` — `feat(kellner): local state manifest (state.db)`
- [ ] `P0-3` — `feat(kellner): device pairing and token storage` _(decision §10.2)_
- [ ] `P0-4` — `docs(kellner): README and authority-split ADR`

**Freigabe: ja**

### P1 — Traktor upload

**Outcome**: kellner auto-detects the Traktor `collection.nml` on macOS/Windows (config
override on Linux), watches it, and uploads it to the hub every N minutes via a new
`POST /api/hub/traktor/import`. playcount / rating / playlists land in the hub. A seeded
NML round-trips end-to-end in tests.

**Leaf checklist**

- [ ] `P1-1` — `feat(hub): POST /api/hub/traktor/import endpoint`
- [ ] `P1-2` — `feat(kellner): collection.nml detection and watch (cross-platform)`
- [ ] `P1-3` — `feat(kellner): Traktor upload client`
- [ ] `P1-4` — `test(hub): traktor import integration tests`
- [ ] `P1-5` — `test(kellner): traktor upload integration (mock hub)`

### P2 — Backpack pull

**Outcome**: `GET /api/hub/backpacks` lists the user's backpacks + active set;
`GET /api/hub/backpacks/{id}/export` returns a flat `{track, tags[], bpm/key/energy}`
list (from `hub_track_resolved_tags` + `hub_track_audio`). kellner diffs the export
against its manifest, orders missing ISRCs at music-api, polls state, streams ready
files flat into the collection dir, and soft-moves files that left all active backpacks.

**Leaf checklist**

- [ ] `P2-1` — `feat(hub): GET /api/hub/backpacks and /{id}/export`
- [ ] `P2-2` — `test(hub): backpack export integration tests`
- [ ] `P2-3` — `feat(kellner): backpack diff and order missing ISRCs`
- [ ] `P2-4` — `feat(kellner): stream ready files into the collection folder`
- [ ] `P2-5` — `feat(kellner): prune removed backpack tracks (soft-move)`
- [ ] `P2-6` — `test(kellner): backpack sync integration (mock hub + music-api)`

### P3 — Comment templating

**Outcome**: A template engine renders the resolved tag set (v1 default: alphabetical
list, optionally + BPM/key/energy) and writes it into each file's Comment field
per-format (FLAC / ID3 / M4A). The write records `tags_rendered` and uses a content-hash
strategy that does not break object-store dedup.

**Leaf checklist**

- [ ] `P3-1` — `feat(kellner): comment template engine (alphabetical default)`
- [ ] `P3-2` — `feat(kellner): write Comment field to files (per-format)`
- [ ] `P3-3` — `feat(kellner): content-hash strategy and tags_rendered tracking` _(decision §10.3)_
- [ ] `P3-4` — `test(kellner): comment render and write tests`

### P4 — Backup

**Outcome**: kellner walks the collection dir, hashes each file, and uploads to
music-api's object store with content-hash dedup. Uploads are resumable.

**Leaf checklist**

- [ ] `P4-1` — `feat(kellner): library walk, hash and object-store upload`
- [ ] `P4-2` — `feat(kellner): resumable/chunked upload` _(decision §10.4)_
- [ ] `P4-3` — `test(kellner): backup integration (mock music-api)`

### P5 — Service/tray

**Outcome**: A background loop runs the sync tasks on a schedule as launchd (macOS) /
systemd (Linux) / Task Scheduler (Windows); the active backpack set is selectable (CLI
flag first, tray later).

**Leaf checklist**

- [ ] `P5-1` — `feat(kellner): background loop and service units`
- [ ] `P5-2` — `feat(kellner): active-backpacks selection (CLI flag)`

### P6 — Tests + docs

**Outcome**: A self-contained end-to-end harness (mock hub + music-api) proves the sync
loops; docs/AGENT/roadmap/CHANGELOG are closed out.

**Leaf checklist**

- [ ] `P6-1` — `test(kellner): end-to-end harness (mock hub + music-api)`
- [ ] `P6-2` — `docs(kellner): README, AGENT, roadmap, CHANGELOG close-out`

---

## 3. Leaf-issue backlog

### P0 — foundation

#### P0-1 — `feat(kellner): scaffold crate, CLI surface and config`

- **Goal / why**: Give every later issue a home and a config source; keep the current
  stubbed CLI surface (`backpack` / `backup` / `traktor-upload` / `downloads`).
- **Scope**:
  - `kellner/Cargo.toml` — add deps: `tokio`, `reqwest` (rustls), `serde`/`serde_json`,
    `toml`, `directories`; keep `clap` + `anyhow`.
  - `kellner/src/config.rs` — `~/.config/kellner/config.toml` + env override
    (`KELLNER_*`), resolving `collection_dir`, `hub_url`, `music_api_url`.
  - `kellner/src/main.rs` — wire config into the existing subcommands (still stubs).
- **Write set**: `kellner/Cargo.toml`, `kellner/Cargo.lock`, `kellner/src/config.rs`,
  `kellner/src/main.rs`
- **Acceptance criteria**:
  - [ ] `cd kellner && cargo build` succeeds
  - [ ] `cargo run -- --help` lists the four subcommands
  - [ ] config resolves from TOML with env override (unit test)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: —
- **Size**: M · **Migration**: no

#### P0-2 — `feat(kellner): local state manifest (state.db)`

- **Goal / why**: Incremental sync and prune must be idempotent — this needs a local
  manifest (not a shared DB).
- **Scope**:
  - `kellner/src/state.rs` — SQLite at `~/.config/kellner/state.db`; create/migrate
    `local_files`, `backpack_membership`, `sync_state` (plan §4).
  - `track_key` supports `isrc | basename-fingerprint | local:<id>`; provenance
    `source ∈ user | backpack:<id>`.
- **Write set**: `kellner/src/state.rs`, `kellner/src/main.rs` (init call)
- **Acceptance criteria**:
  - [ ] Fresh `state.db` is created with all three tables
  - [ ] Upsert/read of `local_files` + `backpack_membership` is idempotent (unit test)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`, `area:db`
- **Depends on**: `P0-1`
- **Size**: M · **Migration**: no

#### P0-3 — `feat(kellner): device pairing and token storage`

- **Goal / why**: kellner needs a hub identity to talk to hub + music-api; the flow is
  **resolved** (ADR-081 §10.2).
- **Scope**:
  - `kellner/src/pairing.rs` — create a device token, open the browser at
    `https://hub.zukkafabrik.de/kellner/anheuern` (default host), poll until the logged-in
    user approves (“diesen kellner anheuern?”); store the token (`0600`); `sync_state`
    holds the paired identity; the same token is presented for delegated music-api calls.
- **Write set**: `kellner/src/pairing.rs`, `kellner/src/config.rs`, `kellner/src/main.rs`
- **Acceptance criteria**:
  - [ ] A paired device can present credentials to hub + music-api
  - [ ] Tokens are stored with `0600` perms and never logged; token invalidated after binding
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P0-1`, and the hub registry/approval endpoint (`#363`; ADR-081 §10.2)
- **Size**: M · **Migration**: no

#### P0-4 — `docs(kellner): README and authority-split ADR`

- **Goal / why**: Record the authority split and give the crate a landing page.
- **Scope**:
  - `kellner/README.md` — replace the stub: what kellner is, how to run it.
  - `kellner/AGENT.md` — update "Status: skeleton" → P0 in progress.
  - `docs/DECISIONS.md` — ADR: kellner authority split (Vault = intrinsic, hub =
    attributed) + the two-way read model.
- **Write set**: `kellner/README.md`, `kellner/AGENT.md`, `docs/DECISIONS.md`
- **Acceptance criteria**:
  - [ ] ADR follows the ADR-### format (date, status, context, decision, consequences)
  - [ ] `cd kellner && cargo test` still passes (docs must not break the build)
- **Labels**: `docs`
- **Depends on**: `P0-1`
- **Size**: S · **Migration**: no

### P1 — Traktor upload

#### P1-1 — `feat(hub): POST /api/hub/traktor/import endpoint`

- **Goal / why**: kellner uploads `collection.nml` over HTTP; today only a hub CLI
  (`music-hub import-traktor`) exists.
- **Scope**:
  - `music-hub/src/api.rs` — `POST /api/hub/traktor/import` (session-gated, multipart or
    raw body) writing to a temp file and reusing the existing import path.
  - Reuse the existing Traktor parse + merge logic; no schema change.
- **Write set**: `music-hub/src/api.rs`, `music-hub/src/main.rs`
- **Acceptance criteria**:
  - [ ] Uploaded `collection.nml` updates playcount/rating/playlists for the session user
  - [ ] Non-NML / oversized bodies are rejected with a clear error
  - [ ] Validation: `cd music-hub && cargo test`
- **Labels**: `enhancement`, `area:hub`, `area:api`
- **Depends on**: —
- **Size**: M · **Migration**: no

#### P1-2 — `feat(kellner): collection.nml detection and watch (cross-platform)`

- **Goal / why**: The upload loop needs the NML path without user fiddling.
- **Scope**:
  - `kellner/src/sync/traktor.rs` — detect `collection.nml` on macOS
    (`~/Music/Native Instruments/Traktor*/`) and Windows
    (`%USERPROFILE%\Documents\Native Instruments\Traktor*/`); config override otherwise.
  - Poll every N minutes (interval from config); detect mtime change.
- **Write set**: `kellner/src/sync/traktor.rs`, `kellner/src/config.rs`
- **Acceptance criteria**:
  - [ ] Detects the NML on the two default layouts; honours a config override
  - [ ] Reports "not found" clearly when absent (no panic)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P0-1`, `P0-2`
- **Size**: M · **Migration**: no

#### P1-3 — `feat(kellner): Traktor upload client`

- **Goal / why**: Actually ship the NML to the hub.
- **Scope**:
  - `kellner/src/client/hub.rs` — authenticated `POST /api/hub/traktor/import`.
  - Wire the poll loop (P1-2) to the client; record `last upload` in `sync_state`.
- **Write set**: `kellner/src/client/hub.rs`, `kellner/src/sync/traktor.rs`
- **Acceptance criteria**:
  - [ ] A changed NML triggers exactly one upload; an unchanged one does not
  - [ ] Upload failures are retried with bounded backoff (ADR-061 pattern)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P1-1`, `P1-2`
- **Size**: M · **Migration**: no

#### P1-4 — `test(hub): traktor import integration tests`

- **Goal / why**: Prove the endpoint writes playcount/rating/playlists end-to-end.
- **Scope**:
  - `music-hub/tests/traktor_import.rs` — seed a small NML fixture, POST it, assert the
    resulting hub rows.
- **Write set**: `music-hub/tests/traktor_import.rs`, `music-hub/tests/fixtures/*.nml`
- **Acceptance criteria**:
  - [ ] Seeded NML → expected playcount/rating/playlist rows (exact counts)
  - [ ] Idempotent re-import (COALESCE semantics preserved)
  - [ ] Validation: `cd music-hub && cargo test`
- **Labels**: `test`, `area:hub`
- **Depends on**: `P1-1`
- **Size**: M · **Migration**: no

#### P1-5 — `test(kellner): traktor upload integration (mock hub)`

- **Goal / why**: Prove the kellner upload loop against a stub hub.
- **Scope**:
  - Mock HTTP server asserting the upload body; test the mtime-change trigger.
- **Write set**: `kellner/tests/traktor_upload.rs`
- **Acceptance criteria**:
  - [ ] Changed NML → one upload; unchanged → zero
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `test`, `area:kellner`
- **Depends on**: `P1-3`
- **Size**: M · **Migration**: no

### P2 — Backpack pull

#### P2-1 — `feat(hub): GET /api/hub/backpacks and /{id}/export`

- **Goal / why**: The one new hub read kellner needs — the active Backpack as a flat,
  resolved list.
- **Scope**:
  - `GET /api/hub/backpacks` — the user's backpacks + active set.
  - `GET /api/hub/backpacks/{id}/export` — `{track, isrc, tags[], bpm/key/energy}` from
    `hub_track_resolved_tags` + `hub_track_audio` (vault read-model). No schema change.
  - v1 "active backpack" = subscribed playlists + the user's tags (ADR-063) until
    `hub-backpacks-0.24.0` lands.
- **Write set**: `music-hub/src/api.rs`, `music-hub/src/main.rs`
- **Acceptance criteria**:
  - [ ] Export returns resolved tags + BPM/key/energy per track for the session user
  - [ ] Unknown/empty backpack → empty list (not 500)
  - [ ] Validation: `cd music-hub && cargo test`
- **Labels**: `enhancement`, `area:hub`, `area:api`
- **Depends on**: —
- **Size**: M · **Migration**: no

#### P2-2 — `test(hub): backpack export integration tests`

- **Goal / why**: Prove the export shape and per-user scoping.
- **Scope**:
  - `music-hub/tests/backpack_export.rs` — seed tags + resolved tags + audio rows,
    assert the exported JSON.
- **Write set**: `music-hub/tests/backpack_export.rs`
- **Acceptance criteria**:
  - [ ] Exact tag lists + BPM/key/energy per track
  - [ ] A second user does not see the first user's backpack
  - [ ] Validation: `cd music-hub && cargo test`
- **Labels**: `test`, `area:hub`
- **Depends on**: `P2-1`
- **Size**: M · **Migration**: no

#### P2-3 — `feat(kellner): backpack diff and order missing ISRCs`

- **Goal / why**: Only fetch what's missing; never re-download.
- **Scope**:
  - `kellner/src/sync/backpack.rs` — diff export vs manifest; `POST /orders` for missing
    ISRCs at music-api; poll `GET /tracks?isrcs=` for `ready`.
- **Write set**: `kellner/src/sync/backpack.rs`, `kellner/src/client/api.rs`
- **Acceptance criteria**:
  - [ ] Only ISRCs absent from the manifest are ordered
  - [ ] A re-run after a successful fetch orders nothing
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P0-2`, `P2-1`
- **Size**: M · **Migration**: no

#### P2-4 — `feat(kellner): stream ready files into the collection folder`

- **Goal / why**: Get the bytes onto disk, flat, and record provenance.
- **Scope**:
  - `kellner/src/sync/backpack.rs` — `GET /isrc/{isrc}/{format}` → write flat into
    `collection_dir`; insert `local_files` + `backpack_membership` (`source=backpack:<id>`).
- **Write set**: `kellner/src/sync/backpack.rs`, `kellner/src/client/api.rs`
- **Acceptance criteria**:
  - [ ] Ready tracks land flat; manifest + membership updated
  - [ ] Partial/failed downloads are not recorded as complete
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P2-3`
- **Size**: M · **Migration**: no

#### P2-5 — `feat(kellner): prune removed backpack tracks (soft-move)`

- **Goal / why**: A track that left all active backpacks must not linger — but never be
  hard-deleted.
- **Scope**:
  - Soft-move to `.kellner-trash/`; only `source = backpack:*` files are candidates
    (never `user`); reuses the prune-safety logic (`relax-prune-safety-gates`).
- **Write set**: `kellner/src/sync/backpack.rs`
- **Acceptance criteria**:
  - [ ] Removed-from-all-backpacks files are soft-moved; `user` files never are
  - [ ] Nothing is hard-deleted
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P2-4`
- **Size**: M · **Migration**: no

#### P2-6 — `test(kellner): backpack sync integration (mock hub + music-api)`

- **Goal / why**: Prove the full pull loop: diff → order → fetch → record → prune.
- **Scope**:
  - Mock hub (`/backpacks`, `/export`) + mock music-api (`/orders`, `/tracks`,
    `/isrc/...`); assert the on-disk result and manifest.
- **Write set**: `kellner/tests/backpack_sync.rs`
- **Acceptance criteria**:
  - [ ] Missing tracks fetched exactly once; membership recorded
  - [ ] A track leaving the backpack is soft-moved
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `test`, `area:kellner`
- **Depends on**: `P2-5`
- **Size**: L · **Migration**: no

### P3 — Comment templating

#### P3-1 — `feat(kellner): comment template engine (alphabetical default)`

- **Goal / why**: The core feature — turn the resolved tag set into the Comment string.
- **Scope**:
  - `kellner/src/tag/comment.rs` — v1 default: alphabetical tag list (stable sort),
    optional BPM/key/energy tokens. Input = export row (P2-1).
- **Write set**: `kellner/src/tag/comment.rs`
- **Acceptance criteria**:
  - [ ] Deterministic, stable output for a given tag set
  - [ ] Empty tag set yields an empty/neutral comment (documented)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P2-1`
- **Size**: M · **Migration**: no

#### P3-2 — `feat(kellner): write Comment field to files (per-format)`

- **Goal / why**: Traktor must see the tags; the write is format-specific.
- **Scope**:
  - `kellner/src/tag/comment.rs` — write Comment for FLAC (Vorbis), MP3 (ID3), M4A
    (atoms). Mechanism per **plan §5.2** (file metadata vs `collection.nml` vs sidecar).
- **Write set**: `kellner/src/tag/comment.rs`, `kellner/Cargo.toml` (tag crate)
- **Acceptance criteria**:
  - [ ] Comment round-trips per format (write → read back)
  - [ ] Unchanged content is not rewritten (mtime preserved)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P3-1`
- **Size**: M · **Migration**: no

#### P3-3 — `feat(kellner): content-hash strategy and tags_rendered tracking`

- **Goal / why**: Writing tags must not break backup dedup — **resolved** (ADR-081 §10.3).
- **Scope**:
  - Hash the **canonicalised** file (Comment cleared) → `content_hash`; record
    `tags_rendered` + `comment_written_at` in `local_files`. No sidecar, no raw-file hash.
- **Write set**: `kellner/src/tag/comment.rs`, `kellner/src/state.rs`
- **Acceptance criteria**:
  - [ ] Comment write does not cause a spurious backup re-upload (canonical hash)
  - [ ] `tags_rendered` reflects the last written string
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P3-2`
- **Size**: M · **Migration**: no

#### P3-4 — `test(kellner): comment render and write tests`

- **Goal / why**: Lock down the render + write behaviour per format.
- **Scope**:
  - Fixtures per format; assert render output + round-tripped Comment field.
- **Write set**: `kellner/tests/comment.rs`, `kellner/tests/fixtures/**`
- **Acceptance criteria**:
  - [ ] Render + write covered for FLAC/ID3/M4A
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `test`, `area:kellner`
- **Depends on**: `P3-2`
- **Size**: M · **Migration**: no

### P4 — Backup

#### P4-1 — `feat(kellner): library walk, hash and object-store upload`

- **Goal / why**: Back up the local library; dedup by content hash.
- **Scope**:
  - `kellner/src/sync/backup.rs` — walk `collection_dir`, hash, upload to music-api's
    object store; skip already-present hashes.
- **Write set**: `kellner/src/sync/backup.rs`, `kellner/src/client/api.rs`
- **Acceptance criteria**:
  - [ ] Content-hash dedup skips already-backed-up files
  - [ ] Applied to the same folder as backpack download without collisions (plan §5.5)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P0-2`
- **Size**: M · **Migration**: no

#### P4-2 — `feat(kellner): resumable/chunked upload`

- **Goal / why**: v1 uses whole-file idempotent `PUT /objects/{sha256}` + `POST
/objects/check` skip (ADR-081 §10.4). Chunked/resumable is a **v2** upgrade, not
  v1-blocking.
- **Scope**:
  - Chunked/resumable upload; resume from `sync_state`.
- **Write set**: `kellner/src/sync/backup.rs`, `kellner/src/client/api.rs`
- **Acceptance criteria**:
  - [ ] An interrupted upload resumes rather than restarting
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P4-1`
- **Size**: M · **Migration**: no

#### P4-3 — `test(kellner): backup integration (mock music-api)`

- **Goal / why**: Prove walk + dedup + resume.
- **Scope**:
  - Mock object-store endpoint; assert dedup and resume behaviour.
- **Write set**: `kellner/tests/backup.rs`
- **Acceptance criteria**:
  - [ ] Dedup skips known hashes; resume continues a partial upload
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `test`, `area:kellner`
- **Depends on**: `P4-2`
- **Size**: M · **Migration**: no

### P5 — Service/tray

#### P5-1 — `feat(kellner): background loop and service units`

- **Goal / why**: Run the sync tasks unattended on each OS.
- **Scope**:
  - A single background loop scheduling P1/P2/P3/P4 tasks; launchd (macOS), systemd
    (Linux), Task Scheduler (Windows) unit templates.
- **Write set**: `kellner/src/main.rs`, `kellner/deploy/**`
- **Acceptance criteria**:
  - [ ] `kellner run` loops on the configured intervals without panic
  - [ ] Unit templates install on each OS (documented)
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P1-3`, `P2-4`, `P4-1`
- **Size**: M · **Migration**: no

#### P5-2 — `feat(kellner): tray / menu-bar widget (feierabend, re-auth)`

- **Goal / why**: kellner v1 **is** a tray/menu-bar widget (battery-friendly — ADR-081 §8).
  The active-backpack set is configured in the Hub (#369), **not** locally, so there is no
  local selection command.
- **Scope**:
  - Tray icon + status (idle / syncing / error); right-click = exactly **“feierabend”**
    (quit + stop the loop) and **“re-auth”** (re-run the browser approval).
- **Write set**: `kellner/src/main.rs`, `kellner/src/tray.rs`
- **Acceptance criteria**:
  - [ ] Tray shows status; the two items work; no other config surface on the device
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `enhancement`, `area:kellner`
- **Depends on**: `P5-1`, `P0-3`
- **Size**: M · **Migration**: no

### P6 — Tests + docs

#### P6-1 — `test(kellner): end-to-end harness (mock hub + music-api)`

- **Goal / why**: One harness proving the whole loop.
- **Scope**:
  - Combined mock hub + music-api; end-to-end: upload → export → order → fetch → tag →
    backup.
- **Write set**: `kellner/tests/e2e.rs`
- **Acceptance criteria**:
  - [ ] Full loop passes deterministically
  - [ ] Validation: `cd kellner && cargo test`
- **Labels**: `test`, `area:kellner`
- **Depends on**: `P1-3`, `P2-4`, `P3-2`, `P4-1`
- **Size**: L · **Migration**: no

#### P6-2 — `docs(kellner): README, AGENT, roadmap, CHANGELOG close-out`

- **Goal / why**: Keep the repo docs and the plan index truthful.
- **Scope**:
  - `kellner/README.md` + `kellner/AGENT.md` final state; `docs/ROADMAP.md` +
    `plans/README.md` + `kellner/CHANGELOG.md` close-out; move the plan to
    `plans/done/` when shipped.
- **Write set**: `kellner/README.md`, `kellner/AGENT.md`, `docs/ROADMAP.md`,
  `plans/README.md`, `kellner/CHANGELOG.md`
- **Acceptance criteria**:
  - [ ] Docs reflect shipped behaviour; plan lifecycle updated
  - [ ] Validation: docs-only change, `cargo build` in `kellner/` still passes
- **Labels**: `docs`
- **Depends on**: all P0–P5
- **Size**: S · **Migration**: no

---

## 4. Dependency / order table

`→` means "must land before". Items on the same line after `‖` are **file-disjoint and
parallelizable** once their dependencies are met.

### P0 — foundation

| Order | Issues (parallel group) | Notes                                                                              |
| ----- | ----------------------- | ---------------------------------------------------------------------------------- |
| 1     | `P0-1`                  | Scaffold + config must exist first.                                                |
| 2     | `P0-2` ‖ `P0-4`         | State manifest and docs are disjoint.                                              |
| 3     | `P0-3`                  | Pairing needs config + the hub approval endpoint (#363); §10.2 resolved — ADR-081. |

**Parallelizable**: `P0-2`/`P0-4`. `P0-3` also needs the hub-side `#363` endpoint.

### P1 — Traktor upload

| Order | Issues (parallel group) | Notes                                                |
| ----- | ----------------------- | ---------------------------------------------------- |
| 1     | `P1-1` ‖ `P1-2`         | Hub endpoint and kellner NML detection are disjoint. |
| 2     | `P1-4`                  | Hub endpoint tests need `P1-1`.                      |
| 3     | `P1-3`                  | Client needs the endpoint + detection.               |
| 4     | `P1-5`                  | Kellner upload integration needs the client.         |

**Parallelizable**: `P1-1`/`P1-2`, then `P1-4` alongside `P1-3`.

### P2 — Backpack pull

| Order | Issues (parallel group) | Notes                                                 |
| ----- | ----------------------- | ----------------------------------------------------- |
| 1     | `P2-1`                  | The hub export is the foundation; also unblocks P3-1. |
| 2     | `P2-2` ‖ `P2-3`         | Export tests and the kellner diff are disjoint.       |
| 3     | `P2-4`                  | Stream needs the diff/order flow.                     |
| 4     | `P2-5`                  | Prune needs the fetch/provenance recorded.            |
| 5     | `P2-6`                  | Full sync integration.                                |

**Parallelizable**: `P2-2`/`P2-3`. `P2-4` → `P2-5` → `P2-6` are serial.

### P3 — Comment templating

| Order | Issues (parallel group) | Notes                                                               |
| ----- | ----------------------- | ------------------------------------------------------------------- |
| 1     | `P3-1`                  | Renderer needs the export shape (`P2-1`).                           |
| 2     | `P3-2`                  | File write needs the renderer.                                      |
| 3     | `P3-4` ‖ `P3-3`         | Write tests and the hash strategy are disjoint (both after `P3-2`). |

**Parallelizable**: `P3-3`/`P3-4`. `P3-3` is no longer decision-gated (§10.3 resolved —
canonical hash, ADR-081).

### P4 — Backup

| Order | Issues (parallel group) | Notes                                        |
| ----- | ----------------------- | -------------------------------------------- |
| 1     | `P4-1`                  | Walk + hash + upload.                        |
| 2     | `P4-2`                  | v2 upgrade; not v1-blocking (ADR-081 §10.4). |
| 3     | `P4-3`                  | Backup integration tests.                    |

**Parallelizable**: none — serial chain.

### P5 — Service/tray

| Order | Issues (parallel group) | Notes                                        |
| ----- | ----------------------- | -------------------------------------------- |
| 1     | `P5-1` ‖ `P5-2`         | Loop/units and the tray widget are disjoint. |

### P6 — Tests + docs

| Order | Issues (parallel group) | Notes                |
| ----- | ----------------------- | -------------------- |
| 1     | `P6-1`                  | End-to-end harness.  |
| 2     | `P6-2`                  | Docs close-out last. |

### Cross-iteration

P0 → P1 → P2 → P3 → P4 → P5 → P6 are sequential at the iteration level. Within an
iteration, the parallel groups above are the only safe concurrency; a leaf issue never
starts before its listed dependencies have merged to `main`.

**Decision gates — all resolved 2026-10-15** (ADR-081, #361): `P0-3` §10.2 (pairing =
browser approval via `/kellner/anheuern`); `P3-3` §10.3 (canonical hash, no dedup break);
`P4-2` §10.4 (v1 whole-file, chunked = v2); `P1-1` §10.5 (token-gated
`POST /api/hub/traktor/import`). §10.6 moves `#369` before `#365`.

Every leaf issue is one branch, one PR, one `Closes #<n>`, with a Conventional-Commit
title — and none of them ever commits to `main`.
