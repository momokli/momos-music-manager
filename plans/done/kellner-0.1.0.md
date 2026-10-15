# Plan: kellner-0.1.0

**Status**: approved (2026-10-15 — §10 locked, ADR-081)
**Branch**: `feat/kellner-0.1.0`
**Ready for review**: yes — **§10 resolved** 2026-10-15 (ADR-081, comment on #361)
**Depends on**: `music-api-0.3.0` (vault metadata + object store + analysis backfill — largely shipped), hub OIDC sessions (shipped). Full smart-filter Backpacks need `hub-backpacks-0.24.0`; v1 can read the existing tags + subscriptions.
**Migration needed**: kellner — no (new crate); hub/music-api — **additive** only (v1 needs no new columns for the read endpoint)
**Parent epic**: (to be created) `epic: kellner`
**Methodology**: milestone = one iteration = one release; only leaf issues are worked.

> **Detailed design / leaf-issue backlog** lives in [`plans/done/kellner/issues.md`](kellner/issues.md)
> — iteration templates (`P0`–`P6`), the mapping to the GitHub issues, write sets,
> acceptance criteria and a dependency/order table.
>
> **GitHub**: milestone [`kellner-0.1.0`](https://github.com/momokli/momos-music-manager/milestone/37)
> (**#37**, approved) holds the operative leaf issues **#361–#370**; its description links
> back to this plan. **#37 is the current runner focus.** The six §10 decisions were locked
> on 2026-10-15 (**ADR-081**, comment on **#361**), so `#361` is done and the previously
> `question`-gated leaves **#362–#365** are dispatchable again.

### Description

`kellner` is the local, cross-platform (macOS/Windows/Linux) companion that keeps each Mac/PC in sync with the always-on servers. It (1) uploads Traktor `collection.nml` (playcount/rating/playlists) to the hub every few minutes, (2) pulls the user's active Backpacks down — ordering missing tracks via music-api, writing them flat into the collection folder, and rendering the user's tags into each file's Comment field (Traktor-visible), (3) backs up the local library to music-api's object store, and (4) reports state. It talks only to `music-api` and `music-hub` — never to Deezer/Spotify/SoundCloud/YouTube directly.

### 1. Authority split (resolved)

| Concern                                             | Owner                 | Notes                                                                                                       |
| --------------------------------------------------- | --------------------- | ----------------------------------------------------------------------------------------------------------- |
| Providers, downloads, object store, audio analysis  | **music-api**         | Single authority; Essentia = backfill analyzer                                                              |
| BPM/key/energy catalogue (satellites + `resolved`)  | **music-api** (Vault) | Insert-only satellites, `record_source`/`analyzer` provenance, precedence `traktor > essentia > reccobeats` |
| Identity, tags, playlists, Backpacks, smart-filters | **music-hub**         | Unersetzlich, per-User; **nie in den Vault**                                                                |
| Comment-field rendering (local view of "my tags")   | **kellner**           | Hub owns tags; kellner only renders onto local files                                                        |
| Traktor playcount/rating/playlist-order             | **kellner**           | Device is authority; uploads to hub                                                                         |
| Device registry + per-device settings               | **music-hub**         | `hub_devices` (#363); per-device Kellner settings (#369)                                                    |
| Local file↔track mapping, prune                     | **kellner**           | Local manifest                                                                                              |

Principle: **intrinsic facts (audio + analysis) → music-api Vault; attributed facts (tags/playlists/backpacks) → hub; kellner merges both locally.**

### 2. Provenance & satellites (already correct — no change)

The Vault already captures "woher es kam": every satellite is insert-only with `record_source` / `analyzer`. Essentia backfills anything missing (idempotent). Traktor collections land as `source=traktor` satellites via `POST /metadata`. Diffs between sources stay visible because rows are never overwritten; `resolved` only picks per precedence. kellner consumes the resolved value and never re-derives anything.

### 3. Interfaces

**kellner ↔ hub** (session-gated OIDC):

| Method | Path                             | Use                                                                                                        |
| ------ | -------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `GET`  | `/api/hub/backpacks`             | list backpacks + active set                                                                                |
| `GET`  | `/api/hub/backpacks/{id}/export` | **the one new read**: `{track, tags[], bpm/key/energy}` from `hub_track_resolved_tags` + `hub_track_audio` |
| `POST` | `/api/hub/traktor/import`        | HTTP upload of `collection.nml` (mirrors the CLI)                                                          |

**kellner ↔ music-api** (existing contract, `Bearer <MUSIC_API_TOKEN>`): `POST /orders`, `GET /tracks?isrcs=`, `GET /isrc/{isrc}/{format}`, `GET /vault/tracks?isrcs=`.

**Hub-side surface (new — `music-hub` crate, per GH #363 / #369):**

| Method      | Path                             | Use                                                                                       |
| ----------- | -------------------------------- | ----------------------------------------------------------------------------------------- |
| `POST`      | `/api/hub/devices`               | register / re-pair a device → issue a per-device token (#363)                             |
| `GET`       | `/api/hub/devices`               | list the user's devices (#363)                                                            |
| `DELETE`    | `/api/hub/devices/{id}`          | revoke a device token (#363)                                                              |
| `GET`/`PUT` | `/api/hub/devices/{id}/settings` | per-device Kellner settings — active backpacks, preferred format, comment template (#369) |

The device token supersedes the ad-hoc pairing sketch: kellner stores the token from (re)pairing and presents it as its hub credential; per-device config is authoritative in the Hub — **not** locally at all (§10.2). The registry + settings tables are **additive** hub migrations.

**Pairing approval page (hub web route, not in the table above):** `GET /kellner/anheuern`
with the device token — the logged-in user sees “diesen kellner anheuern?” Ja/Nein; “Ja”
binds the device to that user. The link _is_ the capability; the token is invalidated after
binding (and expires unused).

### 4. Local state (`state.db`)

kellner owns a local SQLite manifest (not a shared DB — per `kellner/AGENT.md`) at `~/.config/kellner/state.db` (or platform equivalent). It makes incremental sync and prune idempotent:

```sql
local_files(path TEXT PRIMARY KEY, content_hash TEXT, size INTEGER, mtime INTEGER,
            track_key TEXT,          -- isrc | basename-fingerprint | local:<id>
            tags_rendered TEXT,       -- last written Comment string
            comment_written_at INTEGER);

backpack_membership(track_key TEXT, backpack_id TEXT, added_at INTEGER, source TEXT, PRIMARY KEY(track_key, backpack_id));

sync_state(key TEXT PRIMARY KEY, value TEXT);  -- cursors, last upload, active backpacks, paired identity
```

Provenance (`source = 'user' | 'backpack:<id>'`) tells kellner what is safe to prune — only `backpack:` files are auto-removed when a track leaves all active backpacks.

### 5. Core decisions (forks)

**5.1 File↔track matching.** Join between a local file and its hub track (and its tags). Priority order: (a) ISRC in file tags, (b) `collection.nml` (Traktor already mapped file→track), (c) basename fingerprint (`traktor-basename-matching`), (d) artist+title `COLLATE NOCASE`. Stems have no ISRC — see §5.4.

**5.2 Comment write mechanics — resolved (§10.3).** Traktor reads tags from the file + caches them in `collection.nml`. kellner writes the Comment into the **file** via `lofty` (FLAC Vorbis / ID3 / M4A) **and** mirrors it into `collection.nml` so Traktor needs no manual "Check Consistency" (guard: only when Traktor is closed, else defer to the next run; atomic temp+rename). The apparent `content_hash` conflict is **not real**: the backup hash is the SHA-256 of the **canonicalised** file (Comment cleared) — the same semantics the object store already uses ([`legacy/mmm/src/store.rs`](../../legacy/mmm/src/store.rs), migration 028), so comment-writing does **not** break dedup. The earlier "breaks dedup" note was based on a raw-file hash and is stale.

**5.3 Flat folder + prune.** One flat folder; tags-in-comment replace directories. Prune = soft-move to `.kellner-trash/` (never delete), driven by `backpack_membership`; only `backpack:` files are candidates. Reuses `relax-prune-safety-gates` + `staging-area-pull` logic.

**5.4 Stems are not tracks.** `.stem.mp4` = 4 parts, no ISRC, no BPM/key. A stem inherits its parent track's tags (reuse `wav-source-linking`: `stem_type` + `source_of`).

**5.5 Download vs backup.** Two directions on one folder: backpack sync downloads (order → wait → fetch → tag), backup uploads (hash → object store). `content_hash` dedup means downloaded files aren't re-uploaded; the manifest tracks provenance so prune only touches downloaded files.

**5.6 Templating default (v1).** Alphabetical tag list. Input = resolved tag set + optional BPM/key/energy. Stable sort; the group/phase/energy engine is v2 (§9).

### 6. Phases & acceptance

**P0 — pairing + config + state.** — [ ] `state.db` schema; `~/.config/kellner/config.toml` **only** for the hub-host override (default `hub.zukkafabrik.de`) — no feature config on the device (§10.2); paired-identity record via the hub `hub_devices` registry (#363) through the browser approval URL `/kellner/anheuern`. — [ ] `cargo build` + unit tests for config/manifest.

**P1 — Traktor upload.** — [ ] auto-detect `collection.nml` (macOS/Windows paths + config override); poll every N min; `POST /api/hub/traktor/import`. — [ ] integration test: seeded NML → playcount/rating/playlists land in hub.

**P2 — backpack pull.** — [ ] `GET /api/hub/backpacks` + `/export` return resolved tracks + tags. — [ ] kellner diffs export vs manifest → orders missing ISRCs (`POST /orders`), polls `GET /tracks?isrcs=`, streams ready files flat into the folder, records provenance. — [ ] prune: tracks that left all active backpacks are soft-moved, not deleted.

**P3 — comment templating.** — [ ] render tag list (+BPM/key/energy) via template; write to file (mechanism per §5.2); record `tags_rendered`. — [ ] content-hash strategy resolved so backup dedup survives (§5.2).

**P4 — backup.** — [ ] walk collection dir, hash, upload to object store, content-hash dedup. — [ ] resumable/chunked upload (§10).

**P5 — service/tray.** — [ ] background loop; launchd (macOS) / systemd (Linux) / Task Scheduler (Windows); **tray/menu-bar widget is v1** (battery-friendly), not a later add-on — right-click shows exactly **"feierabend"** (quit) and **"re-auth"**. No local "active backpacks" selection: that set lives in the Hub per device (§10.2/#369).

**P6 — tests + docs.** — [ ] mock HTTP server for hub + music-api in kellner tests; `cargo test` green; README/AGENT/roadmap/ADR updated.

### 7. Files to modify

| File                                            | Change                                                                            |
| ----------------------------------------------- | --------------------------------------------------------------------------------- |
| `kellner/Cargo.toml`                            | add reqwest/tokio, rusqlite, serde, a tag writer (lofty/ID3/flac)                 |
| `kellner/src/config.rs`                         | hub-host override (default `hub.zukkafabrik.de`) + env + collection.nml detection |
| `kellner/src/state.rs`                          | local `state.db` manifest                                                         |
| `kellner/src/pairing.rs`                        | device pairing + identity                                                         |
| `kellner/src/client/{hub,api}.rs`               | HTTP clients for hub + music-api                                                  |
| `kellner/src/sync/{traktor,backpack,backup}.rs` | the three sync loops                                                              |
| `kellner/src/tag/comment.rs`                    | template render + file write                                                      |
| `kellner/src/main.rs`                           | wire commands + background loop                                                   |
| `music-hub/src/api.rs`                          | `GET /backpacks`, `/backpacks/{id}/export`, `POST /traktor/import`                |
| `music-hub/src/devices.rs`                      | `hub_devices` registry + token issue/revoke (#363)                                |
| `music-hub/src/settings.rs`                     | per-device Kellner settings (#369)                                                |
| `music-hub/migrations/NNN_hub_devices.sql`      | additive `hub_devices` + per-device settings tables (#363/#369)                   |
| `music-hub/tests/**`                            | integration tests for the new endpoints                                           |
| `plans/README.md`                               | index entry                                                                       |
| `docs/DECISIONS.md`                             | ADR: kellner authority split + comment-write/content-hash decision                |

### 8. Risks

- **content_hash vs comment-write — resolved (§5.2/§10.3).** Writing tags does _not_ change the backup identity: the hash is taken over the **canonicalised** file (Comment cleared), as the object store already does. No silent dedup break.
- **Two-direction sync collisions** — upload (playcount) and download (backpack) touch the same `collection.nml`/folder; needs per-field authority.
- **Sync correctness regressions** — ADR-060 (mirror-not-append), 061 (rate-limit/backoff), 062 (tolerant verification) must be re-applied or kellner repeats MMM's bugs.
- **Cross-platform metadata** — FLAC/ID3/M4A/`collection.nml` behave differently; exiftool subprocess vs pure-Rust tag crate.
- **Scope creep** — the smart-lense engine is a _hub_ feature (§9); kellner must not re-derive it.

### 9. Out of scope (v2)

- The graphical mascot (waiter / cat).
- The smart-lense engine (relevance + BPM range + "diverse key distribution") — `hub-backpacks-0.24.0` / `hub-recs-0.25.0`.
- Group/phase/energy templating (v1 = alphabetical tag list).
- SoundCloud/YouTube provider work.
- Multi-user on one device.

### 10. Decisions (resolved 2026-10-15 — ADR-081, #361)

All six forks are locked; the milestone is no longer decision-gated.

1. **Data ownership — resolved.** Tags/Backpacks stay in the hub; kellner reads tags from
   the hub and bytes + BPM/key from the music-api Vault (intrinsic → Vault, attributed → hub).
2. **Pairing/auth — resolved.** kellner creates a **device token** on first start and opens
   `https://hub.zukkafabrik.de/kellner/anheuern` (default hub host; overridable via
   `~/.config/kellner/config.toml`). Whoever opens the link logged-in and answers **“diesen
   kellner anheuern?” → Ja** binds the device to that user — the link _is_ the capability.
   The endpoint requires a logged-in session; the token is **invalidated after binding** (and
   expires unused). **No feature config on the device**: the Traktor-upload and
   backpack-sync toggles _and_ the per-device backpack set live in the Hub (#369/#363). Tray
   right-click = exactly **“feierabend”** (quit) and **“re-auth”**. No provider credentials
   in kellner.
3. **Comment write vs content_hash — resolved.** Write the Comment via `lofty` into the
   **file** _and_ mirror it into `collection.nml` (guard: only when Traktor is closed, else
   defer to the next run; atomic temp+rename). The stable backup hash is the **SHA-256 of the
   canonicalised file (comment cleared)** — same semantics as the object-store key, so
   comment-writing does **not** break dedup (§5.2/§8 corrected).
4. **Backup transport — resolved (v1).** Whole-file idempotent `PUT /objects/{sha256}`;
   present objects are skipped via `POST /objects/check`. Chunked/resumable upload is v2.
5. **Traktor upload path — resolved.** New token-gated **`POST /api/hub/traktor/import`**
   (Bearer device token, nml in the body); the hub CLI `import-traktor` stays for manual /
   server-side imports.
6. **Sequencing/scope — resolved.** The per-device backpack assignment is **v1-critical**:
   **#369** (+ **#363**) move **before** the backpack sync. Order becomes
   `#361 → #363 → #369 → #362 → #364 → #365 → #366 → #367 → #368 → #370`; **#368**
   (service + tray) is v1 scope, not late P5.
