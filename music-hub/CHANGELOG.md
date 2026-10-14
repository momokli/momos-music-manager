# Changelog

All notable changes to **music-hub**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `hub-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

### Added

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

**hub-tagging** — Tagging workflow: a track counts as *tagged-complete* when the
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
  *skip this track in the tagging task*, not *done*; wording and buttons updated.
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
scope (my contribution vs whole cluster) and explain *why* each tag/cluster is
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
