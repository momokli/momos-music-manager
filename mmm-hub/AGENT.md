# mmm-hub — Agent Handover

> **Last updated**: 2026-10-10 · branch `feat/mmm-hub` · PR **#202** (open, → `main`)
> Keep this file current. It is the entry point for any agent working on `mmm-hub/`.

---

## 1. What this is

`mmm-hub` is the **multi-user little brother** of Momo's Music Manager: a separate
cargo crate in `momokli/momos-music-manager` (`mmm-hub/`, **not** a workspace member).
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
| App dir on server | `/home/momo/mmm-hub`                                                                 |
| Live DB           | `/home/momo/mmm-hub/hub.db` (+ `-wal`/`-shm`)                                        |
| Sanitized copy    | `hub-public.db` → Datasette `data.zukkafabrik.de`, SchemaSpy `schema.zukkafabrik.de` |
| systemd unit      | `mmm-hub` (`sudo systemctl restart mmm-hub`)                                         |
| Audio analyzer    | Essentia service on `.200:8711` (`deploy/analyzer/`)                                 |
| Git remote        | `github` → `github.com:momokli/momos-music-manager.git`                              |

Login for smoke tests: `Mctoastus` / `1234` (also `momo`, `ANKD`; passwords may differ).

### Deploy loop (copy-paste, works)

```bash
# run from repo root
tar czf - -C mmm-hub --exclude target --exclude 'hub.db*' --exclude .env \
  --exclude 'datasette-*' --exclude 'make-public*' --exclude docs --exclude 'models' --exclude 'tmp' . \
  | ssh music-catalog 'tar xzf - -C ~/mmm-hub 2>/dev/null && find ~/mmm-hub -name "._*" -delete' \
  && ssh music-catalog 'cd ~/mmm-hub && ~/.cargo/bin/cargo build --release --locked 2>&1 | tail -1 && sudo systemctl restart mmm-hub && sleep 2 && systemctl is-active mmm-hub'
```

- **Never ship the DB or `.env`** (excluded above). The tar excludes `docs/`, `models/`,
  `tmp/` too.
- `find … -name "._*" -delete` strips macOS AppleDouble junk.
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

- Server `.env` at `/home/momo/mmm-hub/.env` (+ DB overrides editable at **`/admin`**).
- Keys seen in use: `SPOTIFY_CLIENT_ID/SECRET/REDIRECT_URI`, `MUSIC_API_TOKEN`,
  `COSINECLUB_API`, `FREQBlog_API`, `EFFNET_MODEL`, `EFFNET_LABELS`,
  `HUB_ANALYZER_URL`, `HUB_ANALYZE_TMP`, `HUB_DATABASE_URL`, `HUB_HOST`, `HUB_PORT`.
- **Config priority**: env > `.env` > settings in DB (`hub_settings`, edited on `/admin`) > defaults.
- **Admin-only settings page** `/admin` (`settings::ADMIN_FIELDS`) stores API keys + the
  whole **ranking engine** config (see §7).
- ⚠️ **Rotate secrets**: `DEEMIX_ARL` and `MUSIC_API_TOKEN` were once printed into a chat — the
  user should rotate them.

---

## 4. Source layout

```
mmm-hub/
├── migrations/            001…024 (additive; consolidate per release)
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
│   ├── settings.rs        hub_settings + ADMIN_FIELDS + Engine config
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

JSON API: `GET /api/hub/{health,users,me,tracks/{id},overlap,playlists}`, `POST /api/hub/query`,
`POST /api/hub/services/{service}/sync`, `GET /api/hub/services/{service}/connect`,
`POST /api/hub/playlists/{id}/toggle`, `POST /api/hub/playlists/{enable,disable}-all`.

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
cd mmm-hub && cargo test          # ~85 tests (lib + integration). Must stay green.
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
ssh music-catalog 'cd ~/mmm-hub && DATABASE_URL=sqlite:/home/momo/mmm-hub/hub.db \
  ./target/release/mmm-hub import-mmm-tags --db /home/momo/mmm-tags.db --user momo'
```

Idempotent. Imports: categories→groups, tags (+playlist link), `tag_parents`, `tag_energy_levels`
(marks the category's group `ranked` and sets ranks). **Never let it rebuild per tag** (see gotchas).

---

## 10. Gotchas (learned the hard way)

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
  (`HUB_ANALYZE_TMP=/home/momo/mmm-hub/tmp`).

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
