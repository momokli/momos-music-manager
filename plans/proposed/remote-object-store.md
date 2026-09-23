# Plan: remote object store on .200 — submits instead of rsync backups

**Status**: proposed
**Branch**: `feat/remote-object-store`
**Ready for review**: no
**Depends on**: `feat/music-api` (the `.200` service scaffolding: Axum/SQLite, bearer auth, systemd, Caddy)
**Migration needed**: yes (one additive migration, `028_file_content_hash.sql`)

### Description

Give the `.200` host an HTTP **object store** that accepts uploads, so MMM can
push its FLAC/stem files there and *ask whether they are backed up* — replacing
the SSH/rsync `BackupEngine`, which is brittle (silent partial failures, path and
size based "verification") and keeps the Mac as the primary copy. Once the store
holds the library, the Mac only needs a working set.

### Why this is the right shape

The current backup path is `BackupEngine` over SSH: `copy_file` / `pull_file` /
`verify_file(remote_path, expected_size)` plus `run_sync`, driven by
`folders.backup_path`. That is exactly where we keep seeing failures
(`rsync pull failed for backup:/volume1/media/stems/...`, 2 of 2785 groups).
Verification is *path + size*, so it cannot tell a truncated or replaced file from
a good one, and it depends on the NAS's SSH/rsync being available to every caller.

MMM already models the concept properly: `file_locations(file_id, location_type
IN ('local','backup'), path, file_size, last_verified)` — 9916 backup / 4592 local
rows today. What is missing is a *trustworthy* remote location.

**Important finding**: `files.file_hash` is **not** a content hash. It is
`wav-<size>`, or `calculate_file_hash` = `size-mtime` (see
`src/db/schema.rs:12`). Live distribution: 6255 `wav-<size>`, 4257 `size-mtime`,
only 3443 sha-shaped. It cannot key a deduplicating store.

### Design

**Store (on `.200`, on the 7.1 TB `/data/public` volume)**

Content-addressed by **SHA-256**, not by path: dedupe is free, a name change is
not a new object, and "is it backed up?" becomes one hash lookup.

| Method | Path | Use |
|---|---|---|
| `PUT` | `/objects/{sha256}` | upload; streamed to a temp file, hashed while writing, **rejected if the digest does not match the URL**, then atomically renamed. Idempotent (already present → no-op). |
| `HEAD` | `/objects/{sha256}` | exists? (+ size) |
| `GET` | `/objects/{sha256}` | download, `Range` supported |
| `POST` | `/objects/check` | bulk verify: `{hashes:[…]}` → `{present:[…],missing:[…]}` (chunked by the caller) |
| `GET` | `/objects` | paged listing (for browsing/restore) |

On-disk layout `<root>/<aa>/<bb>/<sha256>`; an optional `objects` table keeps
metadata (original path/name, ISRC, size, `created_at`) supplied via headers
(`X-Original-Path`, `X-ISRC`, `X-Name`) so files can be restored *by name* even
though the store is content-addressed.

Auth: the same bearer token as `music-api`, checked in-app and at the Caddy edge.
Uploads are a **write** surface, so: never trust a client-supplied path, cap the
body size, reject anything whose hash does not match, and keep it off the LAN.

**MMM side**

- New `files.content_hash` column (additive, migration 028) + backfill task that
  computes SHA-256 for local files lacking one (throttled, cancelable). New scans
  compute it directly. The broken `file_hash` column is left alone.
- New `[store]` config (`base_url`, `token`, `enabled`) replacing the per-folder
  `backup_path`.
- **Upload task** (new `TaskType::StoreSync`, shaped like `BackpackSync`): for
  each local file with a `content_hash` and no verified store location, `PUT` it;
  on success write `file_locations('backup', 'store:<sha256>', last_verified=now)`.
- **Verify task**: `POST /objects/check` for the local hashes → refresh
  `last_verified`. "Backed up" is `location_type='backup'` **and** the hash is
  present — no more path/size guessing.
