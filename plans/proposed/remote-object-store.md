# Plan: remote object store on .200 — submits instead of rsync backups

**Status**: in-progress (phase 1 done)
**Branch**: `feat/remote-object-store`
**Ready for review**: yes
**Depends on**: `feat/music-api` (the `.200` service: Axum/SQLite, bearer auth, systemd, Caddy)
**Migration needed**: yes (one additive migration, `028_file_content_hash.sql`)

### Description

Give the `.200` host an HTTP **object store** that accepts uploads, so MMM pushes
its FLAC/stem/mp3 files there and can _ask whether they are backed up_ — replacing
the SSH/rsync `BackupEngine`, which is brittle (silent partial failures; verification
is only path + size). Once the store is trustworthy, the existing prune makes the
Mac a working-set machine instead of the primary copy.

### Decisions (answered)

1. **Same service.** A `store` module inside `music-api` — one deployment, one
   token, one volume.
2. **Upload everything** — but files of the same audio often differ only in the
   MMM **comment** tag, so upload is **canonicalised** first and deduped (below).
3. **Deletion: follow the code.** Auto-deletion already exists:
   `maintainer.auto_prune` → `get_prune_candidates` (files with **both** a `local`
   and a `backup` location, minus Backpack-protected ones) → `start_prune_files_task`.
   The source even says _"No other gates — the user trusts backup."_ No new
   mechanism: the store only has to make the `backup` location **trustworthy**.
4. **Drop the NAS.** Leave the machine alone, but remove `backup:` from code and
   config and stop using it.
5. **Retire `dufs`** (`:5000`, `-A` = all operations) in favour of the store.
6. **Unify with the download store** — one content-addressed tree, so downloads
   dedupe against the library.

### Why this is the right shape

- `file_locations(file_id, location_type IN ('local','backup'), path, file_size,
last_verified)` already models it — 9916 `backup` / 4592 `local` rows today.
  The missing piece is a _trustworthy_ remote location.
- **`files.file_hash` is not usable.** `calculate_file_hash` = `size-mtime`
  (`src/db/schema.rs:12`); live: 6255 `wav-<size>`, 4257 `size-mtime`, only 3443
  sha-shaped. Hence a new `files.content_hash`.

### Canonicalisation + dedup (decision 2)

The comment tag (`ItemKey::Comment` → Vorbis `COMMENT` / ID3 `COMM`, written by
`write_comment_to_file`, `src/db/files.rs:1086`) is MMM-generated and differs
between copies of the same audio.

- **The object is the canonical file**: open with `lofty`, clear the comment tag,
  re-save. Its SHA-256 is `files.content_hash` and the store key.
- Two local files that differ _only_ by comment therefore collapse to one object.
- The store stays **format-agnostic**: it never parses audio. It verifies the raw
  SHA-256 of the bytes it received against the key in the URL, so `PUT` cannot be
  spoofed.
- **Restore writes the comment back** from the DB — the comment is regenerable
  (`comment::generate_comment`), which is why dropping it is safe.
- **Must be verified in Phase 1**: re-saving an unchanged file must be byte-stable
  (same canonical hash), and two files differing only by comment must hash equal.
  If `lofty` proves non-deterministic, fall back to hashing the _audio payload_
  (skip FLAC metadata blocks / leading ID3v2 / trailing ID3v1), storing original
  bytes.

### Store API

| Method | Path                | Use                                                                                                                |
| ------ | ------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `PUT`  | `/objects/{sha256}` | upload; streamed, hashed while writing, **rejected if the digest ≠ the key**, then atomically renamed. Idempotent. |
| `HEAD` | `/objects/{sha256}` | exists? (+ size)                                                                                                   |
| `GET`  | `/objects/{sha256}` | download, `Range` supported                                                                                        |
| `POST` | `/objects/check`    | bulk verify `{hashes:[…]}` → `{present:[…],missing:[…]}`                                                           |
| `GET`  | `/objects`          | paged listing (browsing/restore)                                                                                   |

Layout `<root>/<aa>/<bb>/<sha256>`; metadata (original path/name, ISRC, size) via
headers (`X-Original-Path`, `X-ISRC`, `X-Name`) into an `objects` table so files
can be restored _by name_. Auth: the `music-api` token, checked in-app and at
Caddy; uploads are a write surface → cap body size, never trust a client path,
keep it off the LAN.

### MMM side

- Migration 028: `files.content_hash` + index; backfill task (throttled,
  cancelable); new scans compute it.
- `[store]` config (`base_url`, `token`, `enabled`) replaces per-folder
  `backup_path`.
