# Changelog

All notable changes to **music-hub**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `hub-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

### Added

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

## [0.23.0] — 2026-10-17

**hub-smartplay** — Smart-Tags/Dynamic Lenses, In-Tool-Playlistbau (Track-Vorschläge für
kuratierte Playlists) und geplante Daily/Weekly-Playlist-Generierung.

### Added

- **Smart-Tags / Dynamic Lenses** (#298, PR #485): filterbasierte Tag-Zugehörigkeit, deren
  Track-Menge **zur Laufzeit** berechnet und **nicht materialisiert** wird. Migration `055`
  legt `hub_smart_tags(tag_id UNIQUE FK, owner, filter_json, created_at, updated_at)` an;
  `SmartFilter` (in `src/tags.rs`, Defaults/Clamps, JSON-Roundtrip) und `build_smart_query`
  (durchgängig `QueryBuilder::push_bind` gegen Injection, `ORDER BY` aus festem Whitelist-`match`).
  `tags::rebuild()` lässt Smart-Tags unberührt (Test `smart_tag_not_materialized`); leerer Filter
  wird zweifach abgelehnt; `limit` default 100, `clamp(1,1000)`. Server-seitige
  Ownership-Prüfung `ensure_tag_ids_owned`. `⚡ dynamisch`-Badge in der Tag-Liste und auf der
  Tag-Detailseite; Routen `POST /tags/smart/create` und `POST /tag/{id}/smart` (owner-gebunden).
- **Track-Vorschläge für kuratierte Playlists** (#299, PR #486): neu `Suggestion` + `suggestions()`
  in `music-hub/src/curate.rs` (Seeds via `resolve_track_ids`, Kandidaten = hub_tracks − Seeds −
  Rumpelkiste; additiver Score aus Similarity / Playlist-Co-Occurrence / EffNet-Cosine / Camelot-Key /
  Ripeness, Seed-Aggregation **max**, batched ohne N+1, Gewichte live aus `settings::engine()`).
  Neuer Endpoint `GET /curate/{id}/suggestions` + `POST /curate/{id}/items` (Session-Gate,
  unbekannt/fremd → 404, Owner-Check); htmx-Sektion `_partials/curate_suggestions.html` in
  `curate_detail.html`; 5 neue Integrationstests.
- **Geplante Daily/Weekly-Playlist-Generierung** (#300): Issue #300 wurde geschlossen
  (Kommentar „Done & deployed (PR merged to feat/mmm-hub)"). Ein entsprechender Merge/PR ist in
  **diesem** Repository jedoch **nicht** nachweisbar (kein Scheduler-Code unter `music-hub/src`) —
  Eintrag daher **unbestätigt** (siehe Abnahme des Release-PRs).

## [0.20.0] — 2026-10-17

**hub-playlist-sync** — Playlist-Fetch/Update pro User: prominenter Fetch-Button,
Playlist-Level last-updated + Change-Detection via Snapshot, inkrementelles Re-Fetch
(spart API-Calls) und Bi-way-Sync (Hub → Spotify, write-Scope + Re-Auth nötig).

### Added

- **Inkrementelles Playlist-Re-Fetch via Snapshot** (#281, PR #286): Migration `037`
  fügt `hub_playlists.last_synced_snapshot` hinzu. Der Worker re-fetcht eine Playlist nur
  noch, wenn sich ihr Remote-Snapshot seit dem letzten Item-Fetch geändert hat (vorher:
  nie), und schreibt `last_synced_snapshot` bei Erfolg — das spart Spotify-API-Calls /
  429-Headroom. Neuer Endpoint `POST /api/hub/playlists/{id}/refetch` reiht eine einzelne
  Playlist in die Worker-Queue ein.
- **Fetch-Button pro User + Playlist-Status** (#282, PR #286): in `/me/playlists` ein
  prominenter „Neue Playlists holen“-Button (listet die Playlists neu von der Plattform),
  eine Spalte „Aktualisiert“, ein Changed-Badge, wenn der Snapshot sich bewegte, und ein
  Re-Fetch-Button pro Zeile.
- **Bi-way-Sync: Hub → Spotify** (#283, PR #483): getaggte Tracks werden zurück in die
  Spotify-**Source-Playlist** geschrieben — Add via `POST /playlists/{id}/items`, Untag
  entfernt den Track wieder via `DELETE /playlists/{id}/items` (`unsync_tag_from_playlists`).
  Additive Migration `054` ergänzt `last_push_at` / `last_push_status` / `last_push_error` /
  `last_push_action` an `hub_playlists`. Gates: nur `is_owned=1` **oder** `collaborative=1`,
  nur bei vorhandenem `playlist-modify-*`-Scope, nur mit externer Spotify-ID. Idempotenz:
  die Hub-Spiegel-Zeile (`hub_playlist_tracks`) wird **erst nach erfolgreichem Spotify-Push**
  eingefügt; `keep_on_remove` archiviert statt zu löschen. Gemeinsamer bounded
  `429/Retry-After`-Retry (1×, Cap 60s) + neuer `delete_json`-Helper im Spotify-Client.
- **Push-Status-UI + Re-Auth-Hinweis** (#283, PR #483): Push-Status je Source-Playlist
  (Badges „Push OK“ / „Scope fehlt“ / Quota / übersprungen / Fehler) und ein Re-Auth-Hinweis
  bei fehlendem Write-Scope (`pages.rs` / `templates/tag.html`).
## [0.19.0] — 2026-10-17

**hub-tag-types** — Hard-wired Tag-Typen ersetzen die flexiblen Gruppen. Ein Tag hat
jetzt genau **einen** festen Typ (MOOD / GENRE / VIBE / PHASE_1..5 / MERKMAL / SETLIST /
RUMPELKISTE); die Gruppen-Tabellen und die Gruppen-UI entfallen, der Typ hängt 1:1 am Tag.

### Added

- **`tagtypes`-Modul** (#275, PR #278): `TagType`-Enum + Helfer (`describes`, `weight`,
  `phase_num`, …); per-Typ-Gewichte über die Settings-Registry (`engine_w_*`) im `/admin`
  konfigurierbar.
- **Top-Tags je Typ** (#277, PR #278): die Künstler-Seite zeigt eine Tabelle **welche**
  Tags je Typ vorkommen und wie oft (Name, Typ, Anzahl), sortierbar.

### Changed

- **Tag-Typ-Schema + Gruppen-Drop** (#274, PR #278): additive Migration
  `036_tag_types.sql` ergänzt `hub_tags.type` (CHECK über die festen Typen), backfillt die
  Typen aus `hub_group_tags`/`hub_tag_groups` (setlist→SETLIST, vibe→VIBE, mood→MOOD,
  merkmal→MERKMAL, rumpelkiste→RUMPELKISTE, genre→GENRE, Kollektiv-Phase ranked →
  `PHASE_<rank>`) und droppt `hub_group_tags`, `hub_group_members`, `hub_tag_groups`
  (+ Indizes); Sammel-Index auf `hub_tags(type)`.
- **Engine auf Tag-Typ statt Gruppe** (#275, PR #278): scoring/similarity/tagqueue/
  recommend/digging/artists keyen statt auf group `kind`/`role` auf den Tag-Typ; der
  Rumpelkiste-Ausschluss läuft über Typ `RUMPELKISTE`; Phasen als `PHASE_1..5`
  (Energy 1..5); alle SQL-Joins auf `hub_tags.type` umgestellt.
- **Gruppen-UI raus, Typ-Filter/Edit rein** (#276, PR #278): die Seiten `/groups`,
  `/groups/{id}` samt Nav-Einträgen und Group-Handlern sind entfernt; die Tags-Seite
  filtert nach Typ; Tag anlegen/bearbeiten wählt den Typ, die Tag-Detailseite zeigt ihn;
  `groups.html`/`group.html` gelöscht.
- **Künstler-/Track-Tags: Describe-only** (#277, PR #278): es werden nur die
  beschreibenden Typen (MOOD/GENRE/VIBE/PHASE_*/MERKMAL) angezeigt; `SETLIST` und
  `RUMPELKISTE` sind ausgeblendet.

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