- **Restore / pull**: `BackpackSync`'s "pull from backup" and the restore flow use
  `GET /objects/{sha256}` instead of `BackupEngine::pull_file`.
- **UI**: folder `backup_path` config becomes the store credentials; per-file
  "backed up" state comes from `file_locations`; a "Sync to store" action on the
  Files/Storage page with task progress.
- **Retire**: `BackupEngine` (SSH/rsync), `folders.backup_path` / `auto_backup`
  (columns stay, deprecated), `POST /api/storage/backup/{id}`,
  `backup-wavs`, `discover-backup`.

**Phase 3 — the Mac stays crisp**

- `file_stream_handler` (`src/api/files.rs:2242`, already Range-aware): when a file
  has no local copy but a verified store location, **proxy** `GET /objects/{sha256}`
  so playback/DJ tooling keeps working.
- A prune gate that deletes a local copy *only* once its hash is verified in the
  store, gated by the existing backpack/pin rules and **off by default**.
- Net effect: `.200` holds the library; the Mac holds a working set and can be
  freed whenever needed.

### Phases

1. **Store** — endpoints, layout, auth hardening, systemd + Caddy route, tests.
2. **MMM upload + verify** — migration 028, `content_hash` backfill, `StoreSync`,
   verify, config, UI. Rsync stays available as a fallback during the transition.
3. **Restore by hash** — replace the `BackupEngine` pull paths.
4. **Crisp Mac** — stream-proxy + the opt-in prune gate.
5. **Retire** — delete the `BackupEngine` code paths and their endpoints.

### Files to touch (indicative)

| File | Change |
|---|---|
| `music-api/src/store.rs` (+ `api.rs`, `db`) | object store endpoints + sqlite metadata |
| `migrations/028_file_content_hash.sql` | `files.content_hash` + index |
| `src/store.rs`, `src/store_sync.rs` | MMM client + upload/verify tasks |
| `src/config.rs` | `[store]` section |
| `src/db/files.rs`, `src/db/storage.rs` | content hash, location writes |
| `src/tasks/mod.rs` | `TaskType::StoreSync` |
| `src/api/files.rs` | stream-proxy for remote-only files |
| `src/backup/mod.rs` | retire (phase 5) |
| `frontend/pages/{files,storage}.js` | store config + "backed up" + Sync action |
| `tests/*`, `frontend/tests/*` | store contract + gate tests |
| `docs/DECISIONS.md`, `CHANGELOG.md` | ADR + changelog |

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes (store client/task tests included)
- [ ] `cd frontend && npx playwright test` passes
- [ ] An upload lands under the correct SHA-256 path; a mismatching digest is rejected
- [ ] Re-uploading a present object is a no-op; `POST /objects/check` answers
      present/missing for a bulk list
- [ ] MMM marks `file_locations('backup','store:<sha256>')` only for verified objects
- [ ] A file deleted locally is restorable from the store by hash
- [ ] A remote-only file streams through `file_stream_handler` with Range support
- [ ] The rsync/SSH backup path is no longer used by any default flow

### Open questions (need a decision)

1. **Same service or a new one?** Extend `music-api` (one deployment, one token,
   one volume) or run a separate `store` binary? Recommend: extend `music-api`,
   as a `store` module, since it already owns the volume and auth.
2. **How much gets uploaded?** Only the stems/FLAC library, or everything on the
   Mac (including mp3/128 and duplicates)? Affects the initial ~tens of TB-ish
   migration and how long it runs.
3. **Does the Mac delete local copies automatically** once verified, or only via an
   explicit action? Recommend: explicit "free space" action first, automatic later.
4. **Keep the NAS (`backup:` = 192.168.178.129) as a second replica** or drop it?
5. **`dufs` (`:5000`, `-A` = all operations)**: retire it in favour of the store,
   or leave it for manual browsing?
6. **Unify with music-api's download store?** That store is already
   content-ish (per ISRC); making both one content-addressed tree would dedupe
   downloads against the library.
