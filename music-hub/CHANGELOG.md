# Changelog

All notable changes to **music-hub**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `hub-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

### Added

- feat: **Tag hygiene — rename, merge similar tags, suggestions** (#456) — a tag
  detail page (`/tag/{id}`) now surfaces owner-only **suggestions** for similar
  own tags (`tags::similar_tags`; ranked by normalised-equal > small edit
  distance > token/prefix share > fuzzy) and a **merge** action
  (`tags::merge_tags`) that re-points every tag-referencing table onto the target
  inside one transaction (idempotent: a missing source is a no-op; no duplicate
  `hub_track_resolved_tags`; target keeps its type with a fallback to the source
  and `sync_playlist` OR-ed). A **rename clash** (`tags::rename_tag_outcome`) no
  longer errors: it proposes merging into the existing tag. New session-guarded
  JSON route `GET /tags/{id}/similar?limit=` and form routes `POST /tag/{id}/merge`
  and the extended `POST /tag/{id}/rename`. **No migration, no version bump.**
- feat: **Smart-Tags / Dynamic Lenses** (#298) — a tag can store a filter
  definition (base tags, BPM/energy bounds, limit, sort) whose track set is resolved
  at **runtime** (never materialized), shown like a normal tag with a
  `⚡ dynamisch` badge in the list **and** on the detail page. Additive migration
  `055_smart_tags.sql`; create/edit routes `POST /tags/smart/create` and
  `POST /tag/{id}/smart`.
- feat: **unified tag input** (#449) — one fuzzy combobox replaces the
  `name + type + „Taggen“` form on every track-tagging surface (track page, tag-queue
  detail, tag-task card, track actions, tags page). New session-guarded
  `GET /tags/search?q=&limit=` ranks the caller's **own** tags (exact > prefix >
  word-prefix > substring > fuzzy) and returns `{id,name,typeName,typeLabel,icon}`; the
  tag-add endpoints additionally accept `tag_id` (link an existing tag) or
  `name`+`type` (create-and-link). Shared `_partials/tag_input.html` + inline JS
  (debounced search, ↑/↓/Enter/Esc, native `<dialog>` create modal).
- feat: hub_devices registry — device pairing endpoints (#363)
- **BPM/key backfill from music-api** (#354): `hub_track_features` is now filled from
  music-api's ISRC authority (`GET /metadata?isrcs=`), closing ReccoBeats gaps. New
  `music_api::metadata_range` client, `features::backfill_from_music_api`
  (`source='music-api'`, bounded + chunked, transient-failure-safe), CLI
  `metadata-backfill --limit 2000`, and a "BPM/Key holen" button on the music-api
  console. Migration `050` adds `hub_track_features.metadata_checked_at` (7-day TTL).
- feat: **Bi-way playlist sync** (#283) — tag/track changes are written back to the
  Spotify **source** playlist of a synced tag: adds via `POST /playlists/{id}/items`,
  removes via `DELETE /playlists/{id}/items`, only for owned/collaborative playlists and
  gated by the write scope (re-auth hint as `scope_missing`). A final `429`/`QUOTA_EXCEEDED`
  is surfaced as the `quota` status; writes retry a `429` once (bounded via
  `Retry-After`). New additive migration `054` adds the `last_push_*` bookkeeping columns
  to `hub_playlists`.

### Changed

- **Track-page download UX** (#460): the bare `FLAC laden` / `MP3 laden` links are gone.
  The `Download`-Bucket offers **one button per format the vault says is ready**
  („FLAC", „MP3 320", „MP3 128") and a **passive hint** while nothing is ready — no dead
  link, no manual „holen" (the demand covers the library, ADR-068). Every click goes
  through a new htmx preflight `POST /track/{id}/download/check`: ready → `HX-Redirect`
  to the file, otherwise an **error toast** instead of the raw 502 page. Same for the
  playlist/tag ZIP (`POST /playlist|tag/{id}/download/check`), and the `mp3-320` /
  `mp3-128` tokens now survive format validation and get the right extension/MIME.

### Fixed

- **Toasts mit Nicht-ASCII-Texten** (#469): der `HX-Trigger`-Toast-Wert wurde bei
  Nicht-ASCII-Zeichen (`—`, `…`, Umlauten) verworfen, bevor er den Browser erreichte —
  der Toast feuerte still nicht. Alle Toast-Pfade gehen jetzt über `crate::toast`
  (ASCII-escaptes JSON, das clientseitig identisch dekodiert).
- **Now-Playing zeigt BPM/Key live aus dem Vault** (#466) — das Widget las den
  `hub_track_audio`-Read-Model, der hinterherhinkt (gedeckter Backfill, `ORDER BY id`, 7-Tage-TTL
  für „no data"), und zeigte deshalb „—", während die Track-Page den Vault-Wert hatte. Beide
  lesen jetzt denselben `resolved`-Block über `music_api::resolved_audio`.

## [0.18.0] — 2026-10-17

**hub-tag-clusters** — Semantische Tag-Cluster als Schicht zwischen Gruppe und Tag
(deterministische Namensähnlichkeit als Baseline), collective-gesteuert, plus ein
Per-User Tag-Scope (eigenes vs. Collective).

### Added

- **Tag-Cluster-Schema** (#262, PR #326): additive Migration `043_tag_clusters.sql` legt
  `hub_tag_clusters` + `hub_tag_cluster_members` an (`tag_type`, `label`,
  `ON DELETE CASCADE`); bestehende Migrationen unverändert.
- **Auto Tag-Cluster-Builder** (#264, PR #326): `clusters::rebuild` gruppiert semantisch
  verwandte Tags **desselben Typs** über eine deterministische Namensähnlichkeit
  (normalisieren + Prefix/Edit-Distanz/Token); pure Funktionen, unit-getestet.
- **Cluster-Sicht + Scope-Toggle** (#265, PR #326): Seite `/tag-clusters`
  (Nav → Bibliothek → Cluster) mit Typ-Filter, Scope-Umschaltung (Meine/Alle) und
  „Neu berechnen"; pro Cluster Rename, Tag entfernen, Tag hinzufügen (nur eigene Tags),
  Cluster löschen.
- **Cluster-Steuerung durch Collective-Members** (#266, PR #326): POST-Handler
  `rename` / `member/add` / `member/remove` / `delete` mit htmx + Toast-Feedback
  (`HX-Trigger`); manuelle Edits gewinnen bis zum nächsten Rebuild.
- **Verifikation Cluster + Scope** (#268, PR #479): Integrationstests
  `music-hub/tests/clusters_steering.rs` (8 Tests) — Login-Guard, Schema + CASCADE,
  Scope-Read (`mine`/`all`), Rename, Member add/remove mit Owner-Guard, Delete + CASCADE,
  Rebuild-Determinismus. `cargo test -p music-hub` → 580 passed / 0 failed.

### Changed

- **Per-User Tag-Scope `own` ⇄ `collective`** (#267, PR #478): neues globales
  Nutzer-Setting `tag_scope ∈ {own, collective}` (Default `own`), editierbar unter
  `/settings`. Wirkt einheitlich in similarity/scoring/digging/overlap/tagqueue/recommend/
  track-detail; der Query-Param `?tag_scope=` schlägt das gespeicherte Setting und wirkt
  ohne Reload. Neue additive Migration `053_hub_user_settings` (Tabelle
  `hub_user_settings`); ohne Zeile gilt der Registry-Default — kein Row wird materialisiert.

## [0.28.0] — 2026-10-15

**hub-ops** — The Hub becomes operable: a read-only health/metrics status endpoint for
Hub worker loops and backlog, an admin/ops console to pause/resume workers, retry failed
jobs and watch an event log, and a polished import-history UI.

### Added

- **Observability — worker/metric health** (#353, PR #438): new session-guarded
  `GET /api/hub/status` reports `data.status` (`ok`/`degraded`), per-loop Hub worker
  health (`hub.workers[]`: name/lastRunAt/lastWorked/iterations/lastError for the 5
  loops), on-demand backlog counts (`hub.backlog`), and the proxied music-api
  worker/queue block (`musicApi`; never 500s when unconfigured/unreachable). In-memory
  `worker::WorkerHealth` registry; admin/ops panel `GET /api/hub/status/panel` embedded
  in `/admin` (`hx-trigger="load, every 15s"`, `_partials/hub_status.html`).
  `/api/hub/health` is unchanged.
- **Admin/ops console** (#350, PR #439): `/admin` gains actions — pause/resume the 5
  worker loops (in-memory `WorkerHealth::set_paused`, checked before each iteration),
  a bounded (capacity 200) in-memory event ring-buffer with log view
  `GET /api/hub/ops/logs` (`_partials/hub_logs.html`), and `POST /api/hub/ops/retry` to
  requeue failed `hub_playlists` / `hub_service_accounts` rows.
- **Import-history change-count badges** (#351, PR #440): the runs table shows per-run
  added/changed/removed counts as coloured `+n` / `~n` / `−n` badges, plus a change-count
  event list with kind-filtering and a diff column.

### Changed

- **Import-history UI polish** (#351, PR #440): history page gains a change-kind filter
  (`change=`), a per-run filter with an active chip, a before/after diff cell for change
  events, stats chips per run, and filter-aware empty states (a `q`/`source`/`user` filter
  that matches nothing now reads „Keine Änderungen für diesen Filter" with a reset link
  that clears every filter).

## [0.27.0] — 2026-10-14

**hub-tagging** — Tagging workflow: a track counts as _tagged-complete_ when the
describing tag families (MOOD/GENRE/VIBE/PHASE/MERKMAL) are set, or the user forces
it; a daily/weekly tagging task surfaces the most-played uncomplete tracks as a
card queue; per-type „describes the song" toggles refine which types count as
describing.

### Added

- **Track 'tagged-complete' flag** (#345, PR #401): auto rule (all describing
  families set) plus a per-user manual flag (`complete = auto || manual`); migration
  `045_hub_track_tag_done.sql`, `music-hub/src/tagdone.rs`, setting
  `engine_tag_done_types`, routes `POST /track/{id}/done|undone`, badge + button on
  the track view.
- **Daily/weekly tagging task** (#346, PR #402): `GET /api/hub/tag-task?cadence=daily|weekly`
  returns the user's incomplete tracks most-played first, persisted per
  `(user, cadence, period_start)`; module `music-hub/src/tagtasks.rs`, migration
  `046_tag_task.sql`, settings `engine_tag_task_limit` / `engine_tag_task_cadence`.
- **Tag-task UI** (#347, PR #403): `/tag-task` renders the task as a card grid
  (rank, title, artists, plays, missing-family chips) with tag/done/undone routes and
  a nav link; `music-hub/src/tagtaskui.rs`, `templates/tag_task.html`.
- **Per-type 'describes the song' toggle** (#348, PR #436): 7 per-type boolean
  settings `ENGINE_DESCRIBES_{MOOD,GENRE,VIBE,PHASE,MERKMAL,SETLIST,RUMPELKISTE}`
  (defaults mood/genre/vibe/phase/merkmal=`1`, setlist/rumpelkiste=`0`), editable
  under `/admin`; `artists.rs` / `tagdone.rs` use `describes_map()` with fallback to
  `TagType::describes()`.

### Fixed

- **Tag-queue „nur ungetaggt" filter + tag-task card UX** (#430, PR #431): the
  untagged filter now counts every resolved tag, not only typed ones; tag-task cards
  show resolved tags, a play button, and update in place (htmx).
- **Tag „fertig" reframed as „überspringen"** (#432, PR #433): the manual flag means
  _skip this track in the tagging task_, not _done_; wording and buttons updated.
- **Tag skip action moved off the track page onto the tag-task** (#434, PR #435): the
  track page drops the tag-status block; the task gains an „Übersprungene" view with
  undo.

## [0.26.0] — 2026-10-13

**hub-artist-album** — Artist-explorer depth: the artist profile carries
collaboration partners and b2b sets plus a per-user reference-artist score;
albums become a first-class entity (artist → album → track); profile visibility
defaults to the viewer's own references, with other users selectable.

### Added

- **Artist collaborations + b2b sets** (#341): the artist profile gains a
  „Kollaborationspartner" panel (AR5a, multi-artist tracks) and „b2b-Sets"
  (AR5b, Traktor playlists with `b2b` in the name, from the session/playlist
  ingest) — `music-hub/src/artists.rs`.
- **Reference-artist scoring** (#342): deterministic score over shared tags +
  collaborations + b2b (`REF_TAG_W=3.0`, `REF_COLLAB_W=2.0`, `REF_B2B_W=1.0`,
  configurable), score-desc / name-asc, top 30, filtered to the selected user.
- **Album entity + album view** (#343): new `/albums` list and `/album/{name}`
  detail (cover, artists, tracklist); album names on the artist profile and the
  track view link to the album page; nav gains „Alben" —
  `music-hub/src/albums.rs`, `templates/albums.html`, `templates/album.html`.
- **Artist profile visibility (mine vs others)** (#344): „Meistgespielt" shows
  the viewer's own block plus an „Andere Nutzer" section; reference-artist
  selection chips per user, default = me.

## [0.25.0] — 2026-10-13

**hub-recs** — Similarity v2 (cross-type tag overlap, Rumpelkiste-aware) drives the
digging and overlap ranks; cluster-driven tag recommendations respect a per-user
scope (my contribution vs whole cluster) and explain _why_ each tag/cluster is
recommended.

### Added

- **Similarity v2 — cross-type tag overlap** (#337): track similarity weights
  agreement of tags across types.
- **Cluster-driven tag recommendations + per-user scope** (#336, PR #398):
  sibling tags of the same semantic cluster feed the recommendation engine as a
  new `cluster` signal kind (engine weight `engine_rec_cluster`, default `1`,
  range `0.0–100.0`, editable in the settings registry); `?scope=mine|all`
  restricts the candidate set to the viewer's own contributions, with a toggle
  in the track and queue views.
- **Recommendation „why“ in the UI** (#340): each recommended tag/cluster shows
  the reason it was suggested.

### Changed

- **Similarity wired into digging/overlap** (#339): the digging and overlap
  ranks use similarity v2.

### Fixed

- **Rumpelkiste exception in similarity** (#338): Rumpelkiste playlists are
  ignored in similarity.

## [0.24.0] — 2026-10-13

**hub-backpacks** — multiple named backpacks per user; a backpack is a
collection of sources; exactly one active set per user; the active backpack
acts as a global scope in list views; a backpack resolves to an ordered,
deduped, score-sorted track list and can be exported.

### Added

- **Backpack schema** — `hub_backpacks` + items + active (#327, #379).
- **Backpack sources** — tag / playlist / smart-filter (lens) / manual (#329, #380).
- **Backpack resolver** — union, dedup, score-sorted, limit (#330, #381).
- **Active backpack as a global scope** in list views (#331, #382).
- **Backpack view** — cards, live counts, sources (#332, #387).
- **Backpack CRUD + active toggle UI** (#328, #388).
- **Backpack export** — m3u8 / Traktor / ZIP (#333, #391).
- **Backpack tests** — resolver + scope + UI (#334).

[Unreleased]: https://github.com/momokli/momos-music-manager/compare/hub-v0.27.0...HEAD
[0.27.0]: https://github.com/momokli/momos-music-manager/releases/tag/hub-v0.27.0
[0.26.0]: https://github.com/momokli/momos-music-manager/releases/tag/hub-v0.26.0
[0.25.0]: https://github.com/momokli/momos-music-manager/releases/tag/hub-v0.25.0
[0.24.0]: https://github.com/momokli/momos-music-manager/releases/tag/hub-v0.24.0
