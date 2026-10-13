# Changelog

All notable changes to **music-hub**.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: per-component tag line `hub-v<x.y.z>` (see
[`docs/stack-versioning.md`](../docs/stack-versioning.md)).

## [Unreleased]

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

[Unreleased]: https://github.com/momokli/momos-music-manager/compare/hub-v0.25.0...HEAD
[0.25.0]: https://github.com/momokli/momos-music-manager/releases/tag/hub-v0.25.0
[0.24.0]: https://github.com/momokli/momos-music-manager/releases/tag/hub-v0.24.0
