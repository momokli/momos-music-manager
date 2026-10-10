## Plan: music-api-ingest

**Status**: implemented (Ingest-Core + Part A + Part B + Part C); open questions 2/4/5/6 remain
**Branch**: `feat/music-api-ingest` (core + SSH pull), `feat/music-api-upload` (HTTP upload) — splittable into two PRs
**Ready for review**: no
**Depends on**: `feat/music-api` (store live on `.200`), `feat/mmm-hub` (hub `music_api` client + `/import` page)
**Migration needed**: no numbered migration — `music-api` uses idempotent `db::init` DDL (`CREATE TABLE IF NOT EXISTS` + `ALTER TABLE ADD COLUMN`). The hub needs one additive migration (`mmm-hub/migrations/007_hub_uploads.sql`).

### Description

Make `music-api` the **ISRC metadata + object authority** on `.200`: it ingests
files (stems especially) and serves per-ISRC metadata (BPM, key, title, artist, …)
to anyone who asks.

- **Part A — SSH pull-ingest**: `music-api` pulls a remote tree over SSH/rsync into
  the store (server-side), extracting metadata — especially from **stems**, which
  carry ISRC + key + tags.
- **Part B — HTTP upload**: anyone (hub users) can upload their local FLAC / stem /
  MP3 / WAV / … into the store through a new endpoint.
- **Part C — metadata exchange**: the hub forwards BPM/key it holds (Traktor
  `collection.nml`, MMM data, per-user data) to `music-api`; `music-api` serves
  `GET /metadata/{isrc}` → `{ bpm, key, camelot, title, artist, … }`.

Parts A and B funnel through one shared ingest core (canonicalise → hash → import →
extract tags), which already exists as the one-shot `store-import` CLI.

### Why

- `music-api/src/bin/store_import.rs` already does the hard part: walk a tree,
  canonicalise (clear the Comment tag), SHA-256, `store::import_file`, classify
  stems (`group_key` / `stem_type`). It is a **one-shot CLI that deletes its source**
  and only runs on the host. This plan promotes that logic to a reusable library and
  gives it two service-grade triggers.
- `store_objects` already carries `isrc`, `group_key`, `stem_type` — the store is
  meant to be a metadata-bearing home, not a dumb blob bucket.
- The hub already talks to `music-api` (`mmm-hub/src/music_api.rs`: `order`,
  `status`, `file_bytes`) and has an `/import` page — an upload page is a small,
  natural addition.

### Relationship to ADR-066 (important)

