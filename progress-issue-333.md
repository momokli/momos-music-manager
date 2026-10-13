# Progress — Issue #333: Backpack export (m3u8 / Traktor / ZIP)

- Repo: momokli/momos-music-manager (crate `music-hub/`)
- Basis: `main` @ 3540dbe (== origin/main)
- Fokus-Milestone: hub-backpacks-0.24.0
- Deliverable: gepushter Branch + offener PR gegen `main` mit `Closes #333`
- Branch: `feat/issue-333-backpack-export`
- Arbeitskopie: ~/repos/momos-music-manager-333

## Dispatch-Gültigkeit

Deps #327/#329/#330/#331/#332 gemergt (PRs #379/#380/#381/#382/#387), #328 via #388.
Kein PR/Branch für #333 → normale Pipeline.

## Scope (Milestone-Outcome)

> a backpack resolves to an ordered, deduped, score-sorted track list and can be
> exported (**m3u8 / Traktor / ZIP**).

#333 = Export der **resolveten** Backpack-Trackliste in drei Formaten. Kein CRUD
(#328), keine View (#332), Tests sind #334 (aber eigener Test mitliefern).

## IST-Stand (RE, 2026-10-13)

- `music-hub/src/backpacks.rs`:
  - `resolve(pool, backpack_id, limit)` / `resolve_unbounded(...)` →
    `Vec<ResolvedTrack { track_id: i64, score: i64, sources: Vec<ResolvedSource> }>`
    (bereits sortiert/deduped/score-sorted). **Das ist die Export-Quelle.**
  - `list_backpacks(pool, user_id)`, `Backpack { id, name, is_active, ... }`.
- Track-Metadaten für Playlist-Einträge: `hub_tracks` (id, title, artists) + ISRC via
  `hub_track_external_ids` (Migration 006). Tracks-Download nutzt `t.isrc, t.title, t.artists`.
- Download-/ZIP-Muster bereits vorhanden in `music-hub/src/pages.rs`:
  - Route `GET /playlist/{id}/download` → `playlist_download`; `GET /tag/{id}/download`.
  - `DownloadQuery { format: Option<String> }`, `valid_format()` (default `flac`),
    `zip_for()` → `build_playlist_zip(cfg, tracks, format, tmp)` lädt Bytes über
    `music_api::file_bytes_any(cfg, isrc, format)` und schreibt `{artists} - {title}.{format}`.
  - `zip_response(tmp, filename)` liefert `Content-Disposition: attachment`.
- Route-Registrierung: `pages.rs` `router(state)` (Z. 22–73), Handler-Muster
  `crate::ui::nav(&st, &headers, "...")` → `None` ⇒ `Redirect /login`.
- Kein bestehender m3u/m3u8- oder Traktor-NML-Export im Hub (nur Import in
  `traktor.rs` / `legacy/mmm/src/traktor.rs`). NML-Import kennt das Format.
- Tests: `music-hub/tests/backpack_*.rs` + `tests/common` (temp SQLite, `spawn()`,
  `session_cookie(user_id)`).

## Stage-Status

| Stage | Status | Ergebnis |
|-------|--------|----------|
| 0 RE | done | siehe oben |
| 1 plan | pending | |
| 2 setup | done | Branch `feat/issue-333-backpack-export` von `main` @ 3540dbe (nur lokal). Baseline in `music-hub/`: `cargo build --locked` EXIT 0, `cargo test --locked --no-run` EXIT 0. Vorbestehende Warnungen: `music-hub` (lib) dead_code `tags`/`me` in `src/web.rs:986`; `tests/ui.rs:1713` unused var `cookie`. |
| 3 dev | pending | |
| 4 verify | pending | |
| 5 test | pending | |
| 6 PR | pending | |
| 7 review | pending | |