- `TaskType::StoreSync`: canonicalise → hash → `HEAD`, `PUT` if missing →
  `file_locations('backup', 'store:<sha256>', last_verified=now)`.
- **Verify task**: `POST /objects/check` for local hashes → refresh
  `last_verified`. "Backed up" is now a hash fact, so `get_prune_candidates` and
  `auto_prune` become trustworthy with no change to their logic.
- **Restore** (`BackpackSync` pull, restore flow): `GET /objects/{sha256}`, then
  rewrite the comment from the DB.
- **Stream-proxy** (`file_stream_handler`, `src/api/files.rs:2242`, already
  Range-aware): a remote-only file is proxied from the store, so playback and DJ
  tooling keep working.
- **UI**: store credentials instead of the folder paths; per-file "backed up";
  a Sync action with task progress.
- **Retire**: `BackupEngine` (SSH/rsync), `folders.backup_path`/`auto_backup`
  (columns stay, deprecated), `/api/storage/backup/{id}`, `backup-wavs`,
  `discover-backup`; remove `backup:` from config/env; drop `dufs`.

### Phases

1. **Store** — endpoints, layout, canonical-hash determinism check, auth
   hardening, systemd/Caddy, tests. Includes unifying `music-api`'s existing
   download store onto the same content-addressed tree.
2. **MMM upload + verify** — migration 028, backfill, `StoreSync`, verify, config,
   UI. Rsync remains available as a fallback during the transition.
3. **Restore by hash** — replace the `BackupEngine` pull paths.
4. **Crisp Mac** — stream-proxy; `auto_prune` now works on a real guarantee
   (verify with a couple of files first before letting it loose).
5. **Retire** — delete the `BackupEngine` paths, the NAS config, `dufs`.

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes (store client/task tests included)
- [ ] `cd frontend && npx playwright test` passes
- [x] Canonicalisation is deterministic: re-save ⇒ same hash; two files differing
      only by comment ⇒ same hash  **(probed on FLAC/MP3/stem M4A — all YES)**
- [x] A mismatching digest is rejected; re-upload is a no-op; `/objects/check`
      answers present/missing for a bulk list  **(verified live)**
- [ ] MMM writes `file_locations('backup','store:<sha256>')` only for verified objects
- [ ] A locally deleted file is restorable from the store **with its comment restored**
- [ ] A remote-only file streams through `file_stream_handler` with Range support
- [ ] No default flow uses rsync/SSH; `backup:` is gone from config

### Progress

**Phase 1 — done.** The `store` module is live on `.200`:

- `PUT`/`HEAD`/`GET`/`check`/`list` under `/objects`, keyed by the SHA-256 of the
  bytes sent; the store verifies the digest of what it receives and rejects a
  mismatch, so a `PUT` cannot claim a wrong key. Sharded `<aa>/<bb>/<hash>` layout
  on the 7.1 TB volume, single-range `GET`, sqlite metadata, 4 GiB cap, bearer auth.
- Verified live: a 35 MB FLAC uploads (`201`), `HEAD` reports the length, `GET`
  round-trips **byte-identical**, `check` answers present/missing, a wrong key is
  `400`, `Range` returns the file's leading bytes, an unauthenticated `PUT` is `401`.
- **The canonicalisation question is settled**: `examples/canon_probe.rs` shows
  clearing the Comment tag with `lofty` is byte-stable across two saves **and**
  comment-independent on FLAC, MP3 and stem M4A. The raw-payload-hash fallback in
  the design is therefore *not* needed.

### Files to touch (indicative)

| File                                           | Change                                                              |
| ---------------------------------------------- | ------------------------------------------------------------------- |
| `music-api/src/store.rs` (+ `api.rs`, `db.rs`) | object store endpoints, canonical-agnostic hashing, sqlite metadata |
| `migrations/028_file_content_hash.sql`         | `files.content_hash` + index                                        |
| `src/store.rs`, `src/store_sync.rs`            | client, canonicalisation, upload/verify tasks                       |
| `src/config.rs`                                | `[store]`; remove `backup:`                                         |
| `src/db/files.rs`, `src/db/storage.rs`         | content hash, location writes                                       |
| `src/tasks/mod.rs`                             | `TaskType::StoreSync`                                               |
| `src/api/files.rs`                             | stream-proxy for remote-only files                                  |
| `src/backup/mod.rs`                            | retire (phase 5)                                                    |
| `frontend/pages/{files,storage,folders}.js`    | store config, "backed up", Sync action                              |
| `tests/*`, `frontend/tests/*`                  | store contract + gate tests                                         |
| `docs/DECISIONS.md`, `CHANGELOG.md`            | ADR + changelog                                                     |
