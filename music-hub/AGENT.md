# music-hub — Agent Handover

> **Last updated**: 2026-10-14 · branch `feat/hub-music-status` (→ `feat/music-hub`)
> Keep this file current. It is the entry point for any agent working on `music-hub/`.
>
> **New in this batch (engine + unified UX):** migrations `028`–`035`
> (music-state format/error, track tagging, import history, tag archive, album/artist
> indexes), new module `src/table.rs` (server-side sortable headers + tokenised
> fuzzy/typo search, rolled out to **every** list table), typed settings registry
> (`settings::SETTINGS` — every engine knob configurable at `/admin`), similarity v2
>
> - ripeness wired into digging/overlap, and the tag recommendation engine with
>   explanations. ADRs **[074](../docs/DECISIONS.md)/075/076**. See
>   `plans/music-hub/tagging-spelunking.md`.
>
> **New in this batch (crawl-everything, hub side):** migrations `048`
> (`hub_track_audio` — flat read-model of the vault's resolved best BPM/key) and
> `049` (`hub_crawl_state` + `hub_order_state`). New modules `src/analysis.rs`
> (reads the resolved BPM/key from the music-api vault into `hub_track_audio`)
> and `src/crawl.rs` (download-priority orders + bounded Spotify touch-cascade).
> The cascade crawls **everything**: artist albums across all release groups
> (`album,single,compilation,appears_on`) **and** `/artists/{id}/top-tracks`
> (frontier kinds `artist` / `artist_top`), then each album's tracks. Every
> freshly ingested track is **immediately touched** (`crawl::touch_tracks`:
> order at music-api + seed its artists into the frontier) from `ingest.rs`
> (`store_likes` / `store_playlist_items`), the album/top-track crawl, and
> now-playing. The hub is **only a consumer**: it never analyses and owns no
> vault — music-api is the Data Vault of music. See
> `plans/proposed/hub-crawl-everything.md` and
> `plans/proposed/music-api-data-vault.md`.

---

## 1. What this is

`music-hub` is the **multi-user little brother** of Momo's Music Manager: a separate
cargo crate in `momokli/momos-music-manager` (`music-hub/`, **not** a workspace member).
Each user gets a local account, links their **own Spotify**, and the hub pulls their
likes + playlists into **one shared SQLite DB**. On top of that sit a **tag layer**,
**groups/collectives**, and a **digging/ranking engine**.

Stack: **axum 0.8 + askama 0.14 + sqlx 0.8 (SQLite) + Pico.css + htmx (both CDN)**.
No build step for the frontend. Live at **https://hub.zukkafabrik.de**.

Users (all admins): `momo`, `Mctoastus`, `ANKD`.

---

## 2. Access & hosts

| Thing             | Value                                                                                |
| ----------------- | ------------------------------------------------------------------------------------ |
| Deploy host       | `ssh music-catalog` (alias in `~/.ssh/config`) = **192.168.178.200**                 |
| LAN jump host     | `ssh lan` (Caddy + public reachability live here)                                    |
| Public URL        | **https://hub.zukkafabrik.de** (behind Caddy on `lan`)                               |
| App dir on server | `/home/momo/music-hub`                                                               |
| Live DB           | `/home/momo/music-hub/hub.db` (+ `-wal`/`-shm`)                                      |
| Sanitized copy    | `hub-public.db` → Datasette `data.zukkafabrik.de`, SchemaSpy `schema.zukkafabrik.de` |
| systemd unit      | `music-hub` (`sudo systemctl restart music-hub`)                                     |
| Audio analyzer    | Essentia service on `.200:8711` (`deploy/analyzer/`)                                 |
| Git remote        | `github` → `github.com:momokli/momos-music-manager.git`                              |

> **⚠️ Server rename (one-time, `mmm-hub` → `music-hub`).** The repo dir is now
> `music-hub/`, but the live server still runs from `/home/momo/mmm-hub`
> (systemd unit `mmm-hub`, DB `/home/momo/mmm-hub/hub.db`). Do this **once** on the
> host before the deploy loop below works as written:
>
> 1. Sync the new tree into the **old** dir (so script + unit are present):
>    ```bash
>    tar czf - -C music-hub --exclude target --exclude 'hub.db*' --exclude .env . \
>      | ssh music-catalog 'tar xzf - -C ~/mmm-hub && find ~/mmm-hub -name "._*" -delete'
>    ```
>    (The `find … -name "._*" -delete` strips macOS AppleDouble junk — without it,
>    `sqlx::migrate!` fails at build: _error parsing migration filename ".*NNN*_.sql"\*.)
> 2. Run the rename (stops the unit, moves the dir, fixes `.env` paths, installs
>    `music-hub.service`, verifies):
>    ```bash
>    ssh music-catalog '~/mmm-hub/deploy/rename-music-hub.sh'
>    ```
>
> Afterwards the deploy loop below (`→ ~/music-hub`, unit `music-hub`) is literal.
> Also check Caddy (path-independent) and any Datasette/backup cron that hardcodes
> `/home/momo/mmm-hub`.
>
> **⚠️ Never edit an applied hub migration** (`music-hub/migrations/*`) — SQLx
> checksums the whole file, comments included, and the service then refuses to
> boot with _migration N was previously applied but has been modified_. The
> 2026-10-12 rename changed a comment in `001_hub_schema.sql` via a global
> search/replace and broke prod — keep renames away from `music-hub/migrations/`.

Login for smoke tests: `Mctoastus` / `1234` (also `momo`, `ANKD`; passwords may differ).

### Deploy loop (copy-paste, works)

```bash
# run from repo root
# The tar carries no .git, so pass the git build-info in — the build embeds it.
tar czf - -C music-hub --exclude target --exclude 'hub.db*' --exclude .env \
  --exclude 'datasette-*' --exclude 'make-public*' --exclude docs --exclude 'models' --exclude 'tmp' . \
  | ssh music-catalog 'tar xzf - -C ~/music-hub 2>/dev/null && find ~/music-hub -name "._*" -delete' \
  && ssh music-catalog "cd ~/music-hub && GIT_SHA=$(git rev-parse --short=8 HEAD) GIT_DESCRIBE='$(git describe --tags --always --dirty)' ~/.cargo/bin/cargo build --release --locked 2>&1 | tail -1 && sudo systemctl restart music-hub && sleep 2 && systemctl is-active music-hub"
```

- **Never ship the DB or `.env`** (excluded above). The tar excludes `docs/`, `models/`,
  `tmp/` too.
- `find … -name "._*" -delete` strips macOS AppleDouble junk.
- **Build info / traceability**: every build embeds a git-derived version
  (`github.com/.../build.rs`): `git rev-parse --short=8 HEAD` + `git describe
--tags --always --dirty` (passed as `GIT_SHA` / `GIT_DESCRIBE` here, since the
  tar has no `.git`). Visible at `/api/hub/health`, in the UI footer, in the CLI
  `music-hub --version`, and in the startup log. See `docs/stack-versioning.md`.
- Migrations run automatically on boot (sqlx migrate). Additive only — **never edit
  `001_initial_schema.sql`**.
- Live smoke (cookie auth):
  ```bash
  cd /tmp && rm -f hc.txt
  curl -s -c hc.txt -o /dev/null -d 'username=Mctoastus&password=1234' https://hub.zukkafabrik.de/login
  curl -s -b hc.txt https://hub.zukkafabrik.de/overlap | head
  ```

---

## 3. Config / secrets

- Server `.env` at `/home/momo/music-hub/.env` (+ DB overrides editable at **`/admin`**).
- Keys seen in use: `SPOTIFY_CLIENT_ID/SECRET/REDIRECT_URI`, `MUSIC_API_TOKEN`,
  `COSINECLUB_API`, `FREQBlog_API`, `EFFNET_MODEL`, `EFFNET_LABELS`,
  `HUB_ANALYZER_URL`, `HUB_ANALYZE_TMP`, `HUB_DATABASE_URL`, `HUB_HOST`, `HUB_PORT`.
- **Config priority**: env > `.env` > settings in DB (`hub_settings`, edited on `/admin`) > defaults.
- **Admin-only settings page** `/admin` (`settings::SETTINGS` typed registry) stores API keys + the
  whole **ranking engine** config (see §7).
- ⚠️ **Rotate secrets**: `DEEMIX_ARL` and `MUSIC_API_TOKEN` were once printed into a chat — the
  user should rotate them.

---

## 4. Source layout

```
music-hub/
├── migrations/            001…035 (additive; consolidate per release)
├── templates/             askama: base.html + one per page + _partials/
├── src/
│   ├── main.rs            CLI (serve/auth/ingest/fetch-playlists/backfill/set-password/
│   │                      query/users/seed-demo/import-mmm-tags/analyze)
│   ├── lib.rs             crate wiring
│   ├── api.rs             JSON API (/api/hub/…) + AppState
│   ├── web.rs             auth, sessions, dashboard, /me/playlists, /track/{id}, /
│   ├── pages.rs           ALL other HTML pages: /search /overlap /tags /groups
│   │                      /collectives /digging /sql /admin /settings /user /playlist
│   ├── ui.rs              shared nav (crate::ui::nav) + shell bits
│   ├── db/                connect + migrate; db/testing.rs = Playwright/test seeds
│   ├── spotify.rs         OAuth client + sync
│   ├── ingest.rs          likes + owned/collaborative playlist items
│   ├── worker.rs          background loops (Spotify sync + enrichment)
│   ├── features.rs        ReccoBeats audio features + on-demand request queue
│   ├── freqblog.rs        paid fallback (quota-guarded, ISRC lookup)
│   ├── genres.rs lastfm.rs cosine.rs   other enrichment / digging sources
│   ├── similar.rs         brute-force cosine over EffNet embeddings
│   ├── audio.rs analyzer.rs analyze.rs  local BPM/key + embeddings
│   ├── digging.rs         digging sources (internal_suggestions, presence, Matcher)
│   ├── tags.rs            the tag/group/collective layer (big)
│   ├── traktor.rs         Traktor collection.nml ingest (playcount/rating/playlists)
│   ├── scoring.rs         ripeness score (tag points per group, meta, traktor signal)
│   ├── similarity.rs      similarity v2 (tag overlap cross-group, rumpelkiste)
│   ├── table.rs           shared table UX: sortable headers + fuzzy/typo search
│   ├── tagqueue.rs        /tag-queue (ripeness-sorted) + direct tagging + player
│   ├── albums.rs          /artists + /artist/{name} explorer
│   ├── analysis.rs        reads the vault's resolved BPM/key into hub_track_audio
│   ├── crawl.rs           download-priority orders + bounded touch-cascade (albums + top-tracks)
│   ├── history.rs         import-run ledger + change events (all sources)
│   ├── settings.rs        hub_settings + SETTINGS registry + Engine config
│   └── mmm_import.rs      import tags/categories/parents/energy from MMM library.db
└── tests/                 cargo integration tests (harness = tests/common/mod.rs)
```

Templates: `base.html` shell holds the dense CSS + nav. Pages: `dashboard, playlists,
track, search, overlap, similar, tags, tag, groups, group, collectives, collective,
digging, sql, admin, settings, user, playlist, login, signup, auth, error`.

---

## 5. Data model (live schema is truth: `sqlite3 hub.db .schema`)

Migrations **001–024** (see `migrations/`). Highlights:

- Core: `hub_users`, `hub_service_accounts`, `hub_web_sessions`, `hub_tracks`,
  `hub_track_external_ids`, `hub_playlists`, `hub_playlist_tracks`, `hub_liked_tracks`.
- Features/audio: `hub_track_features` (ReccoBeats: bpm/camelot/energy/…),
  `hub_track_analysis` (local), `hub_track_embeddings` (EffNet), `hub_track_genres`,
  `hub_feature_requests` (on-demand queue). Unified view **`v_track_audio`** (bpm/camelot).
- Tag layer: `hub_tags(owner_user_id, slug, name)` + `hub_tag_sources` (playlist feeders) +
  materialized `hub_track_resolved_tags` (rebuild via `tags::rebuild`), view `hub_v_track_tags`.
  `hub_tag_parents` = MMM parent/alias hierarchy.
- Groups/collectives:
  - `hub_tag_groups(owner_user_id, name, icon, collective_id, weight, ranked)`.
  - `hub_group_members(group_id, user_id, role owner|contributor|subscriber)`.
  - `hub_group_tags(tag_id, group_id, rank)` — **rank 0..5 lives here** (per group membership).
  - `hub_collectives(id, slug, name, icon, owner_user_id)` + `hub_collective_members`.
- Other: `hub_settings`, `hub_task_history`, `hub_api_usage` (FreqBlog quota), `hub_deemix_downloads`.
- Crawl-everything (048/049): `hub_track_audio(track_id, bpm, musical_key, camelot, energy, source, updated_at)` — flat read-model of the **vault's resolved best value** (music-api owns the vault; the hub only consumes `GET /vault/tracks?isrcs=`); `hub_crawl_state(kind, ref, state, …)` — TTL'd touch-cascade frontier (`kind` ∈ `artist` | `artist_top` | `album`); `hub_order_state(track_id, priority, ordered_at)` — idempotency ledger for music-api download-priority orders.
- Metadata backfill (#354, migration **050**): `hub_track_features.metadata_checked_at TEXT` — TTL marker (7 days) for the api→hub BPM/key backfill. `music_api::metadata_range` reads
  `GET /metadata?isrcs=` and `features::backfill_from_music_api` UPSERTs hits into
  `hub_track_features` (`source='music-api'`, `COALESCE` on bpm/camelot/key_pitch/key_mode,
  bounded + chunked, `continue` on a transient error — never a "no data" write). CLI
  `metadata-backfill --limit 2000`; console button „BPM/Key holen“. **music-api is the ISRC
  authority** for BPM/key: push hub analysis with `push-metadata`, pull the resolved range
  back with the backfill. `musicalKey` is parsed to Camelot by `music_api::parse_musical_key`
  (Camelot / Traktor `8m`/`8d` / note names `Am`, `F#m`), inverted by
  `features::camelot_to_pitch_mode` (§Z mapping).

Views of interest: `hub_v_track_presence`, `hub_v_track_playlists`, `hub_v_shared_tracks`,
`hub_v_user_overlap`, `v_track_audio`, `hub_v_track_tags`.

---

## 6. Routes

Web (session-gated): `/`, `/login`, `/signup`, `/logout`, `/me/playlists`, `/search`,
`/overlap` (+ `POST /overlap/enrich`), `/playlists/similar`, `/tags`, `/tag/{id}`
(+ `rename`, `group/add`, `group/remove`), `/groups`, `/groups/create`, `/groups/{id}`
(+ `update`, `weight`, `ranked`, `tag-rank`, `subscribe`, `unsubscribe`, `role`,
`member/remove`, `collective`, `delete`), `/collectives`, `/collectives/create`,
`/collectives/{id}` (+ `update`, `join`, `leave`, `member`, `member/remove`, `delete`),
`/digging` (+ `POST /digging/enrich`), `/admin`, `/settings`, `/user/{slug}`,
`/playlist/{id}` (+ `tag`, `tag/add`, `tag/remove`, `POST order`, `POST refresh`,
`GET progress`, `GET download`), `/tag/{id}` (+ `rename`, `group/*`, `POST order`,
`GET progress`, `GET download`), `/track/{id}/download`, `/sql`.

JSON API: `GET /api/hub/{health,users,me,tracks/{id},tracks/{id}/ripeness,overlap,playlists}`,
`POST /api/hub/query`, `POST /api/hub/services/{service}/sync`, `GET /api/hub/services/{service}/connect`,
`POST /api/hub/playlists/{id}/toggle`, `POST /api/hub/playlists/{enable,disable}-all`.

CLI: `music-hub import-traktor --user <slug> --file collection.nml`.

New pages (this batch): `/tag-queue` (+`/{id}`, `/{id}/tag`, `/{id}/untag`),
`/track/{id}/stream`, `/track/{id}/tag` + `/track/{id}/untag`, `/artists` +
`/artist/{name}`, `/history`. Tag detail gained `/tag/{id}/sync` and
`/tag/{id}/source/keep`; `/tags` supports multi-group `?groups=`.

---

## 7. Tag layer, groups, collectives, engine

- **Tags are explicit & per user.** A tag exists only once a user creates it (usually by
  promoting a playlist → same name, linked by playlist id). A tag can aggregate several
  source playlists. `rebuild()` materializes `hub_track_resolved_tags` (call after changes).
  ⚠️ Bulk callers: use `tags::link_playlist_to_tag` (no rebuild) + one `rebuild` at the end.
- **Groups** = per-user collections of tags (n:m, `hub_group_tags`); roles owner/contributor/
  subscriber. **Collectives** = a set of users that owns groups (`hub_tag_groups.collective_id`);
  every collective member is an effective **contributor** in all the collective's groups
  (`tags::effective_role_of`).
- **Tag parents/aliases** (`hub_tag_parents`): imported from MMM `tag_parents`. Name filters
  are **hierarchy-aware** (`tags::matching_tag_ids` = name match + transitive descendants;
  `tags::related_tag_ids` = ancestors+descendants).
- **Weights** (`hub_tag_groups.weight`): importance per group. Editable on `/groups/{id}`
  and per collective member on `/collectives/{id}`.
- **Ranked groups** (`hub_tag_groups.ranked` + `hub_group_tags.rank` 0..5): energy levels
  (e.g. Phase: end=0, start=1, release=2, sustain=3, build=4, peak=5). Imported from MMM
  `tag_energy_levels`. **A tag has NO ranking of its own — only via its group.**

### Ranking engine (digging)

- Sources: **Hub-intern** (co-occurrence in seed's playlists), **ReccoBeats** (recommendations),
  **cosine.club**, **Last.fm**, **Audio** (EffNet neighbors). Fetched **concurrently**.
- Signals per candidate: `shared_weight` (Σ group weights of tags **shared with the seed**),
  `candidate_weight` (Σ group weights of the candidate's **own** tags), plus base counts
  (users/playlists/likes/sources).
- Formula: `score = users·base_users + playlists·base_playlists + likes·base_likes +
sources·base_sources + shared·shared_factor + candidate·candidate_factor`.
- **All factors configurable at `/admin`** (`settings::ENGINE_*`; `settings::engine()` loads
  them live per request). `engine_parent_match` (1/0) expands seed tags to ancestors/descendants.
- Digging filters: `mine`, `bpm`+`tol` (absolute BPM tolerance), `harm`, `towner`, `tgroup`,
  `powner`, `tag` (hierarchy-aware), `wmin` (min candidate tag weight).
- On-demand enrichment: `POST /overlap/enrich` and `POST /digging/enrich` queue tracks;
  worker drains ReccoBeats first (fast) then a **separate bounded FreqBlog loop** (slow, paid).

### music-api (whole-playlist download)

- Playlist & tag pages (token configured) show **„Fehlende ordern"**, **„Status aktualisieren"**,
  a **format** select and **„… als ZIP laden"**, plus a live **`x/y bereit`** badge (htmx polls
  `/…/{id}/progress` every 10s).
- **Order only missing**: `order_missing` refreshes `hub_music_state` (cache of per-ISRC state,
  migration 025) then orders only ISRCs not yet `ready` (`POST /orders`).
- **Download**: `zip_for` → `build_playlist_zip` fetches every ready track (`GET /isrc/{isrc}/{format}`,
  misses skipped) into a ZIP, then streams it (tmp unlinked before streaming — Linux fd stays valid).
  Single track: `GET /track/{id}/download?format=…`. `format` ∈ flac/mp3/wav/m4a.
- State cache: `music_api::{refresh_states, cached_counts, missing_isrcs}`.

---

## 8. Testing

```bash
cd music-hub && cargo test          # ~145 tests (lib + integration). Must stay green.
```

- Integration harness: `tests/common/mod.rs` → `common::spawn()` boots the real router on a
  temp SQLite with all migrations + seeded fixtures (`src/db/testing.rs`: users alice/bob/carol,
  tracks `t_all/t_two/t_pl/…`, playlists `pl_alice/pl_bob/pl_carol`).
- Test files: `auth.rs`, `read.rs`, `ingest.rs`, `similar.rs`, `ui.rs` (most page tests live
  here), `views.rs`, `harness_smoke.rs`.
- **Every new endpoint/filter gets a test.** There is **no Playwright** for the hub.
- There is no `cargo test` gate that runs the server; tests are self-contained.

---

## 9. MMM import (tags/categories/parents/energy)

MMM DB lives on the **Mac**: `/Users/momo/.local/share/momos-music-manager/library.db` (~127 MB;
the server does **not** have it). Ship a small extract instead:

```bash
rm -f /tmp/mmm-tags.db
sqlite3 /tmp/mmm-tags.db "ATTACH '/Users/momo/.local/share/momos-music-manager/library.db' AS m;
  CREATE TABLE tag_categories AS SELECT * FROM m.tag_categories;
  CREATE TABLE tags AS SELECT * FROM m.tags;
  CREATE TABLE tag_parents AS SELECT * FROM m.tag_parents;
  CREATE TABLE tag_energy_levels AS SELECT * FROM m.tag_energy_levels;"
scp -q /tmp/mmm-tags.db music-catalog:/home/momo/mmm-tags.db
ssh music-catalog 'cd ~/music-hub && DATABASE_URL=sqlite:/home/momo/music-hub/hub.db \
  ./target/release/music-hub import-mmm-tags --db /home/momo/mmm-tags.db --user momo'
```

Idempotent. Imports: categories→groups, tags (+playlist link), `tag_parents`, `tag_energy_levels`
(marks the category's group `ranked` and sets ranks). **Never let it rebuild per tag** (see gotchas).

---

## 10. Gotchas (learned the hard way)

- **NEVER edit an applied migration** (not even a comment) — sqlx checksums migration files and the app will refuse to boot ("migration N was previously applied but has been modified"). Add a new file instead. Consolidate only on a fresh DB (release step).
- **askama `==` inside an HTML attribute tag** breaks the build (`failed to parse template
source`). Precompute a bool field in Rust (e.g. `selected`/`active`) and use `{% if x.y %}`.
- **SQLite bind-variable limit**: `IN (…)` with huge id lists silently fails. **Chunk** them
  (we use 300–900 per chunk; see `dig_details`, `tagged_track_ids`, `enroll`, `similar`).
- **N+1 kills the page**: batch presence/tags (`dig_details`, `digging::presence_many`) and
  parallelize external HTTP (`tokio::join!`). `match_track` per candidate = full scans → use
  the preloaded `digging::Matcher`.
- **Don't block the worker with slow paid calls**: FreqBlog runs in its **own bounded loop**
  (`features::freqblog_priority_once`), not inline in the ReccoBeats/genre loop.
- **Importer perf**: `add_playlist_to_tag` calls `rebuild()`; for bulk use
  `link_playlist_to_tag` + one rebuild at the end (else ~500 rebuilds = hours).
- **Backticks / `$` in shell & `gh` bodies** get command-substituted → avoid in PR/issue bodies.
- `sqlite3` CLI does **not** enforce FK cascade — clean dependent rows manually when deleting
  via CLI.
- On `.200` the CPU is x86-64 **baseline** (no AVX) → prebuilt ONNX Runtime SIGILLs; embeddings
  come from the Essentia analyzer service. Shared tmp dir must match the service
  (`HUB_ANALYZE_TMP=/home/momo/music-hub/tmp`).

---

## 11. Git / PR rules (root AGENT.md)

- **Never commit to `main`.** Branch `feat/…`/`fix/…`, one issue = one branch = one PR,
  Conventional-Commit titles. Sub-agents must **not** `git add`/commit.
- `main` stays linear; PRs are gated by required checks (`Build + Test`, commit-title,
  issue-reference). No checks = not green.
- **Before release**: consolidate same-release migrations into the earliest new one
  (this feature set added `020`–`024`; merge them), write `CHANGELOG.md`, add ADRs in
  `docs/DECISIONS.md`, update README, then `release/<version>` PR (human-merge, never auto).

---

## 12. Open items / next ideas

- **Release consolidation** of migrations `020`–`024` + CHANGELOG/ADRs, then get PR **#202**
  green and merged.
- **Rang-Signal in die Engine** (Phase-Nähe: Seed-Rang ↔ Kandidat-Rang) with a configurable
  factor on `/admin`.
- **Multi-select filters** (several groups/collectives/tags/users at once) across `/tags`,
  `/overlap`, `/digging`.
- Engine factors **per collective** (currently global via `/admin`).
- SoundCloud / YouTube ingest per user (issues **#177/#178**) — needs API keys/OAuth apps.