ADR-066 retired SSH/rsync **on the MMM side** ("SSH wird im Standard-Betrieb nicht
mehr angefasst") in favour of the content-addressed store. Part A reintroduces SSH
**on the server side only** — `music-api` reaches out, MMM still never does. This is
a deliberate, narrower reversal and gets its own ADR (see below). The store stays the
single source of truth; SSH is just another _producer_ into it.

### Ground truth (verified 2026-10-10)

Probed live from the music server (`mmm200` = `music-catalog`, `192.168.178.200`):

- **The SSH backup is the Synology NAS `backup`** — reachable from the music
  server as `ssh backup` (plain DNS name, no `~/.ssh/config` needed; key auth works
  non-interactively). armada370 / DS213j, `/volume1` 2.7 TB (1.5 TB used).
- **`rsync 3.1.2` is installed on the NAS** (and on the music server, alongside
  `ffmpeg`/`ffprobe`). A dry-run pull of the stems succeeded:
  `rsync -an … backup:/var/services/homes/momo/Music/Traktor/Stems/ …` → 7 files, 286 MB.
- **The real stem collection is `/volume1/media/stems`** on the NAS — a flat dir with
  **8653 `.stem.m4a`** + 93 `.flac` (**1.3 TB**), plus 3337 _empty_ subdirs and 76
  hidden partial-download temp files (`.NAME.stem.m4a.XXXXXX`). `~/Music/Traktor/Stems`
  (7 files) is only the subset loaded in Traktor — **not** the source.
- Each stem is a **5-stream AAC mp4/m4a** (master + 4 parts), 44.1 kHz stereo, with
  minimal tags (`creation_time`, `handler_name`). The “metadata” that matters is the
  **stem structure** (5 streams) plus duration — not ID3 tags.
- **Gap: `.stem.mp4` is not accepted today.** The main collection is `.stem.m4a`
  (accepted), but the Traktor subset is `.stem.mp4`; `store_import.rs`’s `AUDIO_EXTS`
  lists `m4a` but not `mp4`, and MMM’s `file_type_from_path` maps `m4a` → `stem.m4a`
  and rejects `mp4`. The ingest should accept `.stem.mp4` too (same container).
- **Skip dotfiles** (the 76 `.NAME.stem.m4a.XXXXXX` partials) and empty dirs.
- **Store root on the music server**: `/data/public/media/music-api/objects`
  (content-addressed, 259 shards), next to `flac/320/128/incoming`.

### Shared core — `music-api/src/ingest.rs`

Promote the `store_import` helpers into a library module and reuse them from the CLI,
the SSH puller and the upload handler:

```rust
pub fn canonicalise(src: &Path, dest: &Path) -> Result<String>;   // copy, clear Comment, save, sha256 (WAV: raw)
pub fn classify(rel: &Path) -> (String, Option<String>);          // (group_key, stem_type)
pub fn content_type(ext: &str) -> &'static str;
pub fn sha256_file(path: &Path) -> Result<String>;

/// Accepted audio extensions. **Add `mp4`** — the NAS stems are `.stem.mp4`
/// (today `AUDIO_EXTS` only has `m4a`, so they would be silently skipped).
pub const AUDIO_EXTS: &[&str] = &["wav", "m4a", "mp4", "flac", "mp3", "aif", "aiff", "ogg", "wma", "alac", "dsf"];

/// Tags read *before* canonicalisation clears the Comment.
pub struct ExtractedTags { isrc, title, artist, album, comment, bpm, musical_key, duration_ms }

pub fn extract_tags(path: &Path) -> ExtractedTags;                // lofty; WAV → empty

/// canonicalise → hash → import_file → record tags. Idempotent (content-addressed).
pub async fn ingest_file(pool, store, src, rel, overrides) -> Result<IngestOutcome>;
```

`store_import.rs` is refactored to call this core (no behaviour change).

**Metadata preservation is the point.** Canonicalisation clears the Comment tag, so
tags must be read **first** and persisted. Extend `store_objects` (idempotent DDL in
`store::init`) with nullable `title`, `artist`, `album`, `comment`, `bpm`,
`musical_key`, `duration_ms`. This is what makes "die stems haben ja metadata" real:
the tags survive the store, and a later restore/consumer can read them back.

The ingest also upserts the per-ISRC `track_metadata` row (Part C) from the file
tags, so a stem alone already answers `GET /metadata/{isrc}` with key + catalog
fields.

### Part A — SSH pull-ingest

**Config** (env, consistent with the rest of `music-api`):

```jsonc
// SSH_SOURCES='[{"name":"nas","host":"backup","user":"momo","port":22,
//   "path":"/volume1/media/stems",
//   "include":["*.stem.m4a","*.stem.mp4","*.flac","*.m4a","*.mp3","*.wav"]}]'
// (the music server reaches the NAS as `ssh backup`; key auth, no config needed)
// SSH_SOURCES_FILE=/opt/music-api/ssh-sources.json   (alternative)
// SSH_INGEST_INTERVAL_SECS=0                          (0 = off, default off)
// SSH_SOURCE_DELETE_AFTER_INGEST=false                (default: keep the remote copy)
```

**Mechanism** — shell out to `rsync` over SSH (same tool the retired `BackupEngine`
used; `ffmpeg`/`ffprobe` are already shell-outs, so this is consistent):

1. `rsync -a --no-perms --no-owner --no-group -e "ssh -i KEY -p PORT -o StrictHostKeyChecking=accept-new" USER@HOST:PATH/ <staging>/`
2. Walk `<staging>` with the ingest core (`ingest_file` per audio file).
3. Remove `<staging>`. Optionally `--remove-source-files` on a second pass when
   `SSH_SOURCE_DELETE_AFTER_INGEST=true`.
4. Record the run in `ingest_runs` (below).

Idempotent by construction: an object already in the store is skipped, so re-running
is cheap and safe. `rsync` is a documented host dependency (fallback: `ssh … tar -cf -`
if `rsync` is unavailable — noted, not built in v1).

**Triggers**:

| Trigger    | Surface                                                                         |
| ---------- | ------------------------------------------------------------------------------- |
| CLI        | `music-api ssh-ingest --source nas` / `--all`                                   |
| HTTP       | `POST /ingest/ssh` `{ "source": "nas" }` or `{ "all": true }` → `202 { runId }` |
| Background | optional tick when `SSH_INGEST_INTERVAL_SECS > 0`                               |

**Run state** — new `ingest_runs` table (idempotent DDL in `db::init`):

```sql
CREATE TABLE IF NOT EXISTS ingest_runs (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT NOT NULL,      -- 'ssh' | 'upload'
  source      TEXT,               -- ssh source name; NULL for upload
  status      TEXT NOT NULL,      -- running | done | failed
  files       INTEGER NOT NULL DEFAULT 0,
  stored      INTEGER NOT NULL DEFAULT 0,
  skipped     INTEGER NOT NULL DEFAULT 0,
  failed      INTEGER NOT NULL DEFAULT 0,
  bytes       INTEGER NOT NULL DEFAULT 0,
  error       TEXT,
  started_at  INTEGER NOT NULL,
  finished_at INTEGER
);
```

Read endpoints: `GET /ingest/runs?limit=` and `GET /ingest/runs/{id}`.

### Part B — HTTP upload

**Endpoint**: `POST /objects` (multipart) — the existing `GET /objects` (listing)
gains a `POST` sibling.

- Fields: `file` (required) plus optional `isrc`, `name`, `group_key`, `stem_type`,
  `original_path`.
- Server: stream to a temp file → canonicalise → hash → `ingest_file` → return
  `{ hash, size, stored, isrc, group_key, stem_type }`. `stored:false` = dedup hit.
- Size cap: `STORE_MAX_UPLOAD_BYTES` (existing, default 4 GiB). Requires
  `DefaultBodyLimit::disable()` (or a raised limit) on this route — axum's default
  body limit is 2 MB.
- Auth: the existing bearer token. **Note**: this is one shared secret; per-user
  tokens are a follow-up (see Out of scope).

**Why not just `PUT /objects/{hash}`?** The `PUT` contract requires the client to
canonicalise and hash the bytes first. A browser (or a DJ) cannot. The upload
endpoint does canonicalisation + hashing **server-side**, so the client just sends
the file.

**Metadata read-back**: add `GET /objects/{hash}/meta` returning the `store_objects`
row (incl. the new tag columns), and extend the `GET /objects` listing to include
`group_key` / `stem_type`. This is how MMM/hub "referenzieren die Objekte".

### Part C — metadata exchange (hub → music-api)

The stems carry **ISRC + key** but **no BPM** (verified: ffmpeg tags, `-f ffmetadata`,
no `tmpo` atom).

**Where BPM/key actually originates: Traktor.** Traktor's analysis writes BPM + key
into the **file tags** (and into `collection.nml`); MMM reads them at scan time
(`ItemKey::Bpm` / `ItemKey::InitialKey`, with an exiftool fallback for m4a/mp4) into
`files.bpm` / `files.musical_key`. The stems kept `initialkey` but dropped BPM.

So the hub aggregates from:

| Source                   | Where                                           | Has                                                |
| ------------------------ | ----------------------------------------------- | -------------------------------------------------- |
| Traktor `collection.nml` | per user                                        | `TEMPO` (BPM) + `MUSICAL_KEY` — **not read today** |
| MMM library              | `library.db` (`files.bpm`, `files.musical_key`) | BPM + key (from Traktor tags)                      |
| Hub's own analysis       | `hub_track_analysis` (`source='essentia'`)      | BPM + key/camelot                                  |
| ReccoBeats               | `hub_track_features`                            | BPM + key/camelot                                  |

**music-api side** — a new `track_metadata` table keyed by ISRC, fed by both the
ingest (Part A/B) and the hub push:

```sql
CREATE TABLE IF NOT EXISTS track_metadata (
  isrc        TEXT PRIMARY KEY,
  bpm         REAL,
  musical_key TEXT,
  camelot     TEXT,
  title       TEXT,
  artist      TEXT,
  album       TEXT,
  genre       TEXT,
  source      TEXT,          -- 'stem-tags' | 'traktor' | 'mmm' | 'hub-analysis'
  updated_at  INTEGER NOT NULL
);
```

- `POST /metadata` — bulk upsert `{ items: [{ isrc, bpm?, key?, camelot?, title?, … , source }] }`.
  Merge rule: a non-null field wins over null; `source` records the last writer.
- `GET /metadata/{isrc}` — serve the merged record (404 if unknown).
- The stem ingest (Part A/B) also upserts `track_metadata` from the file tags
  (ISRC + `initialkey` + title/artist/album/genre), so a stem alone already answers
  key + catalog fields.

**hub side** — a push job (`mmm-hub/src/music_api.rs::push_metadata`):

1. Extend the Traktor import (`mmm-hub/src/traktor.rs`) to read `TEMPO/@BPM` and
   `MUSICAL_KEY/@VALUE` into `hub_track_analysis` (`source='traktor'`).
2. Gather `(isrc, bpm, key, camelot)` from `hub_track_analysis` ∪
   `hub_track_features` (and MMM data where imported), joined to `hub_tracks.isrc`.
3. `POST /metadata` in batches. Triggered by a CLI (`mmm-hub push-metadata`), the
   Traktor import, and/or a periodic tick.

### UI — hub `/upload`

- New page `mmm-hub/templates/upload.html` + route `GET/POST /upload` in
  `mmm-hub/src/pages.rs`; nav entry in the hub nav.
- Browser posts multipart to the hub; the hub forwards to `music-api` via a new
  `music_api::upload_object(cfg, filename, bytes, meta)` (reqwest `multipart`).
- Record each upload in a new hub table `hub_uploads(user_id, hash, name, isrc,
group_key, stem_type, uploaded_at)` (migration `007_hub_uploads.sql`) so "who
  uploaded what" is tracked and objects are referenceable per user.
- Show per-file result: new object vs. dedup hit (`stored:false`), hash, size.
- Requires enabling axum `multipart` (hub) and reqwest `multipart` (hub).

MMM-side upload is **out of scope for v1** (single-user); MMM can already _stream_
and _restore_ store objects (`stream_from_store`, `store::restore_object`), so it can
reference ingested objects without changes.

### Tests

`music-api/tests/api.rs` (extend):

| #   | Test                              | Proves                                                                     |
| --- | --------------------------------- | -------------------------------------------------------------------------- |
| 1   | `upload_object_stores_and_hashes` | multipart upload → object present, hash = canonical sha256                 |
| 2   | `upload_object_dedup_is_noop`     | second upload of the same audio → `stored:false`, one object               |
| 3   | `upload_extracts_tags`            | ISRC/title/comment read into `store_objects` before the Comment is cleared |
| 4   | `upload_rejects_non_audio`        | a `.txt` → `400`                                                           |
| 5   | `upload_enforces_size_cap`        | body over `STORE_MAX_UPLOAD_BYTES` → `413`                                 |
| 6   | `object_meta_returns_tags`        | `GET /objects/{hash}/meta` returns the extracted tags                      |
| 7   | `ssh_ingest_run_recorded`         | `POST /ingest/ssh` (fake source dir) → run row `done`, counts correct      |
| 8   | `ssh_ingest_idempotent`           | re-run → `stored:0`, `skipped:N`                                           |
| 9   | `ssh_ingest_classifies_stems`     | `_vocals`/`_bass` files → `group_key` + `stem_type` set                    |
| 10  | `metadata_upsert_and_get`         | `POST /metadata` then `GET /metadata/{isrc}` reflects it                   |
| 11  | `metadata_merge_keeps_non_null`   | a later push with a null field keeps the earlier value                     |
| 12  | `stem_ingest_feeds_metadata`      | an ingested stem answers `GET /metadata/{isrc}` with key + title           |

Unit tests in `ingest.rs`: `canonicalise` determinism (re-save ⇒ same hash;
comment-only difference ⇒ same hash), `classify`, `extract_tags`.

`mmm-hub/tests/` (extend): upload page renders; a multipart POST records a
`hub_uploads` row and forwards to a mocked `music-api`; the Traktor import reads
`TEMPO`/`MUSICAL_KEY`; `push_metadata` posts the gathered rows.

### Files to modify

| File                                     | Change                                                                                                                                       |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `music-api/src/ingest.rs`                | **new** — shared core (canonicalise, classify, extract_tags, ingest_file)                                                                    |
| `music-api/src/bin/store_import.rs`      | refactor to call the core                                                                                                                    |
| `music-api/src/store.rs`                 | extend `store_objects` DDL + tag columns; `POST /objects` upload; `GET /objects/{hash}/meta`; listing gains `group_key`/`stem_type`          |
| `music-api/src/ssh_ingest.rs`            | **new** — rsync-over-SSH puller + run recording                                                                                              |
| `music-api/src/config.rs`                | `SSH_SOURCES`, `SSH_SOURCES_FILE`, `SSH_INGEST_INTERVAL_SECS`, `SSH_SOURCE_DELETE_AFTER_INGEST`                                              |
| `music-api/src/db.rs`                    | `ingest_runs` + `track_metadata` DDL + helpers                                                                                               |
| `music-api/src/metadata.rs`              | **new** — `POST /metadata`, `GET /metadata/{isrc}`, merge                                                                                    |
| `music-api/src/api.rs`                   | routes: `POST /objects`, `GET /objects/{hash}/meta`, `POST /ingest/ssh`, `GET /ingest/runs[/{id}]`, `POST /metadata`, `GET /metadata/{isrc}` |
| `music-api/src/main.rs`                  | `ssh-ingest` CLI subcommand; optional background tick                                                                                        |
| `music-api/Cargo.toml`                   | axum `multipart` feature                                                                                                                     |
| `music-api/README.md`                    | document the ingest paths + SSH config                                                                                                       |
| `mmm-hub/src/music_api.rs`               | `upload_object()`, `push_metadata()`                                                                                                         |
| `mmm-hub/src/traktor.rs`                 | read `TEMPO`/`MUSICAL_KEY` into `hub_track_analysis`                                                                                         |
| `mmm-hub/src/main.rs`                    | `push-metadata` CLI                                                                                                                          |
| `mmm-hub/src/pages.rs`                   | `GET/POST /upload`                                                                                                                           |
| `mmm-hub/templates/upload.html`          | **new** upload page                                                                                                                          |
| `mmm-hub/migrations/007_hub_uploads.sql` | **new** `hub_uploads` table                                                                                                                  |
| `mmm-hub/Cargo.toml`                     | axum + reqwest `multipart` features                                                                                                          |
| `docs/DECISIONS.md`                      | ADR: server-side SSH ingest + store upload (narrow ADR-066 reversal)                                                                         |
| `CHANGELOG.md`                           | Added entries                                                                                                                                |

### Acceptance Criteria

- [ ] `store_import` behaviour unchanged after the refactor (existing tests pass)
- [ ] `POST /objects` accepts a multipart audio file, canonicalises + hashes it
      server-side, and returns the hash; a duplicate returns `stored:false`
- [ ] Tags (ISRC/title/comment/…) are read **before** canonicalisation and persisted
      in `store_objects`; `GET /objects/{hash}/meta` returns them
- [ ] Non-audio and over-limit uploads are rejected (`400` / `413`)
- [ ] `POST /ingest/ssh` pulls a source via rsync-over-SSH into the store, records an
      `ingest_runs` row, and is idempotent on re-run
- [ ] `.stem.mp4` is accepted (added to `AUDIO_EXTS` + the content-type map); the
      NAS stems ingest instead of being silently skipped
- [ ] Stem files are classified (`group_key` / `stem_type`) on both paths
- [ ] `GET /metadata/{isrc}` returns key + catalog fields from an ingested stem
- [ ] `POST /metadata` upserts BPM/key from the hub; a later `GET` reflects it
- [ ] The Traktor import reads `TEMPO`/`MUSICAL_KEY` into `hub_track_analysis`
- [ ] `mmm-hub push-metadata` forwards the hub's BPM/key to `music-api`
- [ ] `music-api ssh-ingest --source <name>` works from the CLI
- [ ] Hub `/upload` page uploads a file, records a `hub_uploads` row, and shows
      new-vs-dedup
- [ ] `cargo build` + `cargo test` pass in `music-api/` and `mmm-hub/`
- [ ] ADR added to `docs/DECISIONS.md` documenting the server-side SSH reversal
- [ ] `music-api/README.md` documents the SSH source config and the upload endpoint

### Progress (2026-10-10)

**Done — Ingest-Core, Part A, Part C** (branch `feat/music-api-ingest`):

- `music-api/src/ingest.rs` — shared core (canonicalise, classify, extract_tags,
  ingest_file, glob); `AUDIO_EXTS` includes `mp4`. `store-import` refactored onto it.
- `store_objects` gained tag columns; `GET /objects/{hash}/meta`; listing includes
  `group_key`/`stem_type`.
- `ingest_runs` + `track_metadata` tables; `POST /ingest/ssh`, `GET /ingest/runs[/{id}]`,
  `POST /metadata`, `GET /metadata/{isrc}`; `music-api ssh-ingest` CLI + background tick.
- `SSH_SOURCES` / `SSH_SOURCES_FILE` / `SSH_INGEST_INTERVAL_SECS` config.
- Hub: Traktor import reads `TEMPO/@BPM` + `INFO/@KEY` into `hub_track_analysis`
  (`source='traktor'`); `music_api::push_metadata`; `mmm-hub push-metadata` CLI.
- Tests: `music-api` 34 (22 unit + 12 integration), `mmm-hub` traktor 3 — all green.

**Done — Part B**:

- `POST /objects` (multipart) — the server canonicalises + hashes (a browser
  cannot), so the same audio dedupes. Optional fields: `isrc`, `name`, `groupKey`,
  `stemType`, `originalPath`. `DefaultBodyLimit` is disabled on the route (the
  handler enforces `STORE_MAX_UPLOAD_BYTES`).
- Hub: migration `034_hub_uploads.sql` (`hub_uploads`), `music_api::upload_object`,
  `/upload` page + nav entry, `hub_uploads` history.
- Tests: `music-api` 37 (22 unit + 15 integration), `mmm-hub` ui 47 (+2 upload) —
  all green.

### Out of scope (v1)

- **Per-user upload auth** — v1 uses the single `MUSIC_API_TOKEN`. Per-user tokens /
  hub-proxied auth is a follow-up.
- **Push-mirror** (music-api → remote) — the user chose pull-ingest only.
- **MMM-side upload UI** — hub is the multi-user surface; MMM can already stream/restore.
- **Reconciling ingested objects back into MMM's `files` table** — the store listing +
  `GET /objects/{hash}/meta` make it _possible_; wiring it into MMM's scan is a
  separate plan.
- **Range/partial uploads**, resumable uploads, and a web UI for browsing the store.

### Open questions

1. ~~Which host(s) is the SSH backup?~~ **Answered**: the Synology NAS `backup`
   (see Ground truth). More sources can be added later via `SSH_SOURCES`.
2. **Delete the remote source after ingest?** Default is _keep_ (safe); confirm.
3. ~~Is `rsync` installed?~~ **Answered**: yes — `rsync 3.1.2` on the NAS, `rsync`
   on the music server. The `ssh … tar` fallback is not needed.
4. **Should the hub upload be gated** (e.g. only certain users/roles), or open to all
   logged-in hub users?
5. **Stem extension**: accept `.stem.mp4` as-is, or normalise to `.stem.m4a` on
   ingest (MMM’s canonical stem type)? Recommend accept-as-is + record the original
   name; normalisation is a separate concern.
6. **Metadata merge precedence**: when a stem tag and a hub push disagree on `key`
   (or title), who wins? Proposed: last non-null writer wins, with `source` recorded
   so a later policy can prefer e.g. Traktor over stem tags. Confirm.

---
